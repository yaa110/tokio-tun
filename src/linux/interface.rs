use super::params::Params;
use super::request::ifreq;
use crate::VIRTIO_NET_HDR_LEN;
use crate::address::{AddressInfo, ipv4_broadcast};
use crate::linux::address::ipv4_from_sockaddr;
use crate::linux::netlink;
use std::ffi::CString;
use std::io;
use std::mem;
use std::net::Ipv4Addr;

use crate::builder::MAX_INTERFACE_NAME_LEN;

fn ioctl_int_ptr(fd: i32, request: libc::Ioctl, value: *const libc::c_int) -> io::Result<()> {
    // SAFETY: the caller supplies a valid request that takes a pointer argument.
    let result = unsafe { libc::ioctl(fd, request, value) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_offload(fd: i32, features: Option<u32>) -> io::Result<()> {
    if let Some(features) = features {
        let hdr_size = VIRTIO_NET_HDR_LEN as libc::c_int;
        ioctl_int_ptr(fd, libc::TUNSETVNETHDRSZ, &hdr_size)?;
        // TUNSETOFFLOAD takes the offload mask as the ioctl argument, not as a
        // pointer (despite its `_IOW` encoding).
        ioctl_int(fd, libc::TUNSETOFFLOAD, features as libc::c_ulong)
    } else {
        // A persistent device can retain its offload features after all queues
        // close. Clear them when the caller does not request offload so the
        // device state always matches `vnet_hdr_len`.
        ioctl_int(fd, libc::TUNSETOFFLOAD, 0)
    }
}

pub(crate) fn set_carrier(fd: i32, carrier: bool) -> io::Result<()> {
    let value: libc::c_uint = u32::from(carrier);
    // SAFETY: `fd` is an open TUN/TAP descriptor and TUNSETCARRIER expects a
    // pointer to an unsigned int.
    let result = unsafe { libc::ioctl(fd, libc::TUNSETCARRIER, &value) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn set_queue(fd: i32, detach: bool) -> io::Result<()> {
    let mut req = ifreq::new("");
    req.ifr_ifru.ifru_flags = if detach {
        libc::IFF_DETACH_QUEUE as i16
    } else {
        libc::IFF_ATTACH_QUEUE as i16
    };
    ioctl_write(fd, libc::TUNSETQUEUE, &req)
}

fn ioctl_read(fd: i32, request: libc::Ioctl, req: *mut ifreq) -> io::Result<()> {
    // SAFETY: The caller guarantees that `req` points to a valid `ifreq` for
    // the duration of the call and that `request` accepts a pointer argument.
    let result = unsafe { libc::ioctl(fd, request, req) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn tun_flags(fd: i32) -> io::Result<i16> {
    let mut req = ifreq::new("");
    // SAFETY: `req` is a writable, initialized ifreq and TUNGETIFF copies the
    // current flags into it.
    let result = unsafe { libc::ioctl(fd, libc::TUNGETIFF, &mut req) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful TUNGETIFF initialized `ifru_flags`.
    Ok(unsafe { req.ifr_ifru.ifru_flags })
}

/// Returns the virtio-net header length of an externally supplied descriptor,
/// or `0` when `IFF_VNET_HDR` is clear.
pub(crate) fn tun_framing(fd: i32) -> io::Result<usize> {
    let flags = tun_flags(fd)?;
    if flags & libc::IFF_VNET_HDR as i16 == 0 {
        return Ok(0);
    }
    vnet_hdr_size(fd)
}

/// Reads `IFF_NO_PI` from sysfs for a descriptor with a known interface name.
///
/// `TUNGETIFF` cannot be used for this: when no socket filter is attached, the
/// kernel adds `IFF_NOFILTER`, which has the same value (`0x1000`) as
/// `IFF_NO_PI`. The sysfs `tun_flags` attribute exposes the device's real
/// `tun->flags`. Returns `None` when sysfs is unavailable or unreadable.
pub(crate) fn packet_info_from_sysfs(fd: i32) -> Option<bool> {
    let name = tun_name(fd).ok()?;
    let flags = std::fs::read_to_string(format!("/sys/class/net/{name}/tun_flags")).ok()?;
    let flags = parse_tun_flags(&flags)?;
    Some(flags & libc::IFF_NO_PI as u32 == 0)
}

fn tun_name(fd: i32) -> io::Result<String> {
    let mut req = ifreq::new("");
    ioctl_read(fd, libc::TUNGETIFF as libc::Ioctl, &mut req)?;
    req.name()
        .map(str::to_owned)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid interface name"))
}

fn parse_tun_flags(value: &str) -> Option<u32> {
    let value = value.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u32::from_str_radix(value, 16).ok()
}

/// Returns the `TUNGETVNETHDRSZ` value of `fd`.
///
/// Must only be called when the device has `IFF_VNET_HDR` enabled; the kernel
/// does not check the flag on this ioctl.
pub(crate) fn vnet_hdr_size(fd: i32) -> io::Result<usize> {
    let mut len: libc::c_int = 0;
    // SAFETY: `fd` is an open TUN/TAP descriptor and TUNGETVNETHDRSZ expects an
    // int pointer.
    let result = unsafe { libc::ioctl(fd, libc::TUNGETVNETHDRSZ, &mut len) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(len as usize)
}

fn ioctl_write(fd: i32, request: libc::Ioctl, req: *const ifreq) -> io::Result<()> {
    // SAFETY: The caller guarantees that `req` points to a valid `ifreq` for
    // the duration of the call and that `request` accepts a pointer argument.
    let result = unsafe { libc::ioctl(fd, request, req) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ioctl_int(fd: i32, request: libc::Ioctl, value: libc::c_ulong) -> io::Result<()> {
    // SAFETY: The caller guarantees that `request` accepts an integer argument.
    let result = unsafe { libc::ioctl(fd, request, value) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    index: u32,
    name: String,
    is_tap: bool,
    vnet_hdr_len: usize,
    offload_features: u32,
    /// Whether this call created the interface (rather than attaching to an
    /// existing one). Used to avoid rolling back persistence state that a
    /// previous owner configured.
    pub(crate) created: bool,
}

impl Interface {
    pub fn new(
        fds: Vec<i32>,
        name: &str,
        mut flags: i16,
        multi_queue: bool,
        offload: Option<u32>,
        cloexec: bool,
    ) -> io::Result<Self> {
        if name.len() > MAX_INTERFACE_NAME_LEN || name.contains('\0') {
            close_fds(&fds);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid interface name",
            ));
        }

        // `is_tap` is only meaningful for diagnostics and MAC ioctls; when
        // attaching to an existing device the kernel readback wins over the
        // requested flags.
        let is_tap = if fds.is_empty() {
            (flags & (libc::IFF_TAP as i16)) != 0
        } else {
            false
        };
        let created = if name.is_empty() {
            true
        } else {
            let cname = CString::new(name).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name")
            })?;
            // SAFETY: `cname` is a valid NUL-terminated string.
            unsafe { libc::if_nametoindex(cname.as_ptr()) == 0 }
        };
        let extra_flags = if cloexec { libc::O_CLOEXEC } else { 0 };
        let mut req = ifreq::new(name);
        if multi_queue {
            flags |= libc::IFF_MULTI_QUEUE as i16;
        }
        if let Some(features) = offload {
            flags |= libc::IFF_VNET_HDR as i16;
            // NAPI enables GRO on the userspace-to-kernel write path. Header-only
            // mode (features == 0) requests a stable virtio-net prefix without
            // enabling NAPI/GRO behavior.
            if features != 0 {
                flags |= libc::IFF_NAPI as i16;
            }
        }
        req.ifr_ifru.ifru_flags = flags;
        for &fd in &fds {
            if let Err(err) = ioctl_read(fd, libc::TUNSETIFF, &mut req) {
                close_fds(&fds);
                return Err(err);
            }
        }
        // The kernel decides the final device type and vnet state on
        // `TUNSETIFF`; read both back instead of trusting the requested flags.
        let (is_tap, actual_vnet_hdr) = if fds.is_empty() {
            (is_tap, false)
        } else {
            let actual_flags = match tun_flags(fds[0]) {
                Ok(flags) => flags,
                Err(err) => {
                    close_fds(&fds);
                    return Err(err);
                }
            };
            (
                (actual_flags & libc::IFF_TAP as i16) != 0,
                (actual_flags & libc::IFF_VNET_HDR as i16) != 0,
            )
        };
        if actual_vnet_hdr != offload.is_some() && !fds.is_empty() {
            close_fds(&fds);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "offload must match the existing TUN/TAP device state",
            ));
        }
        // A pre-existing device may carry a vnet header size different from the
        // 10-byte virtio header used for packet framing; refusing is safer than
        // silently rewriting another program's setting.
        let actual_hdr_len = if actual_vnet_hdr {
            let hdr_len = match vnet_hdr_size(fds[0]) {
                Ok(hdr_len) => hdr_len,
                Err(err) => {
                    close_fds(&fds);
                    return Err(err);
                }
            };
            if hdr_len != VIRTIO_NET_HDR_LEN {
                close_fds(&fds);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("existing device uses {hdr_len}-byte vnet headers"),
                ));
            }
            hdr_len
        } else {
            0
        };
        if !fds.is_empty() {
            for &fd in &fds {
                if let Err(err) = set_offload(fd, offload) {
                    close_fds(&fds);
                    return Err(err);
                }
            }
        }

        let name = match req.name() {
            Ok(name) => name.to_owned(),
            Err(_) => {
                close_fds(&fds);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "interface name is not valid UTF-8",
                ));
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

        // SAFETY: `socket` is called with valid constants and returns an owned
        // descriptor or -1.
        let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | extra_flags, 0) };
        if socket < 0 {
            let err = io::Error::last_os_error();
            close_fds(&fds);
            return Err(err);
        }

        Ok(Interface {
            fds,
            socket,
            index,
            name,
            is_tap,
            vnet_hdr_len: actual_hdr_len,
            offload_features: offload.unwrap_or(0),
            created,
        })
    }

    pub fn init(&self, params: Params) -> io::Result<()> {
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

        let broadcast_capable = (self.flags(None)? & (libc::IFF_BROADCAST as u32)) != 0;
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
        // Addresses apply one `RTM_NEWADDR` at a time; a failure after an
        // earlier address was configured leaves the device partially
        // configured. The failing address is included in the error message.
        // `EEXIST` is tolerated only when the existing address has identical
        // prefix, peer, and broadcast attributes, so re-running a
        // configuration against a persistent device is idempotent.
        netlink::add_addresses(index, &addresses)?;

        if params.up {
            // The kernel owns `IFF_RUNNING` (it derives it from the link
            // operstate), so only `IFF_UP` is requested here.
            self.flags(Some(libc::IFF_UP as i16))?;
        }
        Ok(())
    }

    /// Verifies that the stored interface index still refers to this interface.
    fn verify_index(&self) -> io::Result<u32> {
        let mut name = [0 as libc::c_char; 16];
        // SAFETY: `name` is a writable IFNAMSIZ buffer and `self.index` is a
        // previously resolved interface index.
        let resolved = unsafe { libc::if_indextoname(self.index, name.as_mut_ptr()) };
        if resolved.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `if_indextoname` returned a NUL-terminated name.
        let resolved = unsafe { std::ffi::CStr::from_ptr(resolved) };
        if resolved.to_bytes() != self.name.as_bytes() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "interface identity changed",
            ));
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

    pub fn vnet_hdr_len(&self) -> usize {
        self.vnet_hdr_len
    }

    /// Returns the offload feature mask configured by the builder, or `0` when
    /// offload is disabled.
    pub fn offload_features(&self) -> u32 {
        self.offload_features
    }

    /// Returns the interface index of the device.
    pub fn index(&self) -> io::Result<u32> {
        self.verify_index()
    }

    /// Returns all IPv4 and IPv6 addresses assigned to the device.
    pub fn addresses(&self) -> io::Result<Vec<AddressInfo>> {
        netlink::get_addresses(self.verify_index()?)
    }

    pub fn mtu(&self, mtu: Option<i32>) -> io::Result<i32> {
        self.verify_index()?;
        let mut req = ifreq::new(self.name());
        if let Some(mtu) = mtu {
            req.ifr_ifru.ifru_mtu = mtu;
            ioctl_write(self.socket, libc::SIOCSIFMTU as libc::Ioctl, &req)?;
        } else {
            ioctl_read(self.socket, libc::SIOCGIFMTU as libc::Ioctl, &mut req)?;
        }
        // SAFETY: a successful SIOCGIFMTU initialized `ifru_mtu`.
        Ok(unsafe { req.ifr_ifru.ifru_mtu })
    }

    pub fn netmask(&self) -> io::Result<Ipv4Addr> {
        self.verify_index()?;
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFNETMASK as libc::Ioctl, &mut req)?;
        // SAFETY: a successful SIOCGIFNETMASK initialized `ifru_netmask`.
        let netmask = unsafe { req.ifr_ifru.ifru_netmask };
        Ok(ipv4_from_sockaddr(netmask))
    }

    pub fn broadcast(&self) -> io::Result<Ipv4Addr> {
        self.verify_index()?;
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFBRDADDR as libc::Ioctl, &mut req)?;
        // SAFETY: a successful SIOCGIFBRDADDR initialized `ifru_broadaddr`.
        let broadcast = unsafe { req.ifr_ifru.ifru_broadaddr };
        Ok(ipv4_from_sockaddr(broadcast))
    }

    pub fn mac_address(&self) -> io::Result<[u8; 6]> {
        self.verify_index()?;
        if !self.is_tap {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            ));
        }

        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFHWADDR as libc::Ioctl, &mut req)?;
        // SAFETY: a successful SIOCGIFHWADDR initialized `ifru_hwaddr`.
        let hwaddr = unsafe { req.ifr_ifru.ifru_hwaddr };
        if hwaddr.sa_family != libc::ARPHRD_ETHER {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            ));
        }
        let mut mac = [0u8; 6];
        for (dst, src) in mac.iter_mut().zip(&hwaddr.sa_data[..6]) {
            // `c_char` is signed on some targets and unsigned on others;
            // going through its one-byte representation preserves the octet.
            *dst = src.to_ne_bytes()[0];
        }
        Ok(mac)
    }

    pub fn set_mac_address(&self, mac: [u8; 6]) -> io::Result<()> {
        self.verify_index()?;
        // TUN devices report no hardware address. Detecting this from the
        // device type avoids an extra ioctl and lets all TUN MAC operations
        // return a consistent `Unsupported` error.
        if !self.is_tap {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interface has no hardware address",
            ));
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

    pub fn flags(&self, flags: Option<i16>) -> io::Result<u32> {
        self.verify_index()?;
        let mut req = ifreq::new(self.name());
        ioctl_read(self.socket, libc::SIOCGIFFLAGS as libc::Ioctl, &mut req)?;
        if let Some(flags) = flags {
            // SAFETY: the union was initialized by SIOCGIFFLAGS above.
            unsafe { req.ifr_ifru.ifru_flags |= flags };
            ioctl_write(self.socket, libc::SIOCSIFFLAGS as libc::Ioctl, &req)?;
        }
        // SAFETY: the union was initialized by SIOCGIFFLAGS above.
        Ok(u16::from_ne_bytes(unsafe { req.ifr_ifru.ifru_flags }.to_ne_bytes()) as u32)
    }

    pub fn owner(&self, owner: u32) -> io::Result<()> {
        for fd in self.fds.iter() {
            ioctl_int(*fd, libc::TUNSETOWNER, libc::c_ulong::from(owner))?;
        }
        Ok(())
    }

    pub fn group(&self, group: u32) -> io::Result<()> {
        for fd in self.fds.iter() {
            ioctl_int(*fd, libc::TUNSETGROUP, libc::c_ulong::from(group))?;
        }
        Ok(())
    }
}

pub(crate) fn set_persist(fd: i32, persist: bool) -> io::Result<()> {
    ioctl_int(fd, libc::TUNSETPERSIST, libc::c_ulong::from(persist))
}

impl Drop for Interface {
    fn drop(&mut self) {
        // SAFETY: `Interface` owns `socket` and closes it exactly once.
        unsafe { libc::close(self.socket) };
        close_fds(&self.fds);
    }
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

fn close_fds(fds: &[i32]) {
    for &fd in fds {
        // SAFETY: each descriptor is owned by the caller.
        unsafe { libc::close(fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv6Addr};

    #[test]
    fn index_resolves_loopback() {
        let interface = Interface::new(Vec::new(), "lo", 0, false, None, false).unwrap();
        assert!(interface.index().unwrap() > 0);
    }

    #[test]
    fn addresses_reads_loopback() {
        let interface = Interface::new(Vec::new(), "lo", 0, false, None, false).unwrap();
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
        assert!(Interface::new(Vec::new(), "abcdefghijklmnop", 0, false, None, false).is_err());
        assert!(Interface::new(Vec::new(), "tun0\0evil", 0, false, None, false).is_err());
    }

    #[test]
    fn parses_sysfs_tun_flags() {
        assert_eq!(parse_tun_flags("0x1\n"), Some(1));
        assert_eq!(parse_tun_flags("0x1001\n"), Some(0x1001));
        assert_eq!(parse_tun_flags("1\n"), Some(1));
        assert_eq!(parse_tun_flags("garbage"), None);
    }
}
