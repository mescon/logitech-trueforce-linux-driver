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
    Trigger,
    /// Every slot name, as read back (index 0 = slot 1).
    SlotNames(Vec<String>),
    /// Rename one slot. The attribute reads back as the whole list but writes
    /// one slot at a time, so reads yield `SlotNames` and writes take this.
    SlotName { slot: u8, name: String },
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
