use std::convert::From;
use std::io::{self, IoSlice, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

pub struct TunIo(RawFd);

impl From<RawFd> for TunIo {
    fn from(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl FromRawFd for TunIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl IntoRawFd for TunIo {
    fn into_raw_fd(self) -> RawFd {
        self.0
    }
}

impl AsRawFd for TunIo {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Read for TunIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }
}

impl Write for TunIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.send(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.sendv(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        let ret = unsafe { libc::fsync(self.0) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl TunIo {
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0, buf.as_ptr() as *mut _, buf.len() as _) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe { libc::write(self.0, buf.as_ptr() as *const _, buf.len() as _) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }

    pub fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let iov = bufs
            .iter()
            .map(|buf| libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len() as _,
            })
            .collect::<Vec<_>>();
        let n = unsafe { libc::writev(self.0, iov.as_ptr() as *const _, iov.len() as _) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as _)
    }
}

impl Drop for TunIo {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}
