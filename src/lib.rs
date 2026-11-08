//! Asynchronous TUN/TAP interfaces for Unix.
//!
//! `tokio-tun` allocates and drives native TUN/TAP interfaces on Linux, macOS,
//! FreeBSD, Android, and iOS. Every supported platform exposes the native
//! packet-oriented descriptor through [Tokio](https://tokio.rs) using
//! [`AsyncFd`](tokio::io::unix::AsyncFd); there is no polling thread or
//! blocking fallback.
//!
//! # Platform model
//!
//! The library deliberately does not reduce every platform to a lowest common
//! denominator. Platform-specific capabilities are gated behind `#[cfg]`:
//!
//! | Platform | Device allocation | Packet API | Native extras |
//! | --- | --- | --- | --- |
//! | Linux | `Tun` / `TunBuilder` | `Tun`, [`TunQueue`] | TAP, multi-queue, PI headers, virtio-net offloads, owner/group, persistence |
//! | macOS | `Tun` / `TunBuilder` | `Tun`, [`TunQueue`] | `utun` interface allocation and configuration |
//! | FreeBSD | `Tun` / `TunBuilder` | `Tun`, [`TunQueue`] | TUN/TAP, persistence, TAP MAC/broadcast |
//! | Android | external descriptor | [`TunQueue`] | `VpnService`-supplied descriptor |
//! | iOS | external descriptor | [`TunQueue`] | Network Extension / socket-pair descriptor |
//!
//! Android and iOS do not expose `Tun` or `TunBuilder`: the application owns the
//! VPN integration and transfers an owning descriptor to [`TunQueue`].
//!
//! # TUN and TAP
//!
//! A **TUN** device is layer 3: each packet is an IPv4 or IPv6 datagram. A
//! **TAP** device is layer 2: each packet is an Ethernet frame. TAP allocation
//! is available on Linux and FreeBSD. macOS `utun` and the mobile integration
//! models are TUN-only.
//!
//! # Platform-specific documentation
//!
//! docs.rs builds a separate API surface for every supported platform. Use the
//! **Platform** selector in the docs.rs top bar to switch between Linux, macOS,
//! FreeBSD, Android, and iOS. Items that are only available on some platforms
//! carry an “Available on …” badge.
//!
//! # Device and queue lifecycle
//!
//! `TunBuilder::build` returns one `Tun` per queue. On Linux,
//! `TunBuilder::queues` puts the interface into multi-queue mode; each returned
//! `Tun` owns one queue descriptor. For a single-queue device, the returned
//! vector has exactly one element. `Tun` also owns the interface accessors (name, index, flags, addresses, MTU). Converting a `Tun` into a
//! [`TunQueue`] drops those accessors and transfers the queue descriptor.
//!
//! The kernel interface lives while at least one queue descriptor is open
//! (unless persistence is requested on Linux or FreeBSD). Closing the last
//! descriptor removes a non-persistent interface.
//!
//! # Packet I/O
//!
//! `Tun::recv` and [`TunQueue::recv`] return exactly one [`Packet`] (payload plus
//! optional Linux offload header). `Tun::send` and [`TunQueue::send`] accept a
//! [`Packet`] or a payload slice. A short native write is reported as an error;
//! the remainder is never silently re-sent as a second packet. `Tun::sendv` and
//! [`TunQueue::sendv`] write one payload-only packet assembled from multiple
//! slices with a single native vectored write.
//!
//! `AsyncRead` and `AsyncWrite` are implemented for compatibility with Tokio
//! utilities. `poll_read`/`poll_write` still operate on one packet per call.
//! Stream helpers such as `read_exact` and `copy` can combine multiple packets,
//! and `write_all` can combine a multi-packet buffer into one write. A short
//! native write is an error, so `write_all` cannot split a single packet by
//! retrying the remainder. On Linux offload, `AsyncRead`/`AsyncWrite` strip/add
//! the virtio header automatically but do not expose its metadata; use
//! `recv`/`send` for the full [`Packet`] value.
//!
//! # Tokio runtime
//!
//! `TunBuilder::build` and [`TunQueue::new`] must be called from inside a Tokio
//! runtime with the I/O driver enabled. They panic if no runtime is available
//! because Tokio's `AsyncFd` registration has that contract.
//!
//! # Descriptor ownership
//!
//! `Tun` and [`TunQueue`] own their descriptors and close them exactly once on
//! drop. [`TunQueue::new`] takes ownership of an
//! [`OwnedFd`](std::os::fd::OwnedFd) (or any type that converts into one), sets
//! `O_NONBLOCK`, and leaves `FD_CLOEXEC` unchanged. `O_NONBLOCK` belongs to the
//! shared open file description, so a descriptor received from another process
//! also becomes non-blocking there.
//!
//! # Permissions
//!
//! Creating and configuring a TUN/TAP interface normally requires root or
//! `CAP_NET_ADMIN` on Linux. macOS `utun` allocation requires root. FreeBSD
//! device creation/configuration requires root. Android and iOS rely on the
//! application's VPN integration and do not require the library to create a
//! device.
//!
//! # Example
//!
//! ```no_run
//! use tokio_tun::Tun;
//!
//! #[tokio::main]
//! async fn main() -> std::io::Result<()> {
//!     let tun = Tun::builder()
//!         .name("")
//!         .address("10.0.0.1".parse().unwrap(), 24)
//!         .destination("10.0.0.2".parse().unwrap())
//!         .up()
//!         .build_one()?;
//!
//!     let mut packet = vec![0u8; 65_535];
//!     loop {
//!         let received = tun.recv(&mut packet).await?;
//!         println!("received {} bytes on {}", received.len(), tun.name());
//!     }
//! }
//! ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(rustdoc::broken_intra_doc_links)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
)))]
compile_error!("tokio-tun supports Linux, macOS, Android, iOS, and FreeBSD only");

#[cfg(target_os = "freebsd")]
mod freebsd {
    pub mod address;
    pub mod interface;
    pub mod io;
    pub mod params;
    pub mod sys;
}

#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod netlink;
    pub mod params;
    pub mod request;
}

#[cfg(target_os = "macos")]
mod macos {
    pub mod address;
    pub mod interface;
    pub mod params;
}

#[cfg(target_os = "macos")]
mod header_io;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
mod raw_io;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod address;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod builder;
#[cfg(target_os = "linux")]
mod offload;
mod packet;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
mod queue;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))
)]
pub use self::address::AddressInfo;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))
)]
pub use self::builder::TunBuilder;
#[cfg(target_os = "linux")]
#[cfg_attr(docsrs, doc(cfg(target_os = "linux")))]
pub use self::offload::{
    DEFAULT_OFFLOAD_FEATURES, TUN_F_CSUM, TUN_F_TSO_ECN, TUN_F_TSO4, TUN_F_TSO6, TUN_F_USO4,
    TUN_F_USO6, VIRTIO_NET_HDR_F_DATA_VALID, VIRTIO_NET_HDR_F_NEEDS_CSUM,
    VIRTIO_NET_HDR_F_RSC_INFO, VIRTIO_NET_HDR_GSO_ECN, VIRTIO_NET_HDR_GSO_NONE,
    VIRTIO_NET_HDR_GSO_TCPV4, VIRTIO_NET_HDR_GSO_TCPV6, VIRTIO_NET_HDR_GSO_UDP,
    VIRTIO_NET_HDR_GSO_UDP_L4, VIRTIO_NET_HDR_LEN, VirtioNetHeader,
};
pub use self::packet::Packet;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
pub use self::queue::TunQueue;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))
)]
pub use self::tun::Tun;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_packet_types_are_send_sync() {
        assert_send_sync::<TunQueue>();
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
        assert_send_sync::<Tun>();
    }
}
