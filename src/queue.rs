#[cfg(target_os = "freebsd")]
use crate::freebsd::io::TunIo;
#[cfg(target_os = "macos")]
use crate::header_io::TunIo;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
use crate::raw_io::TunIo;
use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::pin::Pin;
use std::task::{self, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

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
/// MTU; `recv` may silently truncate a larger packet on Linux, Android, iOS,
/// and FreeBSD.
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
pub struct TunQueue(AsyncFd<TunIo>);

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
    /// # Panics
    ///
    /// This method must be called from within a Tokio runtime with I/O
    /// enabled; otherwise Tokio panics during registration.
    pub fn new<T: Into<OwnedFd>>(io: T) -> io::Result<Self> {
        let fd = io.into();
        set_nonblocking(fd.as_raw_fd())?;
        #[cfg(target_os = "freebsd")]
        let io = {
            let header = crate::freebsd::io::header_enabled(fd.as_raw_fd());
            TunIo::new(fd, header)
        };
        #[cfg(not(target_os = "freebsd"))]
        // SAFETY: ownership is transferred to `TunIo`.
        let io = unsafe { TunIo::from_raw_fd(fd.into_raw_fd()) };
        // SAFETY: `io` is valid and non-blocking for the lifetime of `AsyncFd`.
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io))
    }

    #[cfg(target_os = "freebsd")]
    pub(crate) fn from_freebsd_io(io: TunIo) -> io::Result<Self> {
        set_nonblocking(io.as_raw_fd())?;
        // SAFETY: `io` owns a valid descriptor and remains open for the
        // lifetime of the `AsyncFd`.
        let io = unsafe { AsyncFd::register(io) }.map_err(io::Error::from)?;
        Ok(Self(io))
    }

    /// Wraps a raw TUN/TAP queue descriptor, taking ownership of it.
    ///
    /// The descriptor is set to non-blocking mode and closed if wrapping
    /// fails. See [`TunQueue::new`] for the shared `O_NONBLOCK` caveat.
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

    /// Receives a packet from the queue.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.readable().await?;
            match guard.try_io(|inner| inner.get_ref().recv(buf)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Sends a packet to the queue.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|inner| inner.get_ref().send(buf)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Sends all bytes in `buf` to the queue.
    pub async fn send_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            match self.send(remaining).await? {
                0 => return Err(ErrorKind::WriteZero.into()),
                n => remaining = &remaining[n..],
            }
        }
        Ok(())
    }

    /// Sends vectored buffers to the queue.
    pub async fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        loop {
            let mut guard = self.0.writable().await?;
            match guard.try_io(|inner| inner.get_ref().sendv(bufs)) {
                Ok(res) => return res,
                Err(_) => continue,
            }
        }
    }

    /// Sends all vectored buffers to the queue.
    ///
    /// This method advances the buffers in place as data is written.
    pub async fn sendv_all(&self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        if bufs.iter().all(|buf| buf.is_empty()) {
            return Ok(());
        }

        let mut bufs = bufs;
        while !bufs.is_empty() {
            match self.sendv(bufs).await? {
                0 => {
                    return Err(io::Error::new(
                        ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                n => IoSlice::advance_slices(&mut bufs, n),
            }
        }
        Ok(())
    }

    /// Tries to receive a packet without waiting.
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.get_ref().recv(buf)
    }

    /// Tries to send a packet without waiting.
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.get_ref().send(buf)
    }

    /// Tries to send vectored buffers without waiting.
    pub fn try_sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.get_ref().sendv(bufs)
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
        let self_mut = self.get_mut();
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
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
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
            let mut guard = ready!(self_mut.0.poll_write_ready_mut(cx))?;
            match guard.try_io(|inner| inner.get_mut().write_vectored(bufs)) {
                Ok(result) => return Poll::Ready(result),
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

    fn socket_pair() -> (OwnedFd, OwnedFd) {
        let mut fds = [0i32; 2];
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
        assert_eq!(&buffer[..received], &packet);
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
    async fn sendv_all_accepts_empty_buffers() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let _right = TunQueue::new(right).unwrap();
        let mut buffers = [IoSlice::new(&[]), IoSlice::new(&[])];

        left.sendv_all(&mut buffers).await.unwrap();
    }

    #[tokio::test]
    async fn sendv_all_handles_mixed_empty_buffers() {
        let (left, right) = socket_pair();
        let left = TunQueue::new(left).unwrap();
        let right = TunQueue::new(right).unwrap();
        let packet = [0x45u8; 20];
        let mut buffers = [IoSlice::new(&[]), IoSlice::new(&packet), IoSlice::new(&[])];

        left.sendv_all(&mut buffers).await.unwrap();
        let mut buffer = [0u8; 32];
        let received = right.recv(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..received], &packet);
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
        assert_eq!(&buffer[..received], &packet);
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
        assert_eq!(&buffer[..received], &packet);
    }
}
