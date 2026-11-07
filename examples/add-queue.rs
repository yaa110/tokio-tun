#[cfg(target_os = "linux")]
use std::net::{IpAddr, Ipv4Addr};
#[cfg(target_os = "linux")]
use tokio_tun::{Tun, VIRTIO_NET_HDR_LEN, VirtioNetHeader};

/// Set this to `true` to enable offload. Offload must be enabled consistently
/// on the initial queue and on every attached queue.
#[cfg(target_os = "linux")]
const ENABLE_OFFLOAD: bool = false;

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    // `queues(1)` opens the interface in multi-queue mode. The first queue
    // supplies the address configuration; later queues only attach to it.
    let mut first_builder = Tun::builder()
        .name("")
        .queues(1)
        .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
        .destination(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 1)))
        .up();

    if ENABLE_OFFLOAD {
        first_builder = first_builder.offload();
    }

    let first = first_builder.build().unwrap().pop().unwrap();
    let name = first.name().to_owned();

    println!("created multi-queue interface {name}");
    println!("attach a second queue to {name}");

    // Do not repeat `.address()` or `.up()` here. Only the queue-specific
    // options need to match the original device.
    let mut second_builder = Tun::builder().name(&name).queues(1);
    if ENABLE_OFFLOAD {
        second_builder = second_builder.offload();
    }
    let second = second_builder.build().unwrap().pop().unwrap();

    println!(
        "queue 0 fd={}, queue 1 fd={}",
        std::os::unix::io::AsRawFd::as_raw_fd(&first),
        std::os::unix::io::AsRawFd::as_raw_fd(&second)
    );
    println!("send traffic to 10.1.0.1 to exercise the queues");

    let header_len = if ENABLE_OFFLOAD {
        VIRTIO_NET_HDR_LEN
    } else {
        0
    };
    let mut first_buf = vec![0u8; header_len + 65_536];
    let mut second_buf = vec![0u8; header_len + 65_536];

    loop {
        let (queue, n, buffer) = tokio::select! {
            Ok(n) = first.recv(&mut first_buf) => (0, n, &first_buf[..n]),
            Ok(n) = second.recv(&mut second_buf) => (1, n, &second_buf[..n]),
        };

        if header_len != 0 {
            let header = VirtioNetHeader::decode(&buffer[..header_len]).unwrap();
            println!(
                "queue {queue}: {} byte packet, gso_type={}, payload={} bytes",
                n,
                header.gso_type,
                n - header_len
            );
        } else {
            println!("queue {queue}: {n} byte packet");
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This example requires Linux multi-queue TUN/TAP support.");
}
