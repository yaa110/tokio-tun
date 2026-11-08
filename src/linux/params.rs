use crate::address::AddressConfig;

/// Represents parameters for creating a new Tun/Tap device on Linux.
#[cfg(target_os = "linux")]
pub struct Params {
    pub name: Option<String>,
    pub flags: i16,
    pub offload: Option<u32>,
    pub packet_info: bool,
    pub persist: bool,
    pub up: bool,
    pub mtu: Option<i32>,
    pub owner: Option<u32>,
    pub group: Option<u32>,
    pub mac_address: Option<[u8; 6]>,
    pub addresses: Vec<AddressConfig>,
    pub cloexec: bool,
}
