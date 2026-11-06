use crate::Result;
use crate::address::AddressConfig;
#[cfg(target_os = "freebsd")]
use crate::freebsd::params::Params;
#[cfg(target_os = "linux")]
use crate::linux::params::Params;
#[cfg(target_os = "macos")]
use crate::macos::params::Params;
use crate::tun::Tun;
#[cfg(target_os = "linux")]
use libc::{IFF_NO_PI, IFF_TAP, IFF_TUN};
use std::net::IpAddr;
#[cfg(target_os = "linux")]
use std::net::Ipv4Addr;

const MAX_INTERFACE_NAME_LEN: usize = 15;
#[cfg(target_os = "linux")]
const MAX_QUEUES: usize = 256;

/// Represents a factory to build new instances of [`Tun`](struct.Tun.html).
pub struct TunBuilder {
    name: String,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    is_tap: bool,
    #[cfg(target_os = "linux")]
    packet_info: bool,
    #[cfg(target_os = "linux")]
    offload: bool,
    #[cfg(target_os = "linux")]
    persist: bool,
    up: bool,
    mtu: Option<i32>,
    #[cfg(target_os = "linux")]
    owner: Option<i32>,
    #[cfg(target_os = "linux")]
    group: Option<i32>,
    #[cfg(target_os = "linux")]
    mac_address: Option<[u8; 6]>,
    addresses: Vec<AddressConfig>,
    invalid_reason: Option<&'static str>,
    #[cfg(target_os = "linux")]
    queues: Option<usize>,
    cloexec: bool,
}

impl Default for TunBuilder {
    fn default() -> Self {
        Self {
            name: "".into(),
            #[cfg(target_os = "linux")]
            owner: None,
            #[cfg(target_os = "linux")]
            group: None,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            is_tap: false,
            #[cfg(target_os = "linux")]
            persist: false,
            up: false,
            mtu: None,
            #[cfg(target_os = "linux")]
            packet_info: false,
            #[cfg(target_os = "linux")]
            offload: false,
            #[cfg(target_os = "linux")]
            mac_address: None,
            addresses: Vec::new(),
            invalid_reason: None,
            #[cfg(target_os = "linux")]
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
    ///
    /// On macOS, only an empty name, `utun`, and `utunN` are supported.
    /// On FreeBSD, use an empty name, `tunN` for TUN, or `tapN` together with
    /// `tap()` for TAP. A pre-existing named interface is attached to without
    /// changing whether it is transient.
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
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn tap(mut self) -> Self {
        self.is_tap = true;
        self
    }

    /// Builds one or more instances of [`Tun`](struct.Tun.html) with the
    /// `IFF_MULTI_QUEUE` flag.
    ///
    /// The device is opened in multi-queue mode even when `queues` is `1`, so
    /// more queues can be opened later. When attaching an additional queue,
    /// use the same interface name and `.queues(...)` without repeating the
    /// address configuration. If this method is not called, the device is
    /// opened in single-queue mode. The value must be between `1` and `256`
    /// inclusive; otherwise [`build`](TunBuilder::build) returns an error.
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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

    /// Enables Linux TUN/TAP offloads (TSO/GSO/GRO) using the virtio net
    /// header.
    ///
    /// This sets `IFF_VNET_HDR`, `IFF_NAPI`, configures checksum and TCP
    /// segmentation offloads, and sets the virtio header size to
    /// [`VIRTIO_NET_HDR_LEN`](crate::VIRTIO_NET_HDR_LEN). NAPI enables GRO for
    /// packets written from userspace into the kernel; it does not coalesce
    /// packets delivered to userspace.
    ///
    /// Every packet read from the device then starts with a
    /// [`VirtioNetHeader`](crate::VirtioNetHeader), and every packet written to
    /// it must include that header. The header carries GSO and checksum
    /// metadata in both directions. Callers are responsible for preserving and
    /// interpreting that metadata. Size receive buffers for at least
    /// `VIRTIO_NET_HDR_LEN + 65_536` bytes because GSO packets can exceed the
    /// interface MTU.
    ///
    /// When attaching another queue to an existing multi-queue device, call
    /// `offload()` on that attach too. An offload-state mismatch is rejected.
    ///
    /// This is mutually exclusive with `packet_info`.
    #[cfg(target_os = "linux")]
    pub fn offload(mut self) -> Self {
        self.offload = true;
        self
    }

    /// Sets the owner of device.
    ///
    /// This is the numeric UID of the user who will own the created device.
    #[cfg(target_os = "linux")]
    pub fn owner(mut self, owner: i32) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Sets the group of device.
    ///
    /// This is the numeric GID of the group that will own the created device.
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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
    /// On macOS and FreeBSD, TUN IPv6 destinations require a `/128` prefix.
    /// FreeBSD TAP devices do not support destinations.
    ///
    /// ```no_run
    /// use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    /// use tokio_tun::Tun;
    ///
    /// let tun = Tun::builder()
    ///     .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
    ///     .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
    ///     .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 128)
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
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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
    ///
    /// On macOS, invalid interface names and IPv6 destinations without a
    /// `/128` prefix return an error. On FreeBSD, names must be empty, `tunN`,
    /// or `tapN`.
    pub fn build(self) -> Result<Vec<Tun>> {
        self.validate()?;
        #[cfg(target_os = "linux")]
        if let Some(queues) = self.queues {
            return Tun::new_mq(self.into(), queues);
        }
        Tun::new(self.into()).map(|tun| vec![tun])
    }

    fn validate(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        self.validate_macos()?;
        #[cfg(target_os = "freebsd")]
        self.validate_freebsd()?;

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
        #[cfg(target_os = "linux")]
        {
            if self.offload && self.packet_info {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "offload and packet_info cannot be enabled together",
                )
                .into());
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
                crate::address::validate_mac(mac).map_err(|reason| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, reason)
                })?;
            }
        }
        for address in &self.addresses {
            address
                .validate()
                .map_err(|reason| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason))?;
        }
        Ok(())
    }

    #[cfg(target_os = "freebsd")]
    fn validate_freebsd(&self) -> Result<()> {
        if !self.name.is_empty() {
            let prefix = if self.is_tap { "tap" } else { "tun" };
            let suffix = self.name.strip_prefix(prefix).unwrap_or("");
            if suffix.is_empty()
                || (suffix.len() > 1 && suffix.starts_with('0'))
                || !suffix.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "FreeBSD interface names must be empty, tunN, or tapN",
                )
                .into());
            }
        }
        if self.is_tap
            && self
                .addresses
                .iter()
                .any(|address| address.destination.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "destination addresses are not supported for FreeBSD TAP devices",
            )
            .into());
        }
        if !self.is_tap
            && self.addresses.iter().any(|address| {
                address.address.is_ipv6()
                    && address.destination.is_some()
                    && address.prefix_len != 128
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "FreeBSD TUN requires IPv6 destinations to use a /128 prefix",
            )
            .into());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn validate_macos(&self) -> Result<()> {
        if self
            .addresses
            .iter()
            .any(|address| address.broadcast.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "broadcast addresses are not supported on macOS utun devices",
            )
            .into());
        }
        if self.addresses.iter().any(|address| {
            address.address.is_ipv6() && address.destination.is_some() && address.prefix_len != 128
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "macOS utun requires IPv6 destinations to use a /128 prefix",
            )
            .into());
        }
        if !self.name.is_empty() && crate::macos::params::parse_utun_name(&self.name).is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "interface names on macOS must be empty, \"utun\", or \"utun<N>\"",
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
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
            offload: builder.offload,
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

    #[cfg(target_os = "macos")]
    fn from(builder: TunBuilder) -> Self {
        Params {
            name: if builder.name.is_empty() {
                None
            } else {
                Some(builder.name)
            },
            up: builder.up,
            mtu: builder.mtu,
            addresses: builder.addresses,
            cloexec: builder.cloexec,
        }
    }

    #[cfg(target_os = "freebsd")]
    fn from(builder: TunBuilder) -> Self {
        Params {
            name: if builder.name.is_empty() {
                None
            } else {
                Some(builder.name)
            },
            tap: builder.is_tap,
            up: builder.up,
            mtu: builder.mtu,
            addresses: builder.addresses,
            cloexec: builder.cloexec,
        }
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn build_rejects_invalid_names() {
        assert_invalid_input(TunBuilder::new().name("abcdefghijklmnop").build());
        assert_invalid_input(TunBuilder::new().name("tun0\0evil").build());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn build_rejects_invalid_queue_counts() {
        assert_invalid_input(TunBuilder::new().queues(0).build());
        assert_invalid_input(TunBuilder::new().queues(257).build());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn build_rejects_mac_address_for_tun() {
        assert_invalid_input(
            TunBuilder::new()
                .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
                .build(),
        );
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_utun_names() {
        assert!(TunBuilder::new().name("").validate().is_ok());
        assert!(TunBuilder::new().name("utun").validate().is_ok());
        assert!(TunBuilder::new().name("utun0").validate().is_ok());
        assert!(TunBuilder::new().name("utun12").validate().is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rejects_invalid_utun_names() {
        assert_invalid_input(TunBuilder::new().name("tun0").build());
        assert_invalid_input(TunBuilder::new().name("utun+1").build());
        assert_invalid_input(TunBuilder::new().name("utun01").build());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rejects_ipv6_destination_below_128() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v6("fd00::1"), 64)
                .destination(v6("fd00::2"))
                .build(),
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_ipv6_destination_at_128() {
        assert!(
            TunBuilder::new()
                .address(v6("fd00::1"), 128)
                .destination(v6("fd00::2"))
                .validate()
                .is_ok()
        );
    }
}
