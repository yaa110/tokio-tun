use super::params::Params;
use super::request::ifreq;
use crate::Result;
use crate::address::{AddressInfo, ipv4_broadcast};
use crate::linux::address::ipv4_from_sockaddr;
use crate::linux::netlink;
use std::ffi::CString;
use std::io;
use std::mem;
use std::net::Ipv4Addr;

const MAX_INTERFACE_NAME_LEN: usize = 15;

fn ioctl_read(fd: i32, request: libc::Ioctl, req: *mut ifreq) -> Result<()> {
    // SAFETY: The caller guarantees that `req` points to a valid `ifreq` for
    // the duration of the call and that `request` accepts a pointer argument.
    let result = unsafe { libc::ioctl(fd, request, req) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn ioctl_write(fd: i32, request: libc::Ioctl, req: *const ifreq) -> Result<()> {
    // SAFETY: The caller guarantees that `req` points to a valid `ifreq` for
    // the duration of the call and that `request` accepts a pointer argument.
    let result = unsafe { libc::ioctl(fd, request, req) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn ioctl_int(fd: i32, request: libc::Ioctl, value: libc::c_ulong) -> Result<()> {
    // SAFETY: The caller guarantees that `request` accepts an integer argument.
    let result = unsafe { libc::ioctl(fd, request, value) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    index: u32,
    name: String,
    is_tap: bool,
}

impl Interface {
    pub fn new(fds: Vec<i32>, name: &str, mut flags: i16, cloexec: bool) -> Result<Self> {
        if name.len() > MAX_INTERFACE_NAME_LEN || name.contains('\0') {
            close_fds(&fds);
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name").into(),
            );
        }

        let is_tap = (flags & (libc::IFF_TAP as i16)) != 0;
        let extra_flags = if cloexec { libc::O_CLOEXEC } else { 0 };
        let mut req = ifreq::new(name);
        if fds.len() > 1 {
            flags |= libc::IFF_MULTI_QUEUE as i16;
        }
        req.ifr_ifru.ifru_flags = flags;
        for &fd in &fds {
            if let Err(err) = ioctl_read(fd, libc::TUNSETIFF, &mut req) {
                close_fds(&fds);
                return Err(err);
            }
        }

        let name = match req.name() {
            Ok(name) => name.to_owned(),
            Err(_) => {
                close_fds(&fds);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "interface name is not valid UTF-8",
                )
                .into());
            }
        };
        let index = if name.is_empty() {
            0
        } else {
            match if_nametoindex(&name) {
                Ok(index) => index,
                Err(err) => {
                    close_fds(&fds);
                    return Err(err);
                }
            }
        };

        let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | extra_flags, 0) };
        if socket < 0 {
            let err = io::Error::last_os_error();
            close_fds(&fds);
            return Err(err.into());
        }

        Ok(Interface {
            fds,
            socket,
            index,
            name,
            is_tap,
        })
    }

    pub fn init(&self, params: Params) -> Result<()> {
        let index = self.verify_index()?;
        if let Some(mtu) = params.mtu {
            self.mtu(Some(mtu))?;
        }
        if let Some(owner) = params.owner {
            self.owner(owner)?;
        }
        if let Some(group) = params.group {
            self.group(group)?;
        }
        if let Some(mac) = params.mac_address {
            self.set_mac_address(mac)?;
        }

        let broadcast_capable = (self.flags(None)? & (libc::IFF_BROADCAST as i16)) != 0;
        let mut addresses = Vec::with_capacity(params.addresses.len());
        for config in &params.addresses {
            let mut config = *config;
            config.broadcast = match config.address {
                std::net::IpAddr::V4(address)
                    if config.broadcast.is_none()
                        && config.prefix_len < 31
                        && broadcast_capable =>
                {
                    ipv4_broadcast(address, config.prefix_len)
                }
                _ => config.broadcast,
            };
            addresses.push(config);
        }
        netlink::add_addresses(index, &addresses)?;

        if params.up {
            self.flags(Some(libc::IFF_UP as i16 | libc::IFF_RUNNING as i16))?;
        }
        Ok(())
    }

    /// Verifies that the stored interface index still refers to this interface.
    fn verify_index(&self) -> Result<u32> {
        let mut name = [0 as libc::c_char; 16];
        let resolved = unsafe { libc::if_indextoname(self.index, name.as_mut_ptr()) };
        if resolved.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let resolved = unsafe { std::ffi::CStr::from_ptr(resolved) };
        if resolved.to_bytes() != self.name.as_bytes() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "interface identity changed").into(),
            );
        }
        Ok(self.index)
    }

    /// Returns the TUN/TAP file descriptors, transferring their ownership.
    ///
    /// The caller becomes responsible for closing them.
    pub fn take_files(&mut self) -> Vec<i32> {
        mem::take(&mut self.fds)
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Returns the interface index of the device.
    pub fn index(&self) -> Result<u32> {
        self.verify_index()
    }

    /// Returns all IPv4 and IPv6 addresses assigned to the device.
    pub fn addresses(&self) -> Result<Vec<AddressInfo>> {
        netlink::get_addresses(self.verify_index()?)
    }

    pub fn mtu(&self, mtu: Option<i32>) -> Result<i32> {
        let mut req = ifreq::new(self.name());
        if let Some(mtu) = mtu {
            req.ifr_ifru.ifru_mtu = mtu;
            ioctl_write(self.socket, libc::SIOCSIFMTU as libc::Ioctl, &req)?;
        } else {
            ioctl_read(self.socket, libc::SIOCGIFMTU as libc::Ioctl, &mut req)?;
        }
        Ok(unsafe { req.ifr_ifru.ifru_mtu })
    }

    pub fn netmask(&self) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFNETMASK as libc::Ioctl, &mut req)?;
        let netmask = unsafe { req.ifr_ifru.ifru_netmask };
        Ok(ipv4_from_sockaddr(netmask))
    }

    pub fn broadcast(&self) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFBRDADDR as libc::Ioctl, &mut req)?;
        let broadcast = unsafe { req.ifr_ifru.ifru_broadaddr };
        Ok(ipv4_from_sockaddr(broadcast))
    }

    pub fn mac_address(&self) -> Result<[u8; 6]> {
        self.verify_index()?;
        if !self.is_tap {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            )
            .into());
        }

        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFHWADDR as libc::Ioctl, &mut req)?;
        let hwaddr = unsafe { req.ifr_ifru.ifru_hwaddr };
        if hwaddr.sa_family != libc::ARPHRD_ETHER {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            )
            .into());
        }
        let mut mac = [0u8; 6];
        for (dst, src) in mac.iter_mut().zip(&hwaddr.sa_data[..6]) {
            // `c_char` is signed on some targets and unsigned on others;
            // going through its one-byte representation preserves the octet.
            *dst = src.to_ne_bytes()[0];
        }
        Ok(mac)
    }

    pub fn set_mac_address(&self, mac: [u8; 6]) -> Result<()> {
        self.verify_index()?;
        // TUN devices report no hardware address. Detecting this from the
        // device type avoids an extra ioctl and lets all TUN MAC operations
        // return a consistent `Unsupported` error.
        if !self.is_tap {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            )
            .into());
        }
        crate::address::validate_mac(mac)
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;

        let mut req = ifreq::new(self.name());
        // SAFETY: `ifreq` is fully initialized before the ioctl call.
        unsafe {
            req.ifr_ifru.ifru_hwaddr.sa_family = libc::ARPHRD_ETHER;
            for (dst, src) in req.ifr_ifru.ifru_hwaddr.sa_data.iter_mut().zip(mac) {
                *dst = libc::c_char::from_ne_bytes([src]);
            }
        }
        ioctl_write(self.socket, libc::SIOCSIFHWADDR as libc::Ioctl, &req)
    }

    pub fn flags(&self, flags: Option<i16>) -> Result<i16> {
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFFLAGS as libc::Ioctl, &mut req)?;
        if let Some(flags) = flags {
            unsafe { req.ifr_ifru.ifru_flags |= flags };
            ioctl_write(self.socket, libc::SIOCSIFFLAGS as libc::Ioctl, &req)?;
        }
        Ok(unsafe { req.ifr_ifru.ifru_flags })
    }

    pub fn owner(&self, owner: i32) -> Result<()> {
        for fd in self.fds.iter() {
            ioctl_int(*fd, libc::TUNSETOWNER, owner as libc::c_ulong)?;
        }
        Ok(())
    }

    pub fn group(&self, group: i32) -> Result<()> {
        for fd in self.fds.iter() {
            ioctl_int(*fd, libc::TUNSETGROUP, group as libc::c_ulong)?;
        }
        Ok(())
    }
}

pub(crate) fn set_persist(fd: i32, persist: bool) -> Result<()> {
    ioctl_int(fd, libc::TUNSETPERSIST, libc::c_ulong::from(persist))
}

impl Drop for Interface {
    fn drop(&mut self) {
        unsafe { libc::close(self.socket) };
        close_fds(&self.fds);
    }
}

fn if_nametoindex(name: &str) -> Result<u32> {
    let name = CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "interface name contains a NUL byte",
        )
    })?;
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(index)
}

fn close_fds(fds: &[i32]) {
    for &fd in fds {
        unsafe { libc::close(fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv6Addr};

    #[test]
    fn index_resolves_loopback() {
        let interface = Interface::new(Vec::new(), "lo", 0, false).unwrap();
        assert!(interface.index().unwrap() > 0);
    }

    #[test]
    fn addresses_reads_loopback() {
        let interface = Interface::new(Vec::new(), "lo", 0, false).unwrap();
        let addresses = interface.addresses().unwrap();
        assert!(addresses.iter().any(|info| info.address.is_ipv4()));
        // IPv6 can be disabled in some environments.
        if addresses.iter().any(|info| info.address.is_ipv6()) {
            assert!(
                addresses
                    .iter()
                    .any(|info| info.address == IpAddr::V6(Ipv6Addr::LOCALHOST))
            );
        }
    }

    #[test]
    fn rejects_invalid_interface_names() {
        assert!(Interface::new(Vec::new(), "abcdefghijklmnop", 0, false).is_err());
        assert!(Interface::new(Vec::new(), "tun0\0evil", 0, false).is_err());
    }
}
