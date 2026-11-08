# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
semantic versioning.

## [1.0.0] - 2026-11-08

The first stable release. The public API is now a compatibility commitment for
the 1.x series.

### Added

- Linux `TunBuilder::offload_features(mask)` plus public `TUN_F_CSUM`,
  `TUN_F_TSO4`, `TUN_F_TSO6`, `TUN_F_TSO_ECN`, `TUN_F_USO4`, `TUN_F_USO6`, and
  `DEFAULT_OFFLOAD_FEATURES`. UDP-tunnel GSO and `TUN_F_UFO` are not supported
  in 1.0; their bits are rejected.
- Linux `Tun::offload_features()`, `Tun::set_carrier()`,
  `TunQueue::set_carrier()`, `TunQueue::detach()`, and `TunQueue::attach()`.
- `Packet`, returned by `recv`/`try_recv`, which carries the payload and (on
  Linux offload) the parsed `VirtioNetHeader`. `send`/`try_send` accept a
  `Packet` or a payload slice, so offload framing is automatic and no manual
  `VIRTIO_NET_HDR_LEN` slicing is required.
- The minimum supported Rust version is 1.88 (let-chains are used internally).
- Crate-level documentation covering platform model, lifecycle, packet I/O,
  Tokio requirements, descriptor ownership, and permissions.
- Examples for packet transmission, IPv6, non-blocking I/O, error handling, and
  Linux offload.
- Property-based tests for address broadcast computation and virtio-net header
  encoding/decoding.
- `benches/throughput.rs` for the queue + Tokio `AsyncFd` packet path.

### Changed

- `Tun::recv`/`TunQueue::recv` and the `try_recv` forms now return
  `io::Result<Packet>` instead of a byte count. On Linux offload the packet
  carries the parsed virtio-net header; callers no longer slice with
  `VIRTIO_NET_HDR_LEN`.
- `Tun::send`/`TunQueue::send` and the `try_send` forms accept a `Packet` or a
  `&[u8]` payload. On Linux offload, `AsyncRead`/`AsyncWrite` also strip/add the
  header automatically (without exposing metadata).
- `Tun::sendv`, `Tun::try_sendv`, `TunQueue::sendv`, and
  `TunQueue::try_sendv` now return `io::Result<()>` and are payload-only; on
  Linux offload they prepend a zero virtio-net header.
- `AsyncWrite::poll_write` and `poll_write_vectored` report a short native
  packet write as `io::ErrorKind::WriteZero` rather than returning a partial
  packet length. This prevents `write_all` from splitting a packet. macOS
  `utun` and FreeBSD TUN now use the same `WriteZero` kind as the raw backends.
- `Tun::offload()` now uses `DEFAULT_OFFLOAD_FEATURES`, which includes
  `TUN_F_TSO_ECN` in addition to checksum and TSO4/TSO6.
- `Tun::index()` and the name-based interface accessors verify that the
  interface still matches its cached name; a renamed/reused interface returns
  `InvalidData` rather than acting on the new interface.
- `TunQueue::vnet_hdr_len` now returns `usize` directly instead of
  `io::Result<usize>` and reports the value captured when the queue was
  constructed; changing offload state out-of-band is not observed.
- `TunBuilder::owner` and `TunBuilder::group` now take `u32` (numeric UID/GID)
  instead of `i32`.
- `Tun::flags` now returns `u32`. FreeBSD stores interface flags in two 16-bit
  halves; both are combined instead of returning only the low half.
- Linux `packet_info` remains a raw mode: `Packet::payload` includes the 4-byte
  `tun_pi` prefix, and `send`/`sendv` expect it in the packet. The prefix is not
  parsed or synthesized; use `Tun::packet_info`/`TunQueue::packet_info` to
  detect the mode. Combining `packet_info` with offload remains unsupported.
- Zero-length receive buffers are a no-op on every backend: `recv`, `try_recv`,
  and `AsyncRead::poll_read` return immediately without consuming a packet. A
  zero-byte native read is an EOF signal on every read, not a one-shot event;
  callers must stop reading after observing it.
- `AddressInfo` is `#[non_exhaustive]` so future fields (for example an IPv6
  scope ID) can be added without breaking 1.x.
- Configuration and I/O errors use `std::io::Error`, so new error conditions do
  not require a new public error enum variant.
- Version is now `1.0.0`.

### Removed

- `Tun::send_all` and `Tun::sendv_all`.
- `TunQueue::send_all` and `TunQueue::sendv_all`.

  These methods implied stream semantics. A packet device must not retry the
  unsent remainder of a short write as a new packet.

- The `tokio_tun::Error` and `tokio_tun::Result` aliases. All fallible APIs now
  return `std::io::Result<T>` directly.

### Fixed

- Short packet writes are detected and surfaced instead of being silently
  completed as additional packets, on every platform backend.
- Vectored packet length calculation checks for `usize` overflow.
- Linux netlink address dumps detect truncated datagrams instead of silently
  returning an incomplete list, and sub-microsecond receive deadlines are
  clamped so they cannot disable `SO_RCVTIMEO`.
- macOS short IPv4 sockaddrs are read through header bytes instead of forming a
  full `&sockaddr`, removing a latent out-of-bounds reference.
- FreeBSD error cleanup no longer issues a name-based `SIOCIFDESTROY` after
  closing a transient device, avoiding a name-reuse race.
- `Tun` and `TunQueue` implement `Debug`.
- `Tun::flags` documentation no longer describes MTU flags.

### Migration from 0.23

Return-type changes:

| 0.23 | 1.0 |
|---|---|
| `recv`/`try_recv -> io::Result<usize>` | `io::Result<Packet>` |
| `send`/`sendv`/`try_send`/`try_sendv -> io::Result<usize>` | `io::Result<()>` |
| `Tun::send_all`/`sendv_all` | removed |
| `TunQueue::send_all`/`sendv_all` | removed |
| `AsyncWrite` short write -> partial count | short write -> `WriteZero` error |
| `TunQueue::vnet_hdr_len -> io::Result<usize>` | `TunQueue::vnet_hdr_len -> usize` |
| `TunBuilder::owner`/`group -> i32` | `TunBuilder::owner`/`group -> u32` |
| `Tun::flags -> i16` | `Tun::flags -> u32` |
| `tokio_tun::Error` / `tokio_tun::Result` | `std::io::Error` / `std::io::Result` |

Replace stream-style writes:

```rust
// 0.23
let written = tun.send(&packet).await?;
tun.send_all(&packet).await?;
tun.sendv_all(&mut slices).await?;

// 1.0: one call = one packet; do not retry the remainder
let mut buffer = vec![0u8; 65_535];
let received = tun.recv(&mut buffer).await?;
tun.send(received).await?;
let slices = [received.payload()];
tun.sendv(&slices).await?;
```

`TunBuilder::build()` remains the single allocation entry point and returns one
`Tun` per queue:

```rust
let tun = Tun::builder()
    .build()?
    .pop()
    .expect("one queue");
```

`write_all` cannot split a single packet because a short native write is now an
error, but it can still combine a multi-packet buffer into one write. Use
`send`/`sendv` for packet-oriented code. Configuration and I/O errors are
`std::io::Error` values; use `err.kind()` and `err.raw_os_error()` instead of
matching the removed 0.23 error type.
