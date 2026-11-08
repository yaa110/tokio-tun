use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns the directed broadcast address for an IPv4 prefix.
///
/// Returns `None` for prefixes of 31 or 32 bits, where no directed broadcast
/// address exists.
pub(crate) fn ipv4_broadcast(address: Ipv4Addr, prefix_len: u8) -> Option<Ipv4Addr> {
    if prefix_len >= 31 {
        return None;
    }
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    let network = u32::from_be_bytes(address.octets());
    Some(Ipv4Addr::from((network | !mask).to_be_bytes()))
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn validate_mac(mac: [u8; 6]) -> std::result::Result<(), &'static str> {
    if mac == [0; 6] {
        return Err("MAC address must not be all zeros");
    }
    if mac[0] & 1 != 0 {
        return Err("MAC address must be unicast");
    }
    Ok(())
}

/// Information about an address assigned to a Tun/Tap interface.
///
/// See [`Tun::addresses`](crate::Tun::addresses) for retrieving all addresses
/// assigned to a device.
///
/// This type is `#[non_exhaustive]` so future releases can add fields (for
/// example an IPv6 scope ID) without breaking 1.x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AddressInfo {
    /// IP address of the device.
    pub address: IpAddr,
    /// Prefix length of the address.
    pub prefix_len: u8,
    /// Point-to-point destination address, if one was configured.
    pub destination: Option<IpAddr>,
    /// IPv4 broadcast address reported for this address.
    ///
    /// On Linux this is configured explicitly or derived for a
    /// broadcast-capable interface. On FreeBSD the kernel derives it from the
    /// prefix; prefixes of 31 or 32 bits have no broadcast address.
    pub broadcast: Option<Ipv4Addr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AddressConfig {
    pub address: IpAddr,
    pub prefix_len: u8,
    pub destination: Option<IpAddr>,
    pub broadcast: Option<Ipv4Addr>,
}

impl AddressConfig {
    pub(crate) fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.address.is_unspecified() {
            return Err("address must not be unspecified");
        }
        if self.prefix_len == 0 {
            return Err("prefix length 0 is not allowed");
        }
        if is_forbidden_local(self.address) {
            return Err("address must not be loopback, multicast, or broadcast");
        }
        if let IpAddr::V4(address) = self.address
            && ipv4_broadcast(address, self.prefix_len) == Some(address)
        {
            return Err("address must not be the directed broadcast address");
        }
        let max_prefix = match self.address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if self.prefix_len > max_prefix {
            return Err("prefix length is out of range for the address family");
        }
        if let Some(destination) = self.destination {
            if destination.is_unspecified() {
                return Err("destination must not be unspecified");
            }
            if is_forbidden_destination(destination) {
                return Err("destination must not be loopback, multicast, or broadcast");
            }
            if destination == self.address {
                return Err("destination must differ from the address");
            }
            if destination.is_ipv4() != self.address.is_ipv4() {
                return Err("destination address family must match the address family");
            }
            if let (IpAddr::V4(address), IpAddr::V4(destination)) = (self.address, destination)
                && ipv4_broadcast(address, self.prefix_len) == Some(destination)
            {
                return Err("destination must not be the directed broadcast address");
            }
        }
        if self.broadcast.is_some() && self.address.is_ipv6() {
            return Err("broadcast is only supported for IPv4 addresses");
        }
        if let Some(broadcast) = self.broadcast {
            if self.prefix_len >= 31 {
                return Err("broadcast requires an IPv4 prefix shorter than 31 bits");
            }
            if broadcast.is_unspecified()
                || broadcast.is_multicast()
                || broadcast == Ipv4Addr::BROADCAST
            {
                return Err("invalid broadcast address");
            }
            if let IpAddr::V4(address) = self.address
                && ipv4_broadcast(address, self.prefix_len) != Some(broadcast)
            {
                return Err("broadcast must match the directed broadcast address");
            }
        }
        Ok(())
    }
}

fn is_forbidden_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_forbidden_v4(address, false),
        IpAddr::V6(address) => is_forbidden_v6(address, false),
    }
}

fn is_forbidden_destination(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_forbidden_v4(address, true),
        IpAddr::V6(address) => is_forbidden_v6(address, true),
    }
}

fn is_forbidden_v4(address: Ipv4Addr, include_unspecified: bool) -> bool {
    (include_unspecified && address.is_unspecified())
        || address.is_loopback()
        || address.is_multicast()
        || address.is_broadcast()
}

fn is_forbidden_v6(address: Ipv6Addr, include_unspecified: bool) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_forbidden_v4(mapped, true);
    }
    (include_unspecified && address.is_unspecified())
        || address.is_loopback()
        || address.is_multicast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::net::Ipv6Addr;

    fn config(address: IpAddr, prefix_len: u8) -> AddressConfig {
        AddressConfig {
            address,
            prefix_len,
            destination: None,
            broadcast: None,
        }
    }

    proptest! {
        #[test]
        fn directed_broadcast_preserves_network_bits(address: [u8; 4], prefix_len in 1u8..=30) {
            let address = Ipv4Addr::from(address);
            let broadcast = ipv4_broadcast(address, prefix_len).expect("broadcast");
            let mask = u32::MAX << (32 - u32::from(prefix_len));
            let network = u32::from_be_bytes(address.octets()) & mask;
            let broadcast_bits = u32::from_be_bytes(broadcast.octets());
            prop_assert_eq!(broadcast_bits & mask, network);
            prop_assert_eq!(broadcast_bits | mask, u32::MAX);
        }

        #[test]
        fn host_prefixes_have_no_broadcast(address: [u8; 4], prefix_len in 31u8..=32) {
            prop_assert_eq!(ipv4_broadcast(Ipv4Addr::from(address), prefix_len), None);
        }
    }

    #[test]
    fn accepts_maximum_prefix_lengths() {
        assert!(config("10.0.0.1".parse().unwrap(), 32).validate().is_ok());
        assert!(config("fd00::1".parse().unwrap(), 128).validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_prefix_lengths() {
        assert!(config("10.0.0.1".parse().unwrap(), 33).validate().is_err());
        assert!(config("fd00::1".parse().unwrap(), 129).validate().is_err());
    }

    #[test]
    fn rejects_unspecified_addresses() {
        assert!(
            config(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
                .validate()
                .is_err()
        );
        assert!(
            config(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_destinations() {
        let address: IpAddr = "10.0.0.1".parse().unwrap();

        let mut unspecified = config(address, 24);
        unspecified.destination = Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert!(unspecified.validate().is_err());

        let mut same = config(address, 24);
        same.destination = Some(address);
        assert!(same.validate().is_err());

        let mut wrong_family = config(address, 24);
        wrong_family.destination = Some("fd00::2".parse().unwrap());
        assert!(wrong_family.validate().is_err());

        let mut valid = config(address, 24);
        valid.destination = Some("10.0.0.2".parse().unwrap());
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn rejects_ipv6_broadcast() {
        let mut config = config("fd00::1".parse().unwrap(), 64);
        config.broadcast = Some(Ipv4Addr::new(10, 0, 0, 255));
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_zero_prefix_and_special_addresses() {
        assert!(config("10.0.0.1".parse().unwrap(), 0).validate().is_err());
        assert!(config("fd00::1".parse().unwrap(), 0).validate().is_err());
        assert!(config("127.0.0.1".parse().unwrap(), 8).validate().is_err());
        assert!(config("224.0.0.1".parse().unwrap(), 4).validate().is_err());
        assert!(
            config(IpAddr::V4(Ipv4Addr::BROADCAST), 32)
                .validate()
                .is_err()
        );
        assert!(config("::1".parse().unwrap(), 128).validate().is_err());
        assert!(config("ff02::1".parse().unwrap(), 64).validate().is_err());
    }

    #[test]
    fn rejects_invalid_broadcast_and_destination_specials() {
        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.broadcast = Some(Ipv4Addr::UNSPECIFIED);
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.broadcast = Some(Ipv4Addr::new(224, 0, 0, 1));
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.destination = Some("127.0.0.2".parse().unwrap());
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.destination = Some("224.0.0.2".parse().unwrap());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn computes_directed_broadcast() {
        assert_eq!(
            ipv4_broadcast(Ipv4Addr::new(10, 0, 0, 1), 24),
            Some(Ipv4Addr::new(10, 0, 0, 255))
        );
        assert_eq!(
            ipv4_broadcast(Ipv4Addr::new(10, 0, 0, 1), 8),
            Some(Ipv4Addr::new(10, 255, 255, 255))
        );
        assert_eq!(
            ipv4_broadcast(Ipv4Addr::new(10, 0, 0, 1), 0),
            Some(Ipv4Addr::BROADCAST)
        );
        assert_eq!(ipv4_broadcast(Ipv4Addr::new(10, 0, 0, 1), 31), None);
        assert_eq!(ipv4_broadcast(Ipv4Addr::new(10, 0, 0, 1), 32), None);
    }

    #[test]
    fn rejects_directed_broadcast_addresses_and_mismatched_broadcast() {
        let mut invalid = config("10.0.0.255".parse().unwrap(), 24);
        invalid.broadcast = None;
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.destination = Some("10.0.0.255".parse().unwrap());
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 24);
        invalid.broadcast = Some("192.168.1.255".parse().unwrap());
        assert!(invalid.validate().is_err());

        let mut invalid = config("10.0.0.1".parse().unwrap(), 32);
        invalid.broadcast = Some("10.0.0.255".parse().unwrap());
        assert!(invalid.validate().is_err());

        let mut valid = config("10.0.0.1".parse().unwrap(), 24);
        valid.broadcast = Some("10.0.0.255".parse().unwrap());
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn rejects_ipv4_mapped_special_ipv6_addresses() {
        assert!(
            config("::ffff:127.0.0.1".parse().unwrap(), 128)
                .validate()
                .is_err()
        );
        assert!(
            config("::ffff:224.0.0.1".parse().unwrap(), 128)
                .validate()
                .is_err()
        );
        assert!(
            config("::ffff:0.0.0.0".parse().unwrap(), 128)
                .validate()
                .is_err()
        );
        assert!(
            config("::ffff:255.255.255.255".parse().unwrap(), 128)
                .validate()
                .is_err()
        );
    }
}
