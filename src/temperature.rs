use std::str::FromStr;

pub const UNIT_CELSIUS: &str = "°C";
pub const UNIT_FARENHEIT: &str = "°F";
pub const DEVICE_CLASS_TEMPERATURE: &str = "temperature";

#[allow(unused)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemperatureUnits {
    Celsius,
    CelsiusTimes100,
    Farenheit,
    FarenheitTimes100,
}

impl TemperatureUnits {
    fn factor(&self) -> f64 {
        match self {
            Self::CelsiusTimes100 | Self::FarenheitTimes100 => 100.,
            Self::Celsius | Self::Farenheit => 1.,
        }
    }

    fn scale(&self) -> TemperatureScale {
        match self {
            Self::Celsius | Self::CelsiusTimes100 => TemperatureScale::Celsius,
            Self::Farenheit | Self::FarenheitTimes100 => TemperatureScale::Farenheit,
        }
    }

    #[allow(unused)]
    pub fn unit_of_measurement(&self) -> Option<&'static str> {
        let factor = self.factor();
        let scale = self.scale();
        if factor == 1. {
            Some(scale.unit_of_measurement())
        } else {
            None
        }
    }
}

impl From<TemperatureScale> for TemperatureUnits {
    fn from(scale: TemperatureScale) -> TemperatureUnits {
        match scale {
            TemperatureScale::Celsius => Self::Celsius,
            TemperatureScale::Farenheit => Self::Farenheit,
        }
    }
}

impl std::fmt::Display for TemperatureUnits {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let factor = self.factor();
        let scale = self.scale();
        if factor == 1. {
            scale.fmt(fmt)
        } else {
            write!(fmt, "{scale}*{factor}")
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemperatureScale {
    #[default]
    Celsius,
    Farenheit,
}

impl std::fmt::Display for TemperatureScale {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.write_str(self.unit_of_measurement())
    }
}

impl TemperatureScale {
    pub fn unit_of_measurement(&self) -> &'static str {
        match self {
            Self::Celsius => UNIT_CELSIUS,
            Self::Farenheit => UNIT_FARENHEIT,
        }
    }
}

impl FromStr for TemperatureScale {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<TemperatureScale> {
        match s {
            "c" | "C" | "°c" | "°C" => Ok(Self::Celsius),
            "f" | "F" | "°f" | "°F" => Ok(Self::Farenheit),
            _ => anyhow::bail!("Unknown temperature scale {s}"),
        }
    }
}

/// Convert farenheit to celsius
pub fn ftoc(f: f64) -> f64 {
    (f - 32.) * (5. / 9.)
}

/// Convert farenheit to celsius
pub fn ctof(f: f64) -> f64 {
    (f * 9. / 5.) + 32.
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureValue {
    unit: TemperatureUnits,
    value: f64,
}

impl std::fmt::Display for TemperatureValue {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let normalized = self.normalize();
        write!(fmt, "{}{}", normalized.value, normalized.unit)
    }
}

#[allow(unused)]
impl TemperatureValue {
    pub fn new(value: f64, unit: TemperatureUnits) -> Self {
        Self { value, unit }
    }

    pub fn with_celsius(value: f64) -> Self {
        Self {
            unit: TemperatureUnits::Celsius,
            value,
        }
    }

    pub fn with_farenheit(value: f64) -> Self {
        Self {
            unit: TemperatureUnits::Farenheit,
            value,
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Normalize away scaled temperature units
    pub fn normalize(&self) -> Self {
        let normalized = self.value / self.unit.factor();
        Self::new(normalized, self.unit.scale().into())
    }

    pub fn as_unit(&self, unit: TemperatureUnits) -> Self {
        if self.unit == unit {
            return self.clone();
        }

        let normalized = self.value / self.unit.factor();

        let converted = match (self.unit.scale(), unit.scale()) {
            (TemperatureScale::Celsius, TemperatureScale::Farenheit) => ctof(normalized),
            (TemperatureScale::Farenheit, TemperatureScale::Celsius) => ftoc(normalized),
            (TemperatureScale::Celsius, TemperatureScale::Celsius) => normalized,
            (TemperatureScale::Farenheit, TemperatureScale::Farenheit) => normalized,
        };

        Self {
            unit,
            value: converted * unit.factor(),
        }
    }

    pub fn as_celsius(&self) -> f64 {
        self.as_unit(TemperatureUnits::Celsius).value
    }

    pub fn as_farenheit(&self) -> f64 {
        self.as_unit(TemperatureUnits::Farenheit).value
    }

    pub fn parse_with_optional_scale(
        s: &str,
        scale: Option<TemperatureScale>,
    ) -> anyhow::Result<Self> {
        let (value, optional_scale) = atoi(s)?;

        let scale: TemperatureScale = if optional_scale.is_empty() {
            scale.unwrap_or(TemperatureScale::Celsius)
        } else {
            optional_scale.parse()?
        };

        Ok(Self::new(value, scale.into()))
    }
}

/// Extracts the numeric prefix from the string and any non-numeric suffix
fn atoi<F: FromStr>(input: &str) -> Result<(F, &str), <F as FromStr>::Err> {
    let input = input.trim();
    let i = input
        .find(|c: char| !c.is_numeric() && c != '.')
        .unwrap_or_else(|| input.len());
    let number = input[..i].parse::<F>()?;
    Ok((number, input[i..].trim()))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23", None).unwrap(),
            TemperatureValue::new(23.0, TemperatureUnits::Celsius)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23.3", None).unwrap(),
            TemperatureValue::new(23.3, TemperatureUnits::Celsius)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23C", None).unwrap(),
            TemperatureValue::new(23.0, TemperatureUnits::Celsius)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale(" 23 C ", None).unwrap(),
            TemperatureValue::new(23.0, TemperatureUnits::Celsius)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23C", Some(TemperatureScale::Farenheit))
                .unwrap(),
            TemperatureValue::new(23.0, TemperatureUnits::Celsius)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23", Some(TemperatureScale::Farenheit))
                .unwrap(),
            TemperatureValue::new(23.0, TemperatureUnits::Farenheit)
        );
        assert_eq!(
            TemperatureValue::parse_with_optional_scale("23frogs", None)
                .unwrap_err()
                .to_string(),
            "Unknown temperature scale frogs"
        );
    }

    #[test]
    fn display() {
        assert_eq!(
            TemperatureValue::new(22.0, TemperatureUnits::Celsius).to_string(),
            "22°C"
        );
        assert_eq!(
            TemperatureValue::new(2200.0, TemperatureUnits::CelsiusTimes100).to_string(),
            "22°C"
        );
    }

    #[test]
    fn value_conversion() {
        assert_eq!(
            TemperatureValue::new(76., TemperatureUnits::Farenheit)
                .as_celsius()
                .floor(),
            24.
        );
        assert_eq!(
            TemperatureValue::new(24.444, TemperatureUnits::Celsius)
                .as_farenheit()
                .ceil(),
            76.
        );
        assert_eq!(
            TemperatureValue::new(76., TemperatureUnits::Farenheit)
                .as_unit(TemperatureUnits::FarenheitTimes100)
                .value,
            7600.
        );
        assert_eq!(
            TemperatureValue::new(24., TemperatureUnits::Celsius)
                .as_unit(TemperatureUnits::CelsiusTimes100)
                .value,
            2400.
        );
        assert_eq!(
            TemperatureValue::new(2400., TemperatureUnits::CelsiusTimes100)
                .as_unit(TemperatureUnits::Celsius)
                .value,
            24.
        );
    }
}
