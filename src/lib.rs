#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod io;
    pub mod params;
    pub mod request;
}

mod builder;
mod tun;
mod macaddr;

pub mod result;

pub use self::builder::TunBuilder;
pub use self::tun::Tun;
pub use self::macaddr::MacAddr;
