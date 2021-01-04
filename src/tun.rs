use crate::linux::interface::Interface;
use crate::linux::io::TunIo;
use crate::linux::params::Params;
use crate::result::Result;
use futures::ready;
use std::ffi::CString;
use std::io;
use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::os::unix::io::{AsRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

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
        let mut b = vec![0; buf.capacity()];
        loop {
            let mut guard = ready!(self_mut.io.poll_read_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().read(&mut b)) {
                Ok(n) => {
                    buf.put_slice(&b[..n?]);
                    return Poll::Ready(Ok(()));
                }
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
        let iface = Self::allocate(params, 1)?;
        let fd = iface.files()[0];
        Ok(Self {
            iface: Arc::new(iface),
            io: AsyncFd::new(TunIo::from(fd))?,
        })
    }

    /// Creates a new instance of Tun/Tap device.
    pub(crate) fn new_mq(params: Params, queues: usize) -> Result<Vec<Self>> {
        let iface = Self::allocate(params, queues)?;
        let mut tuns = Vec::with_capacity(queues);
        let files = iface.files().to_vec();
        let iface = Arc::new(iface);
        for fd in files.into_iter() {
            tuns.push(Self {
                iface: iface.clone(),
                io: AsyncFd::new(TunIo::from(fd))?,
            })
        }
        Ok(tuns)
    }

    fn allocate(params: Params, queues: usize) -> Result<Interface> {
        let mut fds = Vec::with_capacity(queues);
        let path = CString::new("/dev/net/tun")?;
        for _ in 0..queues {
            fds.push(unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) });
        }
        let iface = Interface::new(
            fds,
            params.name.as_deref().unwrap_or_default(),
            params.flags,
        )?;
        iface.init(params)?;
        Ok(iface)
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
