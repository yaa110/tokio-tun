use super::address::{
    ipv4_from_sockaddr, ipv6_from_sockaddr, prefix_len_from_ipv4_mask, prefix_len_from_ipv6_mask,
    prefix_to_ipv4_mask, prefix_to_ipv6_mask, sockaddr_v4, sockaddr_v6,
};
use super::params::Params;
use super::sys::*;
use crate::Result;
use crate::address::{AddressConfig, AddressInfo};
use std::ffi::{CStr, CString};
use std::io;
use std::mem;
use std::net::{IpAddr, Ipv4Addr};
use std::ptr;

const MAX_INTERFACE_NAME_LEN: usize = 15;

pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    socket6: Option<i32>,
    index: u32,
    name: String,
    is_tun: bool,
}

impl Interface {
    pub fn new(params: &Params) -> Result<Self> {
        let prefix = if params.tap { "tap" } else { "tun" };
        let requested = params.name.as_deref().unwrap_or("");
        let existed = !requested.is_empty() && if_nametoindex(requested).is_ok();
        let (fd, name) = open_device(prefix, requested, params.cloexec)?;

        if !params.tap {
            // BSD TUN devices prepend address-family headers only when
            // TUNSIFHEAD is enabled. Enable it so IPv4 and IPv6 both work.
            let enable: libc::c_int = 1;
            // SAFETY: `fd` is open and TUNSIFHEAD expects an `int` pointer.
            if unsafe { libc::ioctl(fd, TUNSIFHEAD, &enable) } < 0 {
                let err = io::Error::last_os_error();
                // SAFETY: `fd` is owned by this function on the error path.
                unsafe { libc::close(fd) };
                return Err(err.into());
            }
        }

        if !existed {
            let transient: libc::c_int = 1;
            // SAFETY: `fd` is open and TUNSTRANSIENT expects an `int` pointer.
            if unsafe { libc::ioctl(fd, TUNSTRANSIENT, &transient) } < 0 {
                let err = io::Error::last_os_error();
                // SAFETY: `fd` is owned by this function on the error path.
                unsafe { libc::close(fd) };
                return Err(err.into());
            }
        }

        let index = if_nametoindex(&name).inspect_err(|_| {
            // SAFETY: `fd` is owned by this function on the error path.
            unsafe { libc::close(fd) };
        })?;
        let extra_flags = if params.cloexec {
            libc::SOCK_CLOEXEC
        } else {
            0
        };
        let socket = create_socket(libc::AF_INET, extra_flags).inspect_err(|_| {
            // SAFETY: `fd` is owned by this function on the error path.
            unsafe { libc::close(fd) };
        })?;
        let socket6 = if params
            .addresses
            .iter()
            .any(|address| address.address.is_ipv6())
        {
            match create_socket(libc::AF_INET6, extra_flags) {
                Ok(socket) => Some(socket),
                Err(err) => {
                    // SAFETY: both descriptors are owned by this function.
                    unsafe {
                        libc::close(fd);
                        libc::close(socket);
                    }
                    return Err(err.into());
                }
            }
        } else {
            None
        };

        Ok(Self {
            fds: vec![fd],
            socket,
            socket6,
            index,
            name,
            is_tun: !params.tap,
        })
    }

    pub fn init(&self, params: &Params) -> Result<()> {
        if let Some(mtu) = params.mtu {
            self.mtu(Some(mtu))?;
        }
        for address in &params.addresses {
            self.add_address(address)?;
        }
        if params.up {
            self.flags(Some(libc::IFF_UP as i16))?;
        }
        Ok(())
    }

    pub fn take_files(&mut self) -> Vec<i32> {
        mem::take(&mut self.fds)
    }

    pub fn is_tun(&self) -> bool {
        self.is_tun
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn index(&self) -> Result<u32> {
        Ok(self.index)
    }

    pub fn mtu(&self, mtu: Option<i32>) -> Result<i32> {
        let mut req = ifreq_for(&self.name);
        if let Some(mtu) = mtu {
            req.ifr_ifru.ifru_mtu = mtu;
            ioctl_ptr(self.socket, SIOCSIFMTU, &req)?;
            Ok(mtu)
        } else {
            ioctl_ptr_mut(self.socket, SIOCGIFMTU, &mut req)?;
            Ok(unsafe { req.ifr_ifru.ifru_mtu })
        }
    }

    pub fn flags(&self, flags: Option<i16>) -> Result<i16> {
        let mut req = ifreq_for(&self.name);
        ioctl_ptr_mut(self.socket, SIOCGIFFLAGS, &mut req)?;
        if let Some(flags) = flags {
            unsafe {
                req.ifr_ifru.ifru_flags[0] |= flags;
            }
            ioctl_ptr(self.socket, SIOCSIFFLAGS, &req)?;
        }
        Ok(unsafe { req.ifr_ifru.ifru_flags[0] })
    }

    pub fn netmask(&self) -> Result<Ipv4Addr> {
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

    pub fn addresses(&self) -> Result<Vec<AddressInfo>> {
        let mut ifap: *mut libc::ifaddrs = ptr::null_mut();
        // SAFETY: `ifap` is a valid out-pointer and the returned list is
        // released by `freeifaddrs` below.
        if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let guard = Ifaddrs(ifap);
        let mut addresses = Vec::new();
        let mut current = guard.0;
        while !current.is_null() {
            // SAFETY: `current` is a node in the list returned by getifaddrs.
            let ifa = unsafe { &*current };
            let name = if ifa.ifa_name.is_null() {
                None
            } else {
                // SAFETY: `ifa_name` is a NUL-terminated string owned by the list.
                Some(unsafe { CStr::from_ptr(ifa.ifa_name) }.to_string_lossy())
            };
            if name.as_deref() == Some(self.name.as_str()) {
                let family = if ifa.ifa_addr.is_null() {
                    0
                } else {
                    // SAFETY: `ifa_addr` is a getifaddrs sockaddr pointer.
                    i32::from(unsafe { &*ifa.ifa_addr }.sa_family)
                };
                let (min_len, prefix_len) = match family {
                    libc::AF_INET => (
                        mem::size_of::<libc::sockaddr_in>(),
                        if sockaddr_is(
                            ifa.ifa_netmask,
                            libc::AF_INET,
                            mem::size_of::<libc::sockaddr_in>(),
                        ) {
                            Some(prefix_len_from_ipv4_mask(ipv4_from_sockaddr(
                                ifa.ifa_netmask,
                            )))
                        } else {
                            None
                        },
                    ),
                    libc::AF_INET6 => (
                        mem::size_of::<libc::sockaddr_in6>(),
                        if sockaddr_is(
                            ifa.ifa_netmask,
                            libc::AF_INET6,
                            mem::size_of::<libc::sockaddr_in6>(),
                        ) {
                            Some(prefix_len_from_ipv6_mask(ipv6_from_sockaddr(
                                ifa.ifa_netmask,
                            )))
                        } else {
                            None
                        },
                    ),
                    _ => {
                        current = ifa.ifa_next;
                        continue;
                    }
                };
                if !sockaddr_is(ifa.ifa_addr, family, min_len) {
                    current = ifa.ifa_next;
                    continue;
                }
                let Some(prefix_len) = prefix_len else {
                    current = ifa.ifa_next;
                    continue;
                };
                let address = match family {
                    libc::AF_INET => IpAddr::V4(ipv4_from_sockaddr(ifa.ifa_addr)),
                    libc::AF_INET6 => IpAddr::V6(ipv6_from_sockaddr(ifa.ifa_addr)),
                    _ => unreachable!(),
                };
                let destination = if ifa.ifa_flags & libc::IFF_POINTOPOINT as u32 != 0
                    && sockaddr_is(ifa.ifa_dstaddr, family, min_len)
                {
                    let destination = match family {
                        libc::AF_INET => IpAddr::V4(ipv4_from_sockaddr(ifa.ifa_dstaddr)),
                        libc::AF_INET6 => IpAddr::V6(ipv6_from_sockaddr(ifa.ifa_dstaddr)),
                        _ => unreachable!(),
                    };
                    (destination != address && !destination.is_unspecified()).then_some(destination)
                } else {
                    None
                };
                addresses.push(AddressInfo {
                    address,
                    prefix_len,
                    destination,
                    broadcast: None,
                });
            }
            current = ifa.ifa_next;
        }
        Ok(addresses)
    }

    fn add_address(&self, config: &AddressConfig) -> Result<()> {
        match config.address {
            IpAddr::V4(address) => {
                let mut req: IfAliasReq = unsafe { mem::zeroed() };
                copy_name(&self.name, &mut req.ifra_name)?;
                req.ifra_addr = sockaddr_v4(address);
                let destination = if self.is_tun {
                    Some(config.destination.unwrap_or(IpAddr::V4(address)))
                } else {
                    config.destination
                };
                if let Some(destination) = destination {
                    let IpAddr::V4(destination) = destination else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "destination address family mismatch",
                        )
                        .into());
                    };
                    req.ifra_dstaddr = sockaddr_v4(destination);
                }
                req.ifra_mask = sockaddr_v4(prefix_to_ipv4_mask(config.prefix_len));
                ioctl_ptr(self.socket, SIOCAIFADDR, &req)
            }
            IpAddr::V6(address) => {
                let mut req: In6AliasReq = unsafe { mem::zeroed() };
                copy_name(&self.name, &mut req.ifra_name)?;
                req.ifra_addr = sockaddr_v6(address);
                if let Some(destination) = config.destination {
                    let IpAddr::V6(destination) = destination else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "destination address family mismatch",
                        )
                        .into());
                    };
                    req.ifra_dstaddr = sockaddr_v6(destination);
                }
                req.ifra_prefixmask = sockaddr_v6(prefix_to_ipv6_mask(config.prefix_len));
                req.ifra_flags = IN6_IFF_NODAD;
                req.ifra_lifetime = In6AddrLifetime {
                    ia6t_expire: 0,
                    ia6t_preferred: 0,
                    ia6t_vltime: u32::MAX,
                    ia6t_pltime: u32::MAX,
                };
                let socket = self.socket6.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "IPv6 socket is not available")
                })?;
                ioctl_ptr(socket, SIOCAIFADDR_IN6, &req)
            }
        }
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        close_fds(&self.fds);
        // SAFETY: these sockets are owned by this value.
        unsafe {
            libc::close(self.socket);
            if let Some(socket) = self.socket6 {
                libc::close(socket);
            }
        }
    }
}

fn sockaddr_is(address: *const libc::sockaddr, family: libc::c_int, min_len: usize) -> bool {
    if address.is_null() {
        return false;
    }
    // SAFETY: `address` is a getifaddrs sockaddr pointer.
    let address = unsafe { &*address };
    i32::from(address.sa_family) == family && usize::from(address.sa_len) >= min_len
}

struct Ifaddrs(*mut libc::ifaddrs);

impl Drop for Ifaddrs {
    fn drop(&mut self) {
        // SAFETY: the list was allocated by getifaddrs and is owned here.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

fn close_fds(fds: &[i32]) {
    for &fd in fds {
        // SAFETY: each fd is owned by the caller.
        unsafe { libc::close(fd) };
    }
}

fn open_device(prefix: &str, requested: &str, cloexec: bool) -> Result<(i32, String)> {
    if !requested.is_empty() {
        if requested.len() > MAX_INTERFACE_NAME_LEN
            || !requested.starts_with(prefix)
            || requested.contains('\0')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name must be tunN or tapN",
            )
            .into());
        }
        let suffix = &requested[prefix.len()..];
        if suffix.is_empty()
            || (suffix.len() > 1 && suffix.starts_with('0'))
            || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name must be tunN or tapN",
            )
            .into());
        }
        let fd = open_path(&format!("/dev/{requested}"), cloexec)?;
        return Ok((fd, requested.to_owned()));
    }

    let mut last_error = None;
    for index in 0..256 {
        let name = format!("{prefix}{index}");
        match open_path(&format!("/dev/{name}"), cloexec) {
            Ok(fd) => return Ok((fd, name)),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no TUN/TAP device found"))
        .into())
}

fn open_path(path: &str, cloexec: bool) -> io::Result<i32> {
    let path = CString::new(path)?;
    let flags = libc::O_RDWR | libc::O_NONBLOCK | if cloexec { libc::O_CLOEXEC } else { 0 };
    // SAFETY: `path` is NUL-terminated and valid for the call.
    let fd = unsafe { libc::open(path.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn create_socket(family: i32, extra_flags: i32) -> io::Result<i32> {
    // SAFETY: `socket` is called with valid constants.
    let fd = unsafe { libc::socket(family, libc::SOCK_DGRAM | extra_flags, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn if_nametoindex(name: &str) -> io::Result<u32> {
    let name = CString::new(name)?;
    // SAFETY: `name` is NUL-terminated and valid for the call.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(index)
}

fn ifreq_for(name: &str) -> libc::ifreq {
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    // The builder and Interface validate names before this point.
    let bytes = name.as_bytes();
    let len = bytes.len().min(libc::IFNAMSIZ - 1);
    for (dst, src) in req.ifr_name.iter_mut().zip(bytes.iter().take(len)) {
        *dst = *src as libc::c_char;
    }
    req
}

fn copy_name(name: &str, dst: &mut [libc::c_char; libc::IFNAMSIZ]) -> Result<()> {
    if name.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long").into());
    }
    for (dst, src) in dst.iter_mut().zip(name.bytes()) {
        *dst = src as libc::c_char;
    }
    Ok(())
}

fn ioctl_ptr<T>(fd: i32, request: libc::c_ulong, value: &T) -> Result<()> {
    // SAFETY: `fd` is an open socket/device and `value` is valid for the ioctl.
    let result = unsafe { libc::ioctl(fd, request, value as *const T) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn ioctl_ptr_mut<T>(fd: i32, request: libc::c_ulong, value: &mut T) -> Result<()> {
    // SAFETY: `fd` is an open socket/device and `value` is valid for the ioctl.
    let result = unsafe { libc::ioctl(fd, request, value as *mut T) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
