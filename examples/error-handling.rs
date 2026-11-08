//! Demonstrates classified error handling with `std::io::Error`.
//!
//! This example intentionally builds invalid configurations and reports the
//! original `io::ErrorKind` and errno instead of string matching.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::net::IpAddr;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use tokio_tun::Tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[tokio::main]
async fn main() {
    report(
        "prefix out of range",
        Tun::builder()
            .address("10.0.0.1".parse().unwrap(), 33)
            .build(),
    );

    // This may fail with PermissionDenied when not running as root. The error
    // keeps the operating-system errno and kind.
    report(
        "device allocation",
        Tun::builder()
            .name("")
            .address(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1)), 24)
            .up()
            .build(),
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn report<T>(label: &str, result: std::io::Result<T>) {
    match result {
        Ok(_) => println!("{label}: succeeded"),
        Err(err) => {
            println!(
                "{label}: kind={:?}, errno={:?}, message={}",
                err.kind(),
                err.raw_os_error(),
                err
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn main() {
    eprintln!("This example requires Linux, macOS, or FreeBSD.");
}
