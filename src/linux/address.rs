use super::request::sockaddr;
use std::mem;
use std::net::Ipv4Addr;

pub trait Ipv4AddrExt {
    fn to_address(&self) -> sockaddr;
    fn from_address(sock: sockaddr) -> Self;
}

impl Ipv4AddrExt for Ipv4Addr {
    fn to_address(&self) -> sockaddr {
        let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
        addr.sin_family = libc::AF_INET as _;
        addr.sin_addr = libc::in_addr {
            s_addr: u32::from_ne_bytes(self.octets()),
        };
        addr.sin_port = 0;
        unsafe { mem::transmute(addr) }
    }

    fn from_address(addr: sockaddr) -> Self {
        let sock: libc::sockaddr_in = unsafe { mem::transmute(addr) };
        sock.sin_addr.s_addr.to_ne_bytes().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_addr_ext_roundtrip() {
        let ip = Ipv4Addr::new(192, 168, 1, 100);
        let sock = ip.to_address();
        assert_eq!(sock.sa_family, libc::AF_INET as _);
        let recovered = Ipv4Addr::from_address(sock);
        assert_eq!(ip, recovered);
    }
}
