use crate::raw_io::TunIo as RawIo;
use std::io::{self, IoSlice, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

const HEADER_LEN: usize = 4;

/// FreeBSD TUN/TAP packet I/O.
///
/// TUN devices created by this crate have `TUNSIFHEAD` enabled and carry a
/// four-byte address-family header. TAP devices carry raw Ethernet frames.
pub enum TunIo {
    Raw(RawIo),
    Header(HeaderTunIo),
}

impl TunIo {
    pub fn new(fd: OwnedFd, header: bool) -> Self {
        let fd = fd.into_raw_fd();
        if header {
            Self::Header(HeaderTunIo(fd))
        } else {
            // SAFETY: `fd` was just transferred from `OwnedFd`.
            Self::Raw(unsafe { RawIo::from_raw_fd(fd) })
        }
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.recv(buf),
            Self::Header(io) => io.recv(buf),
        }
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.send(buf),
            Self::Header(io) => io.send(buf),
        }
    }

    pub fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.sendv(bufs),
            Self::Header(io) => io.sendv(bufs),
        }
    }
}

impl FromRawFd for TunIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        // SAFETY: the caller transfers ownership of `fd` to the returned value.
        Self::Raw(unsafe { RawIo::from_raw_fd(fd) })
    }
}

impl AsRawFd for TunIo {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Raw(io) => io.as_raw_fd(),
            Self::Header(io) => io.as_raw_fd(),
        }
    }
}

impl IntoRawFd for TunIo {
    fn into_raw_fd(self) -> RawFd {
        match self {
            Self::Raw(io) => io.into_raw_fd(),
            Self::Header(io) => io.into_raw_fd(),
        }
    }
}

impl Read for TunIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.read(buf),
            Self::Header(io) => io.recv(buf),
        }
    }
}

impl Write for TunIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.write(buf),
            Self::Header(io) => io.send(buf),
        }
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        match self {
            Self::Raw(io) => io.write_vectored(bufs),
            Self::Header(io) => io.sendv(bufs),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Returns whether `TUNSIFHEAD` is enabled for this descriptor.
pub fn header_enabled(fd: RawFd) -> bool {
    let mut enabled: libc::c_int = 0;
    // SAFETY: `fd` is an open descriptor and TUNGIFHEAD expects an int pointer.
    unsafe { libc::ioctl(fd, crate::freebsd::sys::TUNGIFHEAD, &mut enabled) == 0 && enabled != 0 }
}

pub struct HeaderTunIo(RawFd);

impl HeaderTunIo {
    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut header = [0u8; HEADER_LEN];
        let mut sentinel = [0u8; 1];
        let mut iov = [
            libc::iovec {
                iov_base: header.as_mut_ptr().cast(),
                iov_len: header.len(),
            },
            libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            },
            libc::iovec {
                iov_base: sentinel.as_mut_ptr().cast(),
                iov_len: sentinel.len(),
            },
        ];
        let received = loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `iov` points to valid buffers for the duration of the call.
            let received = unsafe { libc::readv(self.0, iov.as_mut_ptr(), iov.len() as _) };
            if received < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            break received as usize;
        };

        if received < HEADER_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated TUN header",
            ));
        }
        if received == HEADER_LEN {
            return Ok(0);
        }
        if received == HEADER_LEN + buf.len() + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TUN packet was truncated",
            ));
        }
        let family = u32::from_be_bytes(header) as i32;
        if family != libc::AF_INET && family != libc::AF_INET6 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TUN packet has an unsupported address family",
            ));
        }
        Ok(received - HEADER_LEN)
    }

    fn send(&self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let family = packet_family(buf).ok_or_else(invalid_packet)?;
        let header = [0, 0, 0, family];
        let iov = [
            libc::iovec {
                iov_base: header.as_ptr() as *mut _,
                iov_len: header.len(),
            },
            libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len(),
            },
        ];
        let sent = loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `iov` points to valid buffers for the duration of the call.
            let sent = unsafe { libc::writev(self.0, iov.as_ptr(), iov.len() as _) };
            if sent < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            break sent as usize;
        };
        if sent != HEADER_LEN + buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "short TUN write",
            ));
        }
        Ok(sent - HEADER_LEN)
    }

    fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let Some(family) = packet_family_from_slices(bufs)? else {
            return Ok(0);
        };
        let payload_len: usize = bufs.iter().map(|buf| buf.len()).sum();
        let header = [0, 0, 0, family];
        let mut iov = Vec::with_capacity(bufs.len() + 1);
        iov.push(libc::iovec {
            iov_base: header.as_ptr() as *mut _,
            iov_len: header.len(),
        });
        iov.extend(bufs.iter().map(|buf| libc::iovec {
            iov_base: buf.as_ptr() as *mut _,
            iov_len: buf.len(),
        }));

        let sent = loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `iov` points to valid buffers for the duration of the call.
            let sent = unsafe { libc::writev(self.0, iov.as_ptr(), iov.len() as _) };
            if sent < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            break sent as usize;
        };
        if sent != HEADER_LEN + payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "short TUN write",
            ));
        }
        Ok(sent - HEADER_LEN)
    }
}

impl AsRawFd for HeaderTunIo {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl IntoRawFd for HeaderTunIo {
    fn into_raw_fd(self) -> RawFd {
        let this = std::mem::ManuallyDrop::new(self);
        this.0
    }
}

impl Drop for HeaderTunIo {
    fn drop(&mut self) {
        // SAFETY: this value owns the descriptor and closes it exactly once.
        unsafe { libc::close(self.0) };
    }
}

fn invalid_packet() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "TUN packets must start with an IPv4 or IPv6 header",
    )
}

fn packet_family(buf: &[u8]) -> Option<u8> {
    match buf.first().map(|byte| byte >> 4) {
        Some(4) => Some(libc::AF_INET as u8),
        Some(6) => Some(libc::AF_INET6 as u8),
        _ => None,
    }
}

fn packet_family_from_slices(bufs: &[IoSlice<'_>]) -> io::Result<Option<u8>> {
    for buf in bufs {
        if let Some(first) = buf.first() {
            return packet_family(&[*first])
                .map(Some)
                .ok_or_else(invalid_packet);
        }
    }
    Ok(None)
}
