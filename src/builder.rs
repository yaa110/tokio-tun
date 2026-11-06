use crate::Result;
use crate::address::AddressConfig;
#[cfg(target_os = "linux")]
use crate::linux::params::Params;
#[cfg(target_os = "linux")]
use crate::tun::Tun;
use libc::{IFF_NO_PI, IFF_TAP, IFF_TUN};
use std::net::{IpAddr, Ipv4Addr};

const MAX_INTERFACE_NAME_LEN: usize = 15;
const MAX_QUEUES: usize = 256;

/// Represents a factory to build new instances of [`Tun`](struct.Tun.html).
pub struct TunBuilder {
    name: String,
    is_tap: bool,
    packet_info: bool,
    persist: bool,
    up: bool,
    mtu: Option<i32>,
    owner: Option<i32>,
    group: Option<i32>,
    mac_address: Option<[u8; 6]>,
    addresses: Vec<AddressConfig>,
    invalid_reason: Option<&'static str>,
    queues: Option<usize>,
    cloexec: bool,
}

impl Default for TunBuilder {
    fn default() -> Self {
        Self {
            name: "".into(),
            owner: None,
            group: None,
            is_tap: false,
            persist: false,
            up: false,
            mtu: None,
            packet_info: false,
            mac_address: None,
            addresses: Vec::new(),
            invalid_reason: None,
            queues: None,
            cloexec: true,
        }
    }
}

impl TunBuilder {
    /// Creates a new instance of [`TunBuilder`](struct.TunBuilder.html).
    pub fn new() -> Self {
        Default::default()
    }

    /// Sets the name of device (max length: 15 bytes), if it is empty, then device name is set by kernel. Default value is empty.
    ///
    /// Names longer than 15 bytes or containing a NUL byte cause
    /// [`build`](TunBuilder::build) to return an error.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.into();
        self
    }

    /// Allocates a TAP device. If not set, a TUN device is created.
    ///
    /// *TAP* devices are layer 2 devices which will result in ethernet frames (or other protocols)
    /// being transmitted over this device.
    ///
    /// In contrast, *TUN* devices are layer 3 devices which means that IP packets are transmitted
    /// over it.
    pub fn tap(mut self) -> Self {
        self.is_tap = true;
        self
    }

    /// Builds multiple instances of [`Tun`](struct.Tun.html) with `IFF_MULTI_QUEUE` flag.
    ///
    /// Internally this creates multiple file descriptors to parallelize packet sending and receiving.
    /// The value must be between `1` and `256` inclusive; otherwise
    /// [`build`](TunBuilder::build) returns an error. Default value is `1`.
    pub fn queues(mut self, queues: usize) -> Self {
        self.queues = Some(queues);
        self
    }

    /// Set `packet_info` to `true` (default is `false`), thereby unsetting the `IFF_NO_PI` flag on
    /// allocation.
    ///
    /// Instructing the kernel to provide packet information results in it adding 4 extra bytes to
    /// the beginning of the packet (2 flag bytes and 2 protocol bytes). These bytes tell you
    /// what kind of package was just delivered.
    ///
    /// If you don't need this kind of information, you do not need to set this option. In this
    /// case, only raw packets (whatever they may be) are received.
    pub fn packet_info(mut self) -> Self {
        self.packet_info = true;
        self
    }

    /// Sets the MTU (Maximum Transfer Unit) of device.
    ///
    /// MTU defines the maximum size of packets which this device will allow being transmitted or
    /// received.
    ///
    /// It can also be used as an estimate to size input buffers although the exact packet size
    /// may vary to account for some protocol overhead.
    pub fn mtu(mut self, mtu: i32) -> Self {
        self.mtu = Some(mtu);
        self
    }

    /// Sets the owner of device.
    ///
    /// This is the numeric UID of the user who will own the created device.
    pub fn owner(mut self, owner: i32) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Sets the group of device.
    ///
    /// This is the numeric GID of the group that will own the created device.
    pub fn group(mut self, group: i32) -> Self {
        self.group = Some(group);
        self
    }

    /// Sets the MAC address of a TAP device.
    ///
    /// MAC addresses are only supported for TAP devices, so this method must be
    /// used together with [`tap`](TunBuilder::tap); otherwise
    /// [`build`](TunBuilder::build) returns an error. The address must be a
    /// non-zero unicast address.
    pub fn mac_address(mut self, mac: [u8; 6]) -> Self {
        self.mac_address = Some(mac);
        self
    }

    /// Adds an IPv4 or IPv6 address to the device.
    ///
    /// This method can be called multiple times to assign multiple addresses.
    /// Every address has its own prefix length, so addresses do not share a
    /// netmask. `prefix_len` must be at most `32` for IPv4 addresses and at
    /// most `128` for IPv6 addresses. Prefix length `0` and unspecified,
    /// loopback, multicast, or broadcast addresses are rejected.
    ///
    /// ```no_run
    /// use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    /// use tokio_tun::Tun;
    ///
    /// let tun = Tun::builder()
    ///     .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
    ///     .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 64)
    ///     .up()
    ///     .build()
    ///     .unwrap()
    ///     .pop()
    ///     .unwrap();
    /// ```
    pub fn address(mut self, address: IpAddr, prefix_len: u8) -> Self {
        self.addresses.push(AddressConfig {
            address,
            prefix_len,
            destination: None,
            broadcast: None,
        });
        self
    }

    /// Sets the point-to-point destination address for the most recently added
    /// address.
    ///
    /// Both IPv4 and IPv6 destinations are supported, but the destination
    /// family must match the family of the address it belongs to. Call
    /// [`address`](TunBuilder::address) before this method; otherwise
    /// [`build`](TunBuilder::build) returns an error.
    ///
    /// ```no_run
    /// use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    /// use tokio_tun::Tun;
    ///
    /// let tun = Tun::builder()
    ///     .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
    ///     .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
    ///     .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 64)
    ///     .destination(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2)))
    ///     .up()
    ///     .build()
    ///     .unwrap()
    ///     .pop()
    ///     .unwrap();
    /// ```
    pub fn destination(mut self, destination: IpAddr) -> Self {
        if let Some(config) = self.addresses.last_mut() {
            if config.address.is_ipv4() == destination.is_ipv4() {
                config.destination = Some(destination);
            } else {
                self.invalid_reason =
                    Some("destination address family must match the most recently added address");
            }
        } else {
            self.invalid_reason = Some("destination requires a preceding address");
        }
        self
    }

    /// Sets the IPv4 broadcast address for the most recently added address.
    ///
    /// IPv6 has no broadcast address, so the most recently added address must
    /// be an IPv4 address. Call [`address`](TunBuilder::address) before this
    /// method; otherwise [`build`](TunBuilder::build) returns an error.
    pub fn broadcast(mut self, broadcast: Ipv4Addr) -> Self {
        if let Some(config) = self.addresses.last_mut() {
            if config.address.is_ipv4() {
                config.broadcast = Some(broadcast);
            } else {
                self.invalid_reason = Some("broadcast requires a preceding IPv4 address");
            }
        } else {
            self.invalid_reason = Some("broadcast requires a preceding address");
        }
        self
    }

    /// Removes CLOEXEC flag on all FDs. This will allow passing tun/tap FDs to any exec-ed
    /// child processes.
    /// Default behaviour is to prevent passing fds flag.
    pub fn no_close_on_exec(mut self) -> Self {
        self.cloexec = false;
        self
    }

    /// Adds CLOEXEC flag on all FDs. This will prevent passing FDs to any exec-ed
    /// child processes.
    /// Default behaviour is to prevent passing fds flag.
    pub fn close_on_exec(mut self) -> Self {
        self.cloexec = true;
        self
    }

    /// Makes the device persistent.
    ///
    /// Persistent devices stay registered as long as the computer is not restarted.
    /// If another device with the same name is created afterwards, it will override the existing
    /// one.
    ///
    /// Non-persistent devices on the other hand, are removed as soon as the controlling process
    /// exits.
    pub fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    /// Sets up the device.
    ///
    /// This means the interface is immediately put into the *up* state.
    /// It is thus immediately able to service incoming and outgoing packets.
    pub fn up(mut self) -> Self {
        self.up = true;
        self
    }

    /// Builds a new instance of [`Tun`](struct.Tun.html).
    ///
    /// This method must be called from within a Tokio runtime with I/O enabled.
    pub fn build(self) -> Result<Vec<Tun>> {
        self.validate()?;
        match self.queues {
            Some(queues) if queues > 1 => Tun::new_mq(self.into(), queues),
            _ => Tun::new(self.into()).map(|tun| vec![tun]),
        }
    }

    fn validate(&self) -> Result<()> {
        if let Some(reason) = self.invalid_reason {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, reason).into());
        }
        if !self.name.is_empty() {
            if self.name.len() > MAX_INTERFACE_NAME_LEN {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "interface name must not exceed 15 bytes",
                )
                .into());
            }
            if self.name.contains('\0') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "interface name must not contain a NUL byte",
                )
                .into());
            }
        }
        if matches!(self.queues, Some(queues) if queues == 0 || queues > MAX_QUEUES) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "queues must be between 1 and 256",
            )
            .into());
        }
        if let Some(mac) = self.mac_address {
            if !self.is_tap {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "MAC address is only supported for TAP devices",
                )
                .into());
            }
            crate::address::validate_mac(mac)
                .map_err(|reason| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason))?;
        }
        for address in &self.addresses {
            address
                .validate()
                .map_err(|reason| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason))?;
        }
        Ok(())
    }
}

impl From<TunBuilder> for Params {
    #[cfg(target_os = "linux")]
    fn from(builder: TunBuilder) -> Self {
        Params {
            name: if builder.name.is_empty() {
                None
            } else {
                Some(builder.name)
            },
            flags: {
                let mut flags = if builder.is_tap { IFF_TAP } else { IFF_TUN } as _;
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
            mac_address: builder.mac_address,
            addresses: builder.addresses,
            cloexec: builder.cloexec,
        }
    }

    #[cfg(not(any(target_os = "linux")))]
    fn from(builder: TunBuilder) -> Self {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    fn v6(address: &str) -> IpAddr {
        IpAddr::V6(address.parse::<Ipv6Addr>().unwrap())
    }

    fn assert_invalid_input<T>(result: Result<T>) {
        match result {
            Err(crate::Error::IoError(err)) => {
                assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput)
            }
            Ok(_) => panic!("expected an error"),
        }
    }

    #[test]
    fn address_accumulates_in_order() {
        let builder = TunBuilder::new()
            .address(v4(10, 0, 0, 1), 24)
            .address(v6("fd00::1"), 64);

        assert_eq!(builder.addresses.len(), 2);
        assert_eq!(builder.addresses[0].address, v4(10, 0, 0, 1));
        assert_eq!(builder.addresses[0].prefix_len, 24);
        assert_eq!(builder.addresses[1].address, v6("fd00::1"));
        assert_eq!(builder.addresses[1].prefix_len, 64);
    }

    #[test]
    fn destination_attaches_to_most_recent_address() {
        let builder = TunBuilder::new()
            .address(v4(10, 0, 0, 1), 24)
            .destination(v4(10, 0, 0, 2))
            .address(v6("fd00::1"), 64)
            .destination(v6("fd00::2"));

        assert_eq!(builder.addresses[0].destination, Some(v4(10, 0, 0, 2)));
        assert_eq!(builder.addresses[1].destination, Some(v6("fd00::2")));
    }

    #[test]
    fn broadcast_attaches_to_most_recent_ipv4_address() {
        let builder = TunBuilder::new()
            .address(v4(10, 0, 0, 1), 24)
            .broadcast(Ipv4Addr::new(10, 0, 0, 255));

        assert_eq!(
            builder.addresses[0].broadcast,
            Some(Ipv4Addr::new(10, 0, 0, 255))
        );
    }

    #[test]
    fn build_rejects_ipv4_prefix_above_32() {
        assert_invalid_input(TunBuilder::new().address(v4(10, 0, 0, 1), 33).build());
    }

    #[test]
    fn build_rejects_ipv6_prefix_above_128() {
        assert_invalid_input(TunBuilder::new().address(v6("fd00::1"), 129).build());
    }

    #[test]
    fn build_rejects_mismatched_destination_family() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v4(10, 0, 0, 1), 24)
                .destination(v6("fd00::2"))
                .build(),
        );
    }

    #[test]
    fn build_rejects_destination_without_address() {
        assert_invalid_input(TunBuilder::new().destination(v4(10, 0, 0, 2)).build());
    }

    #[test]
    fn build_rejects_broadcast_for_ipv6() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v6("fd00::1"), 64)
                .broadcast(Ipv4Addr::new(10, 0, 0, 255))
                .build(),
        );
    }

    #[test]
    fn build_rejects_unspecified_address() {
        assert_invalid_input(TunBuilder::new().address(v4(0, 0, 0, 0), 0).build());
    }

    #[test]
    fn build_rejects_unspecified_destination() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v4(10, 0, 0, 1), 24)
                .destination(v4(0, 0, 0, 0))
                .build(),
        );
    }

    #[test]
    fn build_rejects_destination_equal_to_address() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v4(10, 0, 0, 1), 24)
                .destination(v4(10, 0, 0, 1))
                .build(),
        );
    }

    #[test]
    fn build_rejects_invalid_names() {
        assert_invalid_input(TunBuilder::new().name("abcdefghijklmnop").build());
        assert_invalid_input(TunBuilder::new().name("tun0\0evil").build());
    }

    #[test]
    fn build_rejects_invalid_queue_counts() {
        assert_invalid_input(TunBuilder::new().queues(0).build());
        assert_invalid_input(TunBuilder::new().queues(257).build());
    }

    #[test]
    fn build_rejects_mac_address_for_tun() {
        assert_invalid_input(
            TunBuilder::new()
                .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
                .build(),
        );
    }

    #[test]
    fn build_rejects_invalid_mac_addresses() {
        assert_invalid_input(
            TunBuilder::new()
                .tap()
                .mac_address([0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                .build(),
        );
        assert_invalid_input(
            TunBuilder::new()
                .tap()
                .mac_address([0x01, 0x11, 0x22, 0x33, 0x44, 0x55])
                .build(),
        );
    }

    #[test]
    fn validate_accepts_mac_address_for_tap() {
        assert!(
            TunBuilder::new()
                .tap()
                .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn mac_address_can_be_set_before_tap() {
        assert!(
            TunBuilder::new()
                .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
                .tap()
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn validate_accepts_maximum_prefix_lengths() {
        assert!(
            TunBuilder::new()
                .address(v4(10, 0, 0, 1), 32)
                .validate()
                .is_ok()
        );
        assert!(
            TunBuilder::new()
                .address(v6("fd00::1"), 128)
                .validate()
                .is_ok()
        );
    }
}
