//! Demonstrates non-blocking packet I/O with `try_recv` and `try_send`.
//!
//! Run with root/CAP_NET_ADMIN:
//!
//! ```bash
//! sudo -E $(which cargo) run --example nonblocking
//! ```

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::io;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() -> io::Result<()> {
    let tun = Tun::builder()
        .name("")
        .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
        .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        .up()
        .build_one()?;

    let mut buffer = [0u8; 65_535];
    for _ in 0..10 {
        match tun.try_recv(&mut buffer) {
            Ok(packet) => println!("received {} bytes", packet.len()),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                println!("no packet ready; sleeping");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
