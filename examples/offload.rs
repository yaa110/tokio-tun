//! Linux TUN offload example: receive virtio-net framed packets and echo them.
//!
//! Run with root/CAP_NET_ADMIN on a kernel with TUN offload support:
//!
//! ```bash
//! sudo -E $(which cargo) run --example offload
//! ```

#[cfg(target_os = "linux")]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(target_os = "linux")]
use tokio_tun::Tun;

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tun = Tun::builder()
        .name("")
        .offload()
        .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
        .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
        .up()
        .build_one()?;

    println!(
        "created {} with {} offload features",
        tun.name(),
        tun.offload_features()
    );

    let mut packet = vec![0u8; tun.vnet_hdr_len() + 65_536];
    loop {
        let received = tun.recv(&mut packet).await?;
        println!(
            "received {} byte payload: gso_type={:?}",
            received.len(),
            received.offload_header()
        );

        // Echo the packet back with its offload metadata preserved.
        tun.send(received).await?;
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This example requires Linux TUN offload support.");
}
