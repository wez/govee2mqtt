use anyhow::anyhow;
use once_cell::sync::Lazy;
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use serde::{Deserialize, Deserializer};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

static MGR: Lazy<PacketManager> = Lazy::new(PacketManager::new);

#[derive(Clone, PartialEq, Eq)]
pub struct HexBytes(Vec<u8>);

impl std::fmt::Debug for HexBytes {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.write_fmt(format_args!("{:02X?}", self.0))
    }
}

#[allow(clippy::type_complexity)]
pub struct PacketCodec {
    encode: Box<dyn Fn(&dyn Any) -> anyhow::Result<Vec<u8>> + Sync + Send>,
    decode: Box<dyn Fn(&[u8]) -> anyhow::Result<GoveeBlePacket> + Sync + Send>,
    supported_skus: &'static [&'static str],
    type_id: TypeId,
}

impl PacketCodec {
    pub fn new<T: 'static>(
        supported_skus: &'static [&'static str],
        encode: impl Fn(&T) -> anyhow::Result<Vec<u8>> + 'static + Sync + Send,
        decode: impl Fn(&[u8]) -> anyhow::Result<GoveeBlePacket> + 'static + Sync + Send,
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
    fn map_for_sku(&self, sku: &str) -> MappedMutexGuard<'_, HashMap<TypeId, Arc<PacketCodec>>> {
        MutexGuard::map(self.codec_by_sku.lock(), |codecs| {
            codecs.entry(sku.to_string()).or_insert_with(|| {
                let mut map = HashMap::new();

                for codec in &self.all_codecs {
                    if codec.supported_skus.contains(&sku)
                        && map.insert(codec.type_id, codec.clone()).is_some()
                    {
                        eprintln!("Conflicting PacketCodecs for {sku} {:?}", codec.type_id);
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
            ($target:expr, $input:expr, $field_name:ident, $($tail:tt)*) => {
                    $input.$field_name.encode_param($target);
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
                    let maybe_byte = $data.get(0);
                    anyhow::ensure!(maybe_byte == Some(&$expected),"expected {} but got {maybe_byte:?}", $expected);
                    $data = &$data[1..];
                    decode_body!($target, $data, $($tail)*);
            };

            // Match a field; parse it into the struct
            ($target:expr, $data:expr, $field_name:ident, $($tail:tt)*) => {
                    let remain = $target.$field_name.decode_param($data)?;
                    $data = remain;
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
            SetHumidifierMode,
            SetHumidifierMode,
            0x33,
            0x05,
            mode,
            param,
        ));
        all_codecs.push(packet!(
            &["H7160"],
            NotifyHumidifierMode,
            NotifyHumidifierMode,
            0xaa,
            0x05,
            0x00,
            mode,
            param,
        ));
        all_codecs.push(packet!(
            &["H7160"],
            HumidifierAutoMode,
            NotifyHumidifierAutoMode,
            0xaa,
            0x05,
            0x03,
            target_humidity,
        ));
        all_codecs.push(packet!(
            &["H7160"],
            NotifyHumidifierNightlightParams,
            NotifyHumidifierNightlight,
            0xaa,
            0x1b,
            on,
            brightness,
            r,
            g,
            b,
        ));
        all_codecs.push(packet!(
            &["H7160"],
            SetHumidifierNightlightParams,
            SetHumidifierNightlight,
            0x33,
            0x1b,
            on,
            brightness,
            r,
            g,
            b,
        ));
        all_codecs.push(PacketCodec::new(
            &["Generic:Light"],
            SetSceneCode::encode,
            SetSceneCode::decode,
        ));
        all_codecs.push(packet!(
            &["H5086"],
            NotifyEnergyMonitoring,
            NotifyEnergyMonitoring,
            0xaa,
            0x19,
            on_time_seconds,
            energy_deciwatt_hours,
            voltage_centivolts,
            current_centiamps,
            power_centiwatts,
            power_factor_percent,
        ));

        all_codecs.push(packet!(
            &["Generic:Light"],
            SetDevicePower,
            SetDevicePower,
            0x33,
            0x01,
            on,
        ));

        Self {
            codec_by_sku: Mutex::new(HashMap::new()),
            all_codecs: all_codecs.into_iter().map(Arc::new).collect(),
        }
    }
}

pub trait DecodePacketParam {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]>;
    fn encode_param(&self, target: &mut Vec<u8>);
}

impl DecodePacketParam for u8 {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        *self = *data.first().ok_or_else(|| anyhow!("EOF"))?;
        Ok(&data[1..])
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        target.push(*self);
    }
}

impl DecodePacketParam for u16 {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        let lo = *data.first().ok_or_else(|| anyhow!("EOF"))?;
        let hi = *data.get(1).ok_or_else(|| anyhow!("EOF"))?;
        *self = ((hi as u16) << 8) | lo as u16;
        Ok(&data[2..])
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        let hi = (*self >> 8) as u8;
        let lo = (*self & 0xff) as u8;
        target.push(lo);
        target.push(hi);
    }
}

/// A 16-bit value that is encoded big-endian, unlike u16
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BigEndian16(pub u16);

impl DecodePacketParam for BigEndian16 {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        let bytes = data.get(..2).ok_or_else(|| anyhow!("EOF"))?;
        self.0 = u16::from_be_bytes([bytes[0], bytes[1]]);
        Ok(&data[2..])
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        target.extend_from_slice(&self.0.to_be_bytes());
    }
}

/// A 24-bit value that is encoded big-endian
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BigEndian24(pub u32);

impl DecodePacketParam for BigEndian24 {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        let bytes = data.get(..3).ok_or_else(|| anyhow!("EOF"))?;
        self.0 = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        Ok(&data[3..])
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        target.extend_from_slice(&self.0.to_be_bytes()[1..]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SetHumidifierNightlightParams {
    pub on: bool,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8,
}

impl From<NotifyHumidifierNightlightParams> for SetHumidifierNightlightParams {
    fn from(val: NotifyHumidifierNightlightParams) -> Self {
        SetHumidifierNightlightParams {
            on: val.on,
            r: val.r,
            g: val.g,
            b: val.b,
            brightness: val.brightness,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NotifyHumidifierNightlightParams {
    pub on: bool,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8,
}

/// Data is offset by 128 with increments of 1%,
/// so 0% is 128, 100% is 228%
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetHumidity(u8);

impl From<TargetHumidity> for u8 {
    fn from(val: TargetHumidity) -> Self {
        val.0
    }
}

impl DecodePacketParam for TargetHumidity {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        self.0.decode_param(data)
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        target.push(self.0);
    }
}

impl TargetHumidity {
    pub fn as_percent(&self) -> u8 {
        self.0 & 0x7f
    }

    pub fn into_inner(self) -> u8 {
        self.0
    }

    pub fn from_percent(percent: u8) -> Self {
        Self(percent + 128)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SetHumidifierMode {
    pub mode: u8,
    pub param: u8,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct NotifyHumidifierMode {
    pub mode: u8,
    pub param: u8,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HumidifierAutoMode {
    pub target_humidity: TargetHumidity,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SetSceneCode {
    code: u16,
    scence_param: String,
}

impl SetSceneCode {
    pub fn new(code: u16, scence_param: String) -> Self {
        Self { code, scence_param }
    }

    /// For reference, see:
    /// <https://github.com/egold555/Govee-Reverse-Engineering/issues/11#issuecomment-2565692233>
    /// <https://github.com/AlgoClaw/Govee/blob/main/decoded/explanation>
    fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let bytes = data_encoding::BASE64.decode(self.scence_param.as_bytes())?;

        let mut data = vec![0xa3, 0x00, 0x01, 0x00 /* line count */, 0x02];
        let mut num_lines = 0u8;
        let mut last_line_marker = 1;

        for b in bytes {
            if data.len().is_multiple_of(19) {
                num_lines += 1;

                data.push(0xa3);
                last_line_marker = data.len();

                data.push(num_lines);
            }

            data.push(b);
        }
        // The last line uses 0xff as the indicator, rather than its line number
        data[last_line_marker] = 0xff;
        // back-patch the number of lines into the packet
        data[3] = num_lines + 1;

        // Now apply padding and checksums
        let mut padded = vec![];
        for chunk in data.chunks(19) {
            let mut padded_chunk = chunk.to_vec();
            padded_chunk = finish(padded_chunk);
            padded.append(&mut padded_chunk);
        }

        // and finally encode the scene code as the final packet "line"
        let hi = (self.code >> 8) as u8;
        let lo = (self.code & 0xff) as u8;
        padded.append(&mut finish(vec![0x33, 0x05, 0x04, lo, hi]));
        Ok(padded)
    }

    fn decode(_data: &[u8]) -> anyhow::Result<GoveeBlePacket> {
        anyhow::bail!("SetSceneCode::decode is not implemented");
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SetDevicePower {
    pub on: bool,
}

/// Electrical readings reported by smart plugs with energy monitoring,
/// such as the H5086, as part of their IoT status.
/// The plug doesn't always fill in the checksum, but the packet
/// decoder doesn't verify it anyway.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct NotifyEnergyMonitoring {
    /// Seconds that the plug has been switched on since local midnight.
    /// Like energy, it's reset after a reboot until the plug restores it.
    pub on_time_seconds: BigEndian24,
    /// Energy used since local midnight, in units of 0.1Wh
    pub energy_deciwatt_hours: BigEndian24,
    /// Line voltage in units of 0.01V
    pub voltage_centivolts: BigEndian16,
    /// Load current in units of 0.01A
    pub current_centiamps: BigEndian16,
    /// Active power in units of 0.01W
    pub power_centiwatts: BigEndian24,
    /// Power factor in percent
    pub power_factor_percent: u8,
}

impl NotifyEnergyMonitoring {
    /// The plug sometimes reports a current of zero alongside a power
    /// factor of 255, presumably when it is sampled mid-measurement.
    fn current_is_valid(&self) -> bool {
        self.power_factor_percent <= 100
    }

    pub fn voltage(&self) -> f64 {
        self.voltage_centivolts.0 as f64 / 100.
    }

    pub fn current(&self) -> Option<f64> {
        self.current_is_valid()
            .then(|| self.current_centiamps.0 as f64 / 100.)
    }

    pub fn power(&self) -> f64 {
        self.power_centiwatts.0 as f64 / 100.
    }

    pub fn power_factor(&self) -> Option<u8> {
        self.current_is_valid().then_some(self.power_factor_percent)
    }

    pub fn energy_kwh(&self) -> f64 {
        self.energy_deciwatt_hours.0 as f64 / 10_000.
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoveeBlePacket {
    Generic(HexBytes),
    #[allow(unused)] // can remove if/when SetSceneCode::decode has an impl
    SetSceneCode(SetSceneCode),
    SetDevicePower(SetDevicePower),
    SetHumidifierNightlight(SetHumidifierNightlightParams),
    NotifyHumidifierMode(NotifyHumidifierMode),
    SetHumidifierMode(SetHumidifierMode),
    NotifyHumidifierAutoMode(HumidifierAutoMode),
    NotifyHumidifierNightlight(NotifyHumidifierNightlightParams),
    NotifyEnergyMonitoring(NotifyEnergyMonitoring),
}

#[derive(Debug)]
pub struct Base64HexBytes(HexBytes);

impl Base64HexBytes {
    pub fn decode_for_sku(&self, sku: &str) -> GoveeBlePacket {
        MGR.decode_for_sku(sku, &self.0 .0)
    }

    pub fn encode_for_sku<T: 'static>(sku: &str, value: &T) -> anyhow::Result<Self> {
        MGR.encode_for_sku(sku, value)
            .map(|bytes| Base64HexBytes(HexBytes(bytes)))
    }

    pub fn base64(&self) -> Vec<String> {
        let mut result = vec![];
        for chunk in self.0 .0.chunks(20) {
            result.push(data_encoding::BASE64.encode(chunk));
        }
        result
    }

    pub fn with_bytes(bytes: Vec<u8>) -> Self {
        Self(HexBytes(finish(bytes)))
    }
}

impl<'de> Deserialize<'de> for Base64HexBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;
        let encoded = String::deserialize(deserializer)?;
        let decoded = data_encoding::BASE64
            .decode(encoded.as_ref())
            .map_err(|e| D::Error::custom(format!("{e:#}")))?;
        Ok(Self(HexBytes(decoded)))
    }
}

fn calculate_checksum(data: &[u8]) -> u8 {
    let mut checksum: u8 = 0;
    for &b in data {
        checksum ^= b;
    }
    checksum
}

fn finish(mut data: Vec<u8>) -> Vec<u8> {
    let checksum = calculate_checksum(&data);
    data.resize(19, 0);
    data.push(checksum);
    data
}

impl DecodePacketParam for bool {
    fn decode_param<'a>(&mut self, data: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        let mut byte = 0u8;
        let remain = byte.decode_param(data)?;
        *self = itob(&byte);
        Ok(remain)
    }

    fn encode_param(&self, target: &mut Vec<u8>) {
        target.push(btoi(*self));
    }
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

impl GoveeBlePacket {}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn packet_manager() {
        assert_eq!(
            MGR.decode_for_sku(
                "H7160",
                &[0x33, 0x05, 0x01, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 23]
            ),
            GoveeBlePacket::SetHumidifierMode(SetHumidifierMode {
                mode: 1,
                param: 0x20
            })
        );

        assert_eq!(
            MGR.encode_for_sku(
                "H7160",
                &SetHumidifierMode {
                    mode: 1,
                    param: 0x20
                }
            )
            .unwrap(),
            vec![0x33, 0x05, 0x01, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 23]
        );
    }

    fn round_trip<T: 'static + std::fmt::Debug>(sku: &str, value: &T, expect: GoveeBlePacket) {
        let bytes = Base64HexBytes::encode_for_sku(sku, value).unwrap();
        let decoded = bytes.decode_for_sku(sku);
        assert_eq!(decoded, expect);
    }

    #[test]
    fn basic_round_trip() {
        round_trip(
            "Generic:Light",
            &SetDevicePower { on: true },
            GoveeBlePacket::SetDevicePower(SetDevicePower { on: true }),
        );
        round_trip(
            "H7160",
            &SetHumidifierNightlightParams {
                on: true,
                r: 255,
                g: 69,
                b: 42,
                brightness: 100,
            },
            GoveeBlePacket::SetHumidifierNightlight(SetHumidifierNightlightParams {
                on: true,
                r: 255,
                g: 69,
                b: 42,
                brightness: 100,
            }),
        );
    }

    fn decode_base64(sku: &str, encoded: &str) -> GoveeBlePacket {
        let bytes = data_encoding::BASE64.decode(encoded.as_bytes()).unwrap();
        MGR.decode_for_sku(sku, &bytes)
    }

    #[test]
    fn energy_monitoring() {
        // Captured from an H5086 with a laptop charger attached
        let GoveeBlePacket::NotifyEnergyMonitoring(report) =
            decode_base64("H5086", "qhkAE3gAACgwMAA9ABEOOgAAAAA=")
        else {
            panic!("expected NotifyEnergyMonitoring");
        };
        assert_eq!(
            report,
            NotifyEnergyMonitoring {
                on_time_seconds: BigEndian24(4984),
                energy_deciwatt_hours: BigEndian24(40),
                voltage_centivolts: BigEndian16(12336),
                current_centiamps: BigEndian16(61),
                power_centiwatts: BigEndian24(4366),
                power_factor_percent: 58,
            }
        );
        assert_eq!(report.voltage(), 123.36);
        assert_eq!(report.current(), Some(0.61));
        assert_eq!(report.power(), 43.66);
        assert_eq!(report.power_factor(), Some(58));
        assert_eq!(report.energy_kwh(), 0.004);

        // Captured with nothing attached; this one has a checksum
        assert_eq!(
            decode_base64("H5086", "qhkAD/QAAAAwxwAAAAAAAAAAAL8="),
            GoveeBlePacket::NotifyEnergyMonitoring(NotifyEnergyMonitoring {
                on_time_seconds: BigEndian24(4084),
                voltage_centivolts: BigEndian16(12487),
                ..Default::default()
            })
        );

        // Captured mid-measurement: zero current with a power factor of 255
        let GoveeBlePacket::NotifyEnergyMonitoring(report) =
            decode_base64("H5086", "qhkAGkUAAKgwsgAAAAkL/wAAAAA=")
        else {
            panic!("expected NotifyEnergyMonitoring");
        };
        assert_eq!(report.power(), 23.15);
        assert_eq!(report.current(), None);
        assert_eq!(report.power_factor(), None);

        round_trip(
            "H5086",
            &report,
            GoveeBlePacket::NotifyEnergyMonitoring(report),
        );

        // Other packets from the same status message are left alone
        assert!(matches!(
            decode_base64("H5086", "qhcAAAAAAAAAAAAAAAAAAAAAAL0="),
            GoveeBlePacket::Generic(_)
        ));
        // and other devices don't decode this packet
        assert!(matches!(
            decode_base64("H5080", "qhkAE3gAACgwMAA9ABEOOgAAAAA="),
            GoveeBlePacket::Generic(_)
        ));
    }

    #[test]
    fn scene_command() {
        const FOREST_SCENCE_PARAM: &str = "AyYAAQAKAgH/GQG0CgoCyBQF//8AAP//////AP//lP8AFAGWAAAAACMAAg8FAgH/FAH7AAAB+goEBP8AtP8AR///4/8AAAAAAAAAABoAAAABAgH/BQHIFBQC7hQBAP8AAAAAAAAAAA==";
        const FOREST_SCENE_CODE: u16 = 212;

        let command = SetSceneCode::new(FOREST_SCENE_CODE, FOREST_SCENCE_PARAM.to_string());

        let padded = command.encode().unwrap();

        println!("data is:");
        let mut hex = String::new();
        for (idx, b) in padded.iter().enumerate() {
            if idx % 20 == 0 && !hex.is_empty() {
                hex.push('\n');
            } else if !hex.is_empty() {
                hex.push(' ');
            }
            hex.push_str(&format!("{b:02x}"));
        }
        println!("{hex}");

        k9::snapshot!(
            hex,
            "
a3 00 01 07 02 03 26 00 01 00 0a 02 01 ff 19 01 b4 0a 0a d9
a3 01 02 c8 14 05 ff ff 00 00 ff ff ff ff ff 00 ff ff 94 12
a3 02 ff 00 14 01 96 00 00 00 00 23 00 02 0f 05 02 01 ff 0a
a3 03 14 01 fb 00 00 01 fa 0a 04 04 ff 00 b4 ff 00 47 ff b3
a3 04 ff e3 ff 00 00 00 00 00 00 00 00 1a 00 00 00 01 02 5d
a3 05 01 ff 05 01 c8 14 14 02 ee 14 01 00 ff 00 00 00 00 92
a3 ff 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 5c
33 05 04 d4 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 e6
"
        );
    }
}
