use crate::address::AddressConfig;

/// Parameters for creating a FreeBSD TUN/TAP device.
pub struct Params {
    pub name: Option<String>,
    pub tap: bool,
    pub up: bool,
    pub mtu: Option<i32>,
    pub addresses: Vec<AddressConfig>,
    pub cloexec: bool,
}
