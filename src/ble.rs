use anyhow::anyhow;
use serde::{Deserialize, Deserializer};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

#[derive(Clone, PartialEq, Eq)]
pub struct HexBytes(Vec<u8>);

impl std::fmt::Debug for HexBytes {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.write_fmt(format_args!("{:02X?}", self.0))
    }
}

pub struct PacketCodec {
    encode: Box<dyn Fn(&dyn Any) -> anyhow::Result<Vec<u8>>>,
    decode: Box<dyn Fn(&[u8]) -> anyhow::Result<GoveeBlePacket>>,
    supported_skus: &'static [&'static str],
    type_id: TypeId,
}

impl PacketCodec {
    pub fn new<T: 'static>(
        supported_skus: &'static [&'static str],
        encode: impl Fn(&T) -> anyhow::Result<Vec<u8>> + 'static,
        decode: impl Fn(&[u8]) -> anyhow::Result<GoveeBlePacket> + 'static,
    ) -> Self {
        Self {
            encode: Box::new(move |any| {
                let type_id = TypeId::of::<T>();
                let value = any.downcast_ref::<T>().ok_or_else(|| {
                    anyhow!("cannot downcast to {type_id:?} in PacketCodec encoder")
                })?;
                (encode)(value)
            }),
            decode: Box::new(decode),
            supported_skus,
            type_id: TypeId::of::<T>(),
        }
    }
}

pub struct PacketManager {
    codec_by_sku: Mutex<HashMap<String, HashMap<TypeId, Arc<PacketCodec>>>>,
    all_codecs: Vec<Arc<PacketCodec>>,
}

impl PacketManager {
    fn map_for_sku(&self, sku: &str) -> MappedMutexGuard<HashMap<TypeId, Arc<PacketCodec>>> {
        MutexGuard::map(self.codec_by_sku.blocking_lock(), |codecs| {
            codecs.entry(sku.to_string()).or_insert_with(|| {
                let mut map = HashMap::new();

                for codec in &self.all_codecs {
                    if codec.supported_skus.iter().any(|s| *s == sku) {
                        map.insert(codec.type_id.clone(), codec.clone());
                    }
                }

                map
            })
        })
    }

    fn resolve_by_sku(&self, sku: &str, type_id: &TypeId) -> anyhow::Result<Arc<PacketCodec>> {
        let map = self.map_for_sku(sku);

        map.get(type_id)
            .cloned()
            .ok_or_else(|| anyhow!("sku {sku} has no codec for type {type_id:?}"))
    }

    pub fn decode_for_sku(&self, sku: &str, data: &[u8]) -> GoveeBlePacket {
        let map = self.map_for_sku(sku);

        for codec in map.values() {
            if let Ok(value) = (codec.decode)(data) {
                return value;
            }
        }

        GoveeBlePacket::Generic(HexBytes(data.to_vec()))
    }

    pub fn encode_for_sku<T: 'static>(&self, sku: &str, value: &T) -> anyhow::Result<Vec<u8>> {
        let type_id = TypeId::of::<T>();
        let codec = self.resolve_by_sku(sku, &type_id)?;

        (codec.encode)(value)
    }

    pub fn new() -> Self {
        let mut all_codecs = vec![];

        macro_rules! encode_body {
            // Tail case: nothing to do
            ($target:expr,$input:expr,) => {};

            // Match a constant byte; emit it
            ($target:expr,$input:expr, $expected:literal, $($tail:tt)*) => {
                    $target.push($expected);
                    encode_body!($target, $input, $($tail)*);
            };

            // Match a field; emit it from the struct
            ($target:expr, $input:expr, $field_name:ident: $field_type:ty, $($tail:tt)*) => {
                    $target.push($input.$field_name);
                    encode_body!($target, $input, $($tail)*);
            };
        }

        macro_rules! decode_body {
            // Tail case; verify that remaining bytes are zero
            ($target:expr, $data:expr,) => {
                while !$data.is_empty() {
                    anyhow::ensure!($data[0] == 0);
                    $data = &$data[1..];
                }
            };

            // Match a constant byte; check that it is what we expect
            ($target:expr, $data:expr, $expected:literal, $($tail:tt)*) => {
                    anyhow::ensure!($data.get(0) == Some(&$expected));
                    $data = &$data[1..];
                    decode_body!($target, $data, $($tail)*);
            };

            // Match a field; parse it into the struct
            ($target:expr, $data:expr, $field_name:ident: $field_type:ty, $($tail:tt)*) => {
                    $target.$field_name = *$data.get(0).ok_or_else(||anyhow!("EOF"))?;
                    $data = &$data[1..];
                    decode_body!($target, $data, $($tail)*);
            };
        }

        /// Helper for defining a PacketCodec.
        /// The first param is the list of SKUs which are known to support
        /// this packet.
        /// The second parameter is the name of the type which will be
        /// encoded into raw bytes when encoding. It must impl Default.
        /// The third parameter is the name of the GoveeBlePacket enum
        /// variant that holds that type.
        /// The subsequent parameters are rules that match the bytes
        /// in the packet when decoding, or form the bytes in the packet
        /// when encoding. They are listed in the same sequence that they
        /// have in the packet.
        macro_rules! packet {
            ($skus:expr, $struct:ident, $variant:ident, $($body:tt)*) => {
                PacketCodec::new(
                    $skus,
                    |input_value: &$struct| {
                        let mut bytes = vec![];
                        encode_body!(&mut bytes, input_value, $($body)*);
                        Ok(finish(bytes))
                    },
                    |data| {
                        let mut data = &data[0..data.len().saturating_sub(1)];
                        let mut value = $struct::default();
                        decode_body!(&mut value, data, $($body)*);
                        Ok(GoveeBlePacket::$variant(value))
                    }
                )
            }
        }

        all_codecs.push(packet!(
            &["H7160"],
            HumidifierMode,
            SetHumidifierMode,
            0x33,
            0x05,
            mode: u8,
            param: u8,
        ));

        Self {
            codec_by_sku: Mutex::new(HashMap::new()),
            all_codecs: all_codecs.into_iter().map(Arc::new).collect(),
        }
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

/// Data is offset by 128 with increments of 1%,
/// so 0% is 128, 100% is 228%
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetHumidity(u8);

impl TargetHumidity {
    pub fn as_percent(&self) -> u8 {
        self.0 - 128
    }

    pub fn into_inner(self) -> u8 {
        self.0
    }

    pub fn from_percent(percent: u8) -> Self {
        Self(percent + 128)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HumidifierMode {
    pub mode: u8,
    pub param: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoveeBlePacket {
    Generic(HexBytes),
    SetSceneCode(u16),
    #[allow(unused)]
    SetPower(bool),
    SetHumidifierNightlight(HumidifierNightlightParams),
    NotifyHumidifierMode(HumidifierMode),
    SetHumidifierMode(HumidifierMode),
    NotifyHumidifierTimer {
        on: bool,
    },
    NotifyHumidifierAutoMode {
        param: TargetHumidity,
    },
    NotifyHumidifierManualMode {
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
            Self::NotifyHumidifierMode(HumidifierMode { mode, param }) => {
                finish(vec![0xaa, 0x05, 0x0, mode, param])
            }
            Self::SetHumidifierMode(HumidifierMode { mode, param }) => {
                finish(vec![0x33, 0x05, mode, param])
            }
            Self::NotifyHumidifierAutoMode { param } => {
                finish(vec![0xaa, 0x05, 0x03, param.into_inner()])
            }
            Self::NotifyHumidifierManualMode { param } => finish(vec![0xaa, 0x05, 0x01, param]),
            Self::NotifyHumidifierTimer { on } => finish(vec![0xaa, 0x11, btoi(on)]),
        }
    }

    pub fn parse_bytes(data: &[u8]) -> anyhow::Result<Self> {
        if data.is_empty() {
            return Ok(Self::Generic(HexBytes(vec![])));
        }
        let checksum = calculate_checksum(&data[0..data.len().saturating_sub(1)]);
        let cs_byte = *data.last().expect("checked empty above");
        anyhow::ensure!(
            checksum == cs_byte,
            "packet checksum is invalid. Expected {cs_byte} but got {checksum}",
        );

        Ok(match &data[0..data.len().saturating_sub(1)] {
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
            [0x33, 0x05, mode, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::SetHumidifierMode(HumidifierMode {
                    mode: *mode,
                    param: *param,
                })
            }
            [0xaa, 0x05, 0, mode, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierMode(HumidifierMode {
                    mode: *mode,
                    param: *param,
                })
            }
            [0xaa, 0x11, on, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierTimer { on: itob(on) }
            }
            [0xaa, 0x05, 0x03, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierAutoMode {
                    param: TargetHumidity(*param),
                }
            }
            [0xaa, 0x05, 0x01, param, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => {
                Self::NotifyHumidifierManualMode { param: *param }
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

    #[test]
    fn packet_manager() {
        let mgr = PacketManager::new();

        assert_eq!(
            mgr.decode_for_sku(
                "H7160",
                &[0x33, 0x05, 0x01, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 23]
            ),
            GoveeBlePacket::SetHumidifierMode(HumidifierMode {
                mode: 1,
                param: 0x20
            })
        );

        assert_eq!(
            mgr.encode_for_sku(
                "H7160",
                &HumidifierMode {
                    mode: 1,
                    param: 0x20
                }
            )
            .unwrap(),
            vec![0x33, 0x05, 0x01, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 23]
        );
    }

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
            "6gEB6g==",
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
        param: TargetHumidity(
            188,
        ),
    },
    Generic(
        [AA, 05, 02, 00, 09, 00, 3C, 00, 3C, 05, 00, 3C, 00, 3C, 01, FF, FF, FF, FF, A0],
    ),
    NotifyHumidifierManualMode {
        param: 9,
    },
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
    Generic(
        [EA, 01, 01, EA],
    ),
]
"
        );
    }
}
