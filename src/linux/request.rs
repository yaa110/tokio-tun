#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use std::mem;

const IFNAMSIZ: u32 = 16;

#[repr(C)]
#[derive(Copy, Clone)]
/// Network interface ioctl request
///
/// See [man netdevice(7)](https://man7.org/linux/man-pages/man7/netdevice.7.html)
pub struct ifreq {
    pub ifr_ifrn: ifreq_ifrn,
    pub ifr_ifru: ifreq_ifru,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct in6_ifreq {
    pub ifr6_addr: in6_addr,
    pub ifr6_prefixlen: ::std::os::raw::c_uint,
    pub ifr6_ifindex: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union ifreq_ifrn {
    pub ifrn_name: [::std::os::raw::c_char; 16usize],
    align: [u8; 16usize],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union ifreq_ifru {
    pub ifru_addr: sockaddr,
    pub ifru_dstaddr: sockaddr,
    pub ifru_broadaddr: sockaddr,
    pub ifru_netmask: sockaddr,
    pub ifru_hwaddr: sockaddr,
    pub ifru_flags: ::std::os::raw::c_short,
    pub ifru_ifindex: ::std::os::raw::c_int,
    pub ifru_ivalue: ::std::os::raw::c_int,
    pub ifru_mtu: ::std::os::raw::c_int,
    pub ifru_map: ifmap,
    pub ifru_slave: [::std::os::raw::c_char; 16usize],
    pub ifru_newname: [::std::os::raw::c_char; 16usize],
    pub ifru_data: *mut ::std::os::raw::c_char,
    align: [u64; 3usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
/// Linux kernel IPv4 socket address representation
///
/// See [man ip(7)](https://man7.org/linux/man-pages/man7/ip.7.html)
pub struct sockaddr {
    // see https://man7.org/linux/man-pages/man7/ip.7.html for docs
    pub sa_family: ::std::os::raw::c_ushort,
    pub sa_data: [::std::os::raw::c_char; 14usize],
}

// See https://man7.org/linux/man-pages/man7/ipv6.7.html
pub type in6_addr = libc::in6_addr;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ifmap {
    pub mem_start: ::std::os::raw::c_ulong,
    pub mem_end: ::std::os::raw::c_ulong,
    pub base_addr: ::std::os::raw::c_ushort,
    pub irq: ::std::os::raw::c_uchar,
    pub dma: ::std::os::raw::c_uchar,
    pub port: ::std::os::raw::c_uchar,
}

impl ifreq {
    pub fn new(name: &str) -> Self {
        let mut req: ifreq = unsafe { mem::zeroed() };
        if !name.is_empty() {
            let mut ifname: [::std::os::raw::c_char; IFNAMSIZ as _] = [0; IFNAMSIZ as _];
            for (i, c) in name.as_bytes().iter().enumerate() {
                if i > ifname.len() - 1 {
                    break;
                }
                ifname[i] = *c as _;
            }
            req.ifr_ifrn.ifrn_name = ifname;
        }
        req
    }

    pub fn name(&self) -> String {
        let mut name = String::new();
        for i in 0..IFNAMSIZ as _ {
            let c = unsafe { self.ifr_ifrn.ifrn_name }[i] as u8 as char;
            if c != '\0' {
                name.push(c)
            }
        }
        name
    }
}
