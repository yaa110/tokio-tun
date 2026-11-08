//! Illustrative Rust side of the iOS example.
//!
//! The Swift `PacketTunnelProvider` creates a `SOCK_DGRAM` socket pair and
//! passes one end to [`tokio_tun_start`]. Rust wraps that end in `TunQueue`,
//! reads/writes raw IP packets, and aborts the task on `tokio_tun_stop`.
//!
//! This file is an illustrative bridge snippet, not a Cargo example. It is
//! compile-checked through `tests/mobile_bridges.rs`. Build it as a `cdylib`
//! and include a C header/bridging header for the two exported functions. The
//! application must enable Tokio's runtime feature (for example
//! `tokio = { version = "1.54", features = ["rt-multi-thread", "net"] }`).

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::{Mutex, OnceLock};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tun::TunQueue;

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
static TASK: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

fn runtime() -> Option<&'static Runtime> {
    RUNTIME
        .get_or_init(|| Runtime::new().map_err(|err| err.to_string()))
        .as_ref()
        .ok()
}

fn task() -> &'static Mutex<Option<JoinHandle<()>>> {
    TASK.get_or_init(|| Mutex::new(None))
}

/// Wraps an owned fd produced by the Swift socket pair.
///
/// Call this from inside a Tokio runtime with I/O enabled. If construction
/// fails, `TunQueue` closes the fd.
pub fn adopt_fd(fd: OwnedFd) -> io::Result<TunQueue> {
    TunQueue::try_from(fd)
}

/// Reads raw IPv4/IPv6 packets from the socket-pair queue.
pub async fn read_packets(queue: TunQueue) -> io::Result<()> {
    let mut packet = vec![0u8; 65_535];
    loop {
        let received = queue.recv(&mut packet).await?;
        handle_packet(received.payload()).await?;
    }
}

async fn handle_packet(_packet: &[u8]) -> io::Result<()> {
    // Replace this with your tunnel transport or packet-processing pipeline.
    Ok(())
}

/// C ABI entry point called by Swift after `socketpair()`.
///
/// Returns 0 on success and -1 on failure. Rust owns `raw_fd` on both success
/// and failure; on failure it is closed during cleanup.
///
/// # Safety
///
/// The caller must transfer ownership of `raw_fd` to this function. The fd must
/// be open and valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tokio_tun_start(raw_fd: RawFd) -> i32 {
    if raw_fd < 0 {
        return -1;
    }
    // SAFETY: the caller transfers ownership of `raw_fd`. Take ownership
    // before any early return so every failure path closes the descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let Some(runtime) = runtime() else {
        return -1;
    };
    let queue = {
        let _guard = runtime.enter();
        match adopt_fd(fd) {
            Ok(queue) => queue,
            Err(_) => return -1,
        }
    };

    let mut slot = task().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(previous) = slot.take() {
        previous.abort();
    }
    *slot = Some(runtime.spawn(async move {
        // The example discards the task error. A production bridge should
        // report it to Swift and stop the tunnel.
        let _ = read_packets(queue).await;
    }));
    0
}

/// C ABI entry point called by Swift during tunnel shutdown.
#[unsafe(no_mangle)]
pub extern "C" fn tokio_tun_stop() {
    if let Some(handle) = task().lock().unwrap_or_else(|err| err.into_inner()).take() {
        handle.abort();
    }
}
