use super::address::{
    ipv4_from_sockaddr, ipv6_from_sockaddr, prefix_len_from_ipv4_mask, prefix_len_from_ipv6_mask,
    prefix_to_ipv4_mask, prefix_to_ipv6_mask, sockaddr_v4, sockaddr_v6,
};
use super::params::Params;
use super::sys::*;
use crate::address::{AddressConfig, AddressInfo, ipv4_broadcast};
use crate::builder::MAX_INTERFACE_NAME_LEN;
use std::ffi::{CStr, CString};
use std::io;
use std::mem;
use std::net::{IpAddr, Ipv4Addr};
use std::ptr;

// Compile-time layout checks: the `ifaliasreq`/`in6_aliasreq` ioctls encode
// their size in the request code, so struct drift must fail at build time.
const _: () = assert!(mem::size_of::<libc::ifreq>() == 32);

pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    socket6: Option<i32>,
    index: u32,
    name: String,
    is_tun: bool,
}

impl Interface {
    pub fn new(params: &Params) -> io::Result<Self> {
        let prefix = if params.tap { "tap" } else { "tun" };
        let requested = params.name.as_deref().unwrap_or("");
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
                return Err(err);
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
                    return Err(err);
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

    pub fn init(&self, params: &Params) -> io::Result<()> {
        self.index()?;
        if let Some(mtu) = params.mtu {
            self.mtu(Some(mtu))?;
        }
        if let Some(mac) = params.mac_address {
            self.set_mac_address(mac)?;
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
            ioctl_ptr(self.socket, SIOCSIFMTU, &req)?;
            Ok(mtu)
        } else {
            ioctl_ptr_mut(self.socket, SIOCGIFMTU, &mut req)?;
            // SAFETY: a successful SIOCGIFMTU initialized `ifru_mtu`.
            Ok(unsafe { req.ifr_ifru.ifru_mtu })
        }
    }

    pub fn flags(&self, flags: Option<i16>) -> io::Result<u32> {
        self.index()?;
        let mut req = ifreq_for(&self.name);
        ioctl_ptr_mut(self.socket, SIOCGIFFLAGS, &mut req)?;
        if let Some(flags) = flags {
            // SAFETY: the union was initialized by SIOCGIFFLAGS above.
            unsafe {
                req.ifr_ifru.ifru_flags[0] |= flags;
            }
            ioctl_ptr(self.socket, SIOCSIFFLAGS, &req)?;
        }
        // FreeBSD stores interface flags in two 16-bit halves.
        // SAFETY: the union was initialized by SIOCGIFFLAGS above.
        let [low, high] = unsafe { req.ifr_ifru.ifru_flags };
        Ok((low as u16 as u32) | ((high as u16 as u32) << 16))
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
        let mut ifap: *mut libc::ifaddrs = ptr::null_mut();
        // SAFETY: `ifap` is a valid out-pointer and the returned list is
        // released by `freeifaddrs` below.
        if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
            return Err(io::Error::last_os_error());
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
                    sockaddr_family(ifa.ifa_addr)
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
                let broadcast = if let IpAddr::V4(ipv4) = address {
                    if prefix_len < 31
                        && ifa.ifa_flags & libc::IFF_BROADCAST as u32 != 0
                        && sockaddr_is(
                            ifa.ifa_dstaddr,
                            libc::AF_INET,
                            mem::size_of::<libc::sockaddr_in>(),
                        )
                    {
                        let broadcast = ipv4_from_sockaddr(ifa.ifa_dstaddr);
                        (ipv4_broadcast(ipv4, prefix_len) == Some(broadcast)).then_some(broadcast)
                    } else {
                        None
                    }
                } else {
                    None
                };
                addresses.push(AddressInfo {
                    address,
                    prefix_len,
                    destination,
                    broadcast,
                });
            }
            current = ifa.ifa_next;
        }
        Ok(addresses)
    }

    pub fn mac_address(&self) -> io::Result<[u8; 6]> {
        self.index()?;
        if self.is_tun {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            ));
        }

        let mut ifap: *mut libc::ifaddrs = ptr::null_mut();
        // SAFETY: `ifap` is a valid out-pointer and the returned list is
        // released by `freeifaddrs` below.
        if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let guard = Ifaddrs(ifap);
        let mut current = guard.0;
        while !current.is_null() {
            // SAFETY: `current` is a node in the list returned by getifaddrs.
            let ifa = unsafe { &*current };
            if !ifa.ifa_name.is_null()
                && unsafe { CStr::from_ptr(ifa.ifa_name) }.to_bytes() == self.name.as_bytes()
                && !ifa.ifa_addr.is_null()
                && sockaddr_family(ifa.ifa_addr) == libc::AF_LINK
            {
                // `sockaddr_dl` fields are read from raw bytes so a short
                // link-layer sockaddr never becomes a full `&sockaddr_dl`.
                let base = ifa.ifa_addr.cast::<u8>();
                // SAFETY: every sockaddr_dl has sdl_len at offset 0.
                let sdl_len = unsafe { base.read() } as usize;
                if sdl_len >= 8 {
                    // SAFETY: `sdl_len >= 8` makes sdl_nlen (offset 5) and
                    // sdl_alen (offset 6) readable.
                    let sdl_nlen = unsafe { base.add(5).read() } as usize;
                    // SAFETY: see above.
                    let sdl_alen = unsafe { base.add(6).read() } as usize;
                    let start = 8usize.saturating_add(sdl_nlen);
                    let end = start.saturating_add(6);
                    if sdl_alen == 6 && sdl_len >= end {
                        // SAFETY: `sdl_len` bounds the sockaddr_dl allocation
                        // and `end <= sdl_len`.
                        let data = unsafe { std::slice::from_raw_parts(base.add(start), 6) };
                        let mut mac = [0u8; 6];
                        mac.copy_from_slice(data);
                        return Ok(mac);
                    }
                }
            }
            current = ifa.ifa_next;
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "interface has no hardware address",
        ))
    }

    pub fn set_mac_address(&self, mac: [u8; 6]) -> io::Result<()> {
        self.index()?;
        if self.is_tun {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            ));
        }
        crate::address::validate_mac(mac)
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;

        let mut req = ifreq_for(&self.name);
        // SAFETY: `ifreq` is fully initialized before the ioctl call.
        unsafe {
            req.ifr_ifru.ifru_addr.sa_len = 6;
            req.ifr_ifru.ifru_addr.sa_family = libc::AF_LINK as _;
            for (dst, src) in req.ifr_ifru.ifru_addr.sa_data.iter_mut().zip(mac) {
                *dst = src as libc::c_char;
            }
        }
        ioctl_ptr(self.socket, SIOCSIFLLADDR, &req)
    }

    pub fn broadcast(&self) -> io::Result<Ipv4Addr> {
        self.index()?;
        if self.is_tun {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no broadcast address",
            ));
        }
        self.addresses()?
            .iter()
            .find(|address| address.address.is_ipv4())
            .and_then(|address| address.broadcast)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "interface has no broadcast address",
                )
            })
    }

    fn add_address(&self, config: &AddressConfig) -> io::Result<()> {
        match config.address {
            IpAddr::V4(address) => {
                // SAFETY: `IfAliasReq` is a C POD type; all-zero is valid.
                let mut req: IfAliasReq = unsafe { mem::zeroed() };
                copy_name(&self.name, &mut req.ifra_name)?;
                req.ifra_addr = sockaddr_v4(address);
                let destination = if self.is_tun {
                    Some(config.destination.unwrap_or(IpAddr::V4(address)))
                } else {
                    None
                };
                if let Some(destination) = destination {
                    let IpAddr::V4(destination) = destination else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "destination address family mismatch",
                        ));
                    };
                    req.ifra_broadaddr = sockaddr_v4(destination);
                } else if let Some(broadcast) = config.broadcast {
                    req.ifra_broadaddr = sockaddr_v4(broadcast);
                }
                req.ifra_mask = sockaddr_v4(prefix_to_ipv4_mask(config.prefix_len));
                ioctl_ptr(self.socket, SIOCAIFADDR, &req)
            }
            IpAddr::V6(address) => {
                // SAFETY: `In6AliasReq` is a C POD type; all-zero is valid.
                let mut req: In6AliasReq = unsafe { mem::zeroed() };
                copy_name(&self.name, &mut req.ifra_name)?;
                req.ifra_addr = sockaddr_v6(address);
                if let Some(destination) = config.destination {
                    let IpAddr::V6(destination) = destination else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "destination address family mismatch",
                        ));
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

pub(crate) fn set_persist(fd: i32, persist: bool) -> io::Result<()> {
    let transient: libc::c_int = if persist { 0 } else { 1 };
    ioctl_ptr(fd, TUNSTRANSIENT, &transient)
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

fn sockaddr_len(ptr: *const libc::sockaddr) -> usize {
    // SAFETY: every getifaddrs sockaddr starts with the one-byte `sa_len`.
    unsafe { ptr.cast::<u8>().read() as usize }
}

fn sockaddr_family(ptr: *const libc::sockaddr) -> i32 {
    // SAFETY: every getifaddrs sockaddr has the one-byte `sa_family` at offset
    // 1.
    unsafe { ptr.cast::<u8>().add(1).read() as i32 }
}

fn sockaddr_is(address: *const libc::sockaddr, family: libc::c_int, min_len: usize) -> bool {
    if address.is_null() {
        return false;
    }
    sockaddr_family(address) == family && sockaddr_len(address) >= min_len
}

struct Ifaddrs(*mut libc::ifaddrs);

impl Drop for Ifaddrs {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
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

fn device_name(fd: i32) -> io::Result<String> {
    let mut req = ifreq_for("");
    // SAFETY: TUNGIFNAME writes the interface name into the ifreq buffer.
    if unsafe { libc::ioctl(fd, TUNGIFNAME, &mut req) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes: Vec<u8> = req
        .ifr_name
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid interface name"))
}

fn destroy_device(req: &libc::ifreq) {
    if let Ok(socket) = create_socket(libc::AF_INET, 0) {
        let _ = ioctl_ptr(socket, SIOCIFDESTROY, req);
        // SAFETY: `socket` is owned by this function.
        unsafe { libc::close(socket) };
    }
}

fn create_device(name: &str, cloexec: bool) -> io::Result<i32> {
    let socket = create_socket(libc::AF_INET, 0)?;
    let mut req = ifreq_for(name);
    let result = ioctl_ptr_mut(socket, SIOCIFCREATE2, &mut req);
    // SAFETY: `socket` is owned by this function.
    unsafe { libc::close(socket) };
    result?;

    let fd = match open_path(&format!("/dev/{name}"), cloexec) {
        Ok(fd) => fd,
        Err(err) => {
            destroy_device(&req);
            return Err(err);
        }
    };
    if let Err(err) = set_persist(fd, false) {
        // Closing the last descriptor of a transient device destroys it by
        // identity. Do not follow up with a name-based `SIOCIFDESTROY`: the
        // name could already have been reused by another process.
        // SAFETY: `fd` is owned by this function on the error path.
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

fn open_device(prefix: &str, requested: &str, cloexec: bool) -> io::Result<(i32, String)> {
    if !requested.is_empty() {
        if requested.len() > MAX_INTERFACE_NAME_LEN
            || !requested.starts_with(prefix)
            || requested.contains('\0')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name must be tunN or tapN",
            ));
        }
        let suffix = &requested[prefix.len()..];
        if suffix.is_empty()
            || (suffix.len() > 1 && suffix.starts_with('0'))
            || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name must be tunN or tapN",
            ));
        }
        let exists = if_nametoindex(requested).is_ok();
        let fd = if exists {
            open_path(&format!("/dev/{requested}"), cloexec)?
        } else {
            create_device(requested, cloexec)?
        };
        match device_name(fd) {
            Ok(actual) if actual == requested => return Ok((fd, requested.to_owned())),
            Ok(actual) => {
                // SAFETY: `fd` is owned by this function on the error path.
                // Closing the last descriptor of a transient device destroys
                // it; destroying by name afterwards would race with reuse.
                unsafe { libc::close(fd) };
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("device {requested} is attached to interface {actual}"),
                ));
            }
            Err(err) => {
                // SAFETY: `fd` is owned by this function on the error path.
                // Closing the last descriptor of a transient device destroys
                // it; destroying by name afterwards would race with reuse.
                unsafe { libc::close(fd) };
                return Err(err);
            }
        }
    }

    let mut last_error = None;
    for index in 0..256 {
        let name = format!("{prefix}{index}");
        if if_nametoindex(&name).is_ok() {
            continue;
        }
        match create_device(&name, cloexec) {
            Ok(fd) => match device_name(fd) {
                Ok(actual) if actual == name => return Ok((fd, name)),
                Ok(_) => {
                    // SAFETY: `fd` is owned by this function on the error path.
                    // Closing the last descriptor of a transient device
                    // destroys it; destroying by name would race with reuse.
                    unsafe { libc::close(fd) };
                    last_error = Some(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("device {name} is attached to a renamed interface"),
                    ));
                }
                Err(err) => {
                    // SAFETY: `fd` is owned by this function on the error path.
                    // Closing the last descriptor of a transient device
                    // destroys it; destroying by name would race with reuse.
                    unsafe { libc::close(fd) };
                    last_error = Some(err);
                }
            },
            Err(err) => {
                if matches!(
                    err.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                ) {
                    return Err(err);
                }
                last_error = Some(err);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no TUN/TAP device found")))
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
    // SAFETY: `ifreq` is a C POD type; all-zero is valid.
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    // The builder and Interface validate names before this point.
    let bytes = name.as_bytes();
    let len = bytes.len().min(libc::IFNAMSIZ - 1);
    for (dst, src) in req.ifr_name.iter_mut().zip(bytes.iter().take(len)) {
        *dst = *src as libc::c_char;
    }
    req
}

fn copy_name(name: &str, dst: &mut [libc::c_char; libc::IFNAMSIZ]) -> io::Result<()> {
    if name.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interface name too long",
        ));
    }
    for (dst, src) in dst.iter_mut().zip(name.bytes()) {
        *dst = src as libc::c_char;
    }
    Ok(())
}

fn ioctl_ptr<T>(fd: i32, request: libc::c_ulong, value: &T) -> io::Result<()> {
    // SAFETY: `fd` is an open socket/device and `value` is valid for the ioctl.
    let result = unsafe { libc::ioctl(fd, request, value as *const T) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ioctl_ptr_mut<T>(fd: i32, request: libc::c_ulong, value: &mut T) -> io::Result<()> {
    // SAFETY: `fd` is an open socket/device and `value` is valid for the ioctl.
    let result = unsafe { libc::ioctl(fd, request, value as *mut T) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_short_sockaddr_header_without_full_reference() {
        let mut short = [0u8; 5];
        short[0] = 5;
        short[1] = libc::AF_INET as u8;
        let ptr = short.as_ptr().cast::<libc::sockaddr>();
        assert_eq!(sockaddr_len(ptr), 5);
        assert_eq!(sockaddr_family(ptr), libc::AF_INET);
        assert!(sockaddr_is(ptr, libc::AF_INET, 4));
    }
}
