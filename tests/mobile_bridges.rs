#![cfg(any(target_os = "android", target_os = "ios"))]

#[path = "../examples/android/tun_bridge.rs"]
mod android_bridge;

#[path = "../examples/ios/tun_bridge.rs"]
mod ios_bridge;

#[test]
fn mobile_bridge_sources_compile() {
    let _ = android_bridge::adopt_fd;
    let _ = ios_bridge::adopt_fd;
}
