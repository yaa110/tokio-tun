# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/workflows/Build/badge.svg)](https://github.com/yaa110/tokio-tun/actions) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](examples)

Asynchronous allocation of TUN/TAP devices in Rust using [`tokio`](https://crates.io/crates/tokio). Use [async-tun](https://crates.io/crates/async-tun) for `async-std` version.

## Getting Started

- Create a tun device using `Tun::builder()` and read from it in a loop:

```rust
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio_tun::Tun;

#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")            // if name is empty, then it is set by kernel.
            .tap()               // uses TAP instead of TUN (default).
            .packet_info()       // avoids setting IFF_NO_PI.
            .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
            .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
            .up()                // or set it up manually using `sudo ip link set <tun-name> up`.
            .close_on_exec()     // or no_close_on_exec()
            .build()
            .unwrap()
            .pop()
            .unwrap(),
    );

    println!(
        "tun created, name: {}, index: {}, fd: {}",
        tun.name(),
        tun.index().unwrap(),
        tun.as_raw_fd()
    );

    let (mut reader, mut _writer) = tokio::io::split(tun);

    // Writer: simply clone Arced Tun.
    let tun_c = tun.clone();
    tokio::spawn(async move{
        let buf = b"data to be written";
        tun_c.send_all(buf).await.unwrap();
    });

    // Reader
    let mut buf = [0u8; 1024];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}
```

- Run the code using `sudo`:

```bash
sudo -E $(which cargo) run
```

- Set the address of device (addresses can also be set using `TunBuilder`):

```bash
sudo ip a add 10.0.0.1/24 dev <tun-name>
```

### Multiple addresses and IPv6

`TunBuilder::address` accepts `std::net::IpAddr` and can be called multiple
times. Every call carries its own prefix length, so IPv4 and IPv6 addresses are
independent. `destination` configures the point-to-point peer of the most
recently added address. `broadcast` also applies to the most recently added
address and is available for IPv4 only, because IPv6 has no broadcast address.

```rust
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio_tun::Tun;

let tun = Tun::builder()
    .name("")
    // Two IPv4 addresses with independent prefixes.
    .address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 24)
    .destination(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))
    .address(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)), 24)
    // Two IPv6 addresses with independent prefixes.
    .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)), 64)
    .destination(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2)))
    .address(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 1, 0, 0, 0, 1)), 64)
    .up()
    .build()
    .unwrap()
    .pop()
    .unwrap();

for address in tun.addresses().unwrap() {
    println!(
        "address: {}, prefix: {}, destination: {:?}, broadcast: {:?}",
        address.address, address.prefix_len, address.destination, address.broadcast
    );
}
```

The builder-level `netmask` method has been replaced by the prefix length passed
to `address`. `Tun::netmask()` still returns the netmask of the primary IPv4
address.

### Migrating from 0.16

- `TunBuilder::address` now takes `IpAddr` plus a prefix length and can be
  called multiple times. The builder-level `netmask` method was removed.
- `Tun::address` now returns `IpAddr`. It prefers the first IPv4 address and
  falls back to the first reported address (which may be IPv6).
- `Tun::destination` now returns `IpAddr` and reports the first peer returned by
  the system, not necessarily the first one configured.
- `Tun::netmask` and `Tun::broadcast` still refer to the primary IPv4 address.
- `Tun::addresses` performs synchronous netlink I/O; avoid calling it from
  latency-sensitive async code.
- Interface names longer than 15 bytes or containing a NUL byte are rejected at
  build time instead of being silently truncated.
- `queues` must be between 1 and 256 inclusive.
- Addresses must not be unspecified, loopback, multicast, or broadcast, and
  prefix length `0` is rejected.
- `Tun::build` must be called from within a Tokio runtime with I/O enabled.
- The minimum supported Rust version is 1.85.

- Ping to read packets:

```bash
ping 10.0.0.2
```

- Display devices and analyze the network traffic:

```bash
ip tuntap
sudo tshark -i <tun-name>
```

## Supported Platforms

- [x] Linux
- [ ] FreeBSD
- [ ] Android
- [ ] OSX
- [ ] iOS
- [ ] Windows

## Examples

- [`read`](examples/read.rs): Split tun to (reader, writer) pair and read packets from reader.
- [`read-mq`](examples/read-mq.rs): Read from multi-queue tun using `tokio::select!`.
- [`multiple-addresses`](examples/multiple-addresses.rs): Assign multiple IPv4 and IPv6 addresses to one tun.

```bash
sudo -E $(which cargo) run --example read
sudo -E $(which cargo) run --example read-mq
sudo -E $(which cargo) run --example multiple-addresses
```
