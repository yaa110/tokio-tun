#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
)))]
compile_error!("tokio-tun supports Linux, macOS, Android, iOS, and FreeBSD only");

#[cfg(target_os = "freebsd")]
mod freebsd {
    pub mod address;
    pub mod interface;
    pub mod io;
    pub mod params;
    pub mod sys;
}

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
    pub mod params;
}

#[cfg(target_os = "macos")]
mod header_io;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
mod raw_io;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod address;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod builder;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
mod queue;
mod result;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
mod tun;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub use self::address::AddressInfo;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub use self::builder::TunBuilder;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    target_os = "freebsd"
))]
pub use self::queue::TunQueue;
pub use self::result::{Error, Result};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub use self::tun::Tun;
