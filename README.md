# Tokio TUN/TAP

[![Build](https://github.com/yaa110/tokio-tun/workflows/Build/badge.svg)](https://github.com/yaa110/tokio-tun/actions) [![crates.io](https://img.shields.io/crates/v/tokio-tun.svg)](https://crates.io/crates/tokio-tun) [![Documentation](https://img.shields.io/badge/docs-tokio--tun-blue.svg)](https://docs.rs/tokio-tun) [![examples](https://img.shields.io/badge/examples-tokio--tun-blue.svg)](examples)

Asynchronous allocation of TUN/TAP devices in Rust using [`tokio`](https://crates.io/crates/tokio). Use [async-tun](https://crates.io/crates/async-tun) for `async-std` version.

## Getting Started

- Create a tun device using `Tun::builder()` and read from it in a loop:

```rust
#[tokio::main]
async fn main() {
    let tun = Arc::new(
        Tun::builder()
            .name("")            // if name is empty, then it is set by kernel.
            .tap(false)          // false (default): TUN, true: TAP.
            .packet_info(false)  // false: IFF_NO_PI, default is true.
            .up()                // or set it up manually using `sudo ip link set <tun-name> up`.
            .try_build()         // or `.try_build_mq(queues)` for multi-queue support.
            .unwrap(),
    );

    println!("tun created, name: {}, fd: {}", tun.name(), tun.as_raw_fd());

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

- Set the address of device (address and netmask could also be set using `TunBuilder`):

```bash
sudo ip a add 10.0.0.1/24 dev <tun-name>
```

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

```bash
sudo -E $(which cargo) run --example read
sudo -E $(which cargo) run --example read-mq
```
