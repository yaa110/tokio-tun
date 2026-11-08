//! Sends IPv4 packets through a TUN device.
//!
//! Run with root/CAP_NET_ADMIN:
//!
//! ```bash
//! sudo -E $(which cargo) run --example send
//! ```

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::io::IoSlice;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tun = Tun::builder()
        .name("")
        .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
        .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        .up()
        .build_one()?;

    let packet = udp_packet(
        Ipv4Addr::new(10, 0, 0, 1),
        Ipv4Addr::new(10, 0, 0, 2),
        12345,
        7777,
        b"hello from tokio-tun",
    );

    // One call writes one packet. The return value is `()`: a short native
    // write is an error, never a reason to send the remainder separately.
    tun.send(&packet).await?;
    println!("sent {} byte packet", packet.len());

    // Vectored writes stay a single packet as well.
    let (first, second) = packet.split_at(20);
    tun.sendv(&[IoSlice::new(first), IoSlice::new(second)])
        .await?;
    println!("sent vectored packet");

    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn udp_packet(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let total_len = 20 + udp_len;
    let mut packet = vec![0u8; total_len];

    packet[0] = 0x45; // IPv4, 20-byte header
    packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    packet[8] = 64; // TTL
    packet[9] = 17; // UDP
    packet[12..16].copy_from_slice(&source.octets());
    packet[16..20].copy_from_slice(&destination.octets());

    let checksum = internet_checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());

    packet[20..22].copy_from_slice(&source_port.to_be_bytes());
    packet[22..24].copy_from_slice(&destination_port.to_be_bytes());
    packet[24..26].copy_from_slice(&(udp_len as u16).to_be_bytes());
    packet[28..].copy_from_slice(payload);

    // A zero UDP checksum is valid for IPv4.
    packet
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
