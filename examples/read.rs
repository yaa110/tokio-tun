#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::os::unix::io::AsRawFd;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tun = Arc::new(
        Tun::builder()
            .name("")
            .mtu(1350)
            .up()
            .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1)))
            .build_one()?,
    );

    println!("-----------");
    println!("tun created");
    println!("-----------");

    println!(
        "┌ name: {}\n├ fd: {}\n├ mtu: {}\n├ flags: {}\n├ address: {}\n├ destination: {}\n└ netmask: {}",
        tun.name(),
        tun.as_raw_fd(),
        tun.mtu()?,
        tun.flags()?,
        tun.address()?,
        tun.destination()?,
        tun.netmask()?,
    );

    println!("---------------------");
    println!("ping 10.1.0.1 to test");
    println!("---------------------");

    // Reader
    let mut buf = vec![0u8; 65_535];
    loop {
        let packet = tun.recv(&mut buf).await?;
        println!("reading {} bytes: {:?}", packet.len(), packet.payload());
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
