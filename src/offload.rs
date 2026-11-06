/// Size of the Linux `virtio_net_hdr` prepended to packets when TUN offload is
/// enabled.
pub const VIRTIO_NET_HDR_LEN: usize = 10;
const _: () = assert!(std::mem::size_of::<VirtioNetHeader>() == VIRTIO_NET_HDR_LEN);

/// `virtio_net_hdr` flags.
pub const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
pub const VIRTIO_NET_HDR_F_DATA_VALID: u8 = 2;
pub const VIRTIO_NET_HDR_F_RSC_INFO: u8 = 4;

/// `virtio_net_hdr` GSO types.
pub const VIRTIO_NET_HDR_GSO_NONE: u8 = 0;
pub const VIRTIO_NET_HDR_GSO_TCPV4: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_UDP: u8 = 3;
pub const VIRTIO_NET_HDR_GSO_TCPV6: u8 = 4;
pub const VIRTIO_NET_HDR_GSO_UDP_L4: u8 = 5;
pub const VIRTIO_NET_HDR_GSO_ECN: u8 = 0x80;

/// The Linux `virtio_net_hdr` structure.
///
/// When [`TunBuilder::offload`](crate::TunBuilder::offload) is enabled, every
/// packet read from the TUN/TAP device starts with this ten-byte header. Packets
/// written to the device must also include it.
///
/// The header is in host byte order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtioNetHeader {
    /// Header flags, such as [`VIRTIO_NET_HDR_F_NEEDS_CSUM`].
    pub flags: u8,
    /// GSO type, such as [`VIRTIO_NET_HDR_GSO_TCPV4`].
    pub gso_type: u8,
    /// Header length for GSO packets.
    pub hdr_len: u16,
    /// Maximum segment size for GSO packets.
    pub gso_size: u16,
    /// Offset at which checksum calculation starts.
    pub csum_start: u16,
    /// Offset at which the checksum result should be stored.
    pub csum_offset: u16,
}

impl VirtioNetHeader {
    /// Decodes a virtio network header from the start of `buf`.
    pub fn decode(buf: &[u8]) -> std::io::Result<Self> {
        if buf.len() < VIRTIO_NET_HDR_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtio net header is too short",
            ));
        }
        Ok(Self {
            flags: buf[0],
            gso_type: buf[1],
            hdr_len: u16::from_ne_bytes([buf[2], buf[3]]),
            gso_size: u16::from_ne_bytes([buf[4], buf[5]]),
            csum_start: u16::from_ne_bytes([buf[6], buf[7]]),
            csum_offset: u16::from_ne_bytes([buf[8], buf[9]]),
        })
    }

    /// Encodes this virtio network header into the start of `buf`.
    pub fn encode(&self, buf: &mut [u8]) -> std::io::Result<()> {
        if buf.len() < VIRTIO_NET_HDR_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtio net header buffer is too short",
            ));
        }
        buf[0] = self.flags;
        buf[1] = self.gso_type;
        buf[2..4].copy_from_slice(&self.hdr_len.to_ne_bytes());
        buf[4..6].copy_from_slice(&self.gso_size.to_ne_bytes());
        buf[6..8].copy_from_slice(&self.csum_start.to_ne_bytes());
        buf[8..10].copy_from_slice(&self.csum_offset.to_ne_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_header() {
        let header = VirtioNetHeader {
            flags: VIRTIO_NET_HDR_F_NEEDS_CSUM,
            gso_type: VIRTIO_NET_HDR_GSO_TCPV4,
            hdr_len: 54,
            gso_size: 1448,
            csum_start: 34,
            csum_offset: 16,
        };
        let mut bytes = [0u8; VIRTIO_NET_HDR_LEN];
        header.encode(&mut bytes).unwrap();
        assert_eq!(VirtioNetHeader::decode(&bytes).unwrap(), header);
    }

    #[test]
    fn rejects_short_buffers() {
        assert!(VirtioNetHeader::decode(&[0; VIRTIO_NET_HDR_LEN - 1]).is_err());
        assert!(
            VirtioNetHeader::default()
                .encode(&mut [0; VIRTIO_NET_HDR_LEN - 1])
                .is_err()
        );
    }
}
