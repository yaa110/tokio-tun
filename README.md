# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/workflows/Build/badge.svg)](https://github.com/yaa110/tokio-tun/actions) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](examples)

Asynchronous allocation of TUN/TAP devices in Rust using [`tokio`](https://crates.io/crates/tokio).

## Getting Started

- Create a tun device using `Tun::builder()` and read from it in a loop:

```rust
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use tokio_tun::Tun;

#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")            // if name is empty, then it is set by kernel.
            .address("10.0.0.1".parse().unwrap(), 24) // can be called multiple times.
            .destination("10.0.0.2".parse().unwrap())
            .up()                // or bring the interface up manually.
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

    // Writer: simply clone Arced Tun.
    let tun_c = tun.clone();
    tokio::spawn(async move {
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

### Multiple addresses and IPv6

`TunBuilder::address` accepts `std::net::IpAddr` and can be called multiple
times. Every call carries its own prefix length, so IPv4 and IPv6 addresses are
independent. `destination` configures the point-to-point peer of the most
recently added address. On Linux, `broadcast` also applies to the most recently
added address and is available for IPv4 only, because IPv6 has no broadcast
address. On macOS, IPv6 destinations require a `/128` prefix.

```rust
use tokio_tun::Tun;

let tun = Tun::builder()
    .name("")
    // Two IPv4 addresses with independent prefixes.
    .address("10.0.0.1".parse().unwrap(), 24)
    .destination("10.0.0.2".parse().unwrap())
    .address("10.0.1.1".parse().unwrap(), 24)
    // Two IPv6 addresses with independent prefixes.
    .address("fd00::1".parse().unwrap(), 64)
    .address("fd00:0:0:1::1".parse().unwrap(), 64)
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

Example output (the kernel also adds a link-local IPv6 address, and ordering may vary):

```text
address: 10.0.0.1, prefix: 24, destination: Some(10.0.0.2), broadcast: None
address: 10.0.1.1, prefix: 24, destination: None, broadcast: None
address: fd00:0:0:1::1, prefix: 64, destination: None, broadcast: None
address: fd00::1, prefix: 64, destination: None, broadcast: None
address: fe80::3545:93f1:f538:3dd1, prefix: 64, destination: None, broadcast: None
```

The builder-level `netmask` method has been replaced by the prefix length passed
to `address`. `Tun::netmask()` still returns the netmask of the primary IPv4
address.

- Ping to read packets:

```bash
ping 10.0.0.2
```

- Display devices and analyze the network traffic (Linux):

```bash
ip tuntap
sudo tshark -i <tun-name>
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
non-blocking there. `FD_CLOEXEC` is left unchanged. On Linux `TunQueue` uses raw
packet I/O, while on macOS it handles the `utun` header.

### Multi-queue with one queue (Linux only)

`queues(1)` opens the device with `IFF_MULTI_QUEUE`, so additional queues can
be attached later using the same interface name. Do not repeat address
configuration when attaching an additional queue.

## Supported Platforms

- [x] Linux
- [x] macOS
- [ ] FreeBSD
- [ ] Android
- [ ] iOS
- [ ] Windows

### macOS limitations

macOS uses the built-in `utun` driver. Only layer-3 TUN is available:

- Interface names must be empty, `utun`, or `utunN`.
- IPv4/IPv6 addresses, MTU, flags, and async I/O are supported.
- IPv4 destinations are supported. IPv6 destinations require a `/128` prefix.
- macOS does not report IPv6 point-to-point destinations through `getifaddrs`, so `Tun::destination()` may return `AddrNotAvailable` for IPv6-only setups.
- TAP, `packet_info`, multi-queue, persist, owner/group, MAC address, and broadcast APIs are Linux-only and are not available on macOS.
- Packets larger than the receive buffer return an error on macOS instead of being truncated.
- `utun` creation requires root privileges.

## Examples

- [`read`](examples/read.rs): Create a TUN device and read packets from it.
- [`read-mq`](examples/read-mq.rs): Read from multi-queue tun using `tokio::select!` (Linux only).
- [`multiple-addresses`](examples/multiple-addresses.rs): Assign multiple IPv4 and IPv6 addresses to one tun.

```bash
sudo -E $(which cargo) run --example read
sudo -E $(which cargo) run --example multiple-addresses
# Linux only:
sudo -E $(which cargo) run --example read-mq
```
