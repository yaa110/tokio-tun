use super::params::Params;
use super::request::ifreq;
use crate::linux::address::Ipv4AddrExt;
use crate::Result;
use std::net::Ipv4Addr;

nix::ioctl_write_int!(tunsetiff, b'T', 202);
nix::ioctl_write_int!(tunsetpersist, b'T', 203);
nix::ioctl_write_int!(tunsetowner, b'T', 204);
nix::ioctl_write_int!(tunsetgroup, b'T', 206);

nix::ioctl_write_ptr_bad!(siocsifmtu, libc::SIOCSIFMTU, ifreq);
nix::ioctl_write_ptr_bad!(siocsifflags, libc::SIOCSIFFLAGS, ifreq);
nix::ioctl_write_ptr_bad!(siocsifaddr, libc::SIOCSIFADDR, ifreq);
nix::ioctl_write_ptr_bad!(siocsifdstaddr, libc::SIOCSIFDSTADDR, ifreq);
nix::ioctl_write_ptr_bad!(siocsifbrdaddr, libc::SIOCSIFBRDADDR, ifreq);
nix::ioctl_write_ptr_bad!(siocsifnetmask, libc::SIOCSIFNETMASK, ifreq);

nix::ioctl_read_bad!(siocgifmtu, libc::SIOCGIFMTU, ifreq);
nix::ioctl_read_bad!(siocgifflags, libc::SIOCGIFFLAGS, ifreq);
nix::ioctl_read_bad!(siocgifaddr, libc::SIOCGIFADDR, ifreq);
nix::ioctl_read_bad!(siocgifdstaddr, libc::SIOCGIFDSTADDR, ifreq);
nix::ioctl_read_bad!(siocgifbrdaddr, libc::SIOCGIFBRDADDR, ifreq);
nix::ioctl_read_bad!(siocgifnetmask, libc::SIOCGIFNETMASK, ifreq);

#[derive(Clone)]
pub struct Interface {
    fds: Vec<i32>,
    socket: i32,
    name: String,
}

impl Interface {
    pub fn new(fds: Vec<i32>, name: &str, mut flags: i16, multi_queue: bool) -> Result<Self> {
        let mut req = ifreq::new(name);
        if multi_queue {
            flags |= libc::IFF_MULTI_QUEUE as i16;
        }
        req.ifr_ifru.ifru_flags = flags;
        for &fd in &fds {
            unsafe { tunsetiff(fd, &req as *const _ as _) }?;
        }
        Ok(Interface {
            fds,
            socket: unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) },
            name: req.name().to_owned(),
        })
    }

    pub fn init(&self, params: Params) -> Result<()> {
        if let Some(mtu) = params.mtu {
            self.mtu(Some(mtu))?;
        }
        if let Some(owner) = params.owner {
            self.owner(owner)?;
        }
        if let Some(group) = params.group {
            self.group(group)?;
        }
        if let Some(address) = params.address {
            self.address(Some(address))?;
        }
        if let Some(netmask) = params.netmask {
            self.netmask(Some(netmask))?;
        }
        if let Some(destination) = params.destination {
            self.destination(Some(destination))?;
        }
        if let Some(broadcast) = params.broadcast {
            self.broadcast(Some(broadcast))?;
        }
        if params.persist {
            self.persist()?;
        }
        if params.up {
            self.flags(Some(libc::IFF_UP as i16 | libc::IFF_RUNNING as i16))?;
        }
        Ok(())
    }

    pub fn files(&self) -> &[i32] {
        &self.fds
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn mtu(&self, mtu: Option<i32>) -> Result<i32> {
        let mut req = ifreq::new(self.name());
        if let Some(mtu) = mtu {
            req.ifr_ifru.ifru_mtu = mtu;
            unsafe { siocsifmtu(self.socket, &req) }?;
        } else {
            unsafe { siocgifmtu(self.socket, &mut req) }?;
        }
        Ok(unsafe { req.ifr_ifru.ifru_mtu })
    }

    pub fn netmask(&self, netmask: Option<Ipv4Addr>) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        if let Some(netmask) = netmask {
            req.ifr_ifru.ifru_netmask = netmask.to_address();
            unsafe { siocsifnetmask(self.socket, &req) }?;
            return Ok(netmask);
        }
        unsafe { siocgifnetmask(self.socket, &mut req) }?;
        Ok(unsafe { Ipv4Addr::from_address(req.ifr_ifru.ifru_netmask) })
    }

    pub fn address(&self, address: Option<Ipv4Addr>) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        if let Some(address) = address {
            req.ifr_ifru.ifru_addr = address.to_address();
            unsafe { siocsifaddr(self.socket, &req) }?;
            return Ok(address);
        }
        unsafe { siocgifaddr(self.socket, &mut req) }?;
        Ok(unsafe { Ipv4Addr::from_address(req.ifr_ifru.ifru_addr) })
    }

    pub fn destination(&self, dst: Option<Ipv4Addr>) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        if let Some(dst) = dst {
            req.ifr_ifru.ifru_dstaddr = dst.to_address();
            unsafe { siocsifdstaddr(self.socket, &req) }?;
            return Ok(dst);
        }
        unsafe { siocgifdstaddr(self.socket, &mut req) }?;
        Ok(unsafe { Ipv4Addr::from_address(req.ifr_ifru.ifru_dstaddr) })
    }

    pub fn broadcast(&self, broadcast: Option<Ipv4Addr>) -> Result<Ipv4Addr> {
        let mut req = ifreq::new(self.name());
        if let Some(broadcast) = broadcast {
            req.ifr_ifru.ifru_broadaddr = broadcast.to_address();
            unsafe { siocsifbrdaddr(self.socket, &req) }?;
            return Ok(broadcast);
        }
        unsafe { siocgifbrdaddr(self.socket, &mut req) }?;
        Ok(unsafe { Ipv4Addr::from_address(req.ifr_ifru.ifru_broadaddr) })
    }

    pub fn flags(&self, flags: Option<i16>) -> Result<i16> {
        let mut req = ifreq::new(self.name());
        unsafe { siocgifflags(self.socket, &mut req) }?;
        if let Some(flags) = flags {
            unsafe { req.ifr_ifru.ifru_flags |= flags };
            unsafe { siocsifflags(self.socket, &req) }?;
        }
        Ok(unsafe { req.ifr_ifru.ifru_flags })
    }

    pub fn owner(&self, owner: i32) -> Result<()> {
        for fd in self.fds.iter() {
            unsafe { tunsetowner(*fd, owner as _) }?;
        }
        Ok(())
    }

    pub fn group(&self, group: i32) -> Result<()> {
        for fd in self.fds.iter() {
            unsafe { tunsetgroup(*fd, group as _) }?;
        }
        Ok(())
    }

    pub fn persist(&self) -> Result<()> {
        for fd in self.fds.iter() {
            unsafe { tunsetpersist(*fd, 1) }?;
        }
        Ok(())
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        unsafe { libc::close(self.socket) };
    }
}
