use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn to_hex(&self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
    pub fn from_hex(s: &str) -> Result<Color, Error> {
        let s = s.trim();
        // Guard is_ascii() so the byte-offset slicing below cannot land inside
        // a multi-byte UTF-8 char (which would panic).
        if s.len() != 6 || !s.is_ascii() {
            return Err(Error::Invalid);
        }
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| Error::Invalid);
        Ok(Color { r: byte(0)?, g: byte(2)?, b: byte(4)? })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Percent(u8),
    Int(i32),
    Enum(u8),
    Bool(bool),
    Text(String),
    Rgb(Vec<Color>),
    Curve(Vec<(u16, u16)>),
    /// A curve is loaded on the wheel, with this many points, but the points
    /// themselves are not readable back.
    ///
    /// The driver reports curves as `"<loaded>/<max> points loaded (0 =
    /// built-in curve)"`. The phrase "built-in" is part of that legend, not a
    /// statement about the current state, and treating any string containing
    /// it as "no curve" made a loaded curve indistinguishable from none. That
    /// mattered because a computer profile then recorded the attribute as
    /// `reset`, so applying the profile later wiped the curve the user had
    /// tuned. Keeping the count means "loaded" and "not loaded" are
    /// different values again, and the one that cannot be reproduced refuses
    /// to be written into a profile rather than being downgraded to `reset`.
    CurveLoaded(u16),
    /// A `(lower, upper)` percent pair, e.g. a pedal deadzone.
    Pair(u8, u8),
    Trigger,
    /// Every slot name, as read back (index 0 = slot 1).
    SlotNames(Vec<String>),
    /// Rename one slot. The attribute reads back as the whole list but writes
    /// one slot at a time, so reads yield `SlotNames` and writes take this.
    SlotName { slot: u8, name: String },
    /// `wheel_texture_rpm`'s "<rpm> <max_rpm> <age_ms>" diagnostic line.
    /// `age_ms` is how long ago the value was last written (by
    /// logi-rpm-bridge, normally), and is what a frontend's status line
    /// uses to decide between showing the live rpm and "no telemetry" (see
    /// `Kind::RpmFeed`'s `display`).
    RpmFeed { rpm: u32, max_rpm: u32, age_ms: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_hex_roundtrip() {
        let c = Color::from_hex("ff8000").unwrap();
        assert_eq!(c, Color { r: 0xff, g: 0x80, b: 0x00 });
        assert_eq!(c.to_hex(), "ff8000");
    }

    #[test]
    fn color_bad_hex_errors() {
        assert!(Color::from_hex("zz0000").is_err());
        assert!(Color::from_hex("fff").is_err());
    }

    #[test]
    fn color_non_ascii_six_bytes_errors() {
        // 6 bytes but 4 chars (a multi-byte char): must Err, not panic.
        assert!(Color::from_hex("ff\u{20ac}0").is_err());
    }
}
