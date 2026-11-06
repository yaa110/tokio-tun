use std::io::{self, IoSlice, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

pub struct TunIo(RawFd);

impl FromRawFd for TunIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        // SAFETY: the caller transfers ownership of `fd` to the returned value.
        Self(fd)
    }
}

impl IntoRawFd for TunIo {
    fn into_raw_fd(self) -> RawFd {
        // Avoid running `Drop`, which would close the descriptor.
        let this = std::mem::ManuallyDrop::new(self);
        this.0
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
        // Writes go directly to the device; there is no userspace buffer to flush.
        Ok(())
    }
}

impl TunIo {
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `buf` is valid for writes of `buf.len()` bytes.
            let n = unsafe { libc::read(self.0, buf.as_mut_ptr().cast(), buf.len() as _) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as _);
        }
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `buf` is valid for reads of `buf.len()` bytes.
            let n = unsafe { libc::write(self.0, buf.as_ptr().cast(), buf.len() as _) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as _);
        }
    }

    pub fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let iov = bufs
            .iter()
            .map(|buf| libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len() as _,
            })
            .collect::<Vec<_>>();
        loop {
            // SAFETY: `self.0` is an open descriptor owned by this value, and
            // `iov` contains valid base/length pairs for the duration of the call.
            let n = unsafe { libc::writev(self.0, iov.as_ptr(), iov.len() as _) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as _);
        }
    }
}

impl Drop for TunIo {
    fn drop(&mut self) {
        // SAFETY: `TunIo` owns `self.0` and closes it exactly once.
        unsafe { libc::close(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn flush_unbuffered_io() {
        // A socket pair exercises the non-fsyncable descriptor without creating
        // a TUN interface or requiring network administration privileges.
        let (writer, mut reader) = UnixStream::pair().unwrap();
        let mut io = unsafe { TunIo::from_raw_fd(writer.into_raw_fd()) };
        io.flush().unwrap();
        io.write_all(b"packet").unwrap();
        io.flush().unwrap();
        let mut data = [0; 6];
        reader.read_exact(&mut data).unwrap();
        assert_eq!(&data, b"packet");
    }
}
