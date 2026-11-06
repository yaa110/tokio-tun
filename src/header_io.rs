use std::io::{self, IoSlice, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

const HEADER_LEN: usize = 4;

pub struct TunIo(RawFd);

impl FromRawFd for TunIo {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
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
        if buf.is_empty() {
            return Ok(0);
        }

        let mut header = [0u8; HEADER_LEN];
        let mut iov = [
            libc::iovec {
                iov_base: header.as_mut_ptr().cast(),
                iov_len: header.len(),
            },
            libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            },
        ];
        let mut message = empty_msghdr();
        message.msg_iov = iov.as_mut_ptr();
        message.msg_iovlen = iov.len() as _;

        let received = loop {
            // SAFETY: `self.0` is a valid open descriptor, `message` points to
            // live iovecs, and the kernel writes at most their combined length.
            let received = unsafe { libc::recvmsg(self.0, &mut message, 0) };
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
                "truncated utun packet",
            ));
        }
        if received == HEADER_LEN {
            return Ok(0);
        }
        if message.msg_flags & libc::MSG_TRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "utun packet was truncated",
            ));
        }

        let family = u32::from_be_bytes(header) as i32;
        if family != libc::AF_INET && family != libc::AF_INET6 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "utun packet has an unsupported address family",
            ));
        }

        Ok(received - HEADER_LEN)
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
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
        let mut message = empty_msghdr();
        message.msg_iov = iov.as_ptr() as *mut _;
        message.msg_iovlen = iov.len() as _;

        let sent = loop {
            // SAFETY: `self.0` is a valid open descriptor and `message` points
            // to live iovecs for the duration of the call.
            let sent = unsafe { libc::sendmsg(self.0, &message, 0) };
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
                "short utun write",
            ));
        }
        Ok(sent - HEADER_LEN)
    }

    pub fn sendv(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let family = match packet_family_from_slices(bufs)? {
            Some(family) => family,
            None => return Ok(0),
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

        let mut message = empty_msghdr();
        message.msg_iov = iov.as_mut_ptr();
        message.msg_iovlen = iov.len() as _;

        let sent = loop {
            // SAFETY: `self.0` is a valid open descriptor and `message` points
            // to live iovecs for the duration of the call.
            let sent = unsafe { libc::sendmsg(self.0, &message, 0) };
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
                "short utun write",
            ));
        }
        Ok(sent - HEADER_LEN)
    }
}

fn packet_family(buf: &[u8]) -> Option<u8> {
    match buf.first().map(|byte| byte >> 4) {
        Some(4) => Some(libc::AF_INET as u8),
        Some(6) => Some(libc::AF_INET6 as u8),
        _ => None,
    }
}

fn packet_family_from_slices(bufs: &[IoSlice<'_>]) -> io::Result<Option<u8>> {
    match bufs.iter().find(|buf| !buf.is_empty()) {
        Some(buf) => packet_family(&buf[..]).map(Some).ok_or_else(invalid_packet),
        None => Ok(None),
    }
}

fn invalid_packet() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "utun packets must start with an IPv4 or IPv6 header",
    )
}

fn empty_msghdr() -> libc::msghdr {
    // SAFETY: `msghdr` is a C POD type and an all-zero value is valid.
    unsafe { std::mem::zeroed() }
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

    fn socket_pair() -> (TunIo, TunIo) {
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
        unsafe { (TunIo::from_raw_fd(fds[0]), TunIo::from_raw_fd(fds[1])) }
    }

    #[test]
    fn sends_and_receives_ipv4_packets() {
        let (sender, receiver) = socket_pair();
        let packet = [0x45u8; 20];

        assert_eq!(sender.send(&packet).unwrap(), packet.len());

        let mut buffer = [0u8; 64];
        let received = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..received], &packet);
    }

    #[test]
    fn sends_and_receives_ipv6_packets() {
        let (sender, receiver) = socket_pair();
        let packet = [0x60u8; 40];

        assert_eq!(sender.send(&packet).unwrap(), packet.len());

        let mut buffer = [0u8; 64];
        let received = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..received], &packet);
    }

    #[test]
    fn sends_vectored_packets() {
        let (sender, receiver) = socket_pair();
        let packet = [0x45u8; 20];
        let bufs = [IoSlice::new(&packet[..8]), IoSlice::new(&packet[8..])];

        assert_eq!(sender.sendv(&bufs).unwrap(), packet.len());

        let mut buffer = [0u8; 64];
        let received = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..received], &packet);
    }

    #[test]
    fn rejects_non_ip_packets() {
        let (sender, _receiver) = socket_pair();
        let error = sender.send(&[0x00u8; 20]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn empty_receive_does_not_consume_packet() {
        let (sender, receiver) = socket_pair();
        let packet = [0x45u8; 20];
        sender.send(&packet).unwrap();

        let mut empty = [];
        assert_eq!(receiver.recv(&mut empty).unwrap(), 0);

        let mut buffer = [0u8; 64];
        let received = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..received], &packet);
    }

    #[test]
    fn rejects_truncated_packets() {
        let (sender, receiver) = socket_pair();
        sender.send(&[0x45u8; 100]).unwrap();

        let mut buffer = [0u8; 10];
        let error = receiver.recv(&mut buffer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn detects_packet_family() {
        assert_eq!(packet_family(&[0x45]), Some(libc::AF_INET as u8));
        assert_eq!(packet_family(&[0x60]), Some(libc::AF_INET6 as u8));
        assert_eq!(packet_family(&[]), None);
        assert_eq!(packet_family(&[0x00]), None);
    }

    #[test]
    fn detects_packet_family_from_vectored_buffers() {
        let first = [];
        let second = [0x60u8, 0, 0, 0];
        let bufs = [IoSlice::new(&first), IoSlice::new(&second)];
        assert_eq!(
            packet_family_from_slices(&bufs).unwrap(),
            Some(libc::AF_INET6 as u8)
        );
        assert_eq!(packet_family_from_slices(&[]).unwrap(), None);

        let invalid = [IoSlice::new(&[0x00u8, 0, 0, 0])];
        assert!(packet_family_from_slices(&invalid).is_err());
    }
}
