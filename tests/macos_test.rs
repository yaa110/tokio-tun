#![cfg(target_os = "macos")]

use std::io::IoSlice;
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::AsRawFd;
use std::time::{Duration as StdDuration, Instant};
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};
use tokio_tun::{Tun, TunQueue};

macro_rules! require_root {
    () => {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            if std::env::var_os("TOKIO_TUN_REQUIRE_PRIVILEGED").is_some() {
                panic!("macOS utun tests require root");
            }
            eprintln!("Skipping macOS utun test: requires root");
            return;
        }
    };
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_device() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .mtu(1400)
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert!(tun.name().starts_with("utun"));
    assert!(tun.index().unwrap() > 0);
    assert_eq!(tun.mtu().unwrap(), 1400);
    assert!(tun.as_raw_fd() > 0);
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_queue_conversion() {
    require_root!();

    let tun = Tun::builder().name("").up().build().unwrap().pop().unwrap();
    let fd = tun.as_raw_fd();
    let name = tun.name().to_owned();
    let index = tun.index().unwrap();

    let queue: TunQueue = tun.into();
    assert_eq!(queue.as_raw_fd(), fd);
    // SAFETY: `fd` is an open descriptor and F_GETFL only reads status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::O_NONBLOCK, 0);

    // The interface metadata is intentionally unavailable after conversion.
    assert!(name.starts_with("utun"));
    assert!(index > 0);
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_ipv4_address() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("10.123.0.1".parse().unwrap(), 24)
        .destination("10.123.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(
        tun.address().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 123, 0, 1))
    );
    assert_eq!(tun.netmask().unwrap(), Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(
        tun.destination().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 123, 0, 2))
    );

    let addresses = tun.addresses().unwrap();
    assert!(addresses.iter().any(|address| {
        address.address == IpAddr::V4(Ipv4Addr::new(10, 123, 0, 1))
            && address.prefix_len == 24
            && address.destination == Some(IpAddr::V4(Ipv4Addr::new(10, 123, 0, 2)))
    }));
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_ipv4_address_without_destination() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("10.204.0.1".parse().unwrap(), 24)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(
        tun.address().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 204, 0, 1))
    );
    assert_eq!(tun.netmask().unwrap(), Ipv4Addr::new(255, 255, 255, 0));
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_packet_io() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("10.202.0.1".parse().unwrap(), 24)
        .destination("10.202.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let socket = UdpSocket::bind("10.202.0.1:0").await.unwrap();
    let payload = b"macos utun packet";
    let mut buffer = [0u8; 1500];
    let start = Instant::now();
    let mut received = None;

    while start.elapsed() < StdDuration::from_secs(3) {
        socket.send_to(payload, "10.202.0.2:7777").await.unwrap();

        if let Ok(Ok(packet)) = timeout(Duration::from_millis(300), tun.recv(&mut buffer)).await
            && packet
                .payload()
                .windows(payload.len())
                .any(|window| window == payload)
        {
            received = Some(packet.len());
            break;
        }
    }

    let n = received.expect("packet was not received from the utun device");
    let packet = &buffer[..n];
    assert!(n >= 20 + 8, "packet is too short: {n} bytes");
    assert_eq!(packet[0] >> 4, 4, "expected an IPv4 packet");
    assert_eq!(&packet[n - payload.len()..], payload);

    let (first, second) = packet.split_at(8);
    let bufs = [IoSlice::new(first), IoSlice::new(second)];
    tun.sendv(&bufs).await.unwrap();
    tun.try_sendv(&bufs).unwrap();
}

/// Exercises the synchronous `try_*` forms on a quiet utun device.
#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_try_io() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("10.207.0.1".parse().unwrap(), 24)
        .destination("10.207.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    // A freshly created quiet utun queue has nothing pending. Other system
    // traffic for the subnet is not expected, so try_recv reports
    // `WouldBlock` (or a zero-length close packet in the rare teardown case).
    let mut buffer = [0u8; 1500];
    match tun.try_recv(&mut buffer) {
        Err(err) => {
            assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        }
        Ok(packet) => assert!(packet.is_empty(), "unexpected quiet-device packet"),
    }

    let packet = [0x45u8; 20];
    tun.try_send(&packet).unwrap();
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_ipv6_address() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("fd00:123::1".parse().unwrap(), 64)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let expected: IpAddr = "fd00:123::1".parse().unwrap();
    let addresses = tun.addresses().unwrap();
    assert!(
        addresses
            .iter()
            .any(|address| address.address == expected && address.prefix_len == 64)
    );
}

#[ignore = "requires root/CAP_NET_ADMIN"]
#[tokio::test]
async fn test_macos_utun_ipv6_point_to_point() {
    require_root!();

    let tun = Tun::builder()
        .name("")
        .address("fd00:124::1".parse().unwrap(), 128)
        .destination("fd00:124::2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let expected: IpAddr = "fd00:124::1".parse().unwrap();
    let destination: IpAddr = "fd00:124::2".parse().unwrap();
    let addresses = tun.addresses().unwrap();
    let info = addresses
        .iter()
        .find(|address| address.address == expected && address.prefix_len == 128)
        .expect("configured IPv6 address");
    assert!(
        info.destination.is_none() || info.destination == Some(destination),
        "unexpected IPv6 destination: {:?}",
        info.destination
    );

    match tun.destination() {
        Ok(reported) => assert_eq!(reported, destination),
        Err(err) => {
            assert_eq!(err.kind(), std::io::ErrorKind::AddrNotAvailable)
        }
    }
}
