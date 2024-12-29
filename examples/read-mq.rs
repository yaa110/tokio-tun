use std::net::Ipv4Addr;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use tokio_tun::Tun;

#[tokio::main]
async fn main() {
    let queues = 3;

    let tuns = Tun::builder()
        .name("")
        .mtu(1350)
        .up()
        .address(Ipv4Addr::new(10, 0, 0, 1))
        .destination(Ipv4Addr::new(10, 1, 0, 1))
        .broadcast(Ipv4Addr::BROADCAST)
        .netmask(Ipv4Addr::new(255, 255, 255, 0))
        .queues(queues)
        .build()
        .unwrap();

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
    let tun0 = Arc::new(tuns.next().unwrap());
    let tun1 = Arc::new(tuns.next().unwrap());
    let tun2 = Arc::new(tuns.next().unwrap());

    let mut buf0 = [0u8; 1024];
    let mut buf1 = [0u8; 1024];
    let mut buf2 = [0u8; 1024];

    loop {
        let (buf, id) = tokio::select! {
            Ok(n) = tun0.recv(&mut buf0) => (&buf0[..n], 0),
            Ok(n) = tun1.recv(&mut buf1) => (&buf1[..n], 1),
            Ok(n) = tun2.recv(&mut buf2) => (&buf2[..n], 2),
        };
        println!("reading {} bytes from tuns[{}]: {:?}", buf.len(), id, buf);
    }
}
