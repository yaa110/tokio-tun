use crate::Result;
use crate::TunBuilder;
use crate::TunQueue;
use crate::address::AddressInfo;
#[cfg(target_os = "freebsd")]
use crate::freebsd::interface::Interface;
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
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(target_os = "linux")]
static TUN: &[u8] = b"/dev/net/tun\0";

#[cfg(target_os = "linux")]
fn close_fds(fds: &[i32]) {
    for &fd in fds {
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
    pub fn builder() -> TunBuilder {
        TunBuilder::new()
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new(params: Params) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let persist = params.persist;
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
                )
                .into());
            }
        };
        #[cfg(target_os = "freebsd")]
        let io = TunQueue::from_freebsd_io(TunIo::new(fd, iface.is_tun()))?;
        #[cfg(not(target_os = "freebsd"))]
        let io = TunQueue::new(fd)?;
        let tun = Self {
            iface: Arc::new(iface),
            io,
        };
        #[cfg(target_os = "linux")]
        if persist && let Err(err) = set_persist(tun.as_raw_fd(), true) {
            let _ = set_persist(tun.as_raw_fd(), false);
            return Err(err);
        }
        Ok(tun)
    }

    /// Creates a new instance of Tun/Tap device.
    #[cfg(target_os = "linux")]
    pub(crate) fn new_mq(params: Params, queues: usize) -> Result<Vec<Self>> {
        if queues == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "queues must be at least 1",
            )
            .into());
        }
        #[cfg(target_os = "linux")]
        let persist = params.persist;
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
        let iface = Arc::new(iface);
        for fd in fds {
            let io = TunQueue::new(fd)?;
            tuns.push(Self {
                iface: iface.clone(),
                io,
            });
        }
        #[cfg(target_os = "linux")]
        if persist {
            for tun in &tuns {
                if let Err(err) = set_persist(tun.as_raw_fd(), true) {
                    for tun in &tuns {
                        let _ = set_persist(tun.as_raw_fd(), false);
                    }
                    return Err(err);
                }
            }
        }
        Ok(tuns)
    }

    #[cfg(target_os = "linux")]
    fn allocate(params: Params, queues: Option<usize>) -> Result<Interface> {
        let extra_flags = if params.cloexec { libc::O_CLOEXEC } else { 0 };
        let count = queues.unwrap_or(1);
        let multi_queue = queues.is_some();

        let mut fds = Vec::with_capacity(count);
        for _ in 0..count {
            let fd = unsafe {
                libc::open(
                    TUN.as_ptr().cast::<c_char>(),
                    libc::O_RDWR | libc::O_NONBLOCK | extra_flags,
                )
            };
            if fd < 0 {
                let err = io::Error::last_os_error();
                close_fds(&fds);
                return Err(err.into());
            }
            fds.push(fd);
        }

        let iface = Interface::new(
            fds,
            params.name.as_deref().unwrap_or_default(),
            params.flags,
            multi_queue,
            params.cloexec,
        )?;
        iface.init(params)?;
        Ok(iface)
    }

    #[cfg(target_os = "freebsd")]
    fn allocate(params: Params, queues: Option<usize>) -> Result<Interface> {
        if queues.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "multi-queue is not supported on FreeBSD",
            )
            .into());
        }
        let iface = Interface::new(&params)?;
        iface.init(&params)?;
        Ok(iface)
    }

    #[cfg(target_os = "macos")]
    fn allocate(params: Params, queues: Option<usize>) -> Result<Interface> {
        if queues.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "multi-queue is not supported on macOS",
            )
            .into());
        }

        let iface = Interface::new(&params)?;
        iface.init(&params)?;
        Ok(iface)
    }

    /// Receives a packet from the Tun/Tap interface.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.recv(buf).await
    }

    /// Sends a buffer to the Tun/Tap interface. Returns the number of bytes written to the device.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.send(buf).await
    }

    /// Sends all of a buffer to the Tun/Tap interface.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        self.io.send_all(buf).await
    }

    /// Sends vectored buffers to the Tun/Tap interface. Returns the number of bytes written to the device.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.io.sendv(bufs).await
    }

    /// Attempts to write an entire vectored buffer to this writer.
    ///
    /// This method will continuously call `sendv` until all data has been
    /// written or an error occurs. This method will not return until all data has
    /// been written.
    ///
    /// This implementation advances the buffers in place as data is written.
    ///
    /// # Errors
    ///
    /// This function will return the first error that `sendv` returns.
    pub async fn sendv_all(&self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.io.sendv_all(bufs).await
    }

    /// Tries to receive a buffer from the Tun/Tap interface.
    ///
    /// When there is no pending data, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.try_recv(buf)
    }

    /// Tries to send a packet to the Tun/Tap interface.
    ///
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.try_send(buf)
    }

    /// Tries to send several different buffers to the Tun/Tap interface.
    ///
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.io.try_sendv(bufs)
    }

    /// Returns the name of Tun/Tap device.
    pub fn name(&self) -> &str {
        self.iface.name()
    }

    /// Returns the interface index of the Tun/Tap device.
    pub fn index(&self) -> Result<u32> {
        self.iface.index()
    }

    /// Returns the value of MTU.
    pub fn mtu(&self) -> Result<i32> {
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
    pub fn addresses(&self) -> Result<Vec<AddressInfo>> {
        self.iface.addresses()
    }

    /// Returns the primary address assigned to the device.
    ///
    /// The first IPv4 address reported by the system is returned when one
    /// exists; otherwise the first reported address (which may be IPv6) is
    /// returned. Use [`addresses`](Tun::addresses) to retrieve every address.
    pub fn address(&self) -> Result<IpAddr> {
        let addresses = self.addresses()?;
        addresses
            .iter()
            .find(|info| info.address.is_ipv4())
            .or_else(|| addresses.first())
            .map(|info| info.address)
            .ok_or_else(|| {
                io::Error::new(ErrorKind::AddrNotAvailable, "interface has no address").into()
            })
    }

    /// Returns the first point-to-point destination address of the device.
    ///
    /// The result is the first peer reported by the system, not necessarily
    /// the first one configured. On macOS, IPv6 point-to-point destinations
    /// are not reported by `getifaddrs`, so IPv6-only setups may return
    /// [`AddrNotAvailable`](std::io::ErrorKind::AddrNotAvailable).
    pub fn destination(&self) -> Result<IpAddr> {
        self.addresses()?
            .iter()
            .find_map(|info| info.destination)
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::AddrNotAvailable,
                    "interface has no destination address",
                )
                .into()
            })
    }

    /// Returns the IPv4 broadcast address of the device.
    #[cfg(target_os = "linux")]
    pub fn broadcast(&self) -> Result<Ipv4Addr> {
        self.iface.broadcast()
    }

    /// Returns the MAC address of the TAP device.
    ///
    /// TUN devices have no hardware address, so this returns an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) error for them.
    #[cfg(target_os = "linux")]
    pub fn mac_address(&self) -> Result<[u8; 6]> {
        self.iface.mac_address()
    }

    /// Sets the MAC address of the TAP device.
    ///
    /// The address must be a non-zero unicast address. TUN devices have no
    /// hardware address and return an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) error.
    #[cfg(target_os = "linux")]
    pub fn set_mac_address(&self, mac: [u8; 6]) -> Result<()> {
        self.iface.set_mac_address(mac)
    }

    /// Returns the IPv4 netmask of the device.
    pub fn netmask(&self) -> Result<Ipv4Addr> {
        self.iface.netmask()
    }

    /// Returns the flags of MTU.
    pub fn flags(&self) -> Result<i16> {
        self.iface.flags(None)
    }
}

impl From<Tun> for TunQueue {
    fn from(tun: Tun) -> Self {
        tun.io
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
        socket_pair_tun_with_sndbuf(None)
    }

    fn socket_pair_tun_with_sndbuf(sndbuf: Option<libc::c_int>) -> (Tun, UnixStream) {
        let (writer, reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        if let Some(sndbuf) = sndbuf {
            let rc = unsafe {
                libc::setsockopt(
                    writer.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &sndbuf as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&sndbuf) as libc::socklen_t,
                )
            };
            assert_eq!(
                rc,
                0,
                "setsockopt(SO_SNDBUF) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let io = TunQueue::new(writer).unwrap();
        // No TUN device is configured by this fixture. The empty interface is
        // only metadata; I/O uses a non-fsyncable socket pair.
        let iface = Arc::new(Interface::new(Vec::new(), "", 0, false, false).unwrap());
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

    #[tokio::test]
    async fn send_all_advances_after_partial_writes() {
        const PAYLOAD_LEN: usize = 1024 * 1024;

        // A tiny send buffer forces the non-blocking socket to accept only part
        // of each write, exercising the slice advancement in `send_all`.
        let (tun, mut reader) = socket_pair_tun_with_sndbuf(Some(4096));
        reader.set_nonblocking(true).unwrap();

        let payload: Vec<u8> = (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();

        let reader_task = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut received = Vec::with_capacity(expected.len());
            let mut buf = [0u8; 16 * 1024];
            while received.len() < expected.len() && std::time::Instant::now() < deadline {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => received.extend_from_slice(&buf[..n]),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(err) => panic!("read failed: {err}"),
                }
            }
            received
        });

        timeout(Duration::from_secs(5), tun.send_all(&payload))
            .await
            .expect("send_all timed out")
            .expect("send_all failed");

        let received = reader_task.await.expect("reader task panicked");
        assert_eq!(received.len(), payload.len());
        assert_eq!(received, payload);
    }
}
