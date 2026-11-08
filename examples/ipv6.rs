//! Creates an IPv6 TUN interface and reads packets.
//!
//! Run with root/CAP_NET_ADMIN:
//!
//! ```bash
//! sudo -E $(which cargo) run --example ipv6
//! ```

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::{IpAddr, Ipv6Addr};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    // macOS and FreeBSD require /128 for an IPv6 point-to-point destination.
    let tun = Tun::builder()
        .name("")
        .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 128)
        .destination(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2)))
        .up()
        .build_one()?;

    println!("created IPv6 tunnel {}", tun.name());
    for address in tun.addresses()? {
        println!(
            "{} /{} -> {:?}",
            address.address, address.prefix_len, address.destination
        );
    }

    let mut packet = vec![0u8; 65_535];
    loop {
        let received = tun.recv(&mut packet).await?;
        println!("received {} byte IPv6 packet", received.len());
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
