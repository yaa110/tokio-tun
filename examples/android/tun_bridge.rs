//! Illustrative Rust side of the Android example.
//!
//! The Kotlin `TokioTunService` calls these JNI entry points after
//! `ParcelFileDescriptor.detachFd()`. The descriptor is owned by the caller
//! and transferred to Rust at the JNI boundary.
//!
//! This file is compiled as an example target. A real application should build
//! it as a `cdylib` and enable Tokio's runtime feature, for example
//! `tokio = { version = "1.54", features = ["rt-multi-thread", "net"] }`.

use std::ffi::{c_int, c_void};
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::{Mutex, OnceLock};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tun::TunQueue;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static TASK: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to create Tokio runtime"))
}

fn task() -> &'static Mutex<Option<JoinHandle<()>>> {
    TASK.get_or_init(|| Mutex::new(None))
}

/// Wraps an owned fd produced by `ParcelFileDescriptor.detachFd()`.
///
/// Call this from inside a Tokio runtime with I/O enabled. If construction
/// fails, `TunQueue` closes the fd.
pub fn adopt_fd(fd: OwnedFd) -> io::Result<TunQueue> {
    TunQueue::try_from(fd)
}

/// Reads raw IPv4/IPv6 packets from the VpnService queue.
pub async fn read_packets(queue: TunQueue) -> io::Result<()> {
    let mut packet = vec![0u8; 65_535];
    loop {
        let len = queue.recv(&mut packet).await?;
        handle_packet(&packet[..len]).await?;
    }
}

async fn handle_packet(_packet: &[u8]) -> io::Result<()> {
    // Replace this with your tunnel transport or packet-processing pipeline.
    Ok(())
}

/// JNI entry point called by `TokioTunService.nativeStart`.
///
/// # Safety
///
/// `raw_fd` must be open, valid, and ownership must have been transferred by
/// `ParcelFileDescriptor.detachFd()`.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_com_example_tokiotun_TokioTunService_nativeStart(
    _env: *mut c_void,
    _this: *mut c_void,
    raw_fd: c_int,
) -> u8 {
    // SAFETY: ParcelFileDescriptor.detachFd() transfers ownership to native code.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd as RawFd) };
    let runtime = runtime();
    let queue = {
        let _guard = runtime.enter();
        match adopt_fd(fd) {
            Ok(queue) => queue,
            Err(_) => return 0,
        }
    };

    let mut slot = task().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(previous) = slot.take() {
        previous.abort();
    }
    *slot = Some(runtime.spawn(async move {
        let _ = read_packets(queue).await;
    }));
    1
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_tokiotun_TokioTunService_nativeStop(
    _env: *mut c_void,
    _this: *mut c_void,
) {
    if let Some(handle) = task().lock().unwrap_or_else(|err| err.into_inner()).take() {
        handle.abort();
    }
}

fn main() {}
