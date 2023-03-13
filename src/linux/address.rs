use super::request::sockaddr;
use std::mem;
use std::net::Ipv4Addr;

pub trait Ipv4AddrExt {
    fn to_address(&self) -> sockaddr;
    fn from_address(sock: sockaddr) -> Self;
}

// Ext suffix is in case a dependency on an external MAC address crate is ever pulled in
// Also matches above naming this way
pub trait MACAddrExt {
    fn to_address(&self) -> sockaddr;
    fn from_address(sock: sockaddr) -> Self;
}

pub struct MACAddr {
    data: [u8; 6],
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

impl MACAddrExt for MACAddr {
    fn to_address(&self) -> sockaddr {
        let mut addr: sockaddr = unsafe { mem::zeroed() };
        addr.sa_family = libc::ARPHRD_ETHER;
        for i in 0..6 {
            addr.sa_data[i] = self.data[i] as libc::c_char;
        }
        addr
    }

    fn from_address(sock: sockaddr) -> Self {
        let mut mac = Self { data: [0; 6] };
        for i in 0..6 {
            mac.data[i] = sock.sa_data[i] as u8;
        }
        mac
    }
}
