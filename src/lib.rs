#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod io;
    pub mod params;
    pub mod request;
}

mod builder;
mod result;
mod tun;

pub use self::builder::TunBuilder;
pub use self::result::{Error, Result};
pub use self::tun::{Tun, TunQueue};
