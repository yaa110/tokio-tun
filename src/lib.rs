#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod params;
    pub mod request;
    pub mod tun;
}

mod builder;

pub mod result;

pub use self::builder::TunBuilder;
pub use self::linux::tun::Tun;
