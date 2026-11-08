# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/yaa110/tokio-tun/actions/workflows/build.yml) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](https://github.com/yaa110/tokio-tun/blob/main/examples)

Asynchronous allocation and packet I/O for native Unix TUN/TAP interfaces,
using [`Tokio`](https://crates.io/crates/tokio).

`tokio-tun` is natively asynchronous on every supported platform. Packet I/O is
registered with Tokio through
[`AsyncFd`](https://docs.rs/tokio/latest/tokio/io/unix/struct.AsyncFd.html);
there is no custom polling thread or blocking fallback.

The crate is **Unix-only**: Linux, macOS, FreeBSD, Android, and iOS. Windows is
not supported and is not part of the API.

## Install

```toml
[dependencies]
tokio-tun = "1"
tokio = { version = "1", features = ["full"] }
```

`tokio-tun` requires Rust 1.88 or newer (edition 2024).

## Getting started

```rust
use tokio_tun::Tun;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tun = Tun::builder()
        .name("") // the kernel chooses a name
        .address("10.0.0.1".parse().unwrap(), 24)
        .destination("10.0.0.2".parse().unwrap())
        .up()
        .build_one()?;

    println!("created {} (fd {})", tun.name(), {
        use std::os::fd::AsRawFd;
        tun.as_raw_fd()
    });

    // One read returns one IP packet. Size the buffer for the MTU; 65,535
    // bytes covers the largest standard IP packet.
    let mut packet = vec![0u8; 65_535];
    loop {
        let received = tun.recv(&mut packet).await?;
        println!(
            "received {} bytes: {:?}",
            received.len(),
            received.payload()
        );
    }
}
```

Creating or configuring a TUN/TAP interface normally requires root or
`CAP_NET_ADMIN` on Linux. Run the examples with `sudo -E $(which cargo) run
--example read`.

## Packet semantics

The explicit packet methods preserve packet boundaries:

- `Tun::recv` / `TunQueue::recv` return one [`Packet`](https://docs.rs/tokio-tun/latest/tokio_tun/struct.Packet.html):
  the payload plus optional Linux offload metadata.
- `Tun::send` / `TunQueue::send` accept a `Packet` or a plain `&[u8]` payload.
- `Tun::sendv` / `TunQueue::sendv` write one payload-only packet assembled from
  multiple slices with a single native vectored write.
- `try_recv`, `try_send`, and `try_sendv` are the non-blocking forms. They
  return `io::ErrorKind::WouldBlock` when the device has no packet or no space.

Linux offload is handled automatically by `recv` and `send`: `recv` parses the
virtio-net header and returns it via `Packet::offload_header()`; `send` writes
the header carried by a `Packet`, or a zero header for payload-only input.
`sendv` is payload-only and uses a zero header. This avoids manual
`VIRTIO_NET_HDR_LEN` slicing while preserving GSO/checksum metadata on
`recv`/`send`.

`send` returns `io::Result<()>`, not a byte count. A native short write is
reported as `io::ErrorKind::WriteZero`; the unsent remainder is **never**
retried as a second packet. This is the core reason the pre-1.0 `send_all` and
`sendv_all` helpers were removed.

`AsyncRead` and `AsyncWrite` are implemented for compatibility with Tokio
utilities. `poll_read`/`poll_write` still operate on one packet per call.
Stream helpers such as `read_exact` and `copy` can combine multiple packets;
`write_all` can combine a multi-packet buffer into one write. A short native
write is an error, so `write_all` cannot split a single packet by retrying the
remainder. On Linux offload, `AsyncRead`/`AsyncWrite` also strip/add the virtio
header automatically but do not expose offload metadata; use `recv`/`send` for
the full `Packet` value. Use `recv`/`send` in packet-oriented code.

A receive buffer that is too small may silently truncate on Linux without
offload, Android, iOS, and FreeBSD TAP devices; macOS `utun`, FreeBSD TUN, and
Linux with offload report truncation as an error. Size receive buffers to at
least the interface MTU (TAP Ethernet frames need `MTU + 14`). For GSO/offload
or very large frames, allow up to 65,536 bytes of payload; TAP frames need an
additional 14 bytes (65,550), and with Linux offload also add
`tun.vnet_hdr_len()`. The Linux offload packet API (`recv`/`try_recv`) detects
truncation and returns an error when a frame does not fit. `recv` then returns
the payload and offload header as separate fields.

## Multiple addresses and IPv6

`TunBuilder::address` accepts `std::net::IpAddr` and can be called multiple
times. Every address has its own prefix length. `destination` configures the
point-to-point peer of the most recently added address. On Linux,
`broadcast` also applies to the most recently added IPv4 address. FreeBSD
supports broadcast on TAP only; `Tun::broadcast()` reports the primary/first
IPv4 address's broadcast, while `Tun::addresses()` reports each address's value.
On macOS and FreeBSD, TUN IPv6 destinations require a `/128` prefix. FreeBSD TAP
devices do not support destinations.

## Device and queue lifecycle

- `TunBuilder::build()` returns one `Tun` per queue. For a single-queue device,
  the vector contains exactly one element.
- On Linux, `TunBuilder::queues(n)` opens the interface in multi-queue mode and
  returns `n` queues. Use `.queues(1)` when you want to attach more queues later.
- A `Tun` owns both the interface accessors (name, index, flags, addresses, MTU)
  and one queue descriptor.
- Converting a `Tun` into a `TunQueue` drops the interface accessors and
  transfers the queue descriptor. Capture anything you need first.
- Closing the last queue descriptor removes a non-persistent interface.

## Passing a queue to another process

`TunQueue` owns a packet queue but not the interface accessors. Its descriptor
can be transferred as an `OwnedFd`:

```rust
use std::os::fd::OwnedFd;
use tokio_tun::{Tun, TunQueue};

fn queue_fd(tun: Tun) -> OwnedFd {
    let queue: TunQueue = tun.into();
    queue.into()
}

// In the receiving process, inside a Tokio runtime with I/O enabled:
fn use_queue(fd: OwnedFd) -> std::io::Result<TunQueue> {
    TunQueue::new(fd)
}
```

`TunQueue::new` takes ownership, sets `O_NONBLOCK`, and leaves `FD_CLOEXEC`
unchanged. `O_NONBLOCK` belongs to the shared open file description, so a
descriptor received from another process also becomes non-blocking there.
`TunQueue` closes the descriptor exactly once on drop, including error paths.

## Linux multi-queue

`queues(1)` opens the device with `IFF_MULTI_QUEUE`, so additional queues can be
attached later using the same interface name. Do not repeat address
configuration when attaching an additional queue. When attaching another queue
to an existing multi-queue device, use the same
`.offload()`/`.offload_features(...)` setting. The kernel exposes whether the
virtio header is enabled, so an on/off mismatch is rejected; it does not expose
the current feature mask, so the caller must keep the mask consistent across
attaches (`TUNSETOFFLOAD` is device-wide). See
[`add-queue`](https://github.com/yaa110/tokio-tun/blob/main/examples/add-queue.rs).

## Linux offload (TSO/GSO/GRO)

On Linux, `TunBuilder::offload()` enables the virtio-net header and the default
offload set (checksum, IPv4 TSO, IPv6 TSO, and TSO ECN). Use
`TunBuilder::offload_features(mask)` to select a different mask, for example to
opt into USO on kernels that support it. Any non-zero mask must include
`TUN_F_CSUM`; USO requires both `TUN_F_USO4` and `TUN_F_USO6`. UDP-tunnel GSO
and the deprecated `TUN_F_UFO` are not supported in 1.0.

```rust
use tokio_tun::Tun;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let tun = Tun::builder()
        .offload()
        .address("10.0.0.1".parse().unwrap(), 24)
        .destination("10.0.0.2".parse().unwrap())
        .up()
        .build_one()?;

    let mut packet = vec![0u8; tun.vnet_hdr_len() + 65_536];
    let received = tun.recv(&mut packet).await?;
    println!(
        "received {} byte payload: {:?}",
        received.len(),
        received.offload_header()
    );
    tun.send(received).await?;
    Ok(())
}
```

`recv`/`send` handle the ten-byte `virtio_net_hdr` automatically: `recv`
returns a `Packet` whose payload starts at the first IP/Ethernet byte and whose
`offload_header()` exposes the metadata; `send` writes a `Packet` with its
header (or a zero header for payload-only input). `sendv` is payload-only and
also uses a zero header. The header carries GSO/checksum metadata in both
directions. NAPI GRO applies to packets written from userspace into the kernel
and does not coalesce packets delivered to userspace. Applications remain
responsible for segmentation/coalescing semantics. Offload is mutually
exclusive with `packet_info`. `packet_info()` is a raw mode: the kernel's
4-byte `tun_pi` prefix is part of `Packet::payload` and must be supplied to
`send`/`sendv`; the crate does not parse or synthesize it.

The `TUN_F_*` constants and `VirtioNetHeader` are re-exported on Linux only.
Features unsupported by the running kernel are reported by `TUNSETOFFLOAD`
(normally `EINVAL`).

## Supported platforms

| Platform | Device allocation | Packet API | TUN | TAP | Multi-queue | Offload | External fd |
|---|---|:---:|:---:|:---:|:---:|:---:|:---:|
| Linux | `Tun` / `TunBuilder` | `Tun`, `TunQueue` | ✅ | ✅ | ✅ | ✅ | ✅ |
| macOS | `Tun` / `TunBuilder` | `Tun`, `TunQueue` | ✅ | ❌ | ❌ | ❌ | ✅ (`utun` fd) |
| FreeBSD | `Tun` / `TunBuilder` | `Tun`, `TunQueue` | ✅ | ✅ | ❌ | ❌ | ✅ |
| Android | application / `VpnService` | `TunQueue` | external | ❌ | ❌ | ❌ | ✅ |
| iOS | application / Network Extension | `TunQueue` | external | ❌ | ❌ | ❌ | ✅ |

Android and iOS are I/O-only: the application owns the VPN integration and
transfers an owning descriptor to `TunQueue`. The crate does not create or
configure Android VPN services or iOS packet tunnel providers.

docs.rs builds Linux, macOS, FreeBSD, Android, and iOS documentation. Use the
**Platform** selector in the docs.rs top bar to switch between the
platform-specific API surfaces. Building locally with `cargo doc --target
<target>` produces the same per-platform view.


### macOS

macOS uses the built-in `utun` driver. Only layer-3 TUN is available.

- Interface names must be empty, `utun`, or `utunN`.
- IPv4/IPv6 addresses, MTU, flags, and async I/O are supported.
- IPv4 destinations are supported. IPv6 destinations require a `/128` prefix.
- macOS does not report IPv6 point-to-point destinations through `getifaddrs`,
  so `Tun::destination()` may return `AddrNotAvailable` for IPv6-only setups.
- TAP, packet information, multi-queue, owner/group, and offload are not
  available.
- Packets larger than the receive buffer return an error instead of being
  truncated.
- `utun` creation requires root.

### FreeBSD

FreeBSD supports TUN and TAP through `/dev/tunN` and `/dev/tapN`.

- Use an empty name, `tunN` for TUN, or `tapN` together with `.tap()` for TAP.
- Crate-created TUN devices use FreeBSD's `TUNSIFHEAD` address-family header;
  `Tun` and `TunQueue::new` detect and handle it automatically. An externally
  supplied TUN descriptor without `TUNSIFHEAD` is treated as raw IPv4 without a
  family header and may silently truncate on receive.
- TUN IPv4 addresses without an explicit destination use the local address as
  the peer. TUN IPv6 destinations require `/128`; TAP destinations are
  rejected.
- Devices created by the crate are transient by default and are destroyed when
  the last descriptor is closed. Use `persist()` to keep the interface identity
  after close. FreeBSD clears addresses and the up state when the last
  descriptor closes, so reapply configuration on reattach.
- TAP MAC addresses are supported through `mac_address()` and
  `set_mac_address()`. Broadcast is supported for TAP only and is stored per
  IPv4 address.
- FreeBSD TUN receive reports truncation; raw TAP descriptors may silently
  truncate.
- Externally supplied descriptors must match their device type. `TUNSIFHEAD` is
  detected and `TAPSVNETHDR` is rejected, but a TUN renamed to a `tapN` devfs
  alias, or a TUN in `TUNSLMODE`, is not auto-detected.
- `packet_info`, owner/group, multi-queue, and Linux offload are not available.

### Android

`Tun` and `TunBuilder` are not available on Android. Use `TunQueue` with the
file descriptor provided by `VpnService`:

```rust
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use tokio_tun::TunQueue;

// `VpnService.Builder.establish()` returns a ParcelFileDescriptor. Detach it on the
// Java/Kotlin side, then transfer ownership of the raw fd to Rust. Call this
// inside a Tokio runtime with I/O enabled.
//
// # Safety
//
// `raw_fd` must be open, valid, and owned by the caller. Do not close it after
// transferring ownership.
unsafe fn wrap(raw_fd: RawFd) -> std::io::Result<TunQueue> {
    // SAFETY: the caller transfers ownership of `raw_fd`.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    TunQueue::try_from(fd)
}
```

- Call `ParcelFileDescriptor.detachFd()` (not `getFd()`) and transfer the raw
  descriptor to `TunQueue`. `TunQueue` owns it and closes it on drop.
- Configure addresses, routes, MTU, DNS, and allowed applications with
  `VpnService.Builder`. Protect the service with `BIND_VPN_SERVICE` and obtain
  consent through `VpnService.prepare()`.
- The descriptor must carry raw IP packets. Each `recv` returns one packet; size
  receive buffers to at least the MTU because raw reads can truncate larger
  packets.
- Use `VpnService.protect()` for sockets that must bypass the tunnel.
- `TunQueue` leaves `FD_CLOEXEC` unchanged. Set it if the process may `exec` a
  child and should not leak the VPN descriptor.
- No root access is required.

### iOS

`Tun` and `TunBuilder` are not available on iOS. iOS does not expose a public
TUN/TAP file descriptor.

- Apps cannot open `/dev/net/tun` or allocate a `utun` device directly. This
  crate does not attempt device allocation on iOS.
- A packet tunnel must run in a Network Extension `NEPacketTunnelProvider` app
  extension. `NEPacketTunnelFlow` is an object API, not a file descriptor.
- Do not extract a descriptor through private KVO paths such as
  `socket.fileDescriptor`. That is non-public API and risks App Store rejection.
- Network Extension processes have tight memory limits. Avoid unbounded packet
  queues and large temporary buffers.

`TunQueue` works with a raw packet descriptor supplied by an app-level bridge:

- Run the bridge and the Rust code using `TunQueue` in the extension process.
  If the containing app owns them, pass the fd across the process boundary
  explicitly.
- Bridge `NEPacketTunnelFlow` packets to a datagram socket pair and pass the
  Rust end to `TunQueue`.
- Preserve packet boundaries; use `SOCK_DGRAM` or an equivalent packet-oriented
  transport. Each `recv` returns one packet; size receive buffers to at least
  the MTU because datagram reads can truncate larger packets.
- Configure addresses, routes, DNS, and MTU with
  `NEPacketTunnelNetworkSettings`.
- The descriptor must carry raw IP packets, without a `utun` header.
- `TunQueue` leaves `FD_CLOEXEC` unchanged.
- Requires the Network Extension entitlement
  (`com.apple.developer.networking.networkextension` with
  `packet-tunnel-provider`) and user approval. No root access is required.

## Examples

- [`read`](https://github.com/yaa110/tokio-tun/blob/main/examples/read.rs): create a TUN device and read packets.
- [`send`](https://github.com/yaa110/tokio-tun/blob/main/examples/send.rs): send and vectored-send IPv4 packets.
- [`ipv6`](https://github.com/yaa110/tokio-tun/blob/main/examples/ipv6.rs): create an IPv6 TUN interface.
- [`multiple-addresses`](https://github.com/yaa110/tokio-tun/blob/main/examples/multiple-addresses.rs): assign multiple IPv4
  and IPv6 addresses to one TUN.
- [`nonblocking`](https://github.com/yaa110/tokio-tun/blob/main/examples/nonblocking.rs): `try_recv`/`WouldBlock` handling.
- [`error-handling`](https://github.com/yaa110/tokio-tun/blob/main/examples/error-handling.rs): classify errors with
  `io::ErrorKind` and raw errno.
- [`read-mq`](https://github.com/yaa110/tokio-tun/blob/main/examples/read-mq.rs): read from a Linux multi-queue device.
- [`add-queue`](https://github.com/yaa110/tokio-tun/blob/main/examples/add-queue.rs): attach a second queue to an existing
  Linux multi-queue device.
- [`offload`](https://github.com/yaa110/tokio-tun/blob/main/examples/offload.rs): Linux virtio-net offload framing.
- [`android`](https://github.com/yaa110/tokio-tun/blob/main/examples/android/README.md): Android `VpnService` + Kotlin/JNI +
  Rust `TunQueue` bridge.
- [`ios`](https://github.com/yaa110/tokio-tun/blob/main/examples/ios/README.md): iOS `NEPacketTunnelProvider` + Swift socket
  pair + Rust `TunQueue` bridge.

```bash
sudo -E $(which cargo) run --example read
sudo -E $(which cargo) run --example send
sudo -E $(which cargo) run --example ipv6
sudo -E $(which cargo) run --example multiple-addresses
sudo -E $(which cargo) run --example nonblocking
cargo run --example error-handling
# Linux only:
sudo -E $(which cargo) run --example read-mq
sudo -E $(which cargo) run --example add-queue
sudo -E $(which cargo) run --example offload
```

## Performance

`benches/throughput.rs` measures packet round trips through the `TunQueue` +
Tokio `AsyncFd` path over an `AF_UNIX`/`SOCK_DGRAM` socket pair, so it runs
without root. Native TUN/TAP throughput additionally depends on the traffic
source, queue count, offload settings, and kernel version. See
[`benches/throughput.rs`](https://github.com/yaa110/tokio-tun/blob/main/benches/throughput.rs)
for the benchmark methodology.

```bash
cargo bench --bench throughput
```

## Documentation

- [docs.rs](https://docs.rs/tokio-tun): API documentation.
- [`CHANGELOG.md`](https://github.com/yaa110/tokio-tun/blob/main/CHANGELOG.md): release history and migration notes.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
