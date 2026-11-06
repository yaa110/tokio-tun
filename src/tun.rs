use crate::Result;
use crate::TunBuilder;
use crate::address::AddressInfo;
use crate::linux::interface::{Interface, set_persist};
use crate::linux::io::TunIo;
use crate::linux::params::Params;
use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::OwnedFd;
use std::os::raw::c_char;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

static TUN: &[u8] = b"/dev/net/tun\0";

fn close_fds(fds: &[i32]) {
    for &fd in fds {
        unsafe { libc::close(fd) };
    }
}

// Taken from the `futures` crate
macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

/// Represents a Tun/Tap device. Use [`TunBuilder`](struct.TunBuilder.html) to create a new instance of [`Tun`](struct.Tun.html).
pub struct Tun {
    iface: Arc<Interface>,
    io: AsyncFd<TunIo>,
}

impl AsRawFd for Tun {
    fn as_raw_fd(&self) -> RawFd {
        self.io.as_raw_fd()
    }
}

impl AsyncRead for Tun {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> task::Poll<io::Result<()>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.io.poll_read_ready_mut(cx))?;

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

impl AsyncWrite for Tun {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> task::Poll<io::Result<usize>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.io.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::result::Result<usize, io::Error>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.io.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write_vectored(bufs)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        // TunIo is unbuffered, so flushing must not wait for write readiness.
        Poll::Ready(self.get_mut().io.get_mut().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Tun {
    pub fn builder() -> TunBuilder {
        TunBuilder::new()
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new(params: Params) -> Result<Self> {
        let persist = params.persist;
        let mut iface = Self::allocate(params, 1)?;
        let mut fds: Vec<OwnedFd> = iface
            .take_files()
            .into_iter()
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .collect();
        let fd = fds.pop().expect("exactly one file descriptor");
        // SAFETY: `fd` is an open and valid file descriptor whose ownership is
        // transferred to `TunIo`, and it remains valid for the lifetime of
        // `AsyncFd`.
        let io = unsafe { AsyncFd::register(TunIo::from_raw_fd(fd.into_raw_fd())) }
            .map_err(io::Error::from)?;
        let tun = Self {
            iface: Arc::new(iface),
            io,
        };
        if persist && let Err(err) = set_persist(tun.as_raw_fd(), true) {
            let _ = set_persist(tun.as_raw_fd(), false);
            return Err(err);
        }
        Ok(tun)
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new_mq(params: Params, queues: usize) -> Result<Vec<Self>> {
        if queues == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "queues must be at least 1",
            )
            .into());
        }
        let persist = params.persist;
        let mut iface = Self::allocate(params, queues)?;
        let fds: Vec<OwnedFd> = iface
            .take_files()
            .into_iter()
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .collect();
        let mut tuns = Vec::with_capacity(queues);
        let iface = Arc::new(iface);
        for fd in fds {
            // SAFETY: `fd` is an open and valid file descriptor whose ownership
            // is transferred to `TunIo`, and it remains valid for the lifetime
            // of `AsyncFd`.
            let io = unsafe { AsyncFd::register(TunIo::from_raw_fd(fd.into_raw_fd())) }
                .map_err(io::Error::from)?;
            tuns.push(Self {
                iface: iface.clone(),
                io,
            });
        }
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

    fn allocate(params: Params, queues: usize) -> Result<Interface> {
        let extra_flags = if params.cloexec { libc::O_CLOEXEC } else { 0 };

        let mut fds = Vec::with_capacity(queues);
        for _ in 0..queues {
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
            params.cloexec,
        )?;
        iface.init(params)?;
        Ok(iface)
    }

    /// Receives a packet from the Tun/Tap interface.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.io.readable().await?;
            match guard.try_io(|inner| inner.get_ref().recv(buf)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Sends a buffer to the Tun/Tap interface. Returns the number of bytes written to the device.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.io.writable().await?;
            match guard.try_io(|inner| inner.get_ref().send(buf)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Sends all of a buffer to the Tun/Tap interface.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            match self.send(remaining).await? {
                0 => return Err(ErrorKind::WriteZero.into()),
                n => {
                    remaining = &remaining[n..];
                }
            }
        }
        Ok(())
    }

    /// Sends vectored buffers to the Tun/Tap interface. Returns the number of bytes written to the device.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub async fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        loop {
            let mut guard = self.io.writable().await?;
            match guard.try_io(|inner| inner.get_ref().sendv(bufs)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Attempts to write an entire vectored buffer to this writer.
    ///
    /// This method will continuously call `sendv` until all data has been
    /// written or an error occurs. This method will not return until all data has
    /// been written.
    ///
    /// This implementation provides optimal performance for high-throughput scenarios
    /// by advancing buffers in-place without allocations.
    ///
    /// # Errors
    ///
    /// This function will return the first error that `sendv` returns.
    pub async fn sendv_all(&self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        let mut bufs = bufs;
        while !bufs.is_empty() {
            match self.sendv(bufs).await? {
                0 => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                n => {
                    IoSlice::advance_slices(&mut bufs, n);
                }
            }
        }
        Ok(())
    }

    /// Tries to receive a buffer from the Tun/Tap interface.
    ///
    /// When there is no pending data, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.get_ref().recv(buf)
    }

    /// Tries to send a packet to the Tun/Tap interface.
    ///
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.get_ref().send(buf)
    }

    /// Tries to send several different buffers to the Tun/Tap interface.
    ///
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.io.get_ref().sendv(bufs)
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
    /// point-to-point destination or IPv4 broadcast address.
    ///
    /// This method performs a synchronous netlink dump and should not be called
    /// from latency-sensitive async code.
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
    /// the first one configured.
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
    pub fn broadcast(&self) -> Result<Ipv4Addr> {
        self.iface.broadcast()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;
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
        // SAFETY: `writer` is a valid open file descriptor that is converted into
        // a raw fd and owned by `TunIo` for the lifetime of `AsyncFd`.
        let io = unsafe { AsyncFd::register(TunIo::from_raw_fd(writer.into_raw_fd())) }.unwrap();
        // No TUN device is configured by this fixture. The empty interface is
        // only metadata; I/O uses a non-fsyncable socket pair.
        let iface = Arc::new(Interface::new(Vec::new(), "", 0, false).unwrap());
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
