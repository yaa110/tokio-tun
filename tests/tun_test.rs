use std::io::IoSlice;
use std::net::Ipv4Addr;
use std::os::unix::io::AsRawFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio_tun::Tun;

macro_rules! skip_without_cap_net_admin {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(tokio_tun::Error::NixError(nix::errno::Errno::EPERM)) => {
                eprintln!("Skipping test: missing CAP_NET_ADMIN or root privileges");
                return;
            }
            Err(tokio_tun::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(libc::EPERM)
                    || e.kind() == std::io::ErrorKind::NotFound =>
            {
                eprintln!("Skipping test: /dev/net/tun unavailable or permission denied");
                return;
            }
            Err(e) => panic!("Unexpected error creating TUN device: {e:?}"),
        }
    };
}

#[tokio::test]
async fn test_tun_builder_defaults_and_kernel_naming() {
    let tun = skip_without_cap_net_admin!(Tun::builder().name("").build())
        .pop()
        .expect("tun instance");

    assert!(
        tun.name().starts_with("tun"),
        "expected kernel-assigned tun name, got {}",
        tun.name()
    );
    assert!(tun.as_raw_fd() > 0);
    assert!(tun.index().unwrap() > 0);
    assert!(tun.mtu().is_ok());
    assert!(tun.flags().is_ok());
}

#[tokio::test]
async fn test_tun_config_getters() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tuncfg0")
            .address(Ipv4Addr::new(10, 50, 0, 1))
            .netmask(Ipv4Addr::new(255, 255, 255, 0))
            .destination(Ipv4Addr::new(10, 50, 0, 2))
            .broadcast(Ipv4Addr::new(10, 50, 0, 255))
            .mtu(1420)
            .up()
            .close_on_exec()
            .build()
    )
    .pop()
    .expect("tun instance");

    assert_eq!(tun.name(), "tuncfg0");
    assert_eq!(tun.address().unwrap(), Ipv4Addr::new(10, 50, 0, 1));
    assert_eq!(tun.netmask().unwrap(), Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(tun.destination().unwrap(), Ipv4Addr::new(10, 50, 0, 2));
    assert_eq!(tun.broadcast().unwrap(), Ipv4Addr::new(10, 50, 0, 255));
    assert_eq!(tun.mtu().unwrap(), 1420);
    assert!(tun.flags().unwrap() & libc::IFF_UP as i16 != 0);
}

#[tokio::test]
async fn test_tap_device() {
    let tap =
        skip_without_cap_net_admin!(Tun::builder().name("taptest0").tap().mtu(1500).up().build())
            .pop()
            .expect("tap instance");

    assert_eq!(tap.name(), "taptest0");
    assert_eq!(tap.mtu().unwrap(), 1500);
    assert!(tap.as_raw_fd() > 0);
}

#[tokio::test]
async fn test_multi_queue() {
    let tuns =
        skip_without_cap_net_admin!(Tun::builder().name("tunmqtest0").queues(3).up().build());

    assert_eq!(tuns.len(), 3);
    let fd0 = tuns[0].as_raw_fd();
    let fd1 = tuns[1].as_raw_fd();
    let fd2 = tuns[2].as_raw_fd();

    assert_ne!(fd0, fd1);
    assert_ne!(fd1, fd2);
    assert_ne!(fd0, fd2);

    for tun in &tuns {
        assert_eq!(tun.name(), "tunmqtest0");
    }
}

#[tokio::test]
async fn test_packet_info() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunpi0")
            .address(Ipv4Addr::new(10, 60, 0, 1))
            .netmask(Ipv4Addr::new(255, 255, 255, 0))
            .destination(Ipv4Addr::new(10, 60, 0, 2))
            .packet_info()
            .up()
            .build()
    )
    .pop()
    .expect("tun instance");

    let socket = UdpSocket::bind("10.60.0.1:0").await.expect("bind udp");
    let payload = b"ping packet info";

    // Read loop to filter out any background kernel packets (like IPv6 router solicitations)
    let mut buf = [0u8; 1500];
    let start = std::time::Instant::now();
    let mut found = false;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        socket
            .send_to(payload, "10.60.0.2:8888")
            .await
            .expect("send_to");
        if let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_millis(300), tun.recv(&mut buf)).await
            && n >= 4
            && buf[4..n].windows(payload.len()).any(|w| w == payload)
        {
            // Packet info adds 4 bytes prefix: 2 bytes flags, 2 bytes proto (ETH_P_IP = 0x0800)
            let proto = u16::from_be_bytes([buf[2], buf[3]]);
            assert_eq!(proto, libc::ETH_P_IP as u16);
            found = true;
            break;
        }
    }
    assert!(found, "expected payload with packet_info not received");
}

#[tokio::test]
async fn test_async_read_write_traits() {
    let mut tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunrw0")
            .address(Ipv4Addr::new(10, 70, 0, 1))
            .netmask(Ipv4Addr::new(255, 255, 255, 0))
            .destination(Ipv4Addr::new(10, 70, 0, 2))
            .up()
            .build()
    )
    .pop()
    .expect("tun instance");

    let socket = UdpSocket::bind("10.70.0.1:0").await.expect("bind udp");
    let payload = b"async read write test";

    let mut buf = [0u8; 1500];
    let start = std::time::Instant::now();
    let mut received_len = 0;
    while start.elapsed() < std::time::Duration::from_secs(3) {
        socket
            .send_to(payload, "10.70.0.2:7777")
            .await
            .expect("send_to");
        if let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_millis(300), tun.read(&mut buf)).await
            && buf[..n].windows(payload.len()).any(|w| w == payload)
        {
            received_len = n;
            break;
        }
    }
    assert!(received_len > 0, "payload not received through AsyncRead");

    // AsyncWrite write and flush
    let dummy_packet = &buf[..received_len];
    tun.write_all(dummy_packet).await.expect("write_all to tun");
    tun.flush().await.expect("flush tun");
}

#[tokio::test]
async fn test_vectored_and_send_methods() {
    let tun = skip_without_cap_net_admin!(Tun::builder().name("tunvec0").up().build())
        .pop()
        .expect("tun instance");

    // Test sendv, sendv_all, try_send, try_sendv, try_recv
    let mut dummy_ip = [0u8; 28];
    dummy_ip[0] = 0x45; // IPv4, header len 20
    dummy_ip[8] = 64; // TTL
    dummy_ip[9] = 17; // UDP

    let part1 = &dummy_ip[..14];
    let part2 = &dummy_ip[14..];
    let bufs = [IoSlice::new(part1), IoSlice::new(part2)];

    let written = tun.sendv(&bufs).await.expect("sendv");
    assert_eq!(written, 28);

    let mut bufs_all = [IoSlice::new(part1), IoSlice::new(part2)];
    tun.sendv_all(&mut bufs_all).await.expect("sendv_all");

    tun.send(&dummy_ip).await.expect("send");
    tun.send_all(&dummy_ip).await.expect("send_all");

    let _ = tun.try_send(&dummy_ip);
    let _ = tun.try_sendv(&bufs);

    let mut recv_buf = [0u8; 1500];
    let _ = tun.try_recv(&mut recv_buf);
}

#[tokio::test]
async fn test_invalid_config_error() {
    // Check permission first
    let _check = skip_without_cap_net_admin!(Tun::builder().name("").build());

    // Setting an invalid MTU (e.g. -1 or 100_000) should be rejected by the Linux kernel with EINVAL
    let result = Tun::builder().name("tunerr0").mtu(-1).build();

    assert!(result.is_err(), "expected error for invalid MTU");
    let err = result.err().unwrap();
    let err_str = err.to_string();
    assert!(!err_str.is_empty());
}
