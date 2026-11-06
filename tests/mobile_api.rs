#![cfg(any(target_os = "android", target_os = "ios"))]

use std::os::fd::OwnedFd;
use tokio_tun::TunQueue;

fn wrap(fd: OwnedFd) -> std::io::Result<TunQueue> {
    TunQueue::try_from(fd)
}

#[test]
fn mobile_tun_queue_api_is_available() {
    let _ = wrap as fn(OwnedFd) -> std::io::Result<TunQueue>;
}
