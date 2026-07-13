use crate::error::Error;
use crate::value::{Color, Value};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Percent,
    IntRange { min: i32, max: i32, step: i32, unit: &'static str },
    Enum(&'static [&'static str]),
    Toggle { off: &'static str, on: &'static str },
    TextField { max_len: usize },
    RgbStrip { leds: usize },
    Curve,
    Action,
}

impl Kind {
    pub fn parse(&self, raw: &str) -> Result<Value, Error> {
        let raw = raw.trim();
        match self {
            Kind::Percent => {
                let n: i32 = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if !(0..=100).contains(&n) {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Percent(n as u8))
            }
            Kind::IntRange { min, max, .. } => {
                let n: i32 = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if n < *min || n > *max {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Int(n))
            }
            Kind::Enum(variants) => {
                let n: usize = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if n >= variants.len() {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Enum(n as u8))
            }
            Kind::Toggle { .. } => match raw {
                "0" => Ok(Value::Bool(false)),
                "1" => Ok(Value::Bool(true)),
                _ => Err(Error::Parse(raw.into())),
            },
            Kind::TextField { .. } => Ok(Value::Text(raw.to_string())),
            Kind::RgbStrip { leds } => {
                let cs: Result<Vec<Color>, Error> =
                    raw.split_whitespace().map(Color::from_hex).collect();
                let cs = cs?;
                if cs.len() != *leds {
                    return Err(Error::Invalid);
                }
                Ok(Value::Rgb(cs))
            }
            Kind::Curve => {
                if raw == "reset" || raw.is_empty() || raw.contains("built-in") {
                    return Ok(Value::Curve(vec![]));
                }
                let mut pts = Vec::new();
                for tok in raw.split_whitespace() {
                    let (a, b) = tok.split_once(':').ok_or(Error::Parse(tok.into()))?;
                    let inp: u16 = a.parse().map_err(|_| Error::Parse(tok.into()))?;
                    let out: u16 = b.parse().map_err(|_| Error::Parse(tok.into()))?;
                    pts.push((inp, out));
                }
                Ok(Value::Curve(pts))
            }
            Kind::Action => Ok(Value::Trigger),
        }
    }

    pub fn format(&self, v: &Value) -> Result<String, Error> {
        Ok(match (self, v) {
            (Kind::Percent, Value::Percent(n)) => n.to_string(),
            (Kind::IntRange { .. }, Value::Int(n)) => n.to_string(),
            (Kind::Enum(_), Value::Enum(n)) => n.to_string(),
            (Kind::Toggle { .. }, Value::Bool(b)) => (if *b { "1" } else { "0" }).into(),
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => {
                cs.iter().map(Color::to_hex).collect::<Vec<_>>().join(" ")
            }
            (Kind::Curve, Value::Curve(pts)) => {
                if pts.is_empty() {
                    "reset".into()
                } else {
                    pts.iter().map(|(a, b)| format!("{a}:{b}")).collect::<Vec<_>>().join(" ")
                }
            }
            (Kind::Action, Value::Trigger) => "1".into(),
            _ => return Err(Error::Invalid),
        })
    }

    pub fn validate(&self, v: &Value) -> Result<(), Error> {
        // parse(format(v)) proves the value satisfies this kind's constraints.
        let s = self.format(v)?;
        match self {
            Kind::Action => Ok(()),
            _ => self.parse(&s).map(|_| ()),
        }
    }

    /// Human-readable rendering of a value for display.
    pub fn display(&self, v: &Value) -> String {
        match (self, v) {
            (Kind::Percent, Value::Percent(n)) => format!("{n}%"),
            (Kind::IntRange { unit, .. }, Value::Int(n)) => format!("{n} {unit}"),
            (Kind::Enum(variants), Value::Enum(n)) => variants
                .get(*n as usize)
                .map(|s| s.to_string())
                .unwrap_or_else(|| n.to_string()),
            (Kind::Toggle { off, on }, Value::Bool(b)) => {
                (if *b { *on } else { *off }).to_string()
            }
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => format!("{} LEDs", cs.len()),
            (Kind::Curve, Value::Curve(p)) if p.is_empty() => "built-in".into(),
            (Kind::Curve, Value::Curve(p)) => format!("{} points", p.len()),
            (Kind::Action, _) => "[trigger]".into(),
            _ => "?".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Color;

    #[test]
    fn percent_roundtrip_and_bounds() {
        let k = Kind::Percent;
        assert_eq!(k.parse("50\n").unwrap(), Value::Percent(50));
        assert_eq!(k.format(&Value::Percent(50)).unwrap(), "50");
        assert!(k.validate(&Value::Percent(100)).is_ok());
        assert!(matches!(k.parse("250"), Err(Error::OutOfRange)));
    }

    #[test]
    fn intrange_range() {
        let k = Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" };
        assert_eq!(k.parse("900").unwrap(), Value::Int(900));
        assert_eq!(k.format(&Value::Int(900)).unwrap(), "900");
        assert!(matches!(k.parse("45"), Err(Error::OutOfRange)));
        assert!(matches!(k.validate(&Value::Int(2701)), Err(Error::OutOfRange)));
    }

    #[test]
    fn enum_index() {
        let k = Kind::Enum(&["kf", "tf"]);
        assert_eq!(k.parse("1").unwrap(), Value::Enum(1));
        assert_eq!(k.format(&Value::Enum(1)).unwrap(), "1");
        assert!(matches!(k.parse("2"), Err(Error::OutOfRange)));
    }

    #[test]
    fn toggle() {
        let k = Kind::Toggle { off: "off", on: "on" };
        assert_eq!(k.parse("1").unwrap(), Value::Bool(true));
        assert_eq!(k.format(&Value::Bool(false)).unwrap(), "0");
    }

    #[test]
    fn rgb_strip_ten_colors() {
        let k = Kind::RgbStrip { leds: 10 };
        let raw = "ff0000 00ff00 0000ff ffffff 000000 111111 222222 333333 444444 555555";
        let v = k.parse(raw).unwrap();
        if let Value::Rgb(cs) = &v {
            assert_eq!(cs.len(), 10);
            assert_eq!(cs[0], Color { r: 255, g: 0, b: 0 });
        } else {
            panic!("not rgb");
        }
        assert_eq!(k.format(&v).unwrap(), raw);
        assert!(matches!(k.parse("ff0000"), Err(Error::Invalid))); // wrong count
    }

    #[test]
    fn curve_reset_and_pairs() {
        let k = Kind::Curve;
        assert_eq!(k.parse("reset").unwrap(), Value::Curve(vec![]));
        assert_eq!(k.format(&Value::Curve(vec![])).unwrap(), "reset");
        let v = k.parse("0:0 32768:16384 65535:65535").unwrap();
        assert_eq!(v, Value::Curve(vec![(0, 0), (32768, 16384), (65535, 65535)]));
        assert_eq!(k.format(&v).unwrap(), "0:0 32768:16384 65535:65535");
    }
}
