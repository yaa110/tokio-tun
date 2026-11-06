# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/yaa110/tokio-tun/actions/workflows/build.yml) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](examples)

Asynchronous allocation of TUN/TAP devices in Rust using [`Tokio`](https://crates.io/crates/tokio).

`tokio-tun` is natively asynchronous. On every supported platform, packet I/O is registered with Tokio through [`AsyncFd`](https://docs.rs/tokio/latest/tokio/io/unix/struct.AsyncFd.html); there is no custom polling loop, blocking fallback, or private descriptor extraction.

## Getting Started

- Create a tun device using `Tun::builder()` and read from it in a loop:

```rust
use std::os::fd::AsRawFd;
use tokio_tun::Tun;

#[tokio::main]
async fn main() {
    let tun = Tun::builder()
        .name("") // if name is empty, then it is set by kernel.
        .address("10.0.0.1".parse().unwrap(), 24) // can be called multiple times.
        .destination("10.0.0.2".parse().unwrap())
        .up() // or bring the interface up manually.
        .close_on_exec() // or no_close_on_exec()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    println!(
        "tun created, name: {}, index: {}, fd: {}",
        tun.name(),
        tun.index().unwrap(),
        tun.as_raw_fd()
    );

    // A TUN read returns one IP packet. Use a buffer at least as large as the
    // interface MTU; 65,535 bytes covers the largest standard IP packet.
    let mut buf = vec![0u8; 65_535];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}
```

### Multiple addresses and IPv6

`TunBuilder::address` accepts `std::net::IpAddr` and can be called multiple
times. Every call carries its own prefix length, so IPv4 and IPv6 addresses are
independent. `destination` configures the point-to-point peer of the most
recently added address. On Linux, `broadcast` also applies to the most recently
added address and is available for IPv4 only, because IPv6 has no broadcast
address. On macOS and FreeBSD, TUN IPv6 destinations require a `/128` prefix.
FreeBSD TAP devices do not support destinations.

Packets written back to a TUN device must be valid IPv4/IPv6 packets. Use
`Tun::send`, `Tun::sendv`, or `Tun::send_all` with an IP packet payload.

- Run one of the included examples using `sudo`:

```bash
sudo -E $(which cargo) run --example read
```

### MAC address (Linux TAP only)

TAP devices have a MAC address; TUN devices do not. Set it when building the
device, or read and change it at runtime:

```rust
use tokio_tun::Tun;

let tap = Tun::builder()
    .tap()
    .mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
    .build()
    .unwrap()
    .pop()
    .unwrap();

let mac = tap.mac_address().unwrap();
println!("MAC address: {mac:?}");
tap.set_mac_address([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee])
    .unwrap();
```

### Passing a device queue to another process

`TunQueue` owns the packet queue, but not the interface accessors. Converting a
`Tun` into a `TunQueue` drops those accessors, so capture anything you need
(such as the interface name) first. The queue's descriptor can then be passed
to another process as an `OwnedFd`:

```rust
use std::os::fd::OwnedFd;
use tokio_tun::{Tun, TunQueue};

fn queue_fd(tun: Tun) -> OwnedFd {
    let queue: TunQueue = tun.into();
    queue.into()
}

// In the receiving process, after taking ownership of the descriptor. Call
// this inside a Tokio runtime with I/O enabled.
fn use_queue(fd: OwnedFd) -> std::io::Result<TunQueue> {
    TunQueue::new(fd)
}
```

`TunQueue::new` accepts any type that converts into an `OwnedFd`; it sets the
descriptor to non-blocking mode. Since `O_NONBLOCK` belongs to the shared open
file description, a descriptor received from another process also becomes
non-blocking there. `FD_CLOEXEC` is left unchanged. On Linux, `TunQueue` uses
the descriptor as-is, including packet information when enabled. Android and
iOS bridges must provide raw IP packets. On macOS, `TunQueue` handles the
`utun` header. On FreeBSD, it detects `TUNSIFHEAD` and handles TUN family
headers automatically; TAP descriptors carry raw Ethernet frames.

### Multi-queue with one queue (Linux only)

`queues(1)` opens the device with `IFF_MULTI_QUEUE`, so additional queues can
be attached later using the same interface name. Do not repeat address
configuration when attaching an additional queue. If the device was created
with `.offload()`, every attached queue must also call `.offload()`; an offload
state mismatch is rejected.

### Linux offload (TSO/GSO/GRO)

On Linux, `TunBuilder::offload()` enables the kernel TUN offloads and the
virtio-net header:

```rust
use tokio_tun::{Tun, VirtioNetHeader, VIRTIO_NET_HDR_LEN};

#[tokio::main]
async fn main() {
    let tun = Tun::builder()
        .offload()
        .address("10.0.0.1".parse().unwrap(), 24)
        .destination("10.0.0.2".parse().unwrap())
        .up()
        .build()
        .unwrap()
        .pop()
        .unwrap();

    let mut packet = vec![0u8; VIRTIO_NET_HDR_LEN + 65_536];
    let n = tun.recv(&mut packet).await.unwrap();
    let header = VirtioNetHeader::decode(&packet[..VIRTIO_NET_HDR_LEN]).unwrap();
    let ip_packet = &packet[VIRTIO_NET_HDR_LEN..n];
}
```

Every received packet starts with a ten-byte `virtio_net_hdr`; every packet
sent to the device must include it. `offload()` configures checksum, TSO4, TSO6,
and NAPI-based GRO, and is mutually exclusive with `packet_info()`. The header
carries GSO/checksum metadata in both directions; NAPI GRO applies to packets
written from userspace into the kernel and does not coalesce packets delivered
to userspace. Applications remain responsible for segmentation/coalescing
semantics. With offload enabled, use a receive buffer of at least
`VIRTIO_NET_HDR_LEN + 65_536` bytes because GSO packets can exceed the
interface MTU.

## Supported Platforms

| Platform | TUN | TAP | Multi-Queue | Offload | Notes |
|---|:---:|:---:|:---:|:---:|---|
| Linux | ✅ | ✅ | ✅ | ✅ | Full API; virtio-net TSO/GSO, NAPI GRO on writes |
| macOS | ✅ | ❌ | ❌ | ❌ | `utun`; TUN only |
| Android | ✅ | ❌ | ❌ | ❌ | `TunQueue` only; `VpnService` fd |
| iOS | ✅ | ❌ | ❌ | ❌ | `TunQueue` only; NetworkExtension bridge |
| FreeBSD | ✅ | ✅ | ❌ | ❌ | `/dev/tun` and `/dev/tap`; TUN uses BSD address-family headers internally |

Mobile support is I/O-only: it wraps an externally supplied packet descriptor
and does not allocate devices or integrate with `VpnService` /
`NEPacketTunnelFlow` directly.

### macOS limitations

macOS uses the built-in `utun` driver. Only layer-3 TUN is available:

- Interface names must be empty, `utun`, or `utunN`.
- IPv4/IPv6 addresses, MTU, flags, and async I/O are supported.
- IPv4 destinations are supported. IPv6 destinations require a `/128` prefix.
- macOS does not report IPv6 point-to-point destinations through `getifaddrs`, so `Tun::destination()` may return `AddrNotAvailable` for IPv6-only setups.
- TAP is available on Linux and FreeBSD, but not macOS. `packet_info`, multi-queue, persist, owner/group, MAC address, and broadcast APIs are Linux-only.
- Packets larger than the receive buffer return an error on macOS instead of being truncated.
- `utun` creation requires root privileges.

### FreeBSD

FreeBSD supports TUN and TAP devices through the standard `/dev/tunN` and
`/dev/tapN` devices. Use an empty name, `tunN` for TUN, or `tapN` together with `.tap()` for TAP.

- TUN and TAP are supported.
- Linux multi-queue is not available.
- TUN packets use FreeBSD's `TUNSIFHEAD` address-family header internally;
  `Tun` and `TunQueue::new` handle it transparently.
- TUN IPv4 addresses without an explicit destination use the local address as
  the peer. TUN IPv6 destinations require `/128`; TAP destinations are rejected.
- Devices created by the crate are transient and are destroyed when the last descriptor is closed. Attaching to an existing named `tunN`/`tapN` device does not change its lifetime.
- FreeBSD TUN receive reports a truncated packet when the buffer is too small; raw TAP descriptors may silently truncate.
- Creating and configuring devices requires root privileges.
- `packet_info`, `persist`, owner/group, MAC address, and Linux broadcast APIs
  are not available on FreeBSD.

### Android

`Tun` and `TunBuilder` are not available on Android. Android apps cannot open
`/dev/net/tun`; use `TunQueue` with the file descriptor provided by
`VpnService`:

```rust
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use tokio_tun::TunQueue;

// `VpnService.establish()` returns a ParcelFileDescriptor. Detach it on the
// Java/Kotlin side, then transfer ownership of the raw fd to Rust. Call this
// inside a Tokio runtime with I/O enabled.
//
// # Safety
//
// `raw_fd` must be open, valid, and owned by the caller. Do not close it after
// transferring ownership to this function.
unsafe fn wrap(raw_fd: RawFd) -> std::io::Result<TunQueue> {
    // SAFETY: the caller transfers ownership of `raw_fd`.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    TunQueue::try_from(fd)
}
```

- Call `ParcelFileDescriptor.detachFd()` (not `getFd()`) and transfer that raw
  descriptor to `TunQueue`. `TunQueue` owns it and closes it on drop; do not
  close it manually while `TunQueue` owns it.
- Configure addresses, routes, MTU, DNS, and allowed applications with
  `VpnService.Builder`. Protect the `VpnService` declaration with the
  `BIND_VPN_SERVICE` permission and obtain user consent through
  `VpnService.prepare()`.
- The descriptor must carry raw IP packets. Each `recv` returns one packet;
  size receive buffers to at least the MTU because raw reads can truncate
  larger packets.
- Use `VpnService.protect()` for sockets that must bypass the tunnel.
- `TunQueue` leaves `FD_CLOEXEC` unchanged. Set it if the process may `exec` a
  child and should not leak the VPN descriptor.
- No root access is required.

### iOS

`Tun` and `TunBuilder` are not available on iOS. iOS does not expose a public
TUN/TAP file descriptor:

- Apps cannot open `/dev/net/tun` or allocate a `utun` device directly. This
  crate does not attempt device allocation on iOS.
- A packet tunnel must run in a Network Extension `NEPacketTunnelProvider` app
  extension. `NEPacketTunnelFlow` is an object API for reading and writing
  packets; it is not a file descriptor.
- Do not extract a descriptor through private KVO paths such as
  `socket.fileDescriptor`. That is non-public API, risks App Store rejection,
  and can break across iOS releases.
- Network Extension processes have tight memory limits. Avoid unbounded packet
  queues and large temporary buffers.

`TunQueue` therefore works only with a raw packet descriptor supplied by an
app-level bridge:

- `NEPacketTunnelFlow` is available only inside the `NEPacketTunnelProvider`
  extension process. Run the bridge and the Rust code using `TunQueue` in that
  extension; if the containing app owns them, the fd must be passed across the
  process boundary explicitly.
- Bridge `NEPacketTunnelFlow` packets to a datagram socket pair and pass the
  Rust end to `TunQueue`.
- Preserve packet boundaries; use `SOCK_DGRAM` or an equivalent packet-oriented
  transport. Each `recv` returns one packet; size receive buffers to at least
  the MTU because datagram reads can truncate larger packets.
- Configure addresses, routes, DNS, and MTU with
  `NEPacketTunnelNetworkSettings`.
- The descriptor must carry raw IP packets, without a `utun` header.
- `TunQueue` leaves `FD_CLOEXEC` unchanged; set it if your extension may `exec`
  a child process.
- Requires the Network Extension entitlement
  (`com.apple.developer.networking.networkextension` with
  `packet-tunnel-provider`) and user approval. No root access is required.

## Examples

- [`read`](examples/read.rs): Create a TUN device and read packets from it.
- [`read-mq`](examples/read-mq.rs): Read from multi-queue tun using `tokio::select!` (Linux only).
- [`multiple-addresses`](examples/multiple-addresses.rs): Assign multiple IPv4 and IPv6 addresses to one tun.
- [`android`](examples/android/README.md): Android `VpnService` + Kotlin/JNI + Rust `TunQueue` bridge.
- [`ios`](examples/ios/README.md): iOS `NEPacketTunnelProvider` + Swift socket-pair + Rust `TunQueue` bridge.

```bash
sudo -E $(which cargo) run --example read
sudo -E $(which cargo) run --example multiple-addresses
# Linux only:
sudo -E $(which cargo) run --example read-mq
```
