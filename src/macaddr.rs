pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub fn new(data: [u8; 6]) -> MacAddr {
        MacAddr(data)
    }
}

impl MacAddr {
    pub fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl From<[u8; 6]> for MacAddr {
  fn from(data: [u8; 6]) -> MacAddr {
        MacAddr::new(data)
    }
}
