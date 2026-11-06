#![cfg(target_os = "linux")]

use std::io::IoSlice;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::io::AsRawFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio_tun::Tun;

macro_rules! skip_without_cap_net_admin {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(err) => {
                let can_skip = matches!(
                    &err,
                    tokio_tun::Error::IoError(e)
                        if e.kind() == std::io::ErrorKind::PermissionDenied
                            || e.raw_os_error() == Some(libc::EPERM)
                            || e.kind() == std::io::ErrorKind::NotFound
                );

                if !can_skip {
                    panic!("Unexpected error creating TUN device: {err:?}");
                }
                if std::env::var_os("TOKIO_TUN_REQUIRE_PRIVILEGED").is_some() {
                    panic!("Privileged TUN test required but unavailable: {err:?}");
                }
                eprintln!(
                    "Skipping test: missing CAP_NET_ADMIN, /dev/net/tun, or permission"
                );
                return;
            }
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
            .address(IpAddr::V4(Ipv4Addr::new(10, 50, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 50, 0, 2)))
            .broadcast(Ipv4Addr::new(10, 50, 0, 255))
            .mtu(1420)
            .up()
            .close_on_exec()
            .build()
    )
    .pop()
    .expect("tun instance");

    assert_eq!(tun.name(), "tuncfg0");
    assert_eq!(
        tun.address().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 50, 0, 1))
    );
    assert_eq!(tun.netmask().unwrap(), Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(
        tun.destination().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 50, 0, 2))
    );
    assert_eq!(tun.broadcast().unwrap(), Ipv4Addr::new(10, 50, 0, 255));
    assert_eq!(tun.mtu().unwrap(), 1420);
    assert!(tun.flags().unwrap() & libc::IFF_UP as i16 != 0);
}

#[tokio::test]
async fn test_multiple_ipv4_and_ipv6_addresses() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunmulti0")
            .address(IpAddr::V4(Ipv4Addr::new(10, 80, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 80, 0, 2)))
            .broadcast(Ipv4Addr::new(10, 80, 0, 255))
            .address(IpAddr::V4(Ipv4Addr::new(10, 81, 0, 1)), 24)
            .address(IpAddr::V6("fd00:80::1".parse().unwrap()), 64)
            .destination(IpAddr::V6("fd00:80::2".parse().unwrap()))
            .address(IpAddr::V6("fd00:81::1".parse().unwrap()), 64)
            .up()
            .build()
    )
    .pop()
    .expect("tun instance");

    let addresses = tun.addresses().unwrap();

    let first_v4 = addresses
        .iter()
        .find(|info| info.address == IpAddr::V4(Ipv4Addr::new(10, 80, 0, 1)))
        .expect("first IPv4 address");
    assert_eq!(first_v4.prefix_len, 24);
    assert_eq!(
        first_v4.destination,
        Some(IpAddr::V4(Ipv4Addr::new(10, 80, 0, 2)))
    );
    assert_eq!(first_v4.broadcast, Some(Ipv4Addr::new(10, 80, 0, 255)));

    assert!(addresses.iter().any(|info| {
        info.address == IpAddr::V4(Ipv4Addr::new(10, 81, 0, 1)) && info.prefix_len == 24
    }));

    let first_v6 = addresses
        .iter()
        .find(|info| info.address == IpAddr::V6("fd00:80::1".parse().unwrap()))
        .expect("first IPv6 address");
    assert_eq!(first_v6.prefix_len, 64);
    assert_eq!(
        first_v6.destination,
        Some(IpAddr::V6("fd00:80::2".parse().unwrap()))
    );

    assert!(addresses.iter().any(|info| {
        info.address == IpAddr::V6("fd00:81::1".parse().unwrap()) && info.prefix_len == 64
    }));

    let primary = tun.address().unwrap();
    assert!(
        primary == IpAddr::V4(Ipv4Addr::new(10, 80, 0, 1))
            || primary == IpAddr::V4(Ipv4Addr::new(10, 81, 0, 1))
    );

    let destination = tun.destination().unwrap();
    assert!(
        destination == IpAddr::V4(Ipv4Addr::new(10, 80, 0, 2))
            || destination == IpAddr::V6("fd00:80::2".parse().unwrap())
    );
}

#[tokio::test]
async fn test_tap_auto_broadcast() {
    let tap = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tapbrd0")
            .tap()
            .address(IpAddr::V4(Ipv4Addr::new(10, 230, 0, 1)), 24)
            .up()
            .build()
    )
    .pop()
    .expect("tap instance");

    assert_eq!(tap.broadcast().unwrap(), Ipv4Addr::new(10, 230, 0, 255));
    assert!(tap.addresses().unwrap().iter().any(|info| {
        info.address == IpAddr::V4(Ipv4Addr::new(10, 230, 0, 1))
            && info.broadcast == Some(Ipv4Addr::new(10, 230, 0, 255))
    }));
}

#[tokio::test]
async fn test_tap_mac_address() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tapmac0")
            .tap()
            .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
            .up()
            .build()
    )
    .pop()
    .expect("tap instance");

    assert_eq!(
        tun.mac_address().unwrap(),
        [0x02, 0x11, 0x22, 0x33, 0x44, 0x55]
    );

    tun.set_mac_address([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee])
        .unwrap();
    assert_eq!(
        tun.mac_address().unwrap(),
        [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]
    );

    for mac in [[0x01, 0x00, 0x00, 0x00, 0x00, 0x00], [0x00; 6], [0xff; 6]] {
        match tun.set_mac_address(mac) {
            Err(tokio_tun::Error::IoError(err)) => {
                assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput)
            }
            Ok(()) => panic!("expected invalid MAC address error for {mac:?}"),
        }
    }
}

#[tokio::test]
async fn test_multi_queue_tap_mac_address() {
    let tuns = skip_without_cap_net_admin!(
        Tun::builder()
            .name("")
            .tap()
            .queues(2)
            .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
            .build()
    );

    assert_eq!(tuns.len(), 2);
    for tun in &tuns {
        assert_eq!(
            tun.mac_address().unwrap(),
            [0x02, 0x11, 0x22, 0x33, 0x44, 0x55]
        );
    }
}

#[tokio::test]
async fn test_tun_mac_address_unsupported() {
    let tun = skip_without_cap_net_admin!(Tun::builder().name("tunmac0").build())
        .pop()
        .expect("tun instance");

    match tun.mac_address() {
        Err(tokio_tun::Error::IoError(err)) => {
            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported)
        }
        Ok(mac) => panic!("expected unsupported error, got {mac:?}"),
    }

    match tun.set_mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]) {
        Err(tokio_tun::Error::IoError(err)) => {
            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported)
        }
        Ok(()) => panic!("expected unsupported error"),
    }

    // Device-type detection must win over argument validation so that every
    // MAC operation on a TUN consistently reports `Unsupported`.
    match tun.set_mac_address([0x00; 6]) {
        Err(tokio_tun::Error::IoError(err)) => {
            assert_eq!(err.kind(), std::io::ErrorKind::Unsupported)
        }
        Ok(()) => panic!("expected unsupported error"),
    }
}

#[tokio::test]
async fn test_failed_build_does_not_leave_interface() {
    let _check = skip_without_cap_net_admin!(Tun::builder().name("").build());

    for name in ["tunleak0", "tunleak1"] {
        assert!(Tun::builder().name(name).mtu(-1).build().is_err());
        assert_eq!(
            interface_index(name),
            0,
            "failed build left interface {name}"
        );
    }

    assert!(
        Tun::builder()
            .name("tunleak2")
            .persist()
            .mtu(-1)
            .build()
            .is_err()
    );
    assert_eq!(interface_index("tunleak2"), 0);
}

fn interface_index(name: &str) -> u32 {
    let name = std::ffi::CString::new(name).unwrap();
    unsafe { libc::if_nametoindex(name.as_ptr()) }
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
async fn test_multi_queue_single_queue() {
    let tuns = skip_without_cap_net_admin!(Tun::builder().name("tunmq1q0").queues(1).up().build());

    assert_eq!(tuns.len(), 1);
    assert_eq!(tuns[0].name(), "tunmq1q0");

    let fd = tuns[0].as_raw_fd();
    let queue: tokio_tun::TunQueue = tuns.into_iter().next().unwrap().into();
    assert_eq!(queue.as_raw_fd(), fd);
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::O_NONBLOCK, 0);

    // Attaching a second queue proves the interface was opened with
    // IFF_MULTI_QUEUE even though only one queue was requested.
    let second = skip_without_cap_net_admin!(Tun::builder().name("tunmq1q0").queues(1).build());
    assert_eq!(second.len(), 1);
    assert_ne!(second[0].as_raw_fd(), fd);
}

#[tokio::test]
async fn test_single_queue_rejects_multi_queue_attach() {
    let tuns = skip_without_cap_net_admin!(Tun::builder().name("tunsq0").up().build());
    assert_eq!(tuns.len(), 1);

    let err = match Tun::builder().name("tunsq0").queues(1).build() {
        Ok(_) => panic!("single-queue device accepted a multi-queue attach"),
        Err(err) => err,
    };
    let tokio_tun::Error::IoError(err) = err;
    assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
    drop(tuns);
}

#[tokio::test]
async fn test_packet_info() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunpi0")
            .address(IpAddr::V4(Ipv4Addr::new(10, 60, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 60, 0, 2)))
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
            .address(IpAddr::V4(Ipv4Addr::new(10, 70, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 70, 0, 2)))
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

#[tokio::test]
async fn test_offload_virtio_header() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunoff0")
            .offload()
            .address(IpAddr::V4(Ipv4Addr::new(10, 70, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 70, 0, 2)))
            .up()
            .build()
    )
    .pop()
    .unwrap();
    assert_eq!(tun.vnet_hdr_len(), tokio_tun::VIRTIO_NET_HDR_LEN);

    let queue: tokio_tun::TunQueue = tun.into();
    assert_eq!(queue.vnet_hdr_len().unwrap(), tokio_tun::VIRTIO_NET_HDR_LEN);

    let socket = UdpSocket::bind("10.70.0.1:0").await.unwrap();
    socket
        .send_to(b"offload packet", "10.70.0.2:7777")
        .await
        .unwrap();

    let mut packet = vec![0u8; 65_535];
    let (n, payload_start) = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), queue.recv(&mut packet))
            .await
            .expect("offload packet timed out")
            .unwrap();
        let payload = &packet[tokio_tun::VIRTIO_NET_HDR_LEN..n];
        if payload
            .windows(b"offload packet".len())
            .any(|w| w == b"offload packet")
        {
            break (n, tokio_tun::VIRTIO_NET_HDR_LEN);
        }
    };

    assert!(n > payload_start);
    let header = tokio_tun::VirtioNetHeader::decode(&packet[..payload_start]).unwrap();
    let payload = &packet[payload_start..n];
    assert!(matches!(payload[0] >> 4, 4 | 6));

    // The virtio header is part of the packet buffer in both directions.
    let mut framed = Vec::with_capacity(tokio_tun::VIRTIO_NET_HDR_LEN + payload.len());
    framed.resize(tokio_tun::VIRTIO_NET_HDR_LEN, 0);
    header.encode(&mut framed).unwrap();
    framed.extend_from_slice(payload);
    queue.send(&framed).await.unwrap();
}

#[tokio::test]
async fn test_offload_rejects_packet_info() {
    let result = Tun::builder().offload().packet_info().build();
    assert!(matches!(
        result,
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[tokio::test]
async fn test_plain_tun_queue_has_no_vnet_header() {
    let tun = skip_without_cap_net_admin!(Tun::builder().name("tunplain0").up().build())
        .pop()
        .unwrap();
    let queue: tokio_tun::TunQueue = tun.into();
    assert_eq!(queue.vnet_hdr_len().unwrap(), 0);
}

#[tokio::test]
async fn test_multi_queue_offload_must_match() {
    let tuns = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunoffmq0")
            .queues(1)
            .offload()
            .up()
            .build()
    );
    assert_eq!(tuns.len(), 1);

    let err = match Tun::builder().name("tunoffmq0").queues(1).build() {
        Ok(_) => panic!("attaching without .offload() unexpectedly succeeded"),
        Err(err) => err,
    };
    let tokio_tun::Error::IoError(err) = err;
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    drop(tuns);
}

#[tokio::test]
async fn test_multi_queue_without_offload_rejects_offload_attach() {
    let tuns =
        skip_without_cap_net_admin!(Tun::builder().name("tunplainmq0").queues(1).up().build());
    assert_eq!(tuns.len(), 1);

    let err = match Tun::builder()
        .name("tunplainmq0")
        .queues(1)
        .offload()
        .build()
    {
        Ok(_) => panic!("attaching with .offload() unexpectedly succeeded"),
        Err(err) => err,
    };
    let tokio_tun::Error::IoError(err) = err;
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    drop(tuns);
}

#[tokio::test]
async fn test_persistent_offload_reattach_without_offload_clears_features() {
    let tun = skip_without_cap_net_admin!(
        Tun::builder()
            .name("tunpersistoff0")
            .persist()
            .offload()
            .address(IpAddr::V4(Ipv4Addr::new(10, 71, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 71, 0, 2)))
            .up()
            .build()
    )
    .pop()
    .unwrap();
    assert_eq!(tun.vnet_hdr_len(), tokio_tun::VIRTIO_NET_HDR_LEN);
    drop(tun);

    // The persistent interface retains its address configuration and up state.
    let plain = skip_without_cap_net_admin!(Tun::builder().name("tunpersistoff0").build())
        .pop()
        .unwrap();
    assert_eq!(plain.vnet_hdr_len(), 0);

    let queue: tokio_tun::TunQueue = plain.into();
    let socket = UdpSocket::bind("10.71.0.1:0").await.unwrap();
    socket
        .send_to(b"persistent offload packet", "10.71.0.2:7777")
        .await
        .unwrap();

    let mut packet = vec![0u8; 65_535];
    let n = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), queue.recv(&mut packet))
            .await
            .expect("persistent offload packet timed out")
            .unwrap();
        if packet[..n]
            .windows(b"persistent offload packet".len())
            .any(|w| w == b"persistent offload packet")
        {
            break n;
        }
    };
    let packet = &packet[..n];

    assert_eq!(packet[0] >> 4, 4);
    let ip_header_len = usize::from(packet[0] & 0x0f) * 4;
    assert!(packet.len() >= ip_header_len + 8);
    assert_eq!(packet[9], 17, "expected UDP");
    assert_eq!(checksum(&packet[..ip_header_len]), 0);
    let udp = &packet[ip_header_len..];
    let udp_len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    assert!(udp_len >= 8 && udp_len <= udp.len());
    assert_ne!([udp[6], udp[7]], [0, 0], "UDP checksum was not computed");
    assert_eq!(udp_checksum(packet, ip_header_len, udp_len), 0);

    // Remove the persistent device so repeated test runs start cleanly.
    unsafe {
        libc::ioctl(queue.as_raw_fd(), libc::TUNSETPERSIST, 0);
    }
    drop(queue);
    assert_eq!(interface_index("tunpersistoff0"), 0);
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn udp_checksum(packet: &[u8], ip_header_len: usize, udp_len: usize) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + udp_len);
    pseudo.extend_from_slice(&packet[12..20]);
    pseudo.push(0);
    pseudo.push(packet[9]);
    pseudo.extend_from_slice(&(udp_len as u16).to_be_bytes());
    pseudo.extend_from_slice(&packet[ip_header_len..ip_header_len + udp_len]);
    checksum(&pseudo)
}
