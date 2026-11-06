use crate::address::AddressConfig;

/// Represents parameters for creating a new Tun device on macOS.
pub struct Params {
    pub name: Option<String>,
    pub up: bool,
    pub mtu: Option<i32>,
    pub addresses: Vec<AddressConfig>,
    pub cloexec: bool,
}

/// Returns the `sc_unit` value for the utun control socket.
///
/// `utun` and an empty name let the kernel choose a unit (`0`). `utunN` maps
/// to `N + 1`, matching the macOS utun control interface.
pub(crate) fn parse_utun_name(name: &str) -> Option<u32> {
    if name.is_empty() || name == "utun" {
        return Some(0);
    }

    let suffix = name.strip_prefix("utun")?;
    if suffix.is_empty() {
        return Some(0);
    }
    if !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if suffix.len() > 1 && suffix.starts_with('0') {
        return None;
    }
    suffix.parse::<u32>().ok()?.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utun_names() {
        assert_eq!(parse_utun_name(""), Some(0));
        assert_eq!(parse_utun_name("utun"), Some(0));
        assert_eq!(parse_utun_name("utun0"), Some(1));
        assert_eq!(parse_utun_name("utun5"), Some(6));
        assert_eq!(parse_utun_name("utun4294967295"), None);
    }

    #[test]
    fn rejects_invalid_utun_names() {
        assert_eq!(parse_utun_name("tun0"), None);
        assert_eq!(parse_utun_name("utunx"), None);
        assert_eq!(parse_utun_name("utun+1"), None);
        assert_eq!(parse_utun_name("utun01"), None);
        assert_eq!(parse_utun_name("utun00"), None);
        assert_eq!(parse_utun_name("utun-1"), None);
        assert_eq!(parse_utun_name("utun 1"), None);
    }
}
