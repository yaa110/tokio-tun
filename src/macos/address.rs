use std::mem;
use std::net::{Ipv4Addr, Ipv6Addr};

pub(crate) fn prefix_to_ipv4_mask(prefix_len: u8) -> Ipv4Addr {
    debug_assert!(prefix_len <= 32);
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    Ipv4Addr::from(mask.to_be_bytes())
}

pub(crate) fn prefix_to_ipv6_mask(prefix_len: u8) -> Ipv6Addr {
    debug_assert!(prefix_len <= 128);
    let mut mask = [0u8; 16];
    let full_bytes = usize::from(prefix_len / 8);
    let remaining_bits = prefix_len % 8;

    mask[..full_bytes].fill(0xff);
    if remaining_bits != 0 {
        mask[full_bytes] = 0xff << (8 - remaining_bits);
    }

    Ipv6Addr::from(mask)
}

pub(crate) fn prefix_len_from_ipv4_mask(mask: Ipv4Addr) -> u8 {
    u32::from_be_bytes(mask.octets()).count_ones() as u8
}

pub(crate) fn prefix_len_from_ipv6_mask(mask: Ipv6Addr) -> u8 {
    mask.octets()
        .iter()
        .map(|byte| byte.count_ones() as u8)
        .sum()
}

pub(crate) fn sockaddr_v4(address: Ipv4Addr) -> libc::sockaddr {
    let address = libc::sockaddr_in {
        sin_len: mem::size_of::<libc::sockaddr_in>() as u8,
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes(address.octets()),
        },
        sin_zero: [0; 8],
    };

    // SAFETY: `sockaddr_in` and `sockaddr` are both 16 bytes on Darwin and
    // the address bytes are copied without interpretation; the size pins
    // below are a compile-time guarantee for the transmute.
    unsafe {
        const _: () = assert!(mem::size_of::<libc::sockaddr_in>() == 16);
        const _: () = assert!(mem::size_of::<libc::sockaddr>() == 16);
        mem::transmute(address)
    }
}

pub(crate) fn sockaddr_v6(address: Ipv6Addr) -> libc::sockaddr_in6 {
    libc::sockaddr_in6 {
        sin6_len: mem::size_of::<libc::sockaddr_in6>() as u8,
        sin6_family: libc::AF_INET6 as libc::sa_family_t,
        sin6_port: 0,
        sin6_flowinfo: 0,
        sin6_addr: libc::in6_addr {
            s6_addr: address.octets(),
        },
        sin6_scope_id: 0,
    }
}

pub(crate) fn ipv4_from_sockaddr(address: *const libc::sockaddr, len: usize) -> Ipv4Addr {
    // macOS omits trailing zero bytes, so only `len - 4` address bytes may be
    // present. Missing bytes are zero.
    let available = len.saturating_sub(4).min(4);
    let mut octets = [0u8; 4];
    if available != 0 {
        // SAFETY: the caller ensures `address` is AF_INET and has at least
        // `len` bytes; `sin_addr` starts at offset 4.
        let bytes = unsafe { std::slice::from_raw_parts((address as *const u8).add(4), available) };
        octets[..available].copy_from_slice(bytes);
    }
    Ipv4Addr::from(octets)
}

pub(crate) fn ipv6_from_sockaddr_in6(address: &libc::sockaddr_in6) -> Ipv6Addr {
    Ipv6Addr::from(address.sin6_addr.s6_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_prefix_masks() {
        assert_eq!(prefix_to_ipv4_mask(0), Ipv4Addr::UNSPECIFIED);
        assert_eq!(prefix_to_ipv4_mask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_to_ipv4_mask(32), Ipv4Addr::BROADCAST);

        assert_eq!(prefix_to_ipv6_mask(0), Ipv6Addr::UNSPECIFIED);
        assert_eq!(
            prefix_to_ipv6_mask(64),
            Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0)
        );
        assert_eq!(
            prefix_to_ipv6_mask(128),
            Ipv6Addr::new(
                0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff
            )
        );
    }

    #[test]
    fn converts_ipv4_sockaddr_round_trip() {
        let address = Ipv4Addr::new(10, 20, 30, 40);
        let sockaddr = sockaddr_v4(address);
        assert_eq!(
            ipv4_from_sockaddr(&sockaddr, sockaddr.sa_len as usize),
            address
        );
    }

    #[test]
    fn converts_ipv6_sockaddr_round_trip() {
        let address: Ipv6Addr = "fd00:123::1".parse().unwrap();
        let sockaddr = sockaddr_v6(address);
        assert_eq!(ipv6_from_sockaddr_in6(&sockaddr), address);
    }

    #[test]
    fn counts_prefix_lengths() {
        assert_eq!(
            prefix_len_from_ipv4_mask(Ipv4Addr::new(255, 255, 255, 0)),
            24
        );
        assert_eq!(prefix_len_from_ipv4_mask(Ipv4Addr::BROADCAST), 32);
        assert_eq!(prefix_len_from_ipv6_mask(prefix_to_ipv6_mask(64)), 64);
        assert_eq!(prefix_len_from_ipv6_mask(prefix_to_ipv6_mask(128)), 128);
    }
}
