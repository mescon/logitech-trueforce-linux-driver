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
    /// Two percent values `"lower upper"` whose sum must not exceed `max`
    /// (a pedal deadzone: dead travel at each end). Yields `Value::Pair`.
    Pair { max: u8 },
    Action,
    /// An attribute that reads back as a `N: name` list but is written one
    /// slot at a time as `N:name` (the onboard profile names). Reads yield
    /// `Value::SlotNames`, writes take `Value::SlotName`.
    SlotText { slots: u8, max_len: usize },
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
            Kind::TextField { max_len } => {
                if raw.chars().count() > *max_len {
                    return Err(Error::Invalid);
                }
                Ok(Value::Text(raw.to_string()))
            }
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
            Kind::Pair { max } => {
                let mut it = raw.split_whitespace();
                let lower = it.next().ok_or_else(|| Error::Parse(raw.into()))?;
                let upper = it.next().ok_or_else(|| Error::Parse(raw.into()))?;
                if it.next().is_some() {
                    return Err(Error::Parse(raw.into()));
                }
                let lower: u8 = lower.parse().map_err(|_| Error::Parse(raw.into()))?;
                let upper: u8 = upper.parse().map_err(|_| Error::Parse(raw.into()))?;
                if lower as u16 + upper as u16 > *max as u16 {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Pair(lower, upper))
            }
            Kind::Action => Ok(Value::Trigger),
            Kind::SlotText { slots, .. } => {
                // Reads back one "N: name" line per slot. Unlisted slots stay
                // empty rather than failing the whole read.
                let mut names = vec![String::new(); *slots as usize];
                for line in raw.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let (n, rest) = line.split_once(':').ok_or(Error::Parse(line.into()))?;
                    let idx: usize =
                        n.trim().parse().map_err(|_| Error::Parse(line.into()))?;
                    if idx >= 1 && idx <= *slots as usize {
                        names[idx - 1] = rest.trim().to_string();
                    }
                }
                Ok(Value::SlotNames(names))
            }
        }
    }

    /// Encode a value to its sysfs string. Does NOT enforce Kind constraints
    /// (range, length, count); call `validate` first for outside input.
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
            (Kind::Pair { .. }, Value::Pair(lo, hi)) => format!("{lo} {hi}"),
            (Kind::Action, Value::Trigger) => "1".into(),
            // Writes rename a single slot; the whole list is not writable.
            (Kind::SlotText { .. }, Value::SlotName { slot, name }) => format!("{slot}:{name}"),
            _ => return Err(Error::Invalid),
        })
    }

    pub fn validate(&self, v: &Value) -> Result<(), Error> {
        // SlotText reads and writes different shapes, so the parse(format(v))
        // round-trip below does not apply: check the write form directly.
        if let Kind::SlotText { slots, max_len } = self {
            return match v {
                Value::SlotName { slot, name } => {
                    let len = name.chars().count();
                    if *slot >= 1 && *slot <= *slots && len >= 1 && len <= *max_len
                        && !name.contains('\n')
                    {
                        Ok(())
                    } else {
                        Err(Error::Invalid)
                    }
                }
                _ => Err(Error::Invalid),
            };
        }
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
            // Collapse newlines so a multi-line value (e.g. the two-part
            // firmware string) renders on the single line the TUI gives it.
            (Kind::TextField { .. }, Value::Text(s)) => s.replace('\n', " / "),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => format!("{} LEDs", cs.len()),
            (Kind::Curve, Value::Curve(p)) if p.is_empty() => "built-in".into(),
            (Kind::Curve, Value::Curve(p)) => format!("{} points", p.len()),
            (Kind::Pair { .. }, Value::Pair(lo, hi)) if *lo == 0 && *hi == 0 => "none".into(),
            (Kind::Pair { .. }, Value::Pair(lo, hi)) => format!("{lo}% / {hi}%"),
            (Kind::Action, _) => "[trigger]".into(),
            (Kind::SlotText { .. }, Value::SlotNames(names)) => names
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{}: {}", i + 1, n))
                .collect::<Vec<_>>()
                .join("  "),
            (Kind::SlotText { .. }, Value::SlotName { slot, name }) => format!("{slot}: {name}"),
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

    #[test]
    fn textfield_max_len_enforced() {
        let k = Kind::TextField { max_len: 8 };
        assert!(k.parse("RACE").is_ok());
        assert!(matches!(k.parse("waytoolongname"), Err(Error::Invalid)));
    }

    #[test]
    fn pair_parse_format_validate() {
        let k = Kind::Pair { max: 99 };
        assert_eq!(k.parse("8 5").unwrap(), Value::Pair(8, 5));
        assert_eq!(k.parse("0 0").unwrap(), Value::Pair(0, 0));
        assert_eq!(k.format(&Value::Pair(8, 5)).unwrap(), "8 5");
        assert!(k.validate(&Value::Pair(50, 49)).is_ok()); // sum 99 exactly
        // sum over max is rejected
        assert!(matches!(k.parse("60 50"), Err(Error::OutOfRange)));
        assert!(matches!(k.validate(&Value::Pair(60, 50)), Err(Error::OutOfRange)));
        // shape errors
        assert!(matches!(k.parse("8"), Err(Error::Parse(_))));
        assert!(matches!(k.parse("8 5 3"), Err(Error::Parse(_))));
        assert!(matches!(k.parse("a b"), Err(Error::Parse(_))));
    }

    #[test]
    fn pair_display() {
        let k = Kind::Pair { max: 99 };
        assert_eq!(k.display(&Value::Pair(0, 0)), "none");
        assert_eq!(k.display(&Value::Pair(8, 5)), "8% / 5%");
    }
}

#[cfg(test)]
mod slot_text_tests {
    use super::*;

    const K: Kind = Kind::SlotText { slots: 5, max_len: 9 };

    #[test]
    fn parses_the_drivers_list_read() {
        let v = K.parse("1: QZX7\n2: GT7\n3: PROFILE 3\n4: PROFILE 4\n5: TEST").unwrap();
        assert_eq!(
            v,
            Value::SlotNames(vec![
                "QZX7".into(),
                "GT7".into(),
                "PROFILE 3".into(),
                "PROFILE 4".into(),
                "TEST".into(),
            ])
        );
    }

    #[test]
    fn missing_slots_read_back_empty() {
        let v = K.parse("2: GT7").unwrap();
        let Value::SlotNames(names) = v else { panic!("wrong variant") };
        assert_eq!(names.len(), 5);
        assert_eq!(names[1], "GT7");
        assert_eq!(names[0], "");
    }

    #[test]
    fn writes_one_slot_as_n_colon_name() {
        let w = Value::SlotName { slot: 3, name: "My Profile".into() };
        assert_eq!(K.format(&w).unwrap(), "3:My Profile");
    }

    #[test]
    fn the_whole_list_is_not_writable() {
        // Reads yield SlotNames; writing it back would send the list to a
        // store that takes a single "N:name".
        assert!(K.format(&Value::SlotNames(vec!["a".into()])).is_err());
        assert!(K.validate(&Value::SlotNames(vec!["a".into()])).is_err());
    }

    #[test]
    fn validate_enforces_the_drivers_limits() {
        let ok = |s: u8, n: &str| K.validate(&Value::SlotName { slot: s, name: n.into() });
        assert!(ok(1, "A").is_ok());
        assert!(ok(5, "PROFILE 3").is_ok()); // 9 = the wheel's own stock name
        assert!(ok(0, "A").is_err(), "slot 0 is below the 1-5 range");
        assert!(ok(6, "A").is_err(), "slot 6 is above the 1-5 range");
        assert!(ok(1, "").is_err(), "empty name is rejected by the driver");
        // The wheel rejects >9 with -EIO (verified on an RS50); 14 is only the
        // HID++ payload cap, so cap at what the hardware actually takes.
        assert!(ok(1, "ABCDEFGHIJ").is_err(), "10 chars is refused by the wheel");
        assert!(ok(1, "two\nlines").is_err());
    }

    #[test]
    fn name_may_contain_spaces_and_colons() {
        // The driver splits on the FIRST colon only, so both survive.
        assert!(K.validate(&Value::SlotName { slot: 2, name: "GT7: race".into() }).is_ok());
        assert_eq!(
            K.format(&Value::SlotName { slot: 2, name: "GT7: race".into() }).unwrap(),
            "2:GT7: race"
        );
    }
}
