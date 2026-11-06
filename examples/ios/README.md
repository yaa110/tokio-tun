# iOS example

This example shows a sound public-API integration path for `tokio-tun` on
iOS: keep packet capture in Swift with NetworkExtension, and move packets
across an app-created `SOCK_DGRAM` socket pair to Rust.

## iOS restrictions

- iOS apps cannot open `/dev/net/tun` or allocate a `utun` interface directly.
  `Tun` and `TunBuilder` are therefore not available on iOS.
- Packet-tunnel functionality must run in a `NEPacketTunnelProvider` app
  extension using the NetworkExtension framework.
- `NEPacketTunnelFlow` is an object API, not a file descriptor. There is no
  public fd to pass to `AsyncFd`.
- Do not extract a private descriptor through KVO paths such as
  `socket.fileDescriptor`. That is non-public API, risks App Store rejection,
  and is not stable across iOS releases.
- Network Extension processes have tight memory limits. Avoid unbounded
  packet queues and large temporary buffers.

## Flow

1. The Packet Tunnel Provider configures `NEPacketTunnelNetworkSettings`.
2. Swift creates a `SOCK_DGRAM` socket pair.
3. Swift passes one end to Rust through `tokio_tun_start`.
4. Rust wraps it with `TunQueue::try_from`, using raw IPv4/IPv6 packets.
5. Swift forwards `NEPacketTunnelFlow` packets to the other end, and reads
   packets from Rust to inject with `writePackets`.

`SOCK_DGRAM` preserves packet boundaries. Do not substitute a stream socket
for packet traffic. Each `recv` returns one packet; buffers must be at least
the MTU because datagram reads can truncate larger packets.

Darwin AF_UNIX datagram sockets also have a platform datagram-size limit
(commonly 2048 bytes); keep the tunnel MTU at a value that fits, or use a
framed transport. The example keeps the Swift side non-blocking and drops
packets on backpressure or `EMSGSIZE` rather than blocking the extension.

## Files

- [`PacketTunnelProvider.swift`](PacketTunnelProvider.swift): NetworkExtension
  provider and socket-pair bridge.
- [`tun_bridge.rs`](tun_bridge.rs): illustrative Rust C-ABI bridge and
  `TunQueue` packet loop.

## Xcode setup

- Add a Network Extension target with the Packet Tunnel provider type.
- Add the Network Extension entitlement
  (`com.apple.developer.networking.networkextension` with
  `packet-tunnel-provider`) to the extension.
- Add a bridging header declaring:

  ```c
  int32_t tokio_tun_start(int32_t raw_fd);
  void tokio_tun_stop(void);
  ```

- Build the Rust bridge as a `cdylib` and link it into the extension.
- Run the Rust and Swift bridge inside the extension process. If the
  containing app owns the bridge, the fd must be passed across the process
  boundary explicitly (for example with XPC file-descriptor passing).

## Production notes

This example is intentionally minimal. A production tunnel should also handle:

- outbound packet writes (`TunQueue::send`/`sendv`); the Rust stub only reads;
- packet backpressure and bounded queues;
- tunnel cancellation and cleanup (the example aborts the Rust task);
- IPv6 network settings and included/excluded routes;
- DNS and proxy settings;
- the `NEPacketTunnelProvider` lifecycle and reconnection policy.
