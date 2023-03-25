use super::request::sockaddr;
use std::mem;
use std::net::Ipv4Addr;

pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub fn new(data: [u8; 6]) -> MacAddr {
        MacAddr(data)
    }
}

impl MacAddr {
    pub fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl From<[u8; 6]> for MacAddr {
  fn from(data: [u8; 6]) -> MacAddr {
        MacAddr::new(data)
    }
}

pub trait SockAddrExt {
    fn to_address(&self) -> sockaddr;
    fn from_address(sock: sockaddr) -> Self;
}

fn hton(octets: [u8; 4]) -> u32 {
    (octets[3] as u32) << 24 | (octets[2] as u32) << 16 | (octets[1] as u32) << 8 | octets[0] as u32
}

fn ntoh(number: u32) -> [u8; 4] {
    [
        (number & 0xff) as u8,
        (number >> 8 & 0xff) as u8,
        (number >> 16 & 0xff) as u8,
        (number >> 24 & 0xff) as u8,
    ]
}

impl SockAddrExt for Ipv4Addr {
    fn to_address(&self) -> sockaddr {
        let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
        addr.sin_family = libc::AF_INET as _;
        addr.sin_addr = libc::in_addr {
            s_addr: hton(self.octets()),
        };
        addr.sin_port = 0;
        unsafe { mem::transmute(addr) }
    }

    fn from_address(addr: sockaddr) -> Self {
        let sock: libc::sockaddr_in = unsafe { mem::transmute(addr) };
        ntoh(sock.sin_addr.s_addr).into()
    }
}

impl SockAddrExt for MacAddr {
    fn to_address(&self) -> sockaddr {
        let octets = self.octets();
        let mut addr: sockaddr = unsafe { mem::zeroed() };
        addr.sa_family = libc::ARPHRD_ETHER;
        for i in 0..6 {
            addr.sa_data[i] = octets[i] as _;
        }
        addr
    }

    fn from_address(sock: sockaddr) -> Self {
        let mut data = [0u8; 6];
        for i in 0..6 {
            data[i] = sock.sa_data[i] as _;
        }
        MacAddr::new(data)
    }
}
