use std::mem;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr;

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

    // SAFETY: `sockaddr_in` and `sockaddr` are both 16 bytes on FreeBSD, and
    // the address bytes are copied without interpretation.
    unsafe { mem::transmute(address) }
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

pub(crate) fn ipv4_from_sockaddr(address: *const libc::sockaddr) -> Ipv4Addr {
    // SAFETY: the caller checks that `address` is AF_INET and points to a
    // `sockaddr_in`. The pointer may not be aligned for `sockaddr_in`.
    let address = unsafe { ptr::read_unaligned(address.cast::<libc::sockaddr_in>()) };
    Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes())
}

pub(crate) fn ipv6_from_sockaddr(address: *const libc::sockaddr) -> Ipv6Addr {
    // SAFETY: the caller checks that `address` is AF_INET6 and points to a
    // `sockaddr_in6`. The pointer may not be aligned for `sockaddr_in6`.
    let address = unsafe { ptr::read_unaligned(address.cast::<libc::sockaddr_in6>()) };
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
    }

    #[test]
    fn converts_sockaddrs() {
        let v4 = Ipv4Addr::new(10, 20, 30, 40);
        let sockaddr = sockaddr_v4(v4);
        assert_eq!(ipv4_from_sockaddr(&sockaddr), v4);

        let v6: Ipv6Addr = "fd00:123::1".parse().unwrap();
        let sockaddr6 = sockaddr_v6(v6);
        assert_eq!(
            ipv6_from_sockaddr((&sockaddr6 as *const libc::sockaddr_in6).cast()),
            v6
        );
    }
}
