use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use tokio_tun::Tun;

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

    let mut buf = [0u8; 1024];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}
