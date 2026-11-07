use libc::{IFNAMSIZ, c_char, c_int, c_ulong, sockaddr, sockaddr_in6, time_t};

pub const IN6_IFF_NODAD: c_int = 0x20;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IfAliasReq {
    pub ifra_name: [c_char; IFNAMSIZ],
    pub ifra_addr: sockaddr,
    pub ifra_broadaddr: sockaddr,
    pub ifra_mask: sockaddr,
    pub ifra_vhid: c_int,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct In6AddrLifetime {
    pub ia6t_expire: time_t,
    pub ia6t_preferred: time_t,
    pub ia6t_vltime: u32,
    pub ia6t_pltime: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct In6AliasReq {
    pub ifra_name: [c_char; IFNAMSIZ],
    pub ifra_addr: sockaddr_in6,
    pub ifra_dstaddr: sockaddr_in6,
    pub ifra_prefixmask: sockaddr_in6,
    pub ifra_flags: c_int,
    pub ifra_lifetime: In6AddrLifetime,
    pub ifra_vhid: c_int,
}

const IOC_IN: c_ulong = 0x8000_0000;
const IOC_OUT: c_ulong = 0x4000_0000;
const IOC_INOUT: c_ulong = 0xc000_0000;

const fn ioctl_req(inout: c_ulong, group: u8, number: u8, len: usize) -> c_ulong {
    inout | ((len as c_ulong) << 16) | ((group as c_ulong) << 8) | number as c_ulong
}

pub const SIOCSIFFLAGS: c_ulong = ioctl_req(IOC_IN, b'i', 16, size_of::<libc::ifreq>());
pub const SIOCGIFFLAGS: c_ulong = ioctl_req(IOC_INOUT, b'i', 17, size_of::<libc::ifreq>());
pub const SIOCSIFMTU: c_ulong = ioctl_req(IOC_IN, b'i', 52, size_of::<libc::ifreq>());
pub const SIOCGIFMTU: c_ulong = ioctl_req(IOC_INOUT, b'i', 51, size_of::<libc::ifreq>());
pub const SIOCSIFLLADDR: c_ulong = ioctl_req(IOC_IN, b'i', 60, size_of::<libc::ifreq>());
pub const SIOCIFDESTROY: c_ulong = ioctl_req(IOC_IN, b'i', 121, size_of::<libc::ifreq>());
pub const SIOCIFCREATE2: c_ulong = ioctl_req(IOC_INOUT, b'i', 124, size_of::<libc::ifreq>());
pub const SIOCAIFADDR: c_ulong = ioctl_req(IOC_IN, b'i', 43, size_of::<IfAliasReq>());
pub const SIOCAIFADDR_IN6: c_ulong = ioctl_req(IOC_IN, b'i', 27, size_of::<In6AliasReq>());
pub const TUNGIFNAME: c_ulong = ioctl_req(IOC_OUT, b't', 93, size_of::<libc::ifreq>());
pub const TUNSIFHEAD: c_ulong = ioctl_req(IOC_IN, b't', 96, size_of::<c_int>());
pub const TUNGIFHEAD: c_ulong = ioctl_req(IOC_OUT, b't', 97, size_of::<c_int>());
pub const TUNSTRANSIENT: c_ulong = ioctl_req(IOC_IN, b't', 98, size_of::<c_int>());
