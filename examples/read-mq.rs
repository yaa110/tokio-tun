#[cfg(target_os = "linux")]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use tokio_tun::Tun;

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let queues = 3;

    let tuns = Tun::builder()
        .name("")
        .mtu(1350)
        .up()
        .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
        .destination(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1)))
        .broadcast(Ipv4Addr::new(10, 0, 0, 255))
        .queues(queues)
        .build()?;

    println!("--------------");
    println!("{queues} tuns created");
    println!("--------------");

    println!(
        "┌ name: {}\n├ fd: {}, {}, {}\n├ mtu: {}\n├ flags: {}\n├ address: {}\n├ destination: {}\n├ broadcast: {}\n└ netmask: {}",
        tuns[0].name(),
        tuns[0].as_raw_fd(),
        tuns[1].as_raw_fd(),
        tuns[2].as_raw_fd(),
        tuns[0].mtu()?,
        tuns[0].flags()?,
        tuns[0].address()?,
        tuns[0].destination()?,
        tuns[0].broadcast()?,
        tuns[0].netmask()?,
    );

    println!("---------------------");
    println!("ping 10.1.0.1 to test");
    println!("---------------------");

    let mut tuns = tuns.into_iter();
    let tun0 = Arc::new(tuns.next().unwrap());
    let tun1 = Arc::new(tuns.next().unwrap());
    let tun2 = Arc::new(tuns.next().unwrap());

    let mut buf0 = vec![0u8; 65_535];
    let mut buf1 = vec![0u8; 65_535];
    let mut buf2 = vec![0u8; 65_535];

    loop {
        let result = tokio::select! {
            result = tun0.recv(&mut buf0) => (0, result),
            result = tun1.recv(&mut buf1) => (1, result),
            result = tun2.recv(&mut buf2) => (2, result),
        };
        let (id, packet) = match result {
            (id, Ok(packet)) => (id, packet),
            (id, Err(err)) => {
                eprintln!("tuns[{id}] receive failed: {err}");
                return Err(err);
            }
        };
        println!(
            "reading {} bytes from tuns[{}]: {:?}",
            packet.len(),
            id,
            packet.payload()
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This example requires Linux multi-queue TUN/TAP support.");
}
