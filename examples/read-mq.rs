use std::net::Ipv4Addr;
use std::os::unix::io::AsRawFd;
use tokio::io::AsyncReadExt;
use tokio_tun::result::Result;
use tokio_tun::TunBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    let queues = 3;

    let tuns = TunBuilder::new()
        .name("")
        .tap(false)
        .packet_info(false)
        .mtu(1350)
        .up()
        .address(Ipv4Addr::new(10, 0, 0, 1))
        .destination(Ipv4Addr::new(10, 1, 0, 1))
        .broadcast(Ipv4Addr::BROADCAST)
        .netmask(Ipv4Addr::new(255, 255, 255, 0))
        .try_build_mq(queues)?;

    println!("--------------");
    println!("{} tuns created", queues);
    println!("--------------");

    println!(
        "┌ name: {}\n├ fd: {}, {}, {}\n├ mtu: {}\n├ flags: {}\n├ address: {}\n├ destination: {}\n├ broadcast: {}\n└ netmask: {}",
        tuns[0].name(),
        tuns[0].as_raw_fd(), tuns[1].as_raw_fd(), tuns[2].as_raw_fd(),
        tuns[0].mtu().unwrap(),
        tuns[0].flags().unwrap(),
        tuns[0].address().unwrap(),
        tuns[0].destination().unwrap(),
        tuns[0].broadcast().unwrap(),
        tuns[0].netmask().unwrap(),
    );

    println!("---------------------");
    println!("ping 10.1.0.2 to test");
    println!("---------------------");

    let mut tuns = tuns.into_iter();
    let mut tun0 = tuns.next().unwrap();
    let mut tun1 = tuns.next().unwrap();
    let mut tun2 = tuns.next().unwrap();

    let mut buf0 = [0u8; 1024];
    let mut buf1 = [0u8; 1024];
    let mut buf2 = [0u8; 1024];

    loop {
        let (buf, id) = tokio::select! {
            Ok(n) = tun0.read(&mut buf0) => (&buf0[..n], 0),
            Ok(n) = tun1.read(&mut buf1) => (&buf1[..n], 1),
            Ok(n) = tun2.read(&mut buf2) => (&buf2[..n], 2),
        };
        println!("reading {} bytes from tuns[{}]: {:?}", buf.len(), id, buf);
    }
}
