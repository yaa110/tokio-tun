#![cfg(target_os = "freebsd")]

use std::io::IoSlice;
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::OwnedFd;
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};
use tokio_tun::Tun;

struct DestroyDevice(String);

impl Drop for DestroyDevice {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/sbin/ifconfig")
            .args([&self.0, "destroy"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

static DEVICE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serial_device_test() -> tokio::sync::MutexGuard<'static, ()> {
    DEVICE_LOCK.lock().await
}

macro_rules! require_root {
    () => {
        if unsafe { libc::geteuid() } != 0 {
            if std::env::var_os("TOKIO_TUN_REQUIRE_PRIVILEGED").is_some() {
                panic!("FreeBSD TUN/TAP tests require root");
            }
            eprintln!("Skipping FreeBSD TUN/TAP test: requires root");
            return;
        }
    };
}

#[tokio::test]
async fn test_freebsd_tun_device() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .mtu(1400)
        .address("10.211.0.1".parse().unwrap(), 24)
        .destination("10.211.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert!(tun.name().starts_with("tun"));
    assert!(tun.index().unwrap() > 0);
    assert_eq!(tun.mtu().unwrap(), 1400);
    assert_eq!(
        tun.address().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 211, 0, 1))
    );
    assert_eq!(tun.netmask().unwrap(), Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(
        tun.destination().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 211, 0, 2))
    );
    assert_ne!(tun.flags().unwrap() & libc::IFF_UP as i16, 0);
}

#[tokio::test]
async fn test_freebsd_tun_packet_io() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("10.212.0.1".parse().unwrap(), 24)
        .destination("10.212.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let tun = std::sync::Arc::new(tun);
    let reader_tun = tun.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut buffer = vec![0u8; 65_535];
        loop {
            let n = reader_tun.recv(&mut buffer).await.unwrap();
            if buffer[..n]
                .windows(b"freebsd tun packet".len())
                .any(|window| window == b"freebsd tun packet")
            {
                let _ = tx.send((n, buffer[..n].to_vec()));
                break;
            }
        }
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    let socket = UdpSocket::bind("10.212.0.1:0").await.unwrap();
    let payload = b"freebsd tun packet";
    socket.send_to(payload, "10.212.0.2:7777").await.unwrap();

    let (n, buffer) = timeout(Duration::from_secs(3), rx)
        .await
        .expect("packet was not received from the FreeBSD tun device")
        .expect("reader task ended");
    reader.abort();
    let packet = &buffer[..n];
    assert!(n >= 20 + 8, "packet is too short: {n} bytes");
    assert_eq!(packet[0] >> 4, 4, "expected an IPv4 packet");
    assert_eq!(&packet[n - payload.len()..], payload);

    let (first, second) = packet.split_at(8);
    let bufs = [IoSlice::new(first), IoSlice::new(second)];
    assert_eq!(tun.sendv(&bufs).await.unwrap(), n);
}

#[tokio::test]
async fn test_freebsd_tap_device() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert!(tap.name().starts_with("tap"));
    assert!(tap.index().unwrap() > 0);
    assert!(tap.mtu().unwrap() > 0);

    let mut frame = [0u8; 64];
    frame[..6].fill(0xff);
    frame[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 1]);
    frame[12..14].copy_from_slice(&[0x08, 0x00]);
    assert_eq!(tap.send(&frame).await.unwrap(), frame.len());
}

#[tokio::test]
async fn test_freebsd_ipv6_address() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("fd00:211::1".parse().unwrap(), 64)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let expected: IpAddr = "fd00:211::1".parse().unwrap();
    let addresses = tun.addresses().unwrap();
    assert!(
        addresses
            .iter()
            .any(|address| address.address == expected && address.prefix_len == 64)
    );
    assert!(
        addresses
            .iter()
            .filter(|address| address.address.is_ipv6())
            .all(|address| address.destination.is_none())
    );
    assert!(tun.destination().is_err());
}

#[tokio::test]
async fn test_freebsd_tun_ipv6_packet_io() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("fd00:212::1".parse().unwrap(), 128)
        .destination("fd00:212::2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let tun = std::sync::Arc::new(tun);
    let reader_tun = tun.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut buffer = vec![0u8; 65_535];
        loop {
            let n = reader_tun.recv(&mut buffer).await.unwrap();
            if buffer[..n]
                .windows(b"freebsd ipv6 packet".len())
                .any(|window| window == b"freebsd ipv6 packet")
            {
                let _ = tx.send((n, buffer[..n].to_vec()));
                break;
            }
        }
    });

    let expected: IpAddr = "fd00:212::1".parse().unwrap();
    for _ in 0..50 {
        if tun
            .addresses()
            .unwrap()
            .iter()
            .any(|address| address.address == expected && address.prefix_len == 128)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut attempts = 0;
    let socket = loop {
        match UdpSocket::bind("[fd00:212::1]:0").await {
            Ok(socket) => break socket,
            Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable && attempts < 50 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => panic!("failed to bind IPv6 tunnel address: {err}"),
        }
    };
    let payload = b"freebsd ipv6 packet";
    socket.send_to(payload, "[fd00:212::2]:7777").await.unwrap();

    let (n, buffer) = timeout(Duration::from_secs(3), rx)
        .await
        .expect("IPv6 packet was not received from the FreeBSD tun device")
        .expect("reader task ended");
    reader.abort();
    let packet = &buffer[..n];
    assert!(n >= 40 + 8, "packet is too short: {n} bytes");
    assert_eq!(packet[0] >> 4, 6, "expected an IPv6 packet");
    assert_eq!(&packet[n - payload.len()..], payload);

    let (first, second) = packet.split_at(8);
    let bufs = [IoSlice::new(first), IoSlice::new(second)];
    assert_eq!(tun.sendv(&bufs).await.unwrap(), n);
}

#[tokio::test]
async fn test_freebsd_tun_queue_transfer() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("10.214.0.1".parse().unwrap(), 24)
        .destination("10.214.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let queue: tokio_tun::TunQueue = tun.into();
    let fd: OwnedFd = queue.into();
    let queue = tokio_tun::TunQueue::new(fd).unwrap();
    let reader_queue = std::sync::Arc::new(queue);
    let reader = reader_queue.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut buffer = vec![0u8; 65_535];
        loop {
            let n = reader.recv(&mut buffer).await.unwrap();
            if buffer[..n]
                .windows(b"freebsd queue transfer".len())
                .any(|window| window == b"freebsd queue transfer")
            {
                let _ = tx.send((n, buffer[..n].to_vec()));
                break;
            }
        }
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    let socket = UdpSocket::bind("10.214.0.1:0").await.unwrap();
    socket
        .send_to(b"freebsd queue transfer", "10.214.0.2:7777")
        .await
        .unwrap();

    let (n, packet) = timeout(Duration::from_secs(3), rx)
        .await
        .expect("packet was not received after queue transfer")
        .expect("reader task ended");
    task.abort();
    assert_eq!(packet[0] >> 4, 4, "queue transfer lost TUNSIFHEAD handling");
    assert!(n >= 20 + 8);
}

#[tokio::test]
async fn test_freebsd_tun_address_without_destination() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("10.215.0.1".parse().unwrap(), 24)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(
        tun.address().unwrap(),
        IpAddr::V4(Ipv4Addr::new(10, 215, 0, 1))
    );
    assert!(tun.destination().is_err());
}

#[tokio::test]
async fn test_freebsd_rejects_non_128_ipv6_destination() {
    let result = Tun::builder()
        .address("fd00:215::1".parse().unwrap(), 64)
        .destination("fd00:215::2".parse().unwrap())
        .build();
    assert!(matches!(
        result,
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[tokio::test]
async fn test_freebsd_tun_truncation_is_reported() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder()
        .name("")
        .address("10.216.0.1".parse().unwrap(), 24)
        .destination("10.216.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;
    let socket = UdpSocket::bind("10.216.0.1:0").await.unwrap();
    socket
        .send_to(b"truncate me", "10.216.0.2:7777")
        .await
        .unwrap();

    let mut small = [0u8; 1];
    let err = timeout(Duration::from_secs(3), tun.recv(&mut small))
        .await
        .expect("truncation test timed out")
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn test_freebsd_rejects_tap_destination() {
    let result = Tun::builder()
        .tap()
        .address("10.217.0.1".parse().unwrap(), 24)
        .destination("10.217.0.2".parse().unwrap())
        .build();
    assert!(matches!(
        result,
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[tokio::test]
async fn test_freebsd_default_build_is_transient() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .build()
        .unwrap()
        .pop()
        .unwrap();
    let name = tap.name().to_owned();
    let name_c = std::ffi::CString::new(name.clone()).unwrap();
    drop(tap);
    assert_eq!(
        unsafe { libc::if_nametoindex(name_c.as_ptr()) },
        0,
        "non-persistent device {name} survived close"
    );
}

#[tokio::test]
async fn test_freebsd_tap_mac_address() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(
        tap.mac_address().unwrap(),
        [0x02, 0x11, 0x22, 0x33, 0x44, 0x55]
    );
    tap.set_mac_address([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee])
        .unwrap();
    assert_eq!(
        tap.mac_address().unwrap(),
        [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]
    );
}

#[tokio::test]
async fn test_freebsd_tun_mac_address_unsupported() {
    require_root!();
    let _serial = serial_device_test().await;

    let tun = Tun::builder().name("").up().build().unwrap().pop().unwrap();
    assert!(matches!(
        tun.mac_address(),
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::Unsupported
    ));
    assert!(matches!(
        tun.set_mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]),
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::Unsupported
    ));
    assert!(matches!(
        tun.broadcast(),
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::Unsupported
    ));
}

#[tokio::test]
async fn test_freebsd_persist_survives_close_and_can_be_reattached() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .persist()
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();
    let name = tap.name().to_owned();
    let _cleanup = DestroyDevice(name.clone());
    drop(tap);

    let name_c = std::ffi::CString::new(name.clone()).unwrap();
    assert!(
        unsafe { libc::if_nametoindex(name_c.as_ptr()) } > 0,
        "persistent device {name} disappeared after close"
    );

    // An unnamed allocation must not adopt the persistent device.
    let other = Tun::builder()
        .tap()
        .name("")
        .build()
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(other.name(), name);
    drop(other);
    assert!(
        unsafe { libc::if_nametoindex(name_c.as_ptr()) } > 0,
        "unnamed allocation adopted and destroyed persistent device {name}"
    );

    let reattached = Tun::builder()
        .tap()
        .name(&name)
        .build()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(reattached.name(), name);
    drop(reattached);
    assert!(
        unsafe { libc::if_nametoindex(name_c.as_ptr()) } > 0,
        "persistent device {name} disappeared after reattach close"
    );

    let status = std::process::Command::new("/sbin/ifconfig")
        .args([&name, "destroy"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "failed to destroy persistent device {name}"
    );
}

#[tokio::test]
async fn test_freebsd_tap_broadcast() {
    require_root!();
    let _serial = serial_device_test().await;

    let broadcast = Ipv4Addr::new(10, 218, 0, 255);
    let tap = Tun::builder()
        .tap()
        .name("")
        .address("10.218.0.1".parse().unwrap(), 24)
        .broadcast(broadcast)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(tap.broadcast().unwrap(), broadcast);
    let address = tap
        .addresses()
        .unwrap()
        .into_iter()
        .find(|address| address.address == IpAddr::V4(Ipv4Addr::new(10, 218, 0, 1)))
        .expect("tap IPv4 address");
    assert_eq!(address.broadcast, Some(broadcast));
}

#[tokio::test]
async fn test_freebsd_tun_rejects_broadcast() {
    let result = Tun::builder()
        .address("10.219.0.1".parse().unwrap(), 24)
        .broadcast(Ipv4Addr::new(10, 219, 0, 255))
        .build();
    assert!(matches!(
        result,
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[tokio::test]
async fn test_freebsd_tap_broadcast_is_per_address() {
    require_root!();
    let _serial = serial_device_test().await;

    let first = Ipv4Addr::new(10, 235, 0, 255);
    let second = Ipv4Addr::new(10, 236, 0, 255);
    let tap = Tun::builder()
        .tap()
        .name("")
        .address("10.235.0.1".parse().unwrap(), 24)
        .broadcast(first)
        .address("10.236.0.1".parse().unwrap(), 24)
        .broadcast(second)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let addresses = tap.addresses().unwrap();
    for (address, broadcast) in [
        (Ipv4Addr::new(10, 235, 0, 1), first),
        (Ipv4Addr::new(10, 236, 0, 1), second),
    ] {
        let info = addresses
            .iter()
            .find(|info| info.address == IpAddr::V4(address))
            .expect("configured IPv4 address");
        assert_eq!(info.broadcast, Some(broadcast));
    }
    assert_eq!(tap.broadcast().unwrap(), first);
}

#[tokio::test]
async fn test_freebsd_tap_host_prefix_has_no_broadcast() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .address("10.234.0.1".parse().unwrap(), 32)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let address = tap
        .addresses()
        .unwrap()
        .into_iter()
        .find(|address| address.address == IpAddr::V4(Ipv4Addr::new(10, 234, 0, 1)))
        .expect("tap /32 address");
    assert_eq!(address.broadcast, None);
    assert!(matches!(
        tap.broadcast(),
        Err(tokio_tun::Error::IoError(ref err))
            if err.kind() == std::io::ErrorKind::AddrNotAvailable
    ));
}

fn freebsd_device_names() -> Vec<String> {
    let output = std::process::Command::new("/sbin/ifconfig")
        .arg("-l")
        .output()
        .expect("ifconfig -l");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter(|name| name.starts_with("tun") || name.starts_with("tap"))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn test_freebsd_failed_persistent_build_does_not_leave_interface() {
    require_root!();
    let _serial = serial_device_test().await;

    let before = freebsd_device_names();
    let result = Tun::builder().tap().name("").persist().mtu(-1).build();
    assert!(result.is_err(), "invalid MTU unexpectedly succeeded");

    let after = freebsd_device_names();
    assert_eq!(after, before, "failed persistent build leaked an interface");
}

#[tokio::test]
async fn test_freebsd_renamed_persistent_device_is_not_adopted() {
    require_root!();
    let _serial = serial_device_test().await;

    let tap = Tun::builder()
        .tap()
        .name("")
        .persist()
        .address("10.237.0.1".parse().unwrap(), 24)
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();
    let name = tap.name().to_owned();
    let _cleanup_original = DestroyDevice(name.clone());
    drop(tap);

    let status = std::process::Command::new("/sbin/ifconfig")
        .args([&name, "inet", "10.237.0.1/24", "up"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "failed to configure persistent device {name}"
    );

    let renamed = "renamedoff0";
    let status = std::process::Command::new("/sbin/ifconfig")
        .args([&name, "name", renamed])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "failed to rename persistent device {name}"
    );
    let _cleanup = DestroyDevice(renamed.to_owned());

    let renamed_c = std::ffi::CString::new(renamed).unwrap();
    assert!(unsafe { libc::if_nametoindex(renamed_c.as_ptr()) } > 0);

    let other = Tun::builder()
        .tap()
        .name("")
        .build()
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(other.name(), name);
    drop(other);
    assert!(
        unsafe { libc::if_nametoindex(renamed_c.as_ptr()) } > 0,
        "unnamed allocation destroyed renamed persistent device"
    );

    let result = Tun::builder().tap().name(&name).build();
    assert!(result.is_err(), "stale device name unexpectedly attached");
    assert!(
        unsafe { libc::if_nametoindex(renamed_c.as_ptr()) } > 0,
        "explicit stale name destroyed renamed persistent device"
    );

    let output = std::process::Command::new("/sbin/ifconfig")
        .arg(renamed)
        .output()
        .unwrap();
    let config = String::from_utf8_lossy(&output.stdout);
    assert!(
        config.contains("10.237.0.1"),
        "renamed interface lost its address"
    );
    assert!(
        config.contains("<UP"),
        "renamed interface lost its up state"
    );
}
