use crate::address::AddressConfig;

/// Represents parameters for creating a new Tun/Tap device on Linux.
#[cfg(target_os = "linux")]
pub struct Params {
    pub name: Option<String>,
    pub flags: i16,
    pub persist: bool,
    pub up: bool,
    pub mtu: Option<i32>,
    pub owner: Option<i32>,
    pub group: Option<i32>,
    pub addresses: Vec<AddressConfig>,
    pub cloexec: bool,
}
