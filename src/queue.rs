use crate::Packet;
#[cfg(target_os = "freebsd")]
use crate::freebsd::io::TunIo;
#[cfg(target_os = "macos")]
use crate::header_io::TunIo;
#[cfg(target_os = "linux")]
use crate::offload::{VIRTIO_NET_HDR_LEN, VirtioNetHeader};
#[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
use crate::raw_io::TunIo;
use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::pin::Pin;
use std::task::{self, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(any(target_os = "linux", target_os = "android"))]
const MAX_IOV: usize = libc::UIO_MAXIOV as usize;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
const MAX_IOV: usize = libc::IOV_MAX as usize;
// The macOS/FreeBSD TUN backends prepend one header iovec, so the payload
// limit is one iovec below the kernel's per-write limit.
const MAX_PACKET_SLICES: usize = MAX_IOV.saturating_sub(1);

// Taken from the `futures` crate
macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

/// Represents one queue/file descriptor of a Tun/Tap device.
///
/// A `TunQueue` wraps an owned packet-oriented descriptor. On Linux the
/// descriptor is used as-is (including packet information when enabled). On
/// Android and iOS the descriptor must carry raw IP packets. On macOS it is a
/// `utun` descriptor, and the four-byte `utun` header is handled automatically.
/// On FreeBSD, `Tun` uses a four-byte address-family header for TUN devices.
/// `TunQueue::new` detects `TUNSIFHEAD` automatically. FreeBSD TAP descriptors
/// carry raw Ethernet frames.
///
/// For packet-oriented descriptors, use [`recv`](TunQueue::recv),
/// [`send`](TunQueue::send), or [`sendv`](TunQueue::sendv) to preserve packet
/// boundaries. Receive buffers should be at least as large as the interface
/// MTU. When Linux offload is active, [`recv`](TunQueue::recv) parses the
/// virtio-net header automatically and returns a [`Packet`] with the payload
/// and optional header; size buffers with `vnet_hdr_len() + 65_536` for TUN
/// GSO (`vnet_hdr_len() + 65_550` for TAP, whose frames additionally carry the
/// Ethernet header). `recv` may silently truncate a larger packet on Linux
/// without offload, Android, iOS, and FreeBSD TAP devices, and every other
/// backend reports the truncation as an error — but no backend preserves the
/// oversize frame, so the packet is lost either way. Size receive buffers for
/// the worst case the remote side may send.
///
/// Offload feature masks are configured on `TunBuilder` and readable through
/// `Tun::offload_features` before a `Tun` is converted into a `TunQueue`; the
/// queue retains only the resulting header length exposed by
/// `TunQueue::vnet_hdr_len`.
///
/// Linux `packet_info` is not interpreted by this crate. When
/// `TunQueue::packet_info` is `true`, every packet starts with the kernel's
/// 4-byte `tun_pi` prefix (2 bytes of flags and 2 bytes of protocol). The
/// prefix is part of [`Packet::payload`](crate::Packet::payload), is returned
/// by `AsyncRead`, and must be supplied to `send`/`sendv`/`AsyncWrite`. Parse
/// it before treating the remaining bytes as an IP packet or Ethernet frame.
///
/// ```no_run
/// use std::os::fd::OwnedFd;
/// use tokio_tun::TunQueue;
///
/// # fn example(fd: OwnedFd) -> std::io::Result<()> {
/// let queue = TunQueue::new(fd)?;
/// # Ok(())
/// # }
/// ```
pub struct TunQueue(
    AsyncFd<TunIo>,
    #[cfg(target_os = "linux")] usize,
    #[cfg(target_os = "linux")] bool,
);

#[cfg(target_os = "linux")]
fn validate_framing(vnet_hdr_len: usize, packet_info: bool) -> io::Result<()> {
    if vnet_hdr_len != 0 {
        if vnet_hdr_len != VIRTIO_NET_HDR_LEN {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported virtio-net header size: {vnet_hdr_len}"),
            ));
        }
        if packet_info {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "combined packet information and virtio-net header framing is not supported",
            ));
        }
    }
    Ok(())
}

impl std::fmt::Debug for TunQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunQueue")
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl TunQueue {
    /// Wraps an owned TUN/TAP queue descriptor and sets it to non-blocking mode.
    ///
    /// `O_NONBLOCK` is shared by all descriptors that refer to the same open
    /// file description, so wrapping a descriptor received from another
    /// process can also make that process's descriptor non-blocking.
    /// `FD_CLOEXEC` is left unchanged; callers that may `exec` a child process
    /// should set it before or after wrapping to avoid leaking the queue.
    /// The descriptor is closed if wrapping fails.
    ///
    /// Framing is platform-specific: on macOS the descriptor must be a `utun`
    /// descriptor (the four-byte `utun` family header is always applied); on
    /// Android and iOS it must carry raw IP packets; on FreeBSD TUN the
    /// four-byte `TUNSIFHEAD` address-family header is detected automatically
    /// (`TAPSVNETHDR` is rejected, but a cross-type devfs alias or `TUNSLMODE`
    /// is not detected); on Linux the virtio-net header size is read from the
    /// descriptor and packet information is read from sysfs when available. A
    /// Linux descriptor that combines packet information with a virtio-net
    /// header is rejected with [`Unsupported`](io::ErrorKind::Unsupported).
    ///
    /// Because `TUNGETIFF` aliases `IFF_NOFILTER` and `IFF_NO_PI`, packet
    /// information cannot always be detected for an externally supplied Linux
    /// descriptor. If sysfs is unavailable, the queue assumes `IFF_NO_PI` is
    /// set (no `tun_pi` prefix); callers that know otherwise should manage the
    /// 4-byte prefix themselves.
    ///
    /// # Panics
    ///
    /// This method must be called from within a Tokio runtime with I/O
    /// enabled; otherwise Tokio panics during registration.
    pub fn new<T: Into<OwnedFd>>(io: T) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        return Self::wrap(io.into(), None);
        #[cfg(not(target_os = "linux"))]
        Self::wrap(io.into())
    }

    /// Wraps a queue descriptor whose `packet_info` state is already known.
    ///
    /// Crate-created devices use this to avoid the `TUNGETIFF`/sysfs probe.
    #[cfg(target_os = "linux")]
    pub(crate) fn new_with_packet_info<T: Into<OwnedFd>>(
        io: T,
        packet_info: bool,
    ) -> io::Result<Self> {
        Self::wrap(io.into(), Some(packet_info))
    }

    #[cfg(target_os = "linux")]
    fn wrap(fd: OwnedFd, packet_info: Option<bool>) -> io::Result<Self> {
        set_nonblocking(fd.as_raw_fd())?;
        let vnet_hdr_len = match crate::linux::interface::tun_framing(fd.as_raw_fd()) {
            Ok(vnet_hdr_len) => vnet_hdr_len,
            // Non-TUN descriptors (for example a plain Unix datagram socket)
            // fail `TUNGETIFF` with `ENOTTY`; they never carry a virtio-net
            // header or packet-information prefix, so treat them as plain
            // packet queues.
            Err(err) if err.raw_os_error() == Some(libc::ENOTTY) => 0,
            Err(err) => return Err(err),
        };
        let packet_info = packet_info
            .or_else(|| crate::linux::interface::packet_info_from_sysfs(fd.as_raw_fd()))
            .unwrap_or(false);
        validate_framing(vnet_hdr_len, packet_info)?;
        // SAFETY: ownership is transferred to `TunIo`.
        let io = unsafe { TunIo::from_raw_fd(fd.into_raw_fd()) };
        // SAFETY: `io` owns a valid, open, non-blocking descriptor and keeps
        // it open for the lifetime of the `AsyncFd`. Tokio 1.54 deprecates the
        // safe `AsyncFd::new` in favor of `register` (tokio-rs/tokio#8587).
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io, vnet_hdr_len, packet_info))
    }

    #[cfg(not(target_os = "linux"))]
    fn wrap(fd: OwnedFd) -> io::Result<Self> {
        set_nonblocking(fd.as_raw_fd())?;
        #[cfg(target_os = "freebsd")]
        let io = {
            let header = crate::freebsd::io::header_enabled(fd.as_raw_fd());
            if !header && crate::freebsd::io::vnet_header_enabled(fd.as_raw_fd()) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "FreeBSD TAP virtio-net headers are not supported",
                ));
            }
            // A descriptor may have been handed off from another process. The
            // kernel still attributes the TUN device to the opener until this
            // ioctl runs; TAP rejects it and the failure is harmless.
            crate::freebsd::io::set_owner_pid(fd.as_raw_fd());
            TunIo::new(fd, header)
        };
        #[cfg(not(target_os = "freebsd"))]
        // SAFETY: ownership is transferred to `TunIo`.
        let io = unsafe { TunIo::from_raw_fd(fd.into_raw_fd()) };
        // SAFETY: `io` owns a valid, open, non-blocking descriptor and keeps
        // it open for the lifetime of the `AsyncFd`. Tokio 1.54 deprecates the
        // safe `AsyncFd::new` in favor of `register` (tokio-rs/tokio#8587).
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io))
    }

    #[cfg(target_os = "freebsd")]
    pub(crate) fn from_freebsd_io(io: TunIo) -> io::Result<Self> {
        set_nonblocking(io.as_raw_fd())?;
        // SAFETY: `io` owns a valid, open, non-blocking descriptor and keeps
        // it open for the lifetime of the `AsyncFd`. Tokio 1.54 deprecates the
        // safe `AsyncFd::new` in favor of `register` (tokio-rs/tokio#8587).
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io))
    }

    /// Constructs a queue with an explicit virtio-net header size, bypassing
    /// the `TUNGETVNETHDRSZ` probe.
    ///
    /// Test-only: real queues discover the header size from the descriptor, so
    /// unit tests inject the value to exercise the offload paths without a
    /// kernel TUN device.
    #[cfg(all(target_os = "linux", test))]
    fn with_forced_vnet_hdr_len(io: OwnedFd, vnet_hdr_len: usize) -> io::Result<Self> {
        set_nonblocking(io.as_raw_fd())?;
        // SAFETY: ownership is transferred to `TunIo`.
        let io = unsafe { TunIo::from_raw_fd(io.into_raw_fd()) };
        // SAFETY: `io` owns a valid, open, non-blocking descriptor and keeps
        // it open for the lifetime of the `AsyncFd`.
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io, vnet_hdr_len, false))
    }

    /// Wraps a raw TUN/TAP queue descriptor, taking ownership of it.
    ///
    /// The descriptor is set to non-blocking mode and closed if wrapping
    /// fails. See [`TunQueue::new`] for the shared `O_NONBLOCK` caveat.
    /// Like Tokio's own `from_raw_fd` conversions, the caller must pass a
    /// pollable descriptor; wrapping a regular file will stall other tasks
    /// sharing the I/O driver.
    ///
    /// # Safety
    ///
    /// The caller must own `fd` and transfer that ownership to the returned
    /// `TunQueue`. The descriptor must remain open and valid.
    ///
    /// # Panics
    ///
    /// This method must be called from within a Tokio runtime with I/O
    /// enabled; otherwise Tokio panics during registration.
    pub unsafe fn from_raw_fd(fd: RawFd) -> io::Result<Self> {
        // SAFETY: the caller guarantees ownership of `fd`.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Self::new(fd)
    }

    /// Receives one packet from the queue.
    ///
    /// The returned [`Packet`] borrows `buf`; no payload copy is made. On Linux
    /// with offload enabled, the virtio-net header is parsed and available via
    /// `Packet::offload_header`; the payload starts at the beginning of the
    /// returned packet. On other platforms the packet is the payload only.
    ///
    /// A zero-length buffer returns an empty packet without consuming anything.
    /// An empty payload (zero bytes received) is the device-closed signal: a
    /// `utun`/TUN control connection that has terminated reports EOF this way,
    /// and `recv` returns an empty [`Packet`] on every subsequent read. Callers
    /// must stop reading after observing it.
    ///
    /// On Linux with `packet_info` enabled, the payload includes the kernel's
    /// 4-byte `tun_pi` prefix; see `TunQueue::packet_info`.
    /// On Android/iOS and FreeBSD TAP devices, a packet larger than `buf` may be
    /// silently truncated by the native `read(2)`. FreeBSD TUN and macOS `utun`
    /// report truncation as an error. On Linux with offload enabled, the packet
    /// API detects truncation and returns an error when the frame does not fit;
    /// the frame is consumed by `readv` even though only an error is reported,
    /// and on macOS/FreeBSD the oversize frame is likewise not preserved.
    ///
    /// # Cancellation and concurrency
    ///
    /// The returned future is cancel-safe: dropping it before the native read
    /// completes does not consume a packet. If the future is dropped after the
    /// kernel read has completed but before the caller observes the result, the
    /// packet may be lost. Concurrent callers each receive a distinct packet.
    pub async fn recv<'a>(&self, buf: &'a mut [u8]) -> io::Result<Packet<'a>> {
        if buf.is_empty() {
            return Ok(Packet::new(&buf[..0]));
        }
        self.check_offload_buffer(buf.len())?;
        let len = self.recv_raw(buf).await?;
        self.packet_from_raw(buf, len)
    }

    async fn recv_raw(&self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(target_os = "linux")]
        if self.1 == VIRTIO_NET_HDR_LEN {
            loop {
                let mut guard = self.0.readable().await?;
                match guard.try_io(|inner| inner.get_ref().recv_checked(buf)) {
                    Ok(res) => return res,
                    Err(_) => continue,
                }
            }
        }
        loop {
            let mut guard = self.0.readable().await?;
            match guard.try_io(|inner| inner.get_ref().recv(buf)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    fn packet_from_raw<'a>(&self, buf: &'a [u8], len: usize) -> io::Result<Packet<'a>> {
        if len == 0 {
            // Zero bytes received means the device terminated the
            // connection; the empty packet is the documented EOF signal.
            return Ok(Packet::new(&buf[..0]));
        }
        #[cfg(target_os = "linux")]
        if self.1 == VIRTIO_NET_HDR_LEN {
            if len < VIRTIO_NET_HDR_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "offload packet is shorter than the virtio-net header",
                ));
            }
            let header = VirtioNetHeader::decode(&buf[..VIRTIO_NET_HDR_LEN])?;
            return Ok(Packet::with_offload(&buf[VIRTIO_NET_HDR_LEN..len], header));
        }
        Ok(Packet::new(&buf[..len]))
    }

    #[cfg(target_os = "linux")]
    fn check_offload_buffer(&self, buf_len: usize) -> io::Result<()> {
        match self.1 {
            0 => Ok(()),
            VIRTIO_NET_HDR_LEN if buf_len >= VIRTIO_NET_HDR_LEN => Ok(()),
            VIRTIO_NET_HDR_LEN => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "offload receive buffer is smaller than the virtio-net header",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "unsupported virtio-net header size",
            )),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn check_offload_buffer(&self, _buf_len: usize) -> io::Result<()> {
        Ok(())
    }

    /// Sends exactly one packet to the queue.
    ///
    /// Accepts a [`Packet`] or a `&[u8]` payload. On Linux with offload enabled,
    /// a [`Packet`] carrying an offload header is written with that header; a
    /// payload-only packet is written with a zero virtio-net header. On Linux
    /// with `packet_info` enabled, the packet must include the 4-byte `tun_pi`
    /// prefix. On other platforms the payload is written unchanged.
    ///
    /// Returns `Ok(())` only when the entire packet was accepted by the native
    /// interface. A short native write is reported as
    /// [`io::ErrorKind::WriteZero`]; the unsent remainder is not retried.
    /// Empty packets are a no-op in every mode, including when the queue has
    /// Linux offload enabled and the packet carries an offload header.
    ///
    /// # Cancellation and concurrency
    ///
    /// The returned future is cancel-safe: dropping it before the native write
    /// completes does not send a partial packet. If the future is dropped after
    /// the kernel write has completed but before the caller observes the
    /// result, the packet may still have been sent. Concurrent callers each send
    /// one packet.
    pub async fn send<'a, P>(&self, packet: P) -> io::Result<()>
    where
        P: Into<Packet<'a>>,
    {
        self.send_packet(packet.into()).await
    }

    async fn send_packet(&self, packet: Packet<'_>) -> io::Result<()> {
        // An empty payload is a no-op in every mode: sending a
        // header-only datagram would be malformed on every backend.
        if packet.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        let has_offload_header = packet.offload_header().is_some();
        #[cfg(target_os = "linux")]
        {
            match self.1 {
                0 => {
                    if has_offload_header {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "offload header provided but offload is not enabled",
                        ));
                    }
                    return self.send_raw(packet.payload()).await;
                }
                VIRTIO_NET_HDR_LEN => {
                    let mut encoded = [0u8; VIRTIO_NET_HDR_LEN];
                    packet
                        .offload_header()
                        .unwrap_or_default()
                        .encode(&mut encoded)?;
                    return self
                        .sendv_raw(&[IoSlice::new(&encoded), IoSlice::new(packet.payload())])
                        .await;
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )),
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.send_raw(packet.payload()).await
    }

    async fn send_raw(&self, packet: &[u8]) -> io::Result<()> {
        // Try the syscall once before waiting. Some packet devices (notably
        // FreeBSD TUN) do not reliably report writability, and immediate
        // errors such as EIO or ENOTCONN must not wait for readiness.
        match self.0.get_ref().send(packet) {
            Ok(written) => return check_full_write(written, packet.len()),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(err),
        }
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|inner| inner.get_ref().send(packet)) {
                Ok(res) => return res.and_then(|written| check_full_write(written, packet.len())),
                Err(_) => continue,
            }
        }
    }

    /// Sends one packet assembled from multiple payload slices.
    ///
    /// On Linux with offload enabled, the slices are payload-only and a zero
    /// virtio-net header is prepended. This method does not carry per-packet
    /// offload metadata; use [`send`](TunQueue::send) with a [`Packet`] to send
    /// a header received from [`recv`](TunQueue::recv). On Linux with
    /// `packet_info` enabled, the slices must include the 4-byte `tun_pi`
    /// prefix.
    ///
    /// Returns `Ok(())` only when every byte was accepted. A short write is
    /// reported as [`io::ErrorKind::WriteZero`] and is not retried. More than
    /// `IOV_MAX - 1` slices are rejected with [`io::ErrorKind::InvalidInput`].
    ///
    /// # Cancellation and concurrency
    ///
    /// The same cancellation and concurrency rules as [`send`](TunQueue::send)
    /// apply.
    pub async fn sendv(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        check_slice_count(packet.len())?;
        if total_len(packet)? == 0 {
            // An empty payload is a no-op in every mode: on an offload queue a
            // header-only write would be malformed.
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            match self.1 {
                0 => return self.sendv_raw(packet).await,
                VIRTIO_NET_HDR_LEN => {
                    let encoded = [0u8; VIRTIO_NET_HDR_LEN];
                    let mut slices = Vec::with_capacity(packet.len() + 1);
                    slices.push(IoSlice::new(&encoded));
                    slices.extend_from_slice(packet);
                    return self.sendv_raw(&slices).await;
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )),
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.sendv_raw(packet).await
    }

    async fn sendv_raw(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        let expected = total_len(packet)?;
        if expected == 0 {
            return Ok(());
        }
        // See `send_raw`: try the write first so a device that never reports
        // writability cannot hang the future.
        match self.0.get_ref().sendv(packet) {
            Ok(written) => return check_full_write(written, expected),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(err),
        }
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|inner| inner.get_ref().sendv(packet)) {
                Ok(res) => return res.and_then(|written| check_full_write(written, expected)),
                Err(_) => continue,
            }
        }
    }

    /// Tries to receive one packet without waiting.
    ///
    /// When there is no pending data, `Err(io::ErrorKind::WouldBlock)` is
    /// returned. On Linux with offload enabled, a frame that does not fit the
    /// buffer returns an error instead of being silently truncated.
    ///
    /// These calls read the descriptor directly; a caller that spins on
    /// `try_recv` against already-cached readiness may see one spurious
    /// `WouldBlock` before the next readiness wakeup. Prefer `recv` unless
    /// poll-only integration with non-Tokio readiness sources is needed.
    pub fn try_recv<'a>(&self, buf: &'a mut [u8]) -> io::Result<Packet<'a>> {
        if buf.is_empty() {
            return Ok(Packet::new(&buf[..0]));
        }
        self.check_offload_buffer(buf.len())?;
        let len = self.try_recv_raw(buf)?;
        self.packet_from_raw(buf, len)
    }

    fn try_recv_raw(&self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(target_os = "linux")]
        if self.1 == VIRTIO_NET_HDR_LEN {
            return self.0.get_ref().recv_checked(buf);
        }
        self.0.get_ref().recv(buf)
    }

    /// Tries to send one packet without waiting.
    ///
    /// Accepts a [`Packet`] or a `&[u8]` payload. When the socket buffer is
    /// full, `Err(io::ErrorKind::WouldBlock)` is returned. A short native write
    /// is reported as [`io::ErrorKind::WriteZero`] and is not retried. On Linux
    /// with `packet_info` enabled, the packet must include the 4-byte `tun_pi`
    /// prefix.
    pub fn try_send<'a, P>(&self, packet: P) -> io::Result<()>
    where
        P: Into<Packet<'a>>,
    {
        let packet = packet.into();
        // An empty payload is a no-op in every mode: sending a
        // header-only datagram would be malformed on every backend.
        if packet.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        let has_offload_header = packet.offload_header().is_some();
        #[cfg(target_os = "linux")]
        {
            match self.1 {
                0 => {
                    if has_offload_header {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "offload header provided but offload is not enabled",
                        ));
                    }
                    self.try_send_raw(packet.payload())
                }
                VIRTIO_NET_HDR_LEN => {
                    let mut encoded = [0u8; VIRTIO_NET_HDR_LEN];
                    packet
                        .offload_header()
                        .unwrap_or_default()
                        .encode(&mut encoded)?;
                    self.try_sendv_raw(&[IoSlice::new(&encoded), IoSlice::new(packet.payload())])
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )),
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.try_send_raw(packet.payload())
    }

    fn try_send_raw(&self, packet: &[u8]) -> io::Result<()> {
        self.0
            .get_ref()
            .send(packet)
            .and_then(|written| check_full_write(written, packet.len()))
    }

    /// Tries to send one packet assembled from multiple payload slices without
    /// waiting.
    ///
    /// On Linux with offload enabled, a zero virtio-net header is prepended.
    /// On Linux with `packet_info` enabled, the slices must include the 4-byte
    /// `tun_pi` prefix. When the socket buffer is full,
    /// `Err(io::ErrorKind::WouldBlock)` is returned. A short native write is
    /// reported as [`io::ErrorKind::WriteZero`] and is not retried.
    pub fn try_sendv(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        check_slice_count(packet.len())?;
        if total_len(packet)? == 0 {
            // An empty payload is a no-op in every mode: on an offload queue a
            // header-only write would be malformed.
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            match self.1 {
                0 => self.try_sendv_raw(packet),
                VIRTIO_NET_HDR_LEN => {
                    let encoded = [0u8; VIRTIO_NET_HDR_LEN];
                    let mut slices = Vec::with_capacity(packet.len() + 1);
                    slices.push(IoSlice::new(&encoded));
                    slices.extend_from_slice(packet);
                    self.try_sendv_raw(&slices)
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )),
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.try_sendv_raw(packet)
    }

    fn try_sendv_raw(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        let expected = total_len(packet)?;
        if expected == 0 {
            return Ok(());
        }
        self.0
            .get_ref()
            .sendv(packet)
            .and_then(|written| check_full_write(written, expected))
    }

    /// Returns the Linux virtio-net header length configured on the descriptor.
    ///
    /// Returns `0` when `IFF_VNET_HDR` is not enabled. The value is captured
    /// when the queue is created; changing offload state out-of-band after
    /// construction is not reflected.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn vnet_hdr_len(&self) -> usize {
        self.1
    }

    /// Returns whether the Linux descriptor carries the kernel's 4-byte
    /// `tun_pi` prefix (`IFF_NO_PI` is clear).
    ///
    /// When this is `true`, [`Packet::payload`](crate::Packet::payload)
    /// includes the prefix and `send`/`sendv` expect it in the packet. The
    /// value is captured when the queue is created.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn packet_info(&self) -> bool {
        self.2
    }

    /// Detaches this queue from its Linux multi-queue device.
    ///
    /// The descriptor remains open and can be reattached with
    /// [`attach`](TunQueue::attach). `TUNSETQUEUE` requires the device to have
    /// been created with [`TunBuilder::queues`](crate::TunBuilder::queues) and
    /// a kernel with multi-queue TUN support (Linux 3.8+).
    #[cfg(target_os = "linux")]
    pub fn detach(&self) -> io::Result<()> {
        crate::linux::interface::set_queue(self.as_raw_fd(), true)
    }

    /// Reattaches a previously detached Linux queue.
    ///
    /// See [`detach`](TunQueue::detach).
    #[cfg(target_os = "linux")]
    pub fn attach(&self) -> io::Result<()> {
        crate::linux::interface::set_queue(self.as_raw_fd(), false)
    }

    /// Sets the Linux carrier state of the device that owns this queue.
    ///
    /// This uses `TUNSETCARRIER` (Linux 4.9+) and requires `CAP_NET_ADMIN`.
    #[cfg(target_os = "linux")]
    pub fn set_carrier(&self, carrier: bool) -> io::Result<()> {
        crate::linux::interface::set_carrier(self.as_raw_fd(), carrier)
    }
}

impl AsRawFd for TunQueue {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl AsFd for TunQueue {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl IntoRawFd for TunQueue {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_inner().into_raw_fd()
    }
}

impl From<TunQueue> for OwnedFd {
    fn from(queue: TunQueue) -> Self {
        let raw = queue.into_raw_fd();
        // SAFETY: `queue` owned the descriptor and `into_raw_fd` transferred
        // ownership to us.
        unsafe { OwnedFd::from_raw_fd(raw) }
    }
}

/// Converts an owned descriptor into a `TunQueue`.
///
/// This is equivalent to [`TunQueue::new`]: ownership is transferred, the
/// descriptor is set to non-blocking mode, and it is closed if wrapping fails.
///
/// # Panics
///
/// This conversion must be called from within a Tokio runtime with I/O
/// enabled; otherwise Tokio panics during registration.
impl TryFrom<OwnedFd> for TunQueue {
    type Error = io::Error;

    fn try_from(fd: OwnedFd) -> io::Result<Self> {
        Self::new(fd)
    }
}

impl AsyncRead for TunQueue {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let self_mut = self.get_mut();
        #[cfg(target_os = "linux")]
        if self_mut.1 != 0 {
            if self_mut.1 != VIRTIO_NET_HDR_LEN {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )));
            }
            loop {
                let mut guard = ready!(self_mut.0.poll_read_ready_mut(cx))?;
                match guard.try_io(|inner| inner.get_mut().recv_payload(buf.initialize_unfilled()))
                {
                    Ok(Ok(n)) => {
                        buf.set_filled(buf.filled().len() + n);
                        return Poll::Ready(Ok(()));
                    }
                    Ok(Err(err)) => return Poll::Ready(Err(err)),
                    Err(_) => continue,
                }
            }
        }
        loop {
            let mut guard = ready!(self_mut.0.poll_read_ready_mut(cx))?;
            match guard.try_io(|inner| inner.get_mut().read(buf.initialize_unfilled())) {
                Ok(Ok(n)) => {
                    buf.set_filled(buf.filled().len() + n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for TunQueue {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let expected = buf.len();
        let self_mut = self.get_mut();
        #[cfg(target_os = "linux")]
        if self_mut.1 != 0 {
            if self_mut.1 != VIRTIO_NET_HDR_LEN {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )));
            }
            let header = [0u8; VIRTIO_NET_HDR_LEN];
            let slices = [IoSlice::new(&header), IoSlice::new(buf)];
            let expected = VIRTIO_NET_HDR_LEN.checked_add(buf.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "packet length overflow")
            })?;
            loop {
                let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
                match guard.try_io(|inner| {
                    let written = inner.get_mut().sendv(&slices)?;
                    check_full_write(written, expected).map(|()| written - VIRTIO_NET_HDR_LEN)
                }) {
                    Ok(Ok(written)) => return Poll::Ready(Ok(written)),
                    Ok(Err(err)) => return Poll::Ready(Err(err)),
                    Err(_) => continue,
                }
            }
        }
        loop {
            let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(Ok(written)) => {
                    return Poll::Ready(check_full_write(written, expected).map(|()| written));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::result::Result<usize, io::Error>> {
        if let Err(err) = check_slice_count(bufs.len()) {
            return Poll::Ready(Err(err));
        }
        let expected = match total_len(bufs) {
            Ok(expected) => expected,
            Err(err) => return Poll::Ready(Err(err)),
        };
        if expected == 0 {
            return Poll::Ready(Ok(0));
        }
        let self_mut = self.get_mut();
        #[cfg(target_os = "linux")]
        if self_mut.1 != 0 {
            if self_mut.1 != VIRTIO_NET_HDR_LEN {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported virtio-net header size",
                )));
            }
            let header = [0u8; VIRTIO_NET_HDR_LEN];
            let mut slices = Vec::with_capacity(bufs.len() + 1);
            slices.push(IoSlice::new(&header));
            slices.extend_from_slice(bufs);
            let expected = VIRTIO_NET_HDR_LEN.checked_add(expected).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "packet length overflow")
            })?;
            loop {
                let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
                match guard.try_io(|inner| {
                    let written = inner.get_mut().sendv(&slices)?;
                    check_full_write(written, expected).map(|()| written - VIRTIO_NET_HDR_LEN)
                }) {
                    Ok(Ok(written)) => return Poll::Ready(Ok(written)),
                    Ok(Err(err)) => return Poll::Ready(Err(err)),
                    Err(_) => continue,
                }
            }
        }
        loop {
            let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
            match guard.try_io(|inner| inner.get_mut().write_vectored(bufs)) {
                Ok(Ok(written)) => {
                    return Poll::Ready(check_full_write(written, expected).map(|()| written));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Poll::Ready(self.get_mut().0.get_mut().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Validates that a packet-oriented native write accepted the whole packet.
fn check_full_write(written: usize, expected: usize) -> io::Result<()> {
    if written == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            ErrorKind::WriteZero,
            format!("short packet write: wrote {written} of {expected} bytes"),
        ))
    }
}

fn check_slice_count(count: usize) -> io::Result<()> {
    if count > MAX_PACKET_SLICES {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("too many packet slices: {count} exceeds the limit of {MAX_PACKET_SLICES}"),
        ));
    }
    Ok(())
}

fn total_len(bufs: &[IoSlice<'_>]) -> io::Result<usize> {
    bufs.iter().try_fold(0usize, |total, slice| {
        total.checked_add(slice.len()).ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                "vectored packet length exceeds usize::MAX",
            )
        })
    })
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is an open descriptor and `flags` came from F_GETFL.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{Duration, timeout};

    fn socket_pair() -> (OwnedFd, OwnedFd) {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is writable for two descriptors and the arguments are
        // valid for an AF_UNIX datagram socket pair.
        let result =
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) };
        assert_eq!(
            result,
            0,
            "socketpair failed: {}",
            io::Error::last_os_error()
        );
        // SAFETY: `socketpair` returned two valid, owned descriptors.
        let left = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        // SAFETY: `socketpair` returned two valid, owned descriptors.
        let right = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        (left, right)
    }

    #[tokio::test]
    async fn wraps_existing_fds() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];

        left.send(&packet).await.unwrap();
        let mut buffer = [0u8; 32];
        let received = right.recv(&mut buffer).await.unwrap();
        assert_eq!(received.payload(), &packet);
    }

    #[tokio::test]
    async fn queue_async_read_write() {
        let (left, right) = socket_pair();
        let mut left = TunQueue::new(left).unwrap();
        let mut right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];

        left.write_all(&packet).await.unwrap();
        let mut buffer = [0u8; 32];
        right.read_exact(&mut buffer[..packet.len()]).await.unwrap();
        assert_eq!(&buffer[..packet.len()], &packet);
    }

    #[tokio::test]
    async fn sendv_accepts_empty_buffers() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let _right = TunQueue::new(right).unwrap();
        let buffers = [IoSlice::new(&[]), IoSlice::new(&[])];

        left.sendv(&buffers).await.unwrap();
    }

    #[tokio::test]
    async fn sendv_handles_mixed_empty_buffers() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];
        let buffers = [IoSlice::new(&[]), IoSlice::new(&packet), IoSlice::new(&[])];

        left.sendv(&buffers).await.unwrap();
        let mut buffer = [0u8; 32];
        let received = right.recv(&mut buffer).await.unwrap();
        assert_eq!(received.payload(), &packet);
    }

    #[tokio::test]
    async fn empty_send_is_a_noop() {
        let (left, _right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        left.send(&[]).await.unwrap();
        left.try_send(&[]).unwrap();
        left.sendv(&[]).await.unwrap();
        left.try_sendv(&[]).unwrap();
    }

    #[tokio::test]
    async fn empty_recv_does_not_consume_packet() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];

        left.send(&packet).await.unwrap();
        let mut empty = [];
        assert!(
            timeout(Duration::from_secs(1), right.recv(&mut empty))
                .await
                .expect("empty recv must not wait")
                .unwrap()
                .is_empty()
        );

        let mut buffer = [0u8; 32];
        let received = timeout(Duration::from_secs(1), right.recv(&mut buffer))
            .await
            .expect("packet was consumed by empty recv")
            .unwrap();
        assert_eq!(received.payload(), &packet);
    }

    #[tokio::test]
    async fn empty_async_read_does_not_consume_packet() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let mut right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];

        left.send(&packet).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), right.read(&mut []))
                .await
                .expect("empty AsyncRead must not wait")
                .unwrap(),
            0
        );

        let mut buffer = [0u8; 32];
        let received = timeout(Duration::from_secs(1), right.recv(&mut buffer))
            .await
            .expect("packet was consumed by empty AsyncRead")
            .unwrap();
        assert_eq!(received.payload(), &packet);
    }

    #[test]
    fn rejects_short_packet_writes() {
        let err = check_full_write(7, 10).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert!(err.to_string().contains("wrote 7 of 10 bytes"));
        check_full_write(10, 10).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validates_linux_framing() {
        validate_framing(0, false).unwrap();
        validate_framing(0, true).unwrap();
        validate_framing(VIRTIO_NET_HDR_LEN, false).unwrap();
        assert_eq!(
            validate_framing(VIRTIO_NET_HDR_LEN, true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            validate_framing(12, false).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[tokio::test]
    async fn rejects_too_many_vectored_slices() {
        let (left, _right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let slices = vec![IoSlice::new(&[]); MAX_PACKET_SLICES + 1];
        let err = left.try_sendv(&slices).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("too many packet slices"));
    }

    #[tokio::test]
    async fn transfers_ownership_through_owned_fd() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();

        let fd: OwnedFd = left.into();
        let left = TunQueue::try_from(fd).unwrap();
        let packet = [0x45u8; 20];
        left.send(&packet).await.unwrap();

        let mut buffer = [0u8; 32];
        let received = right.recv(&mut buffer).await.unwrap();
        assert_eq!(received.payload(), &packet);
    }

    #[tokio::test]
    async fn transfers_ownership_through_raw_fd() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();

        let raw = left.into_raw_fd();
        // SAFETY: ownership of `raw` was transferred to us by `into_raw_fd`.
        let left = unsafe { TunQueue::from_raw_fd(raw) }.unwrap();
        let packet = [0x60u8; 40];
        left.send(&packet).await.unwrap();

        let mut buffer = [0u8; 64];
        let received = right.recv(&mut buffer).await.unwrap();
        assert_eq!(received.payload(), &packet);
    }

    /// Exercises the Linux offload paths without a kernel TUN device by
    /// injecting the virtio-net header size into ordinary socket pairs.
    #[cfg(target_os = "linux")]
    mod offload {
        use super::*;
        use crate::offload::{VIRTIO_NET_HDR_LEN, VirtioNetHeader};

        /// Writes a raw datagram straight into the socket, bypassing the
        /// queue API, to build exact wire frames.
        fn raw_write(fd: &OwnedFd, frame: &[u8]) {
            // SAFETY: `fd` is an open descriptor and `frame` is valid for
            // reads of its length.
            let written =
                unsafe { libc::write(fd.as_raw_fd(), frame.as_ptr().cast(), frame.len()) };
            assert_eq!(written, frame.len() as isize);
        }

        fn framed(header: &VirtioNetHeader, payload: &[u8]) -> (Vec<u8>, [u8; VIRTIO_NET_HDR_LEN]) {
            let mut encoded = [0u8; VIRTIO_NET_HDR_LEN];
            header.encode(&mut encoded).unwrap();
            let mut frame = encoded.to_vec();
            frame.extend_from_slice(payload);
            (frame, encoded)
        }

        #[tokio::test]
        async fn rejects_offload_header_without_offload() {
            let (left, _right) = socket_pair();
            let left = TunQueue::new(left).unwrap();
            let payload = [0x45u8; 20];

            let packet = Packet::with_offload(&payload, VirtioNetHeader::default());
            let err = left.send(packet).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(err.to_string().contains("offload is not enabled"));

            let packet = Packet::with_offload(&payload, VirtioNetHeader::default());
            let err = left.try_send(packet).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }

        #[tokio::test]
        async fn rejects_receive_buffer_smaller_than_header() {
            let (left, right) = socket_pair();
            let (frame, _) = framed(&VirtioNetHeader::default(), &[0x45u8; 20]);
            raw_write(&left, &frame);

            let right = TunQueue::with_forced_vnet_hdr_len(right, VIRTIO_NET_HDR_LEN).unwrap();
            let mut buffer = [0u8; VIRTIO_NET_HDR_LEN - 1];
            let err = right.recv(&mut buffer).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(
                err.to_string()
                    .contains("smaller than the virtio-net header")
            );
            // The datagram is still queued; the error came from validation.
            let mut full = [0u8; 64];
            let received = right.recv(&mut full).await.unwrap();
            assert_eq!(received.payload(), &frame[VIRTIO_NET_HDR_LEN..]);
        }

        #[tokio::test]
        async fn parses_offload_header_and_payload() {
            let (left, right) = socket_pair();
            let header = VirtioNetHeader::default();
            let payload = [0x45u8; 20];
            let (frame, encoded) = framed(&header, &payload);
            raw_write(&left, &frame);

            let right = TunQueue::with_forced_vnet_hdr_len(right, VIRTIO_NET_HDR_LEN).unwrap();
            let mut buffer = [0u8; 64];
            let received = right.recv(&mut buffer).await.unwrap();
            assert_eq!(received.payload(), &payload);
            assert_eq!(received.offload_header(), Some(header));
            // The `encoded` bytes round-trip through the wire format.
            let decoded = received.offload_header().unwrap();
            let mut out = [0u8; VIRTIO_NET_HDR_LEN];
            decoded.encode(&mut out).unwrap();
            assert_eq!(out, encoded);
        }

        #[tokio::test]
        async fn truncation_error_consumes_the_frame() {
            let (left, right) = socket_pair();
            // One byte more than a 64-byte receive buffer can hold.
            let payload = vec![0x45u8; 64 + 1 - VIRTIO_NET_HDR_LEN];
            let (frame, _) = framed(&VirtioNetHeader::default(), &payload);
            raw_write(&left, &frame);

            let right = TunQueue::with_forced_vnet_hdr_len(right, VIRTIO_NET_HDR_LEN).unwrap();
            let mut buffer = [0u8; 64];
            let err = right.recv(&mut buffer).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            assert!(err.to_string().contains("truncated"));

            // The oversize frame was consumed; nothing remains on the socket.
            let err = right.try_recv(&mut buffer).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        }

        #[tokio::test]
        async fn empty_packets_are_noops_on_offload_queue() {
            let (left, right) = socket_pair();
            let left = TunQueue::with_forced_vnet_hdr_len(left, VIRTIO_NET_HDR_LEN).unwrap();
            let right = TunQueue::with_forced_vnet_hdr_len(right, VIRTIO_NET_HDR_LEN).unwrap();

            left.send(&[]).await.unwrap();
            left.try_send(&[]).unwrap();
            let empty: &[u8] = &[];
            left.send(Packet::with_offload(empty, VirtioNetHeader::default()))
                .await
                .unwrap();
            left.try_sendv(&[IoSlice::new(&[]), IoSlice::new(&[])])
                .unwrap();

            // No malformed header-only datagram may have been written.
            let mut buffer = [0u8; 64];
            let err = right.try_recv(&mut buffer).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        }

        #[tokio::test]
        async fn empty_datagram_is_the_eof_signal() {
            let (left, right0) = socket_pair();
            let (left2, right2) = socket_pair();
            let right = TunQueue::new(right0).unwrap();
            let mut right_async = TunQueue::new(right2).unwrap();

            // Zero-length writes enqueue empty datagrams on an AF_UNIX
            // datagram socket pair.
            assert_eq!(
                unsafe { libc::write(left.as_raw_fd(), std::ptr::null(), 0) },
                0
            );
            assert_eq!(
                unsafe { libc::write(left2.as_raw_fd(), std::ptr::null(), 0) },
                0
            );

            // The packet API surfaces zero bytes as an empty packet.
            let mut buffer = [0u8; 64];
            let received = timeout(Duration::from_secs(1), right.recv(&mut buffer))
                .await
                .expect("must not wait")
                .unwrap();
            assert!(received.is_empty());
            assert!(received.offload_header().is_none());

            // `AsyncRead` surfaces the same condition as EOF.
            let n = timeout(Duration::from_secs(1), right_async.read(&mut buffer))
                .await
                .expect("must not wait")
                .unwrap();
            assert_eq!(n, 0);
        }
    }
}
