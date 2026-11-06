#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("tokio-tun supports Linux and macOS only");

#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod io;
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

mod address;
mod builder;
mod result;
mod tun;

pub use self::address::AddressInfo;
pub use self::builder::TunBuilder;
pub use self::result::{Error, Result};
pub use self::tun::{Tun, TunQueue};
