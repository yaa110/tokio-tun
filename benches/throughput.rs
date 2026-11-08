//! Throughput benchmark for the packet queue API.
//!
//! This benchmark uses an `AF_UNIX`/`SOCK_DGRAM` socket pair so it can run
//! without root. It measures the crate's `TunQueue` + Tokio `AsyncFd` **hot
//! path** packet round trip, not the throughput of the kernel TUN/TAP driver.
//!
//! In this mode the peer always has a datagram queued before `recv`, so
//! `AsyncFd` readiness stays sticky after the first packets. These numbers
//! therefore do **not** include a cold reactor wakeup per packet. Use a
//! producer/consumer task benchmark with an empty queue when measuring that
//! path.
//!
//! Run with:
//!
//! ```bash
//! cargo bench --bench throughput
//! ```
//!
//! Record the host, OS, architecture, Rust version, and CPU when publishing
//! numbers. Native TUN/TAP throughput also depends on the traffic source,
//! queue count, offload settings, and kernel version.

use std::hint::black_box;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Instant;

use tokio_tun::TunQueue;

const SAMPLES: usize = 5;
const ITERATIONS: usize = 50_000;
const WARMUP: usize = 5_000;

fn socket_pair() -> (OwnedFd, OwnedFd) {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is writable for two descriptors and the arguments are
    // valid for an AF_UNIX datagram socket pair.
    let result = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) };
    assert_eq!(
        result,
        0,
        "socketpair failed: {}",
        io::Error::last_os_error()
    );
    // SAFETY: socketpair returned two valid, owned descriptors.
    let left = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: socketpair returned two valid, owned descriptors.
    let right = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    (left, right)
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime");
    let (left, right) = socket_pair();
    let (left, right) = runtime.block_on(async {
        (
            TunQueue::new(left).expect("left queue"),
            TunQueue::new(right).expect("right queue"),
        )
    });

    println!("hot-path socketpair round trip (readiness is sticky in this mode)");
    let mut buffer = vec![0u8; 65_536];
    for size in [64usize, 512, 1500] {
        let mut packet: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        // Maximizing the first nibble keeps the packet valid on the macOS
        // utun path, which infers the address family from it.
        packet[0] = 0x45;

        runtime.block_on(async {
            for _ in 0..WARMUP {
                left.send(&packet).await.expect("warmup send");
                let received = right.recv(&mut buffer).await.expect("warmup recv");
                assert_eq!(received.len(), size);
            }
        });

        // Verify the socketpair round trip preserves payload bytes once per
        // size, outside the timed region.
        runtime.block_on(async {
            left.send(&packet).await.expect("verify send");
            let received = right.recv(&mut buffer).await.expect("verify recv");
            assert_eq!(received.payload(), &packet[..]);
        });

        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let start = Instant::now();
            runtime.block_on(async {
                for _ in 0..ITERATIONS {
                    left.send(&packet).await.expect("send");
                    let received = right.recv(&mut buffer).await.expect("recv");
                    assert_eq!(received.len(), size);
                    black_box(received.len());
                }
            });
            samples.push(start.elapsed());
        }
        samples.sort();
        let median = samples[SAMPLES / 2];
        let fastest = samples[0];
        let slowest = samples[SAMPLES - 1];

        let packets_per_second = ITERATIONS as f64 / median.as_secs_f64();
        let bytes_per_second = packets_per_second * size as f64;
        println!(
            "{size:>5} B: {:>8.1} kpps, {:>8.1} MiB/s, {:>7.0} ns/packet \
             (median of {SAMPLES}; range {:?}..{:?})",
            packets_per_second / 1_000.0,
            bytes_per_second / (1024.0 * 1024.0),
            median.as_secs_f64() * 1e9 / ITERATIONS as f64,
            fastest,
            slowest,
        );
    }
}
