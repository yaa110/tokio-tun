# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/workflows/Build/badge.svg)](https://github.com/yaa110/tokio-tun/actions) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](examples)

Asynchronous allocation of TUN/TAP devices in Rust using [`tokio`](https://crates.io/crates/tokio). Use [async-tun](https://crates.io/crates/async-tun) for `async-std` version.

## Getting Started

- Create a tun device using `Tun::builder()` and read from it in a loop:

```rust
use std::sync::Arc;
use tokio_tun::Tun;

#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")            // if name is empty, then it is set by kernel.
            .tap()               // uses TAP instead of TUN (default).
            .packet_info()       // avoids setting IFF_NO_PI.
            .address("10.0.0.1".parse().unwrap(), 24) // can be called multiple times.
            .destination("10.0.0.2".parse().unwrap())
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

### Multiple addresses and IPv6

`TunBuilder::address` accepts `std::net::IpAddr` and can be called multiple
times. Every call carries its own prefix length, so IPv4 and IPv6 addresses are
independent. `destination` configures the point-to-point peer of the most
recently added address. `broadcast` also applies to the most recently added
address and is available for IPv4 only, because IPv6 has no broadcast address.

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
    .destination("fd00::2".parse().unwrap())
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
address: fd00::1, prefix: 64, destination: Some(fd00::2), broadcast: None
address: fe80::3545:93f1:f538:3dd1, prefix: 64, destination: None, broadcast: None
```

The builder-level `netmask` method has been replaced by the prefix length passed
to `address`. `Tun::netmask()` still returns the netmask of the primary IPv4
address.

- Ping to read packets:

```bash
ping 10.0.0.2
```

- Display devices and analyze the network traffic:

```bash
ip tuntap
sudo tshark -i <tun-name>
```

### MAC address (TAP only)

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
