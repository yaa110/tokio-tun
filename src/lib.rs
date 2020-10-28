#[cfg(target_os = "linux")]
mod linux {
    pub mod address;
    pub mod interface;
    pub mod params;
    pub mod request;
}

mod builder;
mod tun;

pub mod result;

pub use self::builder::TunBuilder;
pub use self::tun::Tun;
