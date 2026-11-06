#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::os::unix::io::AsRawFd;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")
            .mtu(1400)
            // Multiple IPv4 addresses, each with its own prefix length.
            .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
            .address(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)), 24)
            // Multiple IPv6 addresses, each with its own prefix length.
            .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 64)
            .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 1, 0, 0, 0, 1)), 64)
            .up()
            .build()
            .unwrap()
            .pop()
            .unwrap(),
    );

    println!("tun created: {}", tun.name());
    println!("fd: {}", tun.as_raw_fd());

    for address in tun.addresses().unwrap() {
        println!(
            "address: {}, prefix: {}, destination: {:?}, broadcast: {:?}",
            address.address, address.prefix_len, address.destination, address.broadcast
        );
    }

    let mut buf = vec![0u8; 65_535];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
