use crate::Packet;
use crate::TunBuilder;
use crate::TunQueue;
use crate::address::AddressInfo;
#[cfg(target_os = "freebsd")]
use crate::freebsd::interface::{Interface, set_persist as set_freebsd_persist};
#[cfg(target_os = "freebsd")]
use crate::freebsd::io::TunIo;
#[cfg(target_os = "freebsd")]
use crate::freebsd::params::Params;
#[cfg(target_os = "linux")]
use crate::linux::interface::{Interface, set_persist};
#[cfg(target_os = "linux")]
use crate::linux::params::Params;
#[cfg(target_os = "macos")]
use crate::macos::interface::Interface;
#[cfg(target_os = "macos")]
use crate::macos::params::Params;
use std::io::{self, ErrorKind, IoSlice};
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::os::raw::c_char;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(target_os = "linux")]
static TUN: &[u8] = b"/dev/net/tun\0";

#[cfg(target_os = "linux")]
fn close_fds(fds: &[i32]) {
    for &fd in fds {
        // SAFETY: each descriptor is owned by the caller on this error path.
        unsafe { libc::close(fd) };
    }
}

/// Represents a Tun/Tap device. Use [`TunBuilder`](struct.TunBuilder.html) to create a new instance of [`Tun`](struct.Tun.html).
///
/// Convert a `Tun` into a [`TunQueue`] to pass the underlying descriptor to
/// another process. The conversion discards the interface accessors, so read
/// any interface metadata you need first.
pub struct Tun {
    iface: Arc<Interface>,
    io: TunQueue,
}

impl std::fmt::Debug for Tun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tun")
            .field("name", &self.name())
            .field("fd", &self.as_raw_fd())
            .finish()
    }
}

impl AsRawFd for Tun {
    fn as_raw_fd(&self) -> RawFd {
        self.io.as_raw_fd()
    }
}

impl AsFd for Tun {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.io.as_fd()
    }
}

impl AsyncRead for Tun {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tun {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::result::Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

impl Tun {
    /// Creates a new [`TunBuilder`] with default settings.
    pub fn builder() -> TunBuilder {
        TunBuilder::new()
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new(params: Params) -> io::Result<Self> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let persist = params.persist;
        #[cfg(target_os = "linux")]
        let packet_info = params.packet_info;
        let mut iface = Self::allocate(params, None)?;
        let mut fds: Vec<OwnedFd> = iface
            .take_files()
            .into_iter()
            .map(|fd| {
                // SAFETY: `take_files` transfers ownership of each descriptor.
                unsafe { OwnedFd::from_raw_fd(fd) }
            })
            .collect();
        let fd = match fds.pop() {
            Some(fd) if fds.is_empty() => fd,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected exactly one file descriptor",
                ));
            }
        };
        #[cfg(target_os = "freebsd")]
        let io = TunQueue::from_freebsd_io(TunIo::new(fd, iface.is_tun()))?;
        #[cfg(target_os = "linux")]
        let io = TunQueue::new_with_packet_info(fd, packet_info)?;
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let io = TunQueue::new(fd)?;
        let tun = Self {
            iface: Arc::new(iface),
            io,
        };
        #[cfg(target_os = "linux")]
        if persist && let Err(err) = set_persist(tun.as_raw_fd(), true) {
            // Do not compensate with a `TUNSETPERSIST(false)` write: the
            // descriptor is dropped when this error returns, which removes a
            // transient device, and an unconditional unset could clear
            // persistence that a previous owner configured.
            return Err(err);
        }
        #[cfg(target_os = "freebsd")]
        if persist {
            set_freebsd_persist(tun.as_raw_fd(), true)?;
        }
        Ok(tun)
    }

    /// Creates a new instance of Tun/Tap device.
    #[cfg(target_os = "linux")]
    pub(crate) fn new_mq(params: Params, queues: usize) -> io::Result<Vec<Self>> {
        if queues == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "queues must be at least 1",
            ));
        }
        #[cfg(target_os = "linux")]
        let persist = params.persist;
        #[cfg(target_os = "linux")]
        let packet_info = params.packet_info;
        let mut iface = Self::allocate(params, Some(queues))?;
        let fds: Vec<OwnedFd> = iface
            .take_files()
            .into_iter()
            .map(|fd| {
                // SAFETY: `take_files` transfers ownership of each descriptor.
                unsafe { OwnedFd::from_raw_fd(fd) }
            })
            .collect();
        let mut tuns = Vec::with_capacity(queues);
        let created = iface.created;
        let iface = Arc::new(iface);
        for fd in fds {
            let io = TunQueue::new_with_packet_info(fd, packet_info)?;
            tuns.push(Self {
                iface: iface.clone(),
                io,
            });
        }
        #[cfg(target_os = "linux")]
        if persist {
            for (index, tun) in tuns.iter().enumerate() {
                if let Err(err) = set_persist(tun.as_raw_fd(), true) {
                    // Persistence is device-wide. Rolling it back is only safe
                    // when this call created the device; an attached device
                    // may already have been persistent before this build.
                    if created {
                        for tun in &tuns[..index] {
                            let _ = set_persist(tun.as_raw_fd(), false);
                        }
                    }
                    return Err(err);
                }
            }
        }
        Ok(tuns)
    }

    #[cfg(target_os = "linux")]
    fn allocate(params: Params, queues: Option<usize>) -> io::Result<Interface> {
        let extra_flags = if params.cloexec { libc::O_CLOEXEC } else { 0 };
        let count = queues.unwrap_or(1);
        let multi_queue = queues.is_some();

        let mut fds = Vec::with_capacity(count);
        for _ in 0..count {
            // SAFETY: `TUN` is a static NUL-terminated path and the open flags
            // are valid. The descriptor is checked and owned on success.
            let fd = unsafe {
                libc::open(
                    TUN.as_ptr().cast::<c_char>(),
                    libc::O_RDWR | libc::O_NONBLOCK | extra_flags,
                )
            };
            if fd < 0 {
                let err = io::Error::last_os_error();
                close_fds(&fds);
                return Err(err);
            }
            fds.push(fd);
        }

        let iface = Interface::new(
            fds,
            params.name.as_deref().unwrap_or_default(),
            params.flags,
            multi_queue,
            params.offload,
            params.cloexec,
        )?;
        iface.init(params)?;
        Ok(iface)
    }

    #[cfg(target_os = "freebsd")]
    fn allocate(params: Params, queues: Option<usize>) -> io::Result<Interface> {
        if queues.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "multi-queue is not supported on FreeBSD",
            ));
        }
        let iface = Interface::new(&params)?;
        iface.init(&params)?;
        Ok(iface)
    }

    #[cfg(target_os = "macos")]
    fn allocate(params: Params, queues: Option<usize>) -> io::Result<Interface> {
        if queues.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "multi-queue is not supported on macOS",
            ));
        }

        let iface = Interface::new(&params)?;
        iface.init(&params)?;
        Ok(iface)
    }

    /// Receives one packet from the Tun/Tap interface.
    ///
    /// The returned [`Packet`] borrows `buf`. On Linux with offload enabled,
    /// the virtio-net header is parsed automatically and available via
    /// `Packet::offload_header`; the payload starts at the beginning of the
    /// packet. A zero-length buffer returns an empty packet without consuming
    /// anything. A zero-byte native read means the device closed: `recv`
    /// returns an empty [`Packet`] on every subsequent read, so stop reading
    /// after observing it.
    ///
    /// On Linux, Android, iOS, and FreeBSD TAP devices, a packet larger than
    /// `buf` may be silently truncated by the native `read(2)`. FreeBSD TUN and
    /// macOS `utun` report truncation as an error. Size receive buffers to at
    /// least the interface MTU; when Linux offload is enabled, add the value
    /// returned by `Tun::vnet_hdr_len` (Linux only) and allow 65,536 bytes for
    /// GSO packets that can exceed the MTU. TAP frames additionally need room
    /// for the 14-byte Ethernet header.
    ///
    /// On Linux with `packet_info` enabled, the payload includes the 4-byte
    /// `tun_pi` prefix; see `Tun::packet_info`.
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct. Each concurrent caller receives a distinct
    /// packet.
    ///
    /// `Tun` adds no packet path of its own; the semantics match
    /// [`TunQueue::recv`] exactly.
    pub async fn recv<'a>(&self, buf: &'a mut [u8]) -> io::Result<Packet<'a>> {
        self.io.recv(buf).await
    }

    /// Sends exactly one packet to the Tun/Tap interface.
    ///
    /// Accepts a [`Packet`] or a `&[u8]` payload. On Linux with offload
    /// enabled, a [`Packet`] carrying an offload header is written with that
    /// header; a payload-only packet is written with a zero virtio-net header.
    /// On Linux with `packet_info` enabled, the packet must include the 4-byte
    /// `tun_pi` prefix. On other platforms the payload is written unchanged.
    ///
    /// Returns `Ok(())` only when the entire packet was accepted by the native
    /// interface. A short native write is reported as
    /// [`io::ErrorKind::WriteZero`]; the unsent remainder is **not** retried,
    /// because doing so would turn the remainder into a second packet. Empty
    /// packets are a no-op.
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct. Each call writes one packet atomically.
    pub async fn send<'a, P>(&self, packet: P) -> io::Result<()>
    where
        P: Into<Packet<'a>>,
    {
        self.io.send(packet).await
    }

    /// Sends one packet assembled from multiple payload slices.
    ///
    /// The slices are written with a single native vectored write, so packet
    /// boundaries are preserved. On Linux with offload enabled, a zero
    /// virtio-net header is prepended. This method does not carry per-packet
    /// offload metadata; use [`send`](Tun::send) with a [`Packet`] to forward a
    /// header received from [`recv`](Tun::recv). On Linux with `packet_info`
    /// enabled, the slices must include the 4-byte `tun_pi` prefix.
    ///
    /// Returns `Ok(())` only when every byte was accepted. A short write is
    /// reported as [`io::ErrorKind::WriteZero`] and is not retried. More than
    /// `IOV_MAX - 1` slices are rejected with [`io::ErrorKind::InvalidInput`].
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct.
    pub async fn sendv(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        self.io.sendv(packet).await
    }

    /// Tries to receive one packet without waiting.
    ///
    /// When there is no pending data, `Err(io::ErrorKind::WouldBlock)` is
    /// returned. On Linux with offload enabled, the virtio-net header is parsed
    /// automatically; see [`recv`](Tun::recv).
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct.
    pub fn try_recv<'a>(&self, buf: &'a mut [u8]) -> io::Result<Packet<'a>> {
        self.io.try_recv(buf)
    }

    /// Tries to send one packet without waiting.
    ///
    /// Accepts a [`Packet`] or a `&[u8]` payload. When the socket buffer is
    /// full, `Err(io::ErrorKind::WouldBlock)` is returned. A short native write
    /// is reported as [`io::ErrorKind::WriteZero`] and is not retried.
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct.
    pub fn try_send<'a, P>(&self, packet: P) -> io::Result<()>
    where
        P: Into<Packet<'a>>,
    {
        self.io.try_send(packet)
    }

    /// Tries to send one payload-only packet assembled from multiple slices
    /// without waiting.
    ///
    /// On Linux with offload enabled, a zero virtio-net header is prepended.
    /// On Linux with `packet_info` enabled, the slices must include the 4-byte
    /// `tun_pi` prefix.
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is
    /// returned. A short native write is reported as
    /// [`io::ErrorKind::WriteZero`] and is not retried.
    ///
    /// This method takes `&self`, so it is possible to call it concurrently with
    /// other methods on this struct.
    pub fn try_sendv(&self, packet: &[IoSlice<'_>]) -> io::Result<()> {
        self.io.try_sendv(packet)
    }

    /// Returns the name of Tun/Tap device.
    pub fn name(&self) -> &str {
        self.iface.name()
    }

    /// Returns the interface index of the Tun/Tap device.
    ///
    /// The index is verified against the interface name. If the interface has
    /// been renamed or removed, an [`InvalidData`](std::io::ErrorKind::InvalidData)
    /// or OS error is returned instead of a stale index.
    pub fn index(&self) -> io::Result<u32> {
        self.iface.index()
    }

    /// Returns the Linux virtio-net header length, or `0` when offload is
    /// disabled.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn vnet_hdr_len(&self) -> usize {
        self.iface.vnet_hdr_len()
    }

    /// Returns whether the Linux device carries the kernel's 4-byte `tun_pi`
    /// packet-information prefix (`IFF_NO_PI` is clear).
    ///
    /// When this is `true`, [`Packet::payload`](crate::Packet::payload) includes
    /// the prefix and [`send`](Tun::send)/[`sendv`](Tun::sendv) expect it in the
    /// packet. The value is captured when the device is built.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn packet_info(&self) -> bool {
        self.io.packet_info()
    }

    /// Returns the Linux offload feature mask configured for this device.
    ///
    /// Returns `0` when offload is disabled, or when the device was built with
    /// [`TunBuilder::offload_features`] using mask `0`. Use
    /// [`vnet_hdr_len`](Tun::vnet_hdr_len) to distinguish the two cases: it is
    /// `VIRTIO_NET_HDR_LEN` whenever the virtio-net header is enabled.
    ///
    /// The mask is the value requested through [`TunBuilder::offload`] or
    /// [`TunBuilder::offload_features`]. This is **not** a kernel readback:
    /// Linux does not expose the effective offload mask through TUN. The
    /// builder rejects unsupported bits and invalid combinations, but callers
    /// must not assume that every requested bit is active on every kernel.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn offload_features(&self) -> u32 {
        self.iface.offload_features()
    }

    /// Sets the Linux carrier state of the device.
    ///
    /// This uses `TUNSETCARRIER` (Linux 4.9+) and requires `CAP_NET_ADMIN`. It
    /// is useful for simulating link up/down transitions. The operation affects
    /// the device, not just this queue.
    #[cfg(target_os = "linux")]
    pub fn set_carrier(&self, carrier: bool) -> std::io::Result<()> {
        crate::linux::interface::set_carrier(self.as_raw_fd(), carrier)
    }

    /// Detaches this queue from its Linux multi-queue device.
    ///
    /// See [`TunQueue::detach`](crate::TunQueue::detach). The interface
    /// accessors remain available because `Tun` still owns them.
    #[cfg(target_os = "linux")]
    pub fn detach(&self) -> io::Result<()> {
        self.io.detach()
    }

    /// Reattaches a previously detached Linux queue.
    ///
    /// See [`TunQueue::attach`](crate::TunQueue::attach).
    #[cfg(target_os = "linux")]
    pub fn attach(&self) -> io::Result<()> {
        self.io.attach()
    }

    /// Returns the value of MTU.
    pub fn mtu(&self) -> io::Result<i32> {
        self.iface.mtu(None)
    }

    /// Returns all IPv4 and IPv6 addresses assigned to the device.
    ///
    /// Each entry contains the address, its prefix length, and any configured
    /// point-to-point destination or IPv4 broadcast address. On macOS, IPv6
    /// point-to-point destinations are not reported by `getifaddrs`.
    ///
    /// This method performs synchronous platform-specific I/O (netlink on
    /// Linux, `getifaddrs` on macOS and FreeBSD) and should not be called from
    /// latency-sensitive async code.
    ///
    /// ```no_run
    /// # use tokio_tun::Tun;
    /// # fn show(tun: &Tun) {
    /// for address in tun.addresses().unwrap() {
    ///     println!(
    ///         "{} /{} destination={:?} broadcast={:?}",
    ///         address.address,
    ///         address.prefix_len,
    ///         address.destination,
    ///         address.broadcast,
    ///     );
    /// }
    /// # }
    /// ```
    pub fn addresses(&self) -> io::Result<Vec<AddressInfo>> {
        self.iface.addresses()
    }

    /// Returns the primary address assigned to the device.
    ///
    /// The first IPv4 address reported by the system is returned when one
    /// exists; otherwise the first reported address (which may be IPv6) is
    /// returned. Use [`addresses`](Tun::addresses) to retrieve every address.
    pub fn address(&self) -> io::Result<IpAddr> {
        let addresses = self.addresses()?;
        addresses
            .iter()
            .find(|info| info.address.is_ipv4())
            .or_else(|| addresses.first())
            .map(|info| info.address)
            .ok_or_else(|| io::Error::new(ErrorKind::AddrNotAvailable, "interface has no address"))
    }

    /// Returns the first point-to-point destination address of the device.
    ///
    /// The result is the first peer reported by the system, not necessarily
    /// the first one configured. On macOS, IPv6 point-to-point destinations
    /// are not reported by `getifaddrs`, so IPv6-only setups may return
    /// [`AddrNotAvailable`](std::io::ErrorKind::AddrNotAvailable).
    pub fn destination(&self) -> io::Result<IpAddr> {
        self.addresses()?
            .iter()
            .find_map(|info| info.destination)
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::AddrNotAvailable,
                    "interface has no destination address",
                )
            })
    }

    /// Returns the IPv4 broadcast address of the device.
    ///
    /// On FreeBSD this is available for TAP devices only and returns the
    /// primary/first IPv4 address's broadcast. Use
    /// [`addresses`](Tun::addresses) for per-address broadcast values.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn broadcast(&self) -> io::Result<Ipv4Addr> {
        self.iface.broadcast()
    }

    /// Returns the MAC address of the TAP device.
    ///
    /// TUN devices have no hardware address, so this returns an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) error for them. On
    /// FreeBSD this is the local/interface link address.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn mac_address(&self) -> io::Result<[u8; 6]> {
        self.iface.mac_address()
    }

    /// Sets the MAC address of the TAP device.
    ///
    /// The address must be a non-zero unicast address. TUN devices have no
    /// hardware address and return an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) error. On FreeBSD this
    /// changes the local/interface link address.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn set_mac_address(&self, mac: [u8; 6]) -> io::Result<()> {
        self.iface.set_mac_address(mac)
    }

    /// Returns the IPv4 netmask of the device.
    pub fn netmask(&self) -> io::Result<Ipv4Addr> {
        self.iface.netmask()
    }

    /// Returns the interface flags as a 32-bit mask.
    ///
    /// Linux and macOS expose 16-bit interface flags; FreeBSD stores them in
    /// two 16-bit halves, which are combined here. Compare against the
    /// platform's `libc::IFF_*` constants cast to `u32`.
    pub fn flags(&self) -> io::Result<u32> {
        self.iface.flags(None)
    }
}

impl From<Tun> for TunQueue {
    fn from(tun: Tun) -> Self {
        tun.io
    }
}

impl IntoRawFd for Tun {
    fn into_raw_fd(self) -> RawFd {
        self.io.into_raw_fd()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use tokio::io::AsyncWriteExt;
    use tokio::time::{Duration, timeout};

    fn socket_pair_tun() -> (Tun, UnixStream) {
        let (writer, reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let io = TunQueue::new(writer).unwrap();
        // No TUN device is configured by this fixture. The empty interface is
        // only metadata; I/O uses a non-fsyncable socket pair.
        let iface = Arc::new(Interface::new(Vec::new(), "", 0, false, None, false).unwrap());
        (Tun { iface, io }, reader)
    }

    #[tokio::test]
    async fn flush_direct_and_split() {
        let (mut tun, mut reader) = socket_pair_tun();
        timeout(Duration::from_secs(1), tun.flush())
            .await
            .unwrap()
            .unwrap();
        tun.write_all(b"first").await.unwrap();
        tun.flush().await.unwrap();
        let mut first = [0; 5];
        reader.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"first");

        let (_read_half, mut writer) = tokio::io::split(tun);
        writer.write_all(b"second").await.unwrap();
        writer.flush().await.unwrap();
        let mut second = [0; 6];
        reader.read_exact(&mut second).unwrap();
        assert_eq!(&second, b"second");
    }
}
