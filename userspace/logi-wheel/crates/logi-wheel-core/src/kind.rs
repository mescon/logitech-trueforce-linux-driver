use crate::error::Error;
use crate::value::{Color, Value};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Percent,
    IntRange { min: i32, max: i32, step: i32, unit: &'static str },
    Enum(&'static [&'static str]),
    /// A raw sysfs integer 0..=`raw_max` that is shown and edited as a
    /// rounded percent 0-100, using the same slider widget and "N%" display
    /// as [`Kind::Percent`] (see `bridge::kind_tag`/`to_setting_row`, whose
    /// `Kind::Percent` branches already cover this variant). The sysfs unit
    /// itself stays raw: the G923's classic `gain`/`autocenter` attrs (0-
    /// 65535) keep that wire format for Oversteer compatibility, but nobody
    /// wants to type "42848" when "65%" means the same thing.
    ///
    /// Rounding is integer round-half-up both ways
    /// (`raw = round(pct * raw_max / 100)`, `pct = round(raw * 100 /
    /// raw_max)`), which round-trips exactly for every percent 0-100 at
    /// `raw_max = 65535` (the only value in use): writing a given percent
    /// and reading it back always shows the same percent again.
    ScaledPercent { raw_max: u32 },
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
    /// A read-only "<rpm> <max_rpm> <age_ms>" diagnostic line
    /// (`wheel_texture_rpm`): parses into [`Value::RpmFeed`]. `display`
    /// shows the live rpm while the reading is fresh and "no telemetry"
    /// once it has gone stale, using a 1 s window - a UI freshness check,
    /// distinct from the driver's own 200 ms texture-merge staleness gate
    /// (see `docs/SYSFS_API.md`'s `wheel_tf_merge` section).
    RpmFeed,
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
            Kind::ScaledPercent { raw_max } => {
                if *raw_max == 0 {
                    return Err(Error::Invalid);
                }
                let n: i64 = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if n < 0 || n > i64::from(*raw_max) {
                    return Err(Error::OutOfRange);
                }
                let pct = (n as u64 * 100 + u64::from(*raw_max / 2)) / u64::from(*raw_max);
                Ok(Value::Percent(pct as u8))
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
                // The driver's read format is
                // "<loaded>/<max> points loaded (0 = built-in curve)". The
                // "built-in" in it is the legend and is present either way,
                // so the count is what has to be read, not the words: 0 means
                // no curve, anything else means a curve whose points this
                // attribute cannot give back.
                if let Some((loaded, _)) = parse_points_loaded(raw) {
                    return Ok(if loaded == 0 {
                        Value::Curve(vec![])
                    } else {
                        Value::CurveLoaded(loaded)
                    });
                }
                if raw == "reset" || raw.is_empty() {
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
            Kind::RpmFeed => {
                let mut it = raw.split_whitespace();
                let field = |it: &mut std::str::SplitWhitespace| -> Result<u32, Error> {
                    it.next()
                        .ok_or_else(|| Error::Parse(raw.into()))?
                        .parse()
                        .map_err(|_| Error::Parse(raw.into()))
                };
                let rpm = field(&mut it)?;
                let max_rpm = field(&mut it)?;
                let age_ms = field(&mut it)?;
                Ok(Value::RpmFeed { rpm, max_rpm, age_ms })
            }
        }
    }

    /// Encode a value to its sysfs string. Does NOT enforce Kind constraints
    /// (range, length, count); call `validate` first for outside input.
    pub fn format(&self, v: &Value) -> Result<String, Error> {
        Ok(match (self, v) {
            (Kind::Percent, Value::Percent(n)) => n.to_string(),
            (Kind::ScaledPercent { raw_max }, Value::Percent(n)) => {
                let raw = (u64::from(*n) * u64::from(*raw_max) + 50) / 100;
                raw.to_string()
            }
            (Kind::IntRange { .. }, Value::Int(n)) => n.to_string(),
            (Kind::Enum(_), Value::Enum(n)) => n.to_string(),
            (Kind::Toggle { .. }, Value::Bool(b)) => (if *b { "1" } else { "0" }).into(),
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => {
                cs.iter().map(Color::to_hex).collect::<Vec<_>>().join(" ")
            }
            // A loaded curve whose points cannot be read must not be
            // formatted. Callers that persist settings treat an error as
            // "skip this attribute", which leaves the wheel's curve alone;
            // emitting "reset" here is what silently erased it.
            (Kind::Curve, Value::CurveLoaded(n)) => {
                return Err(Error::Parse(format!(
                    "{n} curve points are loaded on the wheel but the attribute \
                     cannot read them back, so this curve cannot be reproduced"
                )))
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
            (Kind::RpmFeed, Value::RpmFeed { rpm, max_rpm, age_ms }) => {
                format!("{rpm} {max_rpm} {age_ms}")
            }
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
            (Kind::Percent, Value::Percent(n)) | (Kind::ScaledPercent { .. }, Value::Percent(n)) => {
                format!("{n}%")
            }
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
            (Kind::Curve, Value::CurveLoaded(n)) => format!("{n} points"),
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
            // Fresh under 1 s: show the live number. Stale (or never fed):
            // say so plainly rather than showing a frozen/zero rpm that
            // reads as real data.
            (Kind::RpmFeed, Value::RpmFeed { rpm, age_ms, .. }) => {
                if *age_ms < 1000 { format!("{rpm} rpm") } else { "no telemetry".into() }
            }
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
    fn scaled_percent_roundtrips_key_percents_at_the_g923s_span() {
        // raw = round(pct * 65535 / 100), pct = round(raw * 100 / 65535):
        // writing any of these percents and reading the resulting raw value
        // back must show the same percent again.
        let k = Kind::ScaledPercent { raw_max: 65535 };
        for pct in [0u8, 1, 50, 99, 100] {
            let raw = k.format(&Value::Percent(pct)).unwrap();
            let back = k.parse(&raw).unwrap();
            assert_eq!(back, Value::Percent(pct), "percent {pct} round-trips via raw {raw}");
        }
        // The exact raw values at each end and the midpoint.
        assert_eq!(k.format(&Value::Percent(0)).unwrap(), "0");
        assert_eq!(k.format(&Value::Percent(100)).unwrap(), "65535");
        assert_eq!(k.parse("65535").unwrap(), Value::Percent(100));
        assert_eq!(k.parse("0").unwrap(), Value::Percent(0));
    }

    #[test]
    fn scaled_percent_rounds_raw_values_that_fall_between_percents() {
        let k = Kind::ScaledPercent { raw_max: 65535 };
        // 327 is just under the 1% boundary (655.35 raw per percent), 328 is
        // just over it into 1%.
        assert_eq!(k.parse("327").unwrap(), Value::Percent(0));
        assert_eq!(k.parse("328").unwrap(), Value::Percent(1));
        // Either side of the exact midpoint (32767.5) rounds to 50%.
        assert_eq!(k.parse("32767").unwrap(), Value::Percent(50));
        assert_eq!(k.parse("32768").unwrap(), Value::Percent(50));
        // One below the top raw value still reads back as 100%.
        assert_eq!(k.parse("65534").unwrap(), Value::Percent(100));
    }

    #[test]
    fn scaled_percent_bounds_and_display() {
        let k = Kind::ScaledPercent { raw_max: 65535 };
        assert!(matches!(k.parse("65536"), Err(Error::OutOfRange)));
        assert!(matches!(k.parse("-1"), Err(Error::OutOfRange)));
        assert!(matches!(k.parse("nope"), Err(Error::Parse(_))));
        assert_eq!(k.display(&Value::Percent(65)), "65%");
        assert!(k.validate(&Value::Percent(42)).is_ok());
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

    /// The driver always prints "(0 = built-in curve)" as a legend, whatever
    /// the actual count. Reading the words instead of the number made a
    /// loaded curve look like no curve, and a computer profile then recorded
    /// it as `reset` and wiped it on the next apply.
    #[test]
    fn a_loaded_curve_is_not_mistaken_for_the_built_in_one() {
        let k = Kind::Curve;

        // Nothing loaded: genuinely the built-in curve.
        assert_eq!(
            k.parse("0/64 points loaded (0 = built-in curve)\n").unwrap(),
            Value::Curve(vec![])
        );

        // Loaded: the count survives, and the legend does not fool it.
        for n in [1u16, 17, 64] {
            let raw = format!("{n}/64 points loaded (0 = built-in curve)\n");
            assert_eq!(k.parse(&raw).unwrap(), Value::CurveLoaded(n), "{raw:?}");
            assert_eq!(k.display(&Value::CurveLoaded(n)), format!("{n} points"));
        }
    }

    /// The data-loss step itself: a curve that cannot be read back must not
    /// be formatted into a profile at all. Profile save treats a formatting
    /// error as "skip this attribute", which leaves the wheel's own curve
    /// untouched; emitting "reset" is what erased it.
    #[test]
    fn an_unreadable_curve_refuses_to_be_written_to_a_profile() {
        let k = Kind::Curve;
        assert!(k.format(&Value::CurveLoaded(64)).is_err());
        // An empty curve still means "reset", which is a real user intent.
        assert_eq!(k.format(&Value::Curve(vec![])).unwrap(), "reset");
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

    #[test]
    fn rpm_feed_parse_format_roundtrip() {
        let k = Kind::RpmFeed;
        assert_eq!(
            k.parse("6500 14000 12").unwrap(),
            Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 12 }
        );
        let v = Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 12 };
        assert_eq!(k.format(&v).unwrap(), "6500 14000 12");
        assert!(k.validate(&v).is_ok());
    }

    #[test]
    fn rpm_feed_rejects_malformed_input() {
        let k = Kind::RpmFeed;
        assert!(matches!(k.parse(""), Err(Error::Parse(_))));
        assert!(matches!(k.parse("6500"), Err(Error::Parse(_))));
        assert!(matches!(k.parse("6500 14000"), Err(Error::Parse(_))));
        assert!(matches!(k.parse("a 14000 12"), Err(Error::Parse(_))));
    }

    /// The 1 s freshness window that decides between a live number and "no
    /// telemetry": strictly under 1000 ms is fresh, 1000 ms and up is stale.
    /// Deliberately its own threshold, not the driver's 200 ms merge-gating
    /// window (`wheel_tf_merge` can go quiet on stale data well before this
    /// status line does).
    #[test]
    fn rpm_feed_display_switches_on_the_one_second_staleness_window() {
        let k = Kind::RpmFeed;
        assert_eq!(
            k.display(&Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 0 }),
            "6500 rpm"
        );
        assert_eq!(
            k.display(&Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 999 }),
            "6500 rpm"
        );
        assert_eq!(
            k.display(&Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 1000 }),
            "no telemetry"
        );
        assert_eq!(
            k.display(&Value::RpmFeed { rpm: 0, max_rpm: 0, age_ms: 60_000 }),
            "no telemetry"
        );
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

/// Parse the driver's `"<loaded>/<max> points loaded ..."` curve read, or
/// `None` when `raw` is not that format (a profile file's own `a:b c:d` list,
/// or `reset`).
fn parse_points_loaded(raw: &str) -> Option<(u16, u16)> {
    let head = raw.split_whitespace().next()?;
    let (loaded, max) = head.split_once('/')?;
    Some((loaded.trim().parse().ok()?, max.trim().parse().ok()?))
}

