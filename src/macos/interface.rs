use super::address::{
    ipv4_from_sockaddr, ipv6_from_sockaddr_in6, prefix_len_from_ipv4_mask,
    prefix_len_from_ipv6_mask, prefix_to_ipv4_mask, prefix_to_ipv6_mask, sockaddr_v4, sockaddr_v6,
};
use super::params::{Params, parse_utun_name};
use crate::address::{AddressConfig, AddressInfo};
use std::ffi::{CStr, CString};
use std::io;
use std::mem;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::ptr;

const UTUN_CONTROL_NAME: &[u8] = b"com.apple.net.utun_control";
// macOS reports IPv4 sockaddrs with trailing zero bytes omitted, so the
// minimum header is sa_len, sa_family, and sin_port.
const IPV4_SOCKADDR_LEN: usize = 4;

// These Darwin ABI items are not exposed by the supported `libc` range
// (`>= 0.2.186`); define them locally so the crate does not depend on a newer
// libc for these ioctl constants.
const SIOCAIFADDR: libc::c_ulong = 0x8040_691a;
const SIOCGIFMTU: libc::c_ulong = 0xc020_6933;
const SIOCSIFMTU: libc::c_ulong = 0x8020_6934;
const SIOCGIFFLAGS: libc::c_ulong = 0xc020_6911;
const SIOCSIFFLAGS: libc::c_ulong = 0x8020_6910;
const SIOCAIFADDR_IN6: libc::c_ulong = 0x8080_691a;
const IN6_IFF_NODAD: libc::c_int = 0x0020;

#[repr(C)]
struct IfAliasReq {
    ifra_name: [libc::c_char; libc::IFNAMSIZ],
    ifra_addr: libc::sockaddr,
    ifra_broadaddr: libc::sockaddr,
    ifra_mask: libc::sockaddr,
}

#[repr(C)]
struct In6Addrlifetime {
    ia6t_expire: libc::time_t,
    ia6t_preferred: libc::time_t,
    ia6t_vltime: u32,
    ia6t_pltime: u32,
}

#[repr(C)]
struct In6AliasReq {
    ifra_name: [libc::c_char; libc::IFNAMSIZ],
    ifra_addr: libc::sockaddr_in6,
    ifra_dstaddr: libc::sockaddr_in6,
    ifra_prefixmask: libc::sockaddr_in6,
    ifra_flags: libc::c_int,
    ifra_lifetime: In6Addrlifetime,
}

const _: () = assert!(mem::size_of::<IfAliasReq>() == 64);
const _: () = assert!(mem::size_of::<In6AliasReq>() == 128);

pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    socket6: Option<i32>,
    index: u32,
    name: String,
}

impl Interface {
    pub fn new(params: &Params) -> io::Result<Self> {
        let requested_name = params.name.as_deref().unwrap_or("");
        let unit = parse_utun_name(requested_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface names on macOS must be empty, \"utun\", or \"utun<N>\"",
            )
        })?;

        let utun = create_utun(unit, params.cloexec)?;
        let name = utun_name(utun.as_raw_fd())?;
        if !requested_name.is_empty() && requested_name != "utun" && name != requested_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "kernel assigned a different utun name",
            ));
        }
        let index = if_nametoindex(&name)?;
        let socket = create_socket(libc::AF_INET, params.cloexec)?;
        let socket6 = if params
            .addresses
            .iter()
            .any(|address| address.address.is_ipv6())
        {
            Some(create_socket(libc::AF_INET6, params.cloexec)?)
        } else {
            None
        };

        Ok(Interface {
            fds: vec![utun.into_raw_fd()],
            socket: socket.into_raw_fd(),
            socket6: socket6.map(OwnedFd::into_raw_fd),
            index,
            name,
        })
    }

    pub fn init(&self, params: &Params) -> io::Result<()> {
        self.index()?;
        if let Some(mtu) = params.mtu {
            self.mtu(Some(mtu))?;
        }
        for config in &params.addresses {
            self.add_address(config)?;
        }
        if params.up {
            self.flags(Some(libc::IFF_UP as i16))?;
        }
        Ok(())
    }

    pub fn take_files(&mut self) -> Vec<i32> {
        mem::take(&mut self.fds)
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn index(&self) -> io::Result<u32> {
        let mut name = [0 as libc::c_char; libc::IFNAMSIZ];
        // SAFETY: `name` is a writable IFNAMSIZ buffer and `self.index` is a
        // previously resolved interface index.
        let resolved = unsafe { libc::if_indextoname(self.index, name.as_mut_ptr()) };
        if resolved.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `if_indextoname` returned a NUL-terminated name.
        let resolved = unsafe { CStr::from_ptr(resolved) };
        if resolved.to_bytes() != self.name.as_bytes() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "interface identity changed",
            ));
        }
        Ok(self.index)
    }

    pub fn mtu(&self, mtu: Option<i32>) -> io::Result<i32> {
        self.index()?;
        let mut req = ifreq_for(&self.name);
        if let Some(mtu) = mtu {
            req.ifr_ifru.ifru_mtu = mtu;
            ioctl_const(self.socket, SIOCSIFMTU, &req)?;
            Ok(mtu)
        } else {
            ioctl_mut(self.socket, SIOCGIFMTU, &mut req)?;
            // SAFETY: a successful SIOCGIFMTU initialized `ifru_mtu`.
            Ok(unsafe { req.ifr_ifru.ifru_mtu })
        }
    }

    pub fn flags(&self, flags: Option<i16>) -> io::Result<u32> {
        self.index()?;
        let mut req = ifreq_for(&self.name);
        ioctl_mut(self.socket, SIOCGIFFLAGS, &mut req)?;
        if let Some(flags) = flags {
            // SAFETY: the union was initialized by SIOCGIFFLAGS above.
            unsafe { req.ifr_ifru.ifru_flags |= flags };
            ioctl_const(self.socket, SIOCSIFFLAGS, &req)?;
        }
        // SAFETY: the union was initialized by SIOCGIFFLAGS above.
        Ok(u16::from_ne_bytes(unsafe { req.ifr_ifru.ifru_flags }.to_ne_bytes()) as u32)
    }

    pub fn netmask(&self) -> io::Result<Ipv4Addr> {
        self.index()?;
        let addresses = self.addresses()?;
        let address = addresses
            .iter()
            .find(|info| info.address.is_ipv4())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "interface has no IPv4 address",
                )
            })?;
        Ok(prefix_to_ipv4_mask(address.prefix_len))
    }

    pub fn addresses(&self) -> io::Result<Vec<AddressInfo>> {
        self.index()?;
        Self::addresses_for(&self.name)
    }

    fn addresses_for(interface: &str) -> io::Result<Vec<AddressInfo>> {
        let mut ifap: *mut libc::ifaddrs = ptr::null_mut();
        // SAFETY: `ifap` is a valid out-pointer and the returned list is
        // released by the `Ifaddrs` guard below.
        if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let _guard = Ifaddrs(ifap);

        let mut addresses = Vec::new();
        let mut current = ifap;
        while !current.is_null() {
            // SAFETY: `getifaddrs` returned a valid linked list and `current`
            // points to one of its nodes.
            let ifa = unsafe { &*current };
            current = ifa.ifa_next;

            if ifa.ifa_name.is_null() || ifa.ifa_addr.is_null() {
                continue;
            }
            // SAFETY: `ifa_name` is a NUL-terminated string owned by the list.
            let name = unsafe { CStr::from_ptr(ifa.ifa_name) };
            if name.to_bytes() != interface.as_bytes() {
                continue;
            }

            // SAFETY: `ifa_addr` is non-null and points to a valid sockaddr.
            match sockaddr_family(ifa.ifa_addr) {
                libc::AF_INET if sockaddr_is(ifa.ifa_addr, libc::AF_INET, IPV4_SOCKADDR_LEN) => {
                    let len = sockaddr_len(ifa.ifa_addr);
                    let address = ipv4_from_sockaddr(ifa.ifa_addr, len);
                    let prefix_len = Self::ipv4_prefix_len(ifa);
                    let destination = Self::ipv4_destination(ifa, address);
                    addresses.push(AddressInfo {
                        address: IpAddr::V4(address),
                        prefix_len,
                        destination,
                        broadcast: None,
                    });
                }
                libc::AF_INET6
                    if sockaddr_is(
                        ifa.ifa_addr,
                        libc::AF_INET6,
                        mem::size_of::<libc::sockaddr_in6>(),
                    ) =>
                {
                    // SAFETY: the family and length match `sockaddr_in6`.
                    let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                    let address = ipv6_from_sockaddr_in6(sin6);
                    let prefix_len = Self::ipv6_prefix_len(ifa);
                    let destination = Self::ipv6_destination(ifa, address);
                    addresses.push(AddressInfo {
                        address: IpAddr::V6(address),
                        prefix_len,
                        destination,
                        broadcast: None,
                    });
                }
                _ => {}
            }
        }

        Ok(addresses)
    }

    fn add_address(&self, config: &AddressConfig) -> io::Result<()> {
        if config.broadcast.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "broadcast addresses are not supported on macOS utun devices",
            ));
        }
        match config.address {
            IpAddr::V4(address) => {
                self.add_ipv4_address(address, config.prefix_len, config.destination)
            }
            IpAddr::V6(address) => {
                self.add_ipv6_address(address, config.prefix_len, config.destination)
            }
        }
    }

    fn add_ipv4_address(
        &self,
        address: Ipv4Addr,
        prefix_len: u8,
        destination: Option<IpAddr>,
    ) -> io::Result<()> {
        let peer = match destination {
            Some(IpAddr::V4(peer)) => peer,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination address family must match the address family",
                ));
            }
            None => address,
        };

        // SAFETY: `IfAliasReq` is a C POD type and an all-zero value is valid.
        let mut req: IfAliasReq = unsafe { mem::zeroed() };
        write_ifname(&mut req.ifra_name, &self.name);
        req.ifra_addr = sockaddr_v4(address);
        req.ifra_mask = sockaddr_v4(prefix_to_ipv4_mask(prefix_len));
        req.ifra_broadaddr = sockaddr_v4(peer);
        ioctl_const(self.socket, SIOCAIFADDR, &req)
    }

    fn add_ipv6_address(
        &self,
        address: Ipv6Addr,
        prefix_len: u8,
        destination: Option<IpAddr>,
    ) -> io::Result<()> {
        if destination.is_some() && prefix_len != 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "macOS utun requires IPv6 destinations to use a /128 prefix",
            ));
        }
        let socket6 = self
            .socket6
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "IPv6 is not available"))?;

        // SAFETY: `In6AliasReq` is a C POD type and an all-zero value is valid.
        let mut req: In6AliasReq = unsafe { mem::zeroed() };
        write_ifname(&mut req.ifra_name, &self.name);
        req.ifra_addr = sockaddr_v6(address);
        req.ifra_prefixmask = sockaddr_v6(prefix_to_ipv6_mask(prefix_len));
        if let Some(destination) = destination {
            match destination {
                IpAddr::V6(peer) => req.ifra_dstaddr = sockaddr_v6(peer),
                IpAddr::V4(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "destination address family must match the address family",
                    ));
                }
            }
        }
        req.ifra_flags = IN6_IFF_NODAD;
        req.ifra_lifetime.ia6t_vltime = u32::MAX;
        req.ifra_lifetime.ia6t_pltime = u32::MAX;
        ioctl_const(socket6, SIOCAIFADDR_IN6, &req)
    }

    fn ipv4_prefix_len(ifa: &libc::ifaddrs) -> u8 {
        if !sockaddr_is(ifa.ifa_netmask, libc::AF_INET, IPV4_SOCKADDR_LEN) {
            return 0;
        }
        let len = sockaddr_len(ifa.ifa_netmask);
        prefix_len_from_ipv4_mask(ipv4_from_sockaddr(ifa.ifa_netmask, len))
    }

    fn ipv6_prefix_len(ifa: &libc::ifaddrs) -> u8 {
        if !sockaddr_is(
            ifa.ifa_netmask,
            libc::AF_INET6,
            mem::size_of::<libc::sockaddr_in6>(),
        ) {
            return 0;
        }
        // SAFETY: the family and length match `sockaddr_in6`.
        let mask = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in6) };
        prefix_len_from_ipv6_mask(ipv6_from_sockaddr_in6(mask))
    }

    fn ipv4_destination(ifa: &libc::ifaddrs, address: Ipv4Addr) -> Option<IpAddr> {
        if !sockaddr_is(ifa.ifa_dstaddr, libc::AF_INET, IPV4_SOCKADDR_LEN) {
            return None;
        }
        let len = sockaddr_len(ifa.ifa_dstaddr);
        let destination = ipv4_from_sockaddr(ifa.ifa_dstaddr, len);
        (!destination.is_unspecified() && destination != address).then_some(IpAddr::V4(destination))
    }

    fn ipv6_destination(ifa: &libc::ifaddrs, address: Ipv6Addr) -> Option<IpAddr> {
        if !sockaddr_is(
            ifa.ifa_dstaddr,
            libc::AF_INET6,
            mem::size_of::<libc::sockaddr_in6>(),
        ) {
            return None;
        }
        // SAFETY: the family and length match `sockaddr_in6`.
        let destination = unsafe { &*(ifa.ifa_dstaddr as *const libc::sockaddr_in6) };
        let destination = ipv6_from_sockaddr_in6(destination);
        (!destination.is_unspecified() && destination != address).then_some(IpAddr::V6(destination))
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        // SAFETY: `Interface` owns these descriptors and closes each once.
        unsafe {
            libc::close(self.socket);
        }
        if let Some(socket6) = self.socket6 {
            // SAFETY: `socket6` is an owned descriptor when present.
            unsafe { libc::close(socket6) };
        }
        for &fd in &self.fds {
            // SAFETY: remaining descriptors are owned by `Interface`.
            unsafe { libc::close(fd) };
        }
    }
}

fn sockaddr_len(ptr: *const libc::sockaddr) -> usize {
    // SAFETY: every getifaddrs sockaddr starts with the one-byte `sa_len`.
    unsafe { ptr.cast::<u8>().read() as usize }
}

fn sockaddr_family(ptr: *const libc::sockaddr) -> i32 {
    // SAFETY: every getifaddrs sockaddr has the one-byte `sa_family` at offset
    // 1. This intentionally avoids forming a `&sockaddr`, because Darwin may
    // report a short IPv4 netmask (for example `sa_len == 5`) even though
    // `size_of::<sockaddr>()` is 16.
    unsafe { ptr.cast::<u8>().add(1).read() as i32 }
}

fn sockaddr_is(ptr: *const libc::sockaddr, family: i32, len: usize) -> bool {
    if ptr.is_null() {
        return false;
    }
    sockaddr_family(ptr) == family && sockaddr_len(ptr) >= len
}

struct Ifaddrs(*mut libc::ifaddrs);

impl Drop for Ifaddrs {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the list was allocated by getifaddrs and is owned here.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
}

fn create_utun(unit: u32, cloexec: bool) -> io::Result<OwnedFd> {
    // SAFETY: `socket` is called with valid constants and returns an owned
    // descriptor or -1.
    let fd = unsafe { libc::socket(libc::PF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a valid, owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if cloexec {
        // Set CLOEXEC as early as possible; macOS has no atomic
        // `SOCK_CLOEXEC` equivalent.
        set_cloexec(fd.as_raw_fd())?;
    }

    // SAFETY: `ctl_info` is a C POD type and an all-zero value is valid.
    let mut info: libc::ctl_info = unsafe { mem::zeroed() };
    for (dst, src) in info.ctl_name.iter_mut().zip(UTUN_CONTROL_NAME) {
        *dst = libc::c_char::from_ne_bytes([*src]);
    }
    ioctl_mut(fd.as_raw_fd(), libc::CTLIOCGINFO, &mut info)?;

    let address = libc::sockaddr_ctl {
        sc_len: mem::size_of::<libc::sockaddr_ctl>() as u8,
        sc_family: libc::AF_SYSTEM as u8,
        ss_sysaddr: libc::AF_SYS_CONTROL as u16,
        sc_id: info.ctl_id,
        sc_unit: unit,
        sc_reserved: [0; 5],
    };

    // SAFETY: `address` is a fully initialized `sockaddr_ctl`, the length is
    // correct, and `fd` is an open control socket owned by this function.
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_ctl>() as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    set_nonblocking(fd.as_raw_fd())?;
    Ok(fd)
}

fn create_socket(family: i32, cloexec: bool) -> io::Result<OwnedFd> {
    // SAFETY: `socket` is called with valid constants and returns an owned
    // descriptor or -1.
    let fd = unsafe { libc::socket(family, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a valid, owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if cloexec {
        set_cloexec(fd.as_raw_fd())?;
    }
    Ok(fd)
}

fn utun_name(fd: i32) -> io::Result<String> {
    let mut buffer = [0u8; 256];
    let mut len = buffer.len() as libc::socklen_t;
    // SAFETY: `buffer` is writable, `len` describes its size, and `fd` is an
    // open utun control socket.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SYSPROTO_CONTROL,
            libc::UTUN_OPT_IFNAME,
            buffer.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    let len = (len as usize).min(buffer.len());
    let end = buffer[..len]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(len);
    String::from_utf8(buffer[..end].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "utun name is not valid UTF-8"))
}

fn if_nametoindex(name: &str) -> io::Result<u32> {
    let name = CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "interface name contains a NUL byte",
        )
    })?;
    // SAFETY: `name` is NUL-terminated and valid for the call.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(index)
}

fn set_nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is an open descriptor and `flags` came from F_GETFL.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_cloexec(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is an open descriptor and `flags` came from F_GETFD.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ifreq_for(name: &str) -> libc::ifreq {
    // SAFETY: `ifreq` is a C POD type and an all-zero value is valid.
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    write_ifname(&mut req.ifr_name, name);
    req
}

fn write_ifname(dst: &mut [libc::c_char], name: &str) {
    let bytes = name.as_bytes();
    let len = bytes.len().min(dst.len().saturating_sub(1));
    for (dst, src) in dst.iter_mut().zip(&bytes[..len]) {
        *dst = libc::c_char::from_ne_bytes([*src]);
    }
    if let Some(last) = dst.get_mut(len) {
        *last = 0;
    }
}

fn ioctl_mut<T>(fd: i32, request: libc::c_ulong, arg: &mut T) -> io::Result<()> {
    // SAFETY: the caller provides a valid, writable pointer for `request`.
    let result = unsafe { libc::ioctl(fd, request, arg as *mut T) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ioctl_const<T>(fd: i32, request: libc::c_ulong, arg: &T) -> io::Result<()> {
    // SAFETY: the caller provides a valid pointer for `request`.
    let result = unsafe { libc::ioctl(fd, request, arg as *const T) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_interface_names() {
        let mut name = [0; libc::IFNAMSIZ];
        write_ifname(&mut name, "utun12");
        let bytes: Vec<u8> = name.iter().map(|byte| byte.to_ne_bytes()[0]).collect();
        assert_eq!(&bytes[..6], b"utun12");
        assert_eq!(bytes[6], 0);
    }

    #[test]
    fn reads_loopback_addresses() {
        let addresses = Interface::addresses_for("lo0").unwrap();
        assert!(addresses.iter().any(|address| {
            address.address == IpAddr::V4(Ipv4Addr::LOCALHOST) && address.prefix_len == 8
        }));
        assert!(addresses.iter().any(|address| {
            address.address == IpAddr::V6(Ipv6Addr::LOCALHOST) && address.prefix_len == 128
        }));
    }

    #[test]
    fn reads_short_ipv4_netmask() {
        // SAFETY: `sockaddr`/`ifaddrs` are C POD types; all-zero is valid.
        let mut mask: libc::sockaddr = unsafe { mem::zeroed() };
        mask.sa_family = libc::AF_INET as u8;
        // SAFETY: `ifaddrs` is a C POD type; all-zero is valid.
        let mut ifa: libc::ifaddrs = unsafe { mem::zeroed() };
        ifa.ifa_netmask = &mut mask;

        // macOS omits trailing zero bytes: /8 is reported as only `255`.
        mask.sa_len = 5;
        // SAFETY: `mask` is large enough for the 1-byte address at offset 4.
        unsafe { *(&mut mask as *mut libc::sockaddr as *mut u8).add(4) = 255 };
        assert_eq!(Interface::ipv4_prefix_len(&ifa), 8);

        // A full IPv4 netmask is reported with four address bytes.
        mask.sa_len = 8;
        let mask_bytes = [255u8, 255, 255, 0];
        // SAFETY: `mask` is large enough for the 4-byte address at offset 4.
        unsafe {
            std::ptr::copy_nonoverlapping(
                mask_bytes.as_ptr(),
                (&mut mask as *mut libc::sockaddr as *mut u8).add(4),
                4,
            );
        }
        assert_eq!(Interface::ipv4_prefix_len(&ifa), 24);
    }

    #[test]
    fn ignores_empty_ipv6_destination() {
        let address: Ipv6Addr = "fd00:1::1".parse().unwrap();
        let mut addr = sockaddr_v6(address);
        let mut mask = sockaddr_v6(prefix_to_ipv6_mask(128));
        // SAFETY: `sockaddr`/`ifaddrs` are C POD types; all-zero is valid.
        let mut dst: libc::sockaddr = unsafe { mem::zeroed() };
        // SAFETY: `ifaddrs` is a C POD type; all-zero is valid.
        let mut ifa: libc::ifaddrs = unsafe { mem::zeroed() };
        ifa.ifa_addr = &mut addr as *mut _ as *mut libc::sockaddr;
        ifa.ifa_netmask = &mut mask as *mut _ as *mut libc::sockaddr;
        ifa.ifa_dstaddr = &mut dst;

        assert_eq!(Interface::ipv6_destination(&ifa, address), None);
        assert_eq!(Interface::ipv6_prefix_len(&ifa), 128);
    }

    #[test]
    fn reads_valid_ipv6_destination() {
        let address: Ipv6Addr = "fd00:1::1".parse().unwrap();
        let peer: Ipv6Addr = "fd00:1::2".parse().unwrap();
        let mut addr = sockaddr_v6(address);
        let mut mask = sockaddr_v6(prefix_to_ipv6_mask(128));
        let mut dst = sockaddr_v6(peer);
        // SAFETY: `ifaddrs` is a C POD type; all-zero is valid.
        let mut ifa: libc::ifaddrs = unsafe { mem::zeroed() };
        ifa.ifa_addr = &mut addr as *mut _ as *mut libc::sockaddr;
        ifa.ifa_netmask = &mut mask as *mut _ as *mut libc::sockaddr;
        ifa.ifa_dstaddr = &mut dst as *mut _ as *mut libc::sockaddr;

        assert_eq!(
            Interface::ipv6_destination(&ifa, address),
            Some(IpAddr::V6(peer))
        );
    }

    #[test]
    fn reads_short_sockaddr_header_without_full_reference() {
        // Darwin may report a short IPv4 netmask (sa_len == 5). The helpers
        // must read only the header bytes, never form a full `&sockaddr`.
        let mut short = [0u8; 5];
        short[0] = 5;
        short[1] = libc::AF_INET as u8;
        let ptr = short.as_ptr().cast::<libc::sockaddr>();
        assert_eq!(sockaddr_len(ptr), 5);
        assert_eq!(sockaddr_family(ptr), libc::AF_INET);
        assert!(sockaddr_is(ptr, libc::AF_INET, IPV4_SOCKADDR_LEN));
    }

    #[test]
    fn validates_sockaddr_family_and_length() {
        // SAFETY: `sockaddr` is a C POD type; all-zero is valid.
        let mut sockaddr: libc::sockaddr = unsafe { mem::zeroed() };
        let len = mem::size_of::<libc::sockaddr_in6>();

        sockaddr.sa_len = len as u8;
        sockaddr.sa_family = libc::AF_INET6 as u8;
        assert!(sockaddr_is(&sockaddr, libc::AF_INET6, len));

        sockaddr.sa_len = 0;
        assert!(!sockaddr_is(&sockaddr, libc::AF_INET6, len));
        assert!(!sockaddr_is(ptr::null(), libc::AF_INET6, len));

        // SAFETY: `sockaddr` is a C POD type; all-zero is valid.
        let mut sockaddr4: libc::sockaddr = unsafe { mem::zeroed() };
        sockaddr4.sa_len = IPV4_SOCKADDR_LEN as u8;
        sockaddr4.sa_family = libc::AF_INET as u8;
        assert!(sockaddr_is(&sockaddr4, libc::AF_INET, IPV4_SOCKADDR_LEN));
        sockaddr4.sa_len = 3;
        assert!(!sockaddr_is(&sockaddr4, libc::AF_INET, IPV4_SOCKADDR_LEN));
    }

    #[test]
    fn truncates_long_interface_names() {
        let mut name = [1; libc::IFNAMSIZ];
        write_ifname(&mut name, "abcdefghijklmnop");
        let bytes: Vec<u8> = name.iter().map(|byte| byte.to_ne_bytes()[0]).collect();
        assert_eq!(&bytes[..15], b"abcdefghijklmno");
        assert_eq!(bytes[15], 0);
    }
}
