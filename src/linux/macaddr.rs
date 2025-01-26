use std::fmt::{Display, Formatter};
use std::mem;
use std::str::FromStr;
use crate::linux::address::AddrExt;
use crate::linux::request::sockaddr;

#[derive(Debug, Copy, Clone)]
pub struct MacAddr([u8; 6]);

impl Display for MacAddr{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f,"{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5])
    }
}
impl From<MacAddr> for [u8; 6] {
    fn from(value: MacAddr) -> Self {
        value.0
    }
}

impl From<[u8; 6]> for MacAddr {
    fn from(value: [u8; 6]) -> Self {
        MacAddr(value)
    }
}

impl FromStr for MacAddr {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 17 {
            return Err("unexpected expected MAC address length, expected 17 characters");
        }
        let mut data = [0u8; 6];
        for i in 0..6 {
            // two characters per octet + the colon separator
            let offset = 2 * i + i;

            // just parsing the hex values won't detect an invalid separator character, so
            // look for it explicitly
            if i > 0 && s.as_bytes()[offset-1] != b':'{
                return Err("invalid MAC address format, separator not ':'");
            }

            match u8::from_str_radix(&s[offset..offset + 2], 16) {
                Ok(v) => data[i] = v,
                Err(_) => return Err("invalid MAC address format"),
            }
        }
        Ok(MacAddr(data))
    }
}


impl AddrExt for MacAddr {
    fn to_address(&self) -> sockaddr {
        let mut addr: libc::sockaddr = unsafe { mem::zeroed() };
        addr.sa_family = libc::ARPHRD_ETHER as _;
        for (i, &byte) in self.0.iter().enumerate() {
            if i < 6 {
                addr.sa_data[i] = byte as i8;
            }
        }
        unsafe { mem::transmute(addr) }
    }

    fn from_address(addr: sockaddr) -> Self {
        let hw_addr: [u8; 6] = addr.sa_data[..6]
            .iter()
            .map(|x| *x as u8)
            .collect::<Vec<_>>()
            .try_into()
            .ok()
            .unwrap_or_default();
        MacAddr::from(hw_addr)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_mac_addr_parse_valid() {
        let res = MacAddr::from_str("12:34:56:78:9A:BC");
        assert!(res.is_ok());
        assert_eq!(res.unwrap().0, [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
    }

    #[test]
    fn test_mac_addr_parse_invalid_short() {
        let res = MacAddr::from_str("12:34:56:78:9A:B");
        assert!(res.is_err());
    }
    #[test]
    fn test_mac_addr_parse_invalid_long() {
        let res = MacAddr::from_str("12:34:56:78:9A:BCD");
        assert!(res.is_err());
    }
    #[test]
    fn test_mac_addr_parse_invalid_chars() {
        let res = MacAddr::from_str("G2:34:56:78:9A:BC");
        assert!(res.is_err());
    }
    #[test]
    fn test_mac_addr_parse_invalid_separator() {
        let res = MacAddr::from_str("12:34-56:78:9A:BC");
        assert!(res.is_err());
    }

    #[test]
    fn test_mac_addr_display() {
        let str_format = "12:34:56:78:9A:BC";
        let res = MacAddr::from_str(str_format).unwrap();
        assert_eq!(res.to_string(), str_format)
    }
}
