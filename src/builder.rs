use crate::address::AddressConfig;
#[cfg(target_os = "freebsd")]
use crate::freebsd::params::Params;
#[cfg(target_os = "linux")]
use crate::linux::params::Params;
#[cfg(target_os = "macos")]
use crate::macos::params::Params;
#[cfg(target_os = "linux")]
use crate::offload::SUPPORTED_OFFLOAD_FEATURES;
use crate::tun::Tun;
#[cfg(target_os = "linux")]
use crate::{
    DEFAULT_OFFLOAD_FEATURES, TUN_F_CSUM, TUN_F_TSO_ECN, TUN_F_TSO4, TUN_F_TSO6, TUN_F_USO4,
    TUN_F_USO6,
};
#[cfg(target_os = "linux")]
use libc::{IFF_NO_PI, IFF_TAP, IFF_TUN};
use std::io;
use std::net::IpAddr;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::net::Ipv4Addr;

/// Maximum interface name length (`IFNAMSIZ - 1`); shared by the builder and
/// every platform backend so the limit can never drift apart.
pub(crate) const MAX_INTERFACE_NAME_LEN: usize = 15;
#[cfg(target_os = "linux")]
const MAX_QUEUES: usize = 256;

/// Represents a factory to build new instances of [`Tun`].
///
/// A builder is consumed by [`build`](TunBuilder::build). Builder-level
/// validation happens before allocation; kernel-dependent validation can still
/// fail while the device is configured, and transient devices are cleaned up.
/// The packet methods delegate directly to the internal `TunQueue`; `Tun`
/// adds no separate packet path, so both types share the same cancellation and
/// truncation semantics documented there.
///
/// Platform-specific methods are only present on the platforms that support
/// them. TAP and persistence are available on Linux and FreeBSD; multi-queue,
/// packet information, owner/group, and offload are Linux-only.
#[must_use = "a `TunBuilder` does nothing until its `build` method is called"]
#[derive(Debug)]
pub struct TunBuilder {
    name: String,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    is_tap: bool,
    #[cfg(target_os = "linux")]
    packet_info: bool,
    #[cfg(target_os = "linux")]
    offload: Option<u32>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    persist: bool,
    up: bool,
    mtu: Option<i32>,
    #[cfg(target_os = "linux")]
    owner: Option<u32>,
    #[cfg(target_os = "linux")]
    group: Option<u32>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    mac_address: Option<[u8; 6]>,
    addresses: Vec<AddressConfig>,
    invalid_reason: Option<&'static str>,
    /// Validation failures for individual addresses, recorded by
    /// `destination`/`broadcast`. Failures stay attached to the address they
    /// belong to, so an unrelated later call cannot clear them; only a
    /// successful call affecting the same address replaces its reason.
    address_errors: Vec<Option<&'static str>>,
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
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            persist: false,
            up: false,
            mtu: None,
            #[cfg(target_os = "linux")]
            packet_info: false,
            #[cfg(target_os = "linux")]
            offload: None,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            mac_address: None,
            addresses: Vec::new(),
            invalid_reason: None,
            address_errors: Vec::new(),
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
    /// On Linux, a non-empty name creates a new TUN/TAP device or attaches to
    /// an existing persistent device with that name; an already-open
    /// non-persistent name returns `EBUSY`. On macOS, only an empty name,
    /// `utun`, and `utunN` are supported. On FreeBSD, use an empty name, `tunN`
    /// for TUN, or `tapN` together with `tap()` for TAP. A pre-existing named
    /// interface is attached to without changing whether it is transient
    /// unless `persist()` is used. Note that FreeBSD TUN header mode
    /// (`TUNSIFHEAD`) is a property of the device, not the open descriptor:
    /// attaching to a persistent `tunN` switches it to four-byte family
    /// headers for subsequently created descriptors as well.
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
    ///
    /// This crate does not strip or synthesize the prefix: [`Packet::payload`](crate::Packet::payload)
    /// includes the 4 bytes, and `send`/`sendv` expect them in the packet. Use
    /// [`Tun::packet_info`](crate::Tun::packet_info) or
    /// [`TunQueue::packet_info`](crate::TunQueue::packet_info) to detect the mode.
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
    /// This sets `IFF_VNET_HDR`, `IFF_NAPI`, configures
    /// [`DEFAULT_OFFLOAD_FEATURES`](crate::DEFAULT_OFFLOAD_FEATURES)
    /// (checksum, IPv4 TSO, IPv6 TSO, and TSO ECN), and sets the virtio header
    /// size to [`VIRTIO_NET_HDR_LEN`](crate::VIRTIO_NET_HDR_LEN). NAPI enables
    /// GRO for packets written from userspace into the kernel; it does not
    /// coalesce packets delivered to userspace.
    ///
    /// `recv` then returns a [`Packet`](crate::Packet) with the parsed
    /// [`VirtioNetHeader`](crate::VirtioNetHeader) and payload; the raw
    /// descriptor still carries the header on the wire. `send` writes the
    /// header carried by a `Packet`, or a zero header for a payload slice. The
    /// header carries GSO and checksum metadata in both directions. Callers are
    /// responsible for interpreting that metadata. Size receive buffers for the
    /// header plus 65,536 bytes because GSO packets can exceed the interface
    /// MTU; [`Tun::vnet_hdr_len`](crate::Tun::vnet_hdr_len) reports the header
    /// size.
    ///
    /// When attaching another queue to an existing multi-queue device, call
    /// `offload()` or the same `offload_features()` on that attach too. The
    /// kernel exposes whether the virtio header is enabled, so an on/off
    /// mismatch is rejected. It does not expose the current feature mask, so
    /// the caller must keep the feature mask consistent across attaches;
    /// `TUNSETOFFLOAD` is device-wide.
    ///
    /// This is mutually exclusive with `packet_info`.
    ///
    /// Use [`offload_features`](TunBuilder::offload_features) to select a
    /// different feature mask, for example to opt into USO on kernels that
    /// support it.
    #[cfg(target_os = "linux")]
    pub fn offload(mut self) -> Self {
        self.offload = Some(DEFAULT_OFFLOAD_FEATURES);
        self
    }

    /// Enables Linux virtio-net offloads with an explicit feature mask.
    ///
    /// `features` is a bitwise OR of [`TUN_F_CSUM`](crate::TUN_F_CSUM),
    /// [`TUN_F_TSO4`](crate::TUN_F_TSO4), [`TUN_F_TSO6`](crate::TUN_F_TSO6),
    /// [`TUN_F_TSO_ECN`](crate::TUN_F_TSO_ECN),
    /// [`TUN_F_USO4`](crate::TUN_F_USO4), and
    /// [`TUN_F_USO6`](crate::TUN_F_USO6).
    ///
    /// `TUN_F_CSUM` is required whenever any other feature is requested.
    /// `TUN_F_USO4` and `TUN_F_USO6` must be requested together, and
    /// `TUN_F_TSO_ECN` requires `TUN_F_TSO4` or `TUN_F_TSO6`.
    /// [`build`](TunBuilder::build) rejects masks that violate these rules and
    /// masks containing unsupported bits. Features that the running kernel does
    /// not support are reported by `TUNSETOFFLOAD` (normally `EINVAL`).
    ///
    /// UDP-tunnel GSO is not supported in 1.0: Linux requires a 24-byte
    /// `virtio_net_hdr_v1_hash_tunnel`, while this crate exposes the 10-byte
    /// virtio-net header. `TUN_F_UFO` is deprecated and is not supported.
    ///
    /// Passing `0` enables the virtio-net header with no offload features.
    /// This is useful for callers that only want a stable header prefix.
    ///
    /// This is mutually exclusive with `packet_info`, and replaces any previous
    /// `offload()`/`offload_features()` call.
    #[cfg(target_os = "linux")]
    pub fn offload_features(mut self, features: u32) -> Self {
        self.offload = Some(features);
        self
    }

    /// Sets the owner of device.
    ///
    /// This is the numeric UID of the user who will own the created device.
    #[cfg(target_os = "linux")]
    pub fn owner(mut self, owner: u32) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Sets the group of device.
    ///
    /// This is the numeric GID of the group that will own the created device.
    #[cfg(target_os = "linux")]
    pub fn group(mut self, group: u32) -> Self {
        self.group = Some(group);
        self
    }

    /// Sets the MAC address of a TAP device.
    ///
    /// MAC addresses are only supported for TAP devices, so this method must be
    /// used together with [`tap`](TunBuilder::tap); otherwise
    /// [`build`](TunBuilder::build) returns an error. The address must be a
    /// non-zero unicast address. This is supported on Linux and FreeBSD; on
    /// FreeBSD it changes the local/interface link address.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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
    /// `std::net::Ipv6Addr` does not carry a scope ID. Link-local IPv6
    /// addresses are configured on the interface without an additional scope;
    /// applications that need a non-zero scope ID must use the platform
    /// directly.
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
        self.address_errors.push(None);
        // Do not clear `invalid_reason`: a `destination`/`broadcast` call made
        // without a preceding address must still make `build` fail, as its
        // documentation promises. Silently attaching it to a later address
        // would hide a real configuration mistake.
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
        match self.addresses.last_mut() {
            Some(config) => {
                let reason = if config.address.is_ipv4() == destination.is_ipv4() {
                    config.destination = Some(destination);
                    None
                } else {
                    Some("destination address family must match the most recently added address")
                };
                // Replace this address's failure only; earlier addresses keep
                // their recorded reasons.
                *self
                    .address_errors
                    .last_mut()
                    .expect("address_errors tracks every address") = reason;
            }
            None => {
                self.invalid_reason = Some("destination requires a preceding address");
            }
        }
        self
    }

    /// Sets the IPv4 broadcast address for the most recently added address.
    ///
    /// IPv6 has no broadcast address, so the most recently added address must
    /// be an IPv4 address. Call [`address`](TunBuilder::address) before this
    /// method; otherwise [`build`](TunBuilder::build) returns an error.
    ///
    /// On Linux this applies to TUN and TAP devices; TUN devices are
    /// point-to-point, so a broadcast address is recorded but normally has no
    /// routing effect. On FreeBSD only TAP supports it, and
    /// [`Tun::broadcast`](crate::Tun::broadcast) reports the primary/first IPv4
    /// address's broadcast.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn broadcast(mut self, broadcast: Ipv4Addr) -> Self {
        match self.addresses.last_mut() {
            Some(config) => {
                let reason = if config.address.is_ipv4() {
                    config.broadcast = Some(broadcast);
                    None
                } else {
                    Some("broadcast requires a preceding IPv4 address")
                };
                // Replace this address's failure only; earlier addresses keep
                // their recorded reasons.
                *self
                    .address_errors
                    .last_mut()
                    .expect("address_errors tracks every address") = reason;
            }
            None => {
                self.invalid_reason = Some("broadcast requires a preceding address");
            }
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
    ///
    /// On macOS there is no atomic `SOCK_CLOEXEC` path for the utun/control
    /// sockets; the flag is set immediately after `socket(2)`. A concurrent
    /// `fork`/`exec` in another thread could inherit the descriptor in that
    /// brief window.
    pub fn close_on_exec(mut self) -> Self {
        self.cloexec = true;
        self
    }

    /// Makes the device persistent.
    ///
    /// On Linux, persistent devices stay registered until explicitly removed
    /// (for example with `TUNSETPERSIST` set to 0) or the machine reboots.
    /// Opening a persistent name attaches to the existing device rather than
    /// creating a second interface with the same name. On FreeBSD, this removes
    /// transient mode so the interface identity survives its last descriptor
    /// being closed. FreeBSD clears addresses and the up state when the last
    /// descriptor closes; they must be configured again after reattaching.
    ///
    /// Non-persistent devices are removed when the controlling process exits.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

    /// Builds one or more [`Tun`] instances, one per queue.
    ///
    /// Builder-level validation happens before allocation. Kernel-dependent
    /// validation (MTU ranges, address conflicts, unsupported offload masks)
    /// can still fail while the device is configured; transient devices are
    /// cleaned up on failure. On Linux, a builder configured with `queues`
    /// returns exactly that many queues; otherwise it returns one. For a
    /// single-queue device, the returned vector has exactly one element.
    ///
    /// On macOS, invalid interface names and IPv6 destinations without a
    /// `/128` prefix return an error. On FreeBSD, names must be empty, `tunN`,
    /// or `tapN`.
    ///
    /// # Panics
    ///
    /// This method must be called from within a Tokio runtime with I/O enabled.
    /// It panics if Tokio's `AsyncFd` registration cannot find a runtime,
    /// matching [`TunQueue::new`](crate::TunQueue::new).
    pub fn build(self) -> io::Result<Vec<Tun>> {
        self.validate()?;
        #[cfg(target_os = "linux")]
        if let Some(queues) = self.queues {
            return Tun::new_mq(self.into(), queues);
        }
        Tun::new(self.into()).map(|tun| vec![tun])
    }

    /// Builds exactly one [`Tun`] queue.
    ///
    /// This is the convenience form of [`build`](TunBuilder::build) for the
    /// common single-queue case. It returns an error instead of allocating and
    /// then discarding extra queues when `queues` was called, which makes the
    /// `build()?.pop().expect(...)` pattern unnecessary.
    ///
    /// # Panics
    ///
    /// This method must be called from within a Tokio runtime with I/O enabled.
    /// It panics if Tokio's `AsyncFd` registration cannot find a runtime,
    /// matching [`TunQueue::new`](crate::TunQueue::new).
    pub fn build_one(self) -> io::Result<Tun> {
        #[cfg(target_os = "linux")]
        if self.queues.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "build_one requires a single-queue builder; use build() for multi-queue devices",
            ));
        }
        let mut tuns = self.build()?;
        match tuns.pop() {
            Some(tun) if tuns.is_empty() => Ok(tun),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected exactly one queue",
            )),
        }
    }

    fn validate(&self) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        self.validate_macos()?;
        #[cfg(target_os = "freebsd")]
        self.validate_freebsd()?;

        if let Some(reason) = self.invalid_reason {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                reason,
            ));
        }
        if let Some(reason) = self.address_errors.iter().flatten().copied().next() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                reason,
            ));
        }
        if let Some(mtu) = self.mtu {
            // Kernels reject non-positive MTUs (FreeBSD bounds SIOCSIFMTU
            // through `ifhwioctl`, macOS returns `EINVAL`); validating
            // client-side gives one consistent error kind and avoids failing
            // mid-configuration on kernels without a clamp.
            if mtu <= 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "mtu must be positive",
                ));
            }
        }
        if !self.name.is_empty() {
            if self.name.len() > MAX_INTERFACE_NAME_LEN {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "interface name must not exceed 15 bytes",
                ));
            }
            if self.name.contains('\0') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "interface name must not contain a NUL byte",
                ));
            }
        }
        #[cfg(target_os = "linux")]
        {
            if self.offload.is_some() && self.packet_info {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "offload and packet_info cannot be enabled together",
                ));
            }
            if let Some(features) = self.offload
                && features != 0
            {
                if features & TUN_F_CSUM == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "TUN_F_CSUM is required when enabling Linux offload features",
                    ));
                }
                if features & !SUPPORTED_OFFLOAD_FEATURES != 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "unsupported Linux offload feature bits; UDP-tunnel GSO requires a \
                         24-byte header and TUN_F_UFO is deprecated",
                    ));
                }
                let uso = features & (TUN_F_USO4 | TUN_F_USO6);
                if uso != 0 && uso != (TUN_F_USO4 | TUN_F_USO6) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "TUN_F_USO4 and TUN_F_USO6 must be requested together",
                    ));
                }
                if features & TUN_F_TSO_ECN != 0 && features & (TUN_F_TSO4 | TUN_F_TSO6) == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "TUN_F_TSO_ECN requires TUN_F_TSO4 or TUN_F_TSO6",
                    ));
                }
            }
            if matches!(self.queues, Some(queues) if queues == 0 || queues > MAX_QUEUES) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "queues must be between 1 and 256",
                ));
            }
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(mac) = self.mac_address {
            if !self.is_tap {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "MAC address is only supported for TAP devices",
                ));
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

    #[cfg(target_os = "freebsd")]
    fn validate_freebsd(&self) -> io::Result<()> {
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
                ));
            }
        }
        if !self.is_tap
            && self
                .addresses
                .iter()
                .any(|address| address.broadcast.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "broadcast addresses are not supported for FreeBSD TUN devices",
            ));
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
            ));
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
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn validate_macos(&self) -> io::Result<()> {
        if self
            .addresses
            .iter()
            .any(|address| address.broadcast.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "broadcast addresses are not supported on macOS utun devices",
            ));
        }
        if self.addresses.iter().any(|address| {
            address.address.is_ipv6() && address.destination.is_some() && address.prefix_len != 128
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "macOS utun requires IPv6 destinations to use a /128 prefix",
            ));
        }
        if !self.name.is_empty() && crate::macos::params::parse_utun_name(&self.name).is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "interface names on macOS must be empty, \"utun\", or \"utun<N>\"",
            ));
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
            packet_info: builder.packet_info,
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
            persist: builder.persist,
            up: builder.up,
            mtu: builder.mtu,
            mac_address: builder.mac_address,
            addresses: builder.addresses,
            cloexec: builder.cloexec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::{TUN_F_TSO_ECN, TUN_F_TSO4, TUN_F_USO4, TUN_F_USO6};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    fn v6(address: &str) -> IpAddr {
        IpAddr::V6(address.parse::<Ipv6Addr>().unwrap())
    }

    fn assert_invalid_input<T>(result: io::Result<T>) {
        match result {
            Err(err) => {
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

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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
    fn a_later_success_does_not_clear_an_earlier_destination_failure() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v4(10, 0, 0, 1), 24)
                // Records a family failure for the first address.
                .destination(v6("fd00::2"))
                // An unrelated later success must not erase it.
                .address(v6("fd00::1"), 64)
                .destination(v6("fd00::2"))
                .build(),
        );
    }

    #[test]
    fn a_retry_for_the_same_address_clears_its_destination_failure() {
        let builder = TunBuilder::new()
            .address(v4(10, 0, 0, 1), 24)
            // Wrong family first...
            .destination(v6("fd00::2"))
            // ...then the corrected destination for the same address.
            .destination(v4(10, 0, 0, 2));

        assert_eq!(builder.addresses[0].destination, Some(v4(10, 0, 0, 2)));
        builder.validate().unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn a_later_success_does_not_clear_an_earlier_broadcast_failure() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v6("fd00::1"), 64)
                .broadcast(Ipv4Addr::new(10, 0, 0, 255))
                .address(v4(10, 0, 0, 2), 24)
                .broadcast(Ipv4Addr::new(10, 0, 0, 255))
                .build(),
        );
    }

    #[test]
    fn build_rejects_destination_without_address() {
        assert_invalid_input(TunBuilder::new().destination(v4(10, 0, 0, 2)).build());
    }

    #[test]
    fn build_rejects_destination_before_address() {
        assert_invalid_input(
            TunBuilder::new()
                .destination(v4(10, 0, 0, 2))
                .address(v4(10, 0, 0, 1), 24)
                .build(),
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn build_rejects_broadcast_before_address() {
        assert_invalid_input(
            TunBuilder::new()
                .tap()
                .broadcast(Ipv4Addr::new(10, 0, 0, 255))
                .address(v4(10, 0, 0, 1), 24)
                .build(),
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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
    fn build_rejects_non_positive_mtu() {
        assert_invalid_input(TunBuilder::new().mtu(0).build());
        assert_invalid_input(TunBuilder::new().mtu(-1).build());
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
    fn build_one_rejects_multi_queue_builder() {
        assert_invalid_input(TunBuilder::new().queues(2).build_one());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packet_info_clears_no_pi_flag() {
        let params: Params = TunBuilder::new().packet_info().into();
        assert_eq!(params.flags & libc::IFF_NO_PI as i16, 0);
        let params: Params = TunBuilder::new().into();
        assert_ne!(params.flags & libc::IFF_NO_PI as i16, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn offload_features_require_checksum() {
        assert_invalid_input(TunBuilder::new().offload_features(TUN_F_TSO4).build());
        assert_invalid_input(TunBuilder::new().offload_features(TUN_F_USO4).build());
        assert_invalid_input(TunBuilder::new().offload_features(0x80).build());
        assert_invalid_input(
            TunBuilder::new()
                .offload_features(TUN_F_CSUM | TUN_F_USO4)
                .build(),
        );
        assert_invalid_input(
            TunBuilder::new()
                .offload_features(TUN_F_CSUM | TUN_F_TSO_ECN)
                .build(),
        );
        assert!(
            TunBuilder::new()
                .offload_features(TUN_F_CSUM | TUN_F_TSO4)
                .validate()
                .is_ok()
        );
        assert!(
            TunBuilder::new()
                .offload_features(TUN_F_CSUM | TUN_F_USO4 | TUN_F_USO6)
                .validate()
                .is_ok()
        );
        assert!(TunBuilder::new().offload_features(0).validate().is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn build_rejects_mac_address_for_tun() {
        assert_invalid_input(
            TunBuilder::new()
                .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
                .build(),
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

    #[cfg(target_os = "freebsd")]
    #[test]
    fn freebsd_accepts_tun_and_tap_names() {
        assert!(TunBuilder::new().name("tun0").validate().is_ok());
        assert!(TunBuilder::new().tap().name("tap0").validate().is_ok());
        assert!(TunBuilder::new().name("").validate().is_ok());
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn freebsd_rejects_invalid_names() {
        assert_invalid_input(TunBuilder::new().name("tap0").build());
        assert_invalid_input(TunBuilder::new().tap().name("tun0").build());
        assert_invalid_input(TunBuilder::new().name("tun01").build());
        assert_invalid_input(TunBuilder::new().name("tunx").build());
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn freebsd_rejects_ipv6_destination_below_128() {
        assert_invalid_input(
            TunBuilder::new()
                .address(v6("fd00::1"), 64)
                .destination(v6("fd00::2"))
                .build(),
        );
    }
}
