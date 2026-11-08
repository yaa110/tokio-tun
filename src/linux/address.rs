use super::request::sockaddr;
use std::mem;
use std::net::Ipv4Addr;

/// Converts a C `sockaddr_in` representation into an [`Ipv4Addr`].
pub fn ipv4_from_sockaddr(addr: sockaddr) -> Ipv4Addr {
    // SAFETY: `sockaddr` and `sockaddr_in` are both 16-byte C sockaddr
    // representations; the source is a value, not an unaligned reference.
    let sock: libc::sockaddr_in = unsafe { mem::transmute(addr) };
    sock.sin_addr.s_addr.to_ne_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_from_sockaddr() {
        let ip = Ipv4Addr::new(192, 168, 1, 100);
        // SAFETY: `sockaddr_in` is a C POD type and all-zero is valid.
        let mut sock: libc::sockaddr_in = unsafe { mem::zeroed() };
        sock.sin_family = libc::AF_INET as _;
        sock.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
        // SAFETY: both types are 16-byte C sockaddr representations.
        let sock: sockaddr = unsafe { mem::transmute(sock) };
        assert_eq!(ipv4_from_sockaddr(sock), ip);
    }
}
