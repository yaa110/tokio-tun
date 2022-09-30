use crate::linux::interface::Interface;
use crate::linux::io::TunIo;
use crate::linux::params::Params;
use crate::result::Result;
use std::io;
use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::os::raw::c_char;
use std::os::unix::io::{AsRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        let self_mut = self.get_mut();
        loop {
            let mut guard = ready!(self_mut.io.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().flush()) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Tun {
    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new(params: Params) -> Result<Self> {
        let iface = Self::allocate(params, None)?;
        let fd = iface.files()[0];
        Ok(Self {
            iface: Arc::new(iface),
            io: AsyncFd::new(TunIo::from(fd))?,
        })
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new_mq(params: Params, queues: usize) -> Result<Vec<Self>> {
        let iface = Self::allocate(params, Some(queues))?;
        let mut tuns = Vec::with_capacity(queues);
        let iface = Arc::new(iface);
        for &fd in iface.files() {
            tuns.push(Self {
                iface: iface.clone(),
                io: AsyncFd::new(TunIo::from(fd))?,
            })
        }
        Ok(tuns)
    }

    fn allocate(params: Params, queues: Option<usize>) -> Result<Interface> {
        static TUN: &[u8] = b"/dev/net/tun\0";

        let fds = (0..queues.unwrap_or(1))
            .map(|_| unsafe {
                libc::open(
                    TUN.as_ptr().cast::<c_char>(),
                    libc::O_RDWR | libc::O_NONBLOCK,
                )
            })
            .collect::<Vec<_>>();

        let iface = Interface::new(
            fds,
            params.name.as_deref().unwrap_or_default(),
            params.flags,
            queues.is_some(),
        )?;
        iface.init(params)?;
        Ok(iface)
    }

    /// Receives a packet from the Tun/Tap interface
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

    /// Sends a packet to the Tun/Tap interface
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

    /// Try to receive a packet from the Tun/Tap interface
    ///
    /// When there is no pending data, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.get_ref().recv(buf)
    }

    /// Try to send a packet to the Tun/Tap interface
    ///
    /// When the socket buffer is full, `Err(io::ErrorKind::WouldBlock)` is returned.
    ///
    /// This method takes &self, so it is possible to call this method concurrently with other methods on this struct.
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.get_ref().send(buf)
    }

    /// Returns the name of Tun/Tap device.
    pub fn name(&self) -> &str {
        self.iface.name()
    }

    /// Returns the value of MTU.
    pub fn mtu(&self) -> Result<i32> {
        self.iface.mtu(None)
    }

    /// Returns the IPv4 address of MTU.
    pub fn address(&self) -> Result<Ipv4Addr> {
        self.iface.address(None)
    }

    /// Returns the IPv4 destination address of MTU.
    pub fn destination(&self) -> Result<Ipv4Addr> {
        self.iface.destination(None)
    }

    /// Returns the IPv4 broadcast address of MTU.
    pub fn broadcast(&self) -> Result<Ipv4Addr> {
        self.iface.broadcast(None)
    }

    /// Returns the IPv4 netmask address of MTU.
    pub fn netmask(&self) -> Result<Ipv4Addr> {
        self.iface.netmask(None)
    }

    /// Returns the flags of MTU.
    pub fn flags(&self) -> Result<i16> {
        self.iface.flags(None)
    }
}
