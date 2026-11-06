#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::io::AsRawFd;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")
            .mtu(1350)
            .up()
            .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1)))
            .build()
            .unwrap()
            .pop()
            .unwrap(),
    );

    println!("-----------");
    println!("tun created");
    println!("-----------");

    println!(
        "┌ name: {}\n├ fd: {}\n├ mtu: {}\n├ flags: {}\n├ address: {}\n├ destination: {}\n└ netmask: {}",
        tun.name(),
        tun.as_raw_fd(),
        tun.mtu().unwrap(),
        tun.flags().unwrap(),
        tun.address().unwrap(),
        tun.destination().unwrap(),
        tun.netmask().unwrap(),
    );

    println!("---------------------");
    println!("ping 10.1.0.1 to test");
    println!("---------------------");

    // Reader
    let mut buf = vec![0u8; 65_535];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("This example requires Linux or macOS.");
}
