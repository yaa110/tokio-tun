use super::request::{sockaddr, in6_addr};
use std::mem;
use std::net::{Ipv4Addr, Ipv6Addr};

/// A utility trait used to convert between rust and C IPv4 address representations
pub trait Ipv4AddrExt {
    fn to_address(&self) -> sockaddr;
    fn from_address(sock: sockaddr) -> Self;
}

/// A utility trait used to convert between rust and C IPv6 address representations
pub trait Ipv6AddrExt {
    fn to_address(&self) -> in6_addr;
    fn from_address(address: in6_addr) -> Self;
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

impl Ipv4AddrExt for Ipv4Addr {
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

impl Ipv6AddrExt for Ipv6Addr {
    fn to_address(&self) -> in6_addr {
        in6_addr {
            s6_addr: self.octets(),
        }
    }

    fn from_address(address: in6_addr) -> Self {
        Self::from(address.s6_addr)
    }
}
