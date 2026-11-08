use std::io::{self, IoSlice, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

#[cfg(target_os = "linux")]
use crate::offload::VIRTIO_NET_HDR_LEN;

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

impl std::os::fd::AsFd for TunIo {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        // SAFETY: `self.0` is an owned open descriptor.
        unsafe { std::os::fd::BorrowedFd::borrow_raw(self.0) }
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
        if buf.is_empty() {
            return Ok(0);
        }
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

    /// Receives one packet into `buf` and reports truncation when the packet is
    /// larger than the provided buffer.
    ///
    /// A one-byte sentinel detects whether additional data was available after
    /// filling `buf`. An exact fit (`received == buf.len()`) is not treated as
    /// truncation.
    #[cfg(target_os = "linux")]
    pub fn recv_checked(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut sentinel = [0u8; 1];
        let mut iov = [
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
        if received == buf.len() + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "offload packet was truncated",
            ));
        }
        Ok(received)
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

    /// Receives one offload packet, strips the virtio-net header, and copies
    /// only the payload into `payload`. A sentinel iovec detects truncation.
    #[cfg(target_os = "linux")]
    pub fn recv_payload(&self, payload: &mut [u8]) -> io::Result<usize> {
        if payload.is_empty() {
            return Ok(0);
        }
        let mut header = [0u8; VIRTIO_NET_HDR_LEN];
        let mut sentinel = [0u8; 1];
        let mut iov = [
            libc::iovec {
                iov_base: header.as_mut_ptr().cast(),
                iov_len: header.len(),
            },
            libc::iovec {
                iov_base: payload.as_mut_ptr().cast(),
                iov_len: payload.len(),
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
        if received < VIRTIO_NET_HDR_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "offload packet is shorter than the virtio-net header",
            ));
        }
        if received == VIRTIO_NET_HDR_LEN {
            return Ok(0);
        }
        if received == VIRTIO_NET_HDR_LEN + payload.len() + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "offload packet was truncated",
            ));
        }
        Ok(received - VIRTIO_NET_HDR_LEN)
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
        // SAFETY: `into_raw_fd` transfers ownership of the socket to `TunIo`.
        let mut io = unsafe { TunIo::from_raw_fd(writer.into_raw_fd()) };
        io.flush().unwrap();
        io.write_all(b"packet").unwrap();
        io.flush().unwrap();
        let mut data = [0; 6];
        reader.read_exact(&mut data).unwrap();
        assert_eq!(&data, b"packet");
    }

    #[test]
    fn empty_recv_does_not_consume_packet() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        // SAFETY: `into_raw_fd` transfers ownership of the socket to `TunIo`.
        let mut io = unsafe { TunIo::from_raw_fd(reader.into_raw_fd()) };
        writer.write_all(b"packet").unwrap();

        let mut empty = [];
        assert_eq!(io.recv(&mut empty).unwrap(), 0);

        let mut data = [0; 6];
        io.read_exact(&mut data).unwrap();
        assert_eq!(&data, b"packet");
    }
}
