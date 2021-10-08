use super::result::Result;
#[cfg(target_os = "linux")]
use crate::linux::params::Params;
#[cfg(target_os = "linux")]
use crate::tun::Tun;
use core::convert::From;
use libc::{IFF_NO_PI, IFF_TAP, IFF_TUN};
use std::net::{Ipv4Addr, IpAddr};

/// Represents a factory to build new instances of [`Tun`](struct.Tun.html).
pub struct TunBuilder<'a> {
    name: &'a str,
    is_tap: bool,
    packet_info: bool,
    persist: bool,
    up: bool,
    mtu: Option<i32>,
    owner: Option<i32>,
    group: Option<i32>,
    address: Option<IpAddr>,
    prefix_length: u32,
    destination: Option<Ipv4Addr>,
    broadcast: Option<Ipv4Addr>,
    netmask: Option<Ipv4Addr>,
}

impl<'a> Default for TunBuilder<'a> {
    fn default() -> Self {
        Self {
            name: "",
            owner: None,
            group: None,
            is_tap: false,
            persist: false,
            up: false,
            mtu: None,
            packet_info: true,
            address: None,
            prefix_length: 128,
            destination: None,
            broadcast: None,
            netmask: None,
        }
    }
}

impl<'a> TunBuilder<'a> {
    /// Creates a new instance of [`TunBuilder`](struct.TunBuilder.html).
    pub fn new() -> Self {
        Default::default()
    }

    /// Sets the name of device (max length: 16 characters), if it is empty, then device name is set by kernel. Default value is empty.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = name;
        self
    }

    /// If `is_tap` is true, a TAP device is allocated, otherwise, a TUN device is created. Default value is `false`.
    pub fn tap(mut self, is_tap: bool) -> Self {
        self.is_tap = is_tap;
        self
    }

    /// If `packet_info` is false, then `IFF_NO_PI` flag is set. Default value is `true`.
    pub fn packet_info(mut self, packet_info: bool) -> Self {
        self.packet_info = packet_info;
        self
    }

    /// Sets the MTU of device.
    pub fn mtu(mut self, mtu: i32) -> Self {
        self.mtu = Some(mtu);
        self
    }

    /// Sets the owner of device.
    pub fn owner(mut self, owner: i32) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Sets the group of device.
    pub fn group(mut self, group: i32) -> Self {
        self.group = Some(group);
        self
    }

    /// Sets IPv4 address of device.
    pub fn address(mut self, address: IpAddr) -> Self {
        self.address = Some(address);
        self
    }

    /// Sets the IPv6 prefix length
    ///
    /// This is only useful when setting an IPv6 address via [`TunBuilder::address()`] because only IPv6 addresses
    /// have the concept of prefixes. If IPv4 is used, call [`TunBuilder::netmask()`] instead.
    pub fn prefix_length(mut self, prefix_length: u32) -> Self {
        self.prefix_length = prefix_length;
        self
    }

    /// Sets IPv4 destination address of device.
    pub fn destination(mut self, dst: Ipv4Addr) -> Self {
        self.destination = Some(dst);
        self
    }

    /// Sets IPv4 broadcast address of device.
    pub fn broadcast(mut self, broadcast: Ipv4Addr) -> Self {
        self.broadcast = Some(broadcast);
        self
    }

    /// Sets IPv4 netmask address of device.
    pub fn netmask(mut self, netmask: Ipv4Addr) -> Self {
        self.netmask = Some(netmask);
        self
    }

    /// Makes the device persistent.
    pub fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    /// Sets up the device.
    pub fn up(mut self) -> Self {
        self.up = true;
        self
    }

    /// Builds a new instance of [`Tun`](struct.Tun.html).
    pub fn try_build(self) -> Result<Tun> {
        Tun::new(self.into())
    }

    /// Builds multiple instances of [`Tun`](struct.Tun.html) with `IFF_MULTI_QUEUE` flag.
    #[cfg(target_os = "linux")]
    pub fn try_build_mq(self, queues: usize) -> Result<Vec<Tun>> {
        Tun::new_mq(self.into(), queues)
    }
}

impl<'a> From<TunBuilder<'a>> for Params {
    #[cfg(target_os = "linux")]
    fn from(builder: TunBuilder) -> Self {
        Params {
            name: if builder.name.is_empty() {
                None
            } else {
                Some(builder.name.into())
            },
            flags: {
                let mut flags: i16 = if builder.is_tap { IFF_TAP } else { IFF_TUN } as _;
                if !builder.packet_info {
                    flags |= IFF_NO_PI as i16;
                }
                flags
            },
            persist: builder.persist,
            up: builder.up,
            mtu: builder.mtu,
            owner: builder.owner,
            group: builder.group,
            address: builder.address,
            prefix_length: builder.prefix_length,
            destination: builder.destination,
            broadcast: builder.broadcast,
            netmask: builder.netmask,
        }
    }

    #[cfg(not(any(target_os = "linux")))]
    fn from(builder: TunBuilder) -> Self {
        unimplemented!()
    }
}
