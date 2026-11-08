use std::ops::Deref;

#[cfg(target_os = "linux")]
use crate::VirtioNetHeader;

/// One packet received from a TUN/TAP device.
///
/// [`Packet::payload`] is the IP packet (TUN) or Ethernet frame (TAP). On Linux
/// with offload enabled, `Packet::offload_header` returns the parsed
/// virtio-net header; ordinary packets return `None`.
///
/// On Linux with `packet_info` enabled, the payload includes the kernel's
/// 4-byte `tun_pi` prefix (2 bytes of flags and 2 bytes of protocol). This
/// crate does not strip it; parse it before treating the rest of the payload
/// as an IP packet or Ethernet frame. See `TunQueue::packet_info`.
///
/// `Packet` dereferences to its payload, so `&packet[..]`, `packet.len()`, and
/// `packet.iter()` work directly.
///
/// Equality includes the Linux offload metadata, so two packets with the same
/// payload but different offload headers are not equal. On platforms without
/// offload metadata, equality compares the payload only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    payload: &'a [u8],
    #[cfg(target_os = "linux")]
    offload: Option<VirtioNetHeader>,
}

impl<'a> Packet<'a> {
    /// Creates a packet with no offload metadata.
    #[must_use]
    pub fn new(payload: &'a [u8]) -> Self {
        Self {
            payload,
            #[cfg(target_os = "linux")]
            offload: None,
        }
    }

    /// Creates a packet with an explicit virtio-net header.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn with_offload(payload: &'a [u8], header: VirtioNetHeader) -> Self {
        Self {
            payload,
            offload: Some(header),
        }
    }

    /// Returns the packet payload.
    ///
    /// On Linux with offload enabled, the virtio-net header is stripped. On
    /// Linux with `packet_info` enabled, the 4-byte `tun_pi` prefix is **not**
    /// stripped and is included here.
    #[must_use]
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }

    /// Returns the Linux virtio-net header, if this packet came from an
    /// offload-enabled device and the queue supports the 10-byte header.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn offload_header(&self) -> Option<VirtioNetHeader> {
        self.offload
    }

    /// Returns the payload length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.payload.len()
    }

    /// Returns `true` when the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }
}

impl<'a> Deref for Packet<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.payload
    }
}

impl<'a> AsRef<[u8]> for Packet<'a> {
    fn as_ref(&self) -> &[u8] {
        self.payload
    }
}

impl<'a> From<&'a [u8]> for Packet<'a> {
    fn from(payload: &'a [u8]) -> Self {
        Self::new(payload)
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for Packet<'a> {
    fn from(payload: &'a [u8; N]) -> Self {
        Self::new(payload)
    }
}

impl<'a> From<&'a Vec<u8>> for Packet<'a> {
    fn from(payload: &'a Vec<u8>) -> Self {
        Self::new(payload)
    }
}

impl<'a> From<&'a mut [u8]> for Packet<'a> {
    fn from(payload: &'a mut [u8]) -> Self {
        Self::new(payload)
    }
}

impl<'a> From<&'a mut Vec<u8>> for Packet<'a> {
    fn from(payload: &'a mut Vec<u8>) -> Self {
        Self::new(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_payload() {
        let data = [1u8, 2, 3];
        let packet = Packet::new(&data);
        assert_eq!(packet.payload(), &data);
        assert_eq!(packet.len(), 3);
        assert!(!packet.is_empty());
        assert_eq!(&packet[..], &data);
        assert_eq!(packet.as_ref(), &data);
    }

    #[test]
    fn empty_packet() {
        let data: [u8; 0] = [];
        let packet = Packet::new(&data);
        assert!(packet.is_empty());
        assert_eq!(packet.len(), 0);
    }

    #[test]
    fn from_conversions() {
        let data = [1u8, 2, 3, 4];
        let v = vec![1u8, 2, 3, 4];
        let _ = Packet::from(&data[..]);
        let _ = Packet::from(&data);
        let _ = Packet::from(&v);
        let mut m = [1u8, 2, 3, 4];
        let _ = Packet::from(&mut m[..]);
        let mut mv = vec![1u8, 2, 3, 4];
        let _ = Packet::from(&mut mv);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn with_offload() {
        use crate::VirtioNetHeader;
        let data = [0u8; 4];
        let header = VirtioNetHeader::default();
        let packet = Packet::with_offload(&data, header);
        assert_eq!(packet.offload_header(), Some(header));
        assert_eq!(packet.payload(), &data);
    }
}
