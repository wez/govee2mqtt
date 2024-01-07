use serde::{Deserialize, Deserializer};

#[derive(Clone, PartialEq, Eq)]
pub struct HexBytes(Vec<u8>);

impl std::fmt::Debug for HexBytes {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.write_fmt(format_args!("{:02X?}", self.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HumidifierNightlightParams {
    pub on: bool,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoveeBlePacket {
    Generic(HexBytes),
    SetSceneCode(u16),
    #[allow(unused)]
    SetPower(bool),
    SetHumidifierNightlight(HumidifierNightlightParams),
    NotifyHumidifierMode {
        mode: u8,
        param: u8,
    },
    NotifyHumidifierTimer {
        on: bool,
    },
    NotifyHumidifierAutoMode {
        param: u8,
    },
    NotifyHumidifierNightlight(HumidifierNightlightParams),
}

impl<'de> Deserialize<'de> for GoveeBlePacket {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;
        let text = String::deserialize(deserializer)?;
        Ok(Self::parse_base64(&text).map_err(|e| D::Error::custom(format!("{e:#}")))?)
    }
}

fn calculate_checksum(data: &[u8]) -> u8 {
    let mut checksum: u8 = 0;
    for &b in data {
        checksum = checksum ^ b;
    }
    checksum
}

fn finish(mut data: Vec<u8>) -> Vec<u8> {
    let checksum = calculate_checksum(&data);
    data.resize(19, 0);
    data.push(checksum);
    data
}

fn btoi(on: bool) -> u8 {
    if on {
        1
    } else {
        0
    }
}

fn itob(i: &u8) -> bool {
    *i != 0
}

impl GoveeBlePacket {
    pub fn into_vec(self) -> Vec<u8> {
        match self {
            Self::Generic(HexBytes(v)) => v,
            Self::SetSceneCode(code) => {
                let [lo, hi] = code.to_le_bytes();
                finish(vec![0x33, 0x05, 0x04, lo, hi])
            }
            Self::SetPower(on) => finish(vec![0x33, 0x01, btoi(on)]),
            Self::SetHumidifierNightlight(HumidifierNightlightParams {
                on,
                r,
                g,
                b,
                brightness,
            }) => finish(vec![0x33, 0x1b, btoi(on), brightness, r, g, b]),
            Self::NotifyHumidifierNightlight(HumidifierNightlightParams {
                on,
                r,
                g,
                b,
                brightness,
            }) => finish(vec![0xaa, 0x1b, btoi(on), brightness, r, g, b]),
            Self::NotifyHumidifierMode { mode, param } => {
                finish(vec![0xaa, 0x05, 0x0, mode, param])
            }
            Self::NotifyHumidifierAutoMode { param } => finish(vec![0xaa, 0x05, 0x03, param]),
            Self::NotifyHumidifierTimer { on } => finish(vec![0xaa, 0x11, btoi(on)]),
        }
    }

    pub fn parse_bytes(data: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            data.len() == 20,
            "packet must contain 20 bytes, have {}",
            data.len()
        );
        let checksum = calculate_checksum(&data[0..19]);
        anyhow::ensure!(
            checksum == data[19],
            "packet checksum is invalid. Expected {} but got {checksum}",
            data[19]
        );

        Ok(match &data[0..19] {
            [0x33, 0x01, on, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::SetPower(itob(on))
            }
            [0x33, 0x05, 0x04, lo, hi, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::SetSceneCode(((*hi as u16) << 8) | *lo as u16)
            }
            [0x33, 0x1b, on, brightness, r, g, b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::SetHumidifierNightlight(HumidifierNightlightParams {
                    on: itob(on),
                    r: *r,
                    g: *g,
                    b: *b,
                    brightness: *brightness,
                })
            }
            [0xaa, 0x1b, on, brightness, r, g, b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierNightlight(HumidifierNightlightParams {
                    on: itob(on),
                    r: *r,
                    g: *g,
                    b: *b,
                    brightness: *brightness,
                })
            }
            [0xaa, 0x05, 0, mode, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierMode {
                    mode: *mode,
                    param: *param,
                }
            }
            [0xaa, 0x11, on, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierTimer { on: itob(on) }
            }
            [0xaa, 0x05, 0x03, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierAutoMode { param: *param }
            }
            _ => Self::Generic(HexBytes(data.to_vec())),
        })
    }

    pub fn parse_base64<B: AsRef<[u8]>>(encoded: B) -> anyhow::Result<Self> {
        let decoded = data_encoding::BASE64.decode(encoded.as_ref())?;
        Self::parse_bytes(&decoded)
    }

    pub fn base64(self) -> String {
        data_encoding::BASE64.encode(&self.into_vec())
    }

    pub fn with_bytes(bytes: Vec<u8>) -> Self {
        Self::Generic(HexBytes(finish(bytes)))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn round_trip(value: GoveeBlePacket) {
        let bytes = value.clone().into_vec();
        let decoded = GoveeBlePacket::parse_bytes(&bytes).unwrap();
        assert_eq!(bytes, decoded.into_vec());

        let b64 = value.clone().base64();
        let decoded = GoveeBlePacket::parse_base64(&b64).unwrap();
        assert_eq!(value, decoded);
    }

    #[test]
    fn basic_round_trip() {
        round_trip(GoveeBlePacket::SetSceneCode(123));
        round_trip(GoveeBlePacket::SetPower(true));
        round_trip(GoveeBlePacket::SetHumidifierNightlight(
            HumidifierNightlightParams {
                on: true,
                r: 255,
                g: 69,
                b: 42,
                brightness: 100,
            },
        ));
    }

    #[test]
    fn decode_some_stuff() {
        let input = [
            "qhIAAAAAAAAAAAAAAAAAAAAAALg=",
            "qhEAAAAAAAAAAAAAAAAAAAAAALs=",
            "qgUDvAAAAAAAAAAAAAAAAAAAABA=",
            "qgUCAAkAPAA8BQA8ADwB/////6A=",
            "qgUBCQAAAAAAAAAAAAAAAAAAAKc=",
            "qgUAAQkAAAAAAAAAAAAAAAAAAKc=",
            "qhYB/////wAAAAAAAAAAAAAAAL0=",
            "qhsBZAAAAAAAAAAAAAAAAAAAANQ=",
            "qggYTTMzNc4AAAAAAAAAAAAAAAw=",
            "qhABA2RqAAAAAAAAAAAAAAAAALY=",
            "qhcAAAIAAAAAAAAAAAAAAAAAAL8=",
        ];

        let decoded: Vec<_> = input
            .iter()
            .map(|s| GoveeBlePacket::parse_base64(s).unwrap())
            .collect();

        k9::snapshot!(
            decoded,
            "
[
    Generic(
        [AA, 12, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, B8],
    ),
    NotifyHumidifierTimer {
        on: false,
    },
    NotifyHumidifierAutoMode {
        param: 188,
    },
    Generic(
        [AA, 05, 02, 00, 09, 00, 3C, 00, 3C, 05, 00, 3C, 00, 3C, 01, FF, FF, FF, FF, A0],
    ),
    Generic(
        [AA, 05, 01, 09, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, A7],
    ),
    NotifyHumidifierMode {
        mode: 1,
        param: 9,
    },
    Generic(
        [AA, 16, 01, FF, FF, FF, FF, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, BD],
    ),
    NotifyHumidifierNightlight(
        HumidifierNightlightParams {
            on: true,
            r: 0,
            g: 0,
            b: 0,
            brightness: 100,
        },
    ),
    Generic(
        [AA, 08, 18, 4D, 33, 33, 35, CE, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 0C],
    ),
    Generic(
        [AA, 10, 01, 03, 64, 6A, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, B6],
    ),
    Generic(
        [AA, 17, 00, 00, 02, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, BF],
    ),
]
"
        );
    }
}
