pub struct GoveeBlePacket(Vec<u8>);

impl GoveeBlePacket {
    pub fn finish(mut self) -> Self {
        let mut checksum: u8 = 0;
        for &b in &self.0 {
            checksum = checksum ^ b;
        }
        self.0.resize(19, 0);
        self.0.push(checksum);
        Self(self.0)
    }

    pub fn base64(self) -> String {
        data_encoding::BASE64.encode(&self.0)
    }

    pub fn with_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Compute a Power On/Off packet
    #[allow(unused)]
    pub fn power(on: bool) -> Self {
        Self(vec![0x33, 0x01, if on { 1 } else { 0x00 }]).finish()
    }

    /// Compute a scene code packet
    pub fn scene_code(code: u16) -> Self {
        let [lo, hi] = code.to_le_bytes();
        Self(vec![0x33, 0x05, 0x04, lo, hi]).finish()
    }
}
