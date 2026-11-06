#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
compile_error!("tokio-tun supports Linux, macOS, Android, and iOS only");

#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod netlink;
    pub mod params;
    pub mod request;
}

#[cfg(target_os = "macos")]
mod macos {
    pub mod address;
    pub mod interface;
    pub mod io;
    pub mod params;
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
mod raw_io;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod address;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod builder;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
mod queue;
mod result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod tun;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::address::AddressInfo;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::builder::TunBuilder;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub use self::queue::TunQueue;
pub use self::result::{Error, Result};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use self::tun::Tun;
