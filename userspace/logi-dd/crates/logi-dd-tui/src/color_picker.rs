//! The modal LED color picker for `wheel_led_colors` (the LIGHTSYNC
//! slot's 10-LED strip). It replaces the old raw 10-hex-value line editor
//! with something a person can actually use: a cursor over the 10 LEDs, a
//! 16-swatch palette to paint from, mirror-pair and paint-all shortcuts,
//! and a per-LED 6-hex-digit entry as the precision fallback. Nothing is
//! written until `w` commits; Esc cancels without touching the wheel.
//!
//! The state machine lives here so tests can drive it key by key; the
//! rendering lives in `ui::draw_color_picker` and the commit (with the
//! mirrored-direction write rule) in `App::commit_color_picker`.

use logi_dd_core::lightsync;
use logi_dd_core::{Color, Value};

/// The palette's swatches: broad hue coverage plus white/warm white for
/// number plates and off/black for gaps, matching what the wheel's strip
/// renders well. Navigated as a `PALETTE_COLS`-wide grid.
pub const PALETTE: [(&str, Color); 16] = [
    ("White", Color { r: 0xff, g: 0xff, b: 0xff }),
    ("Warm white", Color { r: 0xff, g: 0xd8, b: 0xa8 }),
    ("Red", Color { r: 0xff, g: 0x00, b: 0x00 }),
    ("Orange", Color { r: 0xff, g: 0x80, b: 0x00 }),
    ("Amber", Color { r: 0xff, g: 0xbf, b: 0x00 }),
    ("Yellow", Color { r: 0xff, g: 0xff, b: 0x00 }),
    ("Lime", Color { r: 0x80, g: 0xff, b: 0x00 }),
    ("Green", Color { r: 0x00, g: 0xff, b: 0x00 }),
    ("Teal", Color { r: 0x00, g: 0xff, b: 0xaa }),
    ("Cyan", Color { r: 0x00, g: 0xff, b: 0xff }),
    ("Blue", Color { r: 0x00, g: 0x00, b: 0xff }),
    ("Indigo", Color { r: 0x40, g: 0x00, b: 0xff }),
    ("Violet", Color { r: 0x80, g: 0x00, b: 0xff }),
    ("Magenta", Color { r: 0xff, g: 0x00, b: 0xff }),
    ("Pink", Color { r: 0xff, g: 0x69, b: 0xb4 }),
    ("Off", Color { r: 0x00, g: 0x00, b: 0x00 }),
];

/// The palette grid's width: 16 swatches as two rows of 8.
pub const PALETTE_COLS: usize = 8;

/// Which half of the picker the arrow keys act on; Tab toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFocus {
    Leds,
    Palette,
}

/// What a key did to the picker; the app reacts to the terminal ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Still editing.
    Open,
    /// `w`: write the strip to the wheel and close.
    Commit,
    /// Esc at the top level: close without writing.
    Cancel,
}

pub struct ColorPicker {
    /// The working copy of the strip; nothing reaches the device until
    /// the commit.
    pub colors: Vec<Color>,
    /// The LED the paint keys act on.
    pub cursor: usize,
    /// The selected palette swatch (an index into `PALETTE`).
    pub palette: usize,
    pub focus: PickerFocus,
    /// The `x` hex entry's draft, `Some` while it is open: up to 6 hex
    /// digits for the cursor LED, Enter applies, Esc backs out to the
    /// picker (not the view).
    pub hex: Option<String>,
}

impl ColorPicker {
    /// Seed from the attribute's current value; `None` when it does not
    /// read as an RGB strip (the caller falls back to reporting).
    pub fn from_value(v: &Value) -> Option<ColorPicker> {
        match v {
            Value::Rgb(colors) if !colors.is_empty() => Some(ColorPicker {
                colors: colors.clone(),
                cursor: 0,
                palette: 0,
                focus: PickerFocus::Leds,
                hex: None,
            }),
            _ => None,
        }
    }

    /// The 10 hex values the commit would write, for the live preview
    /// line (and the tests pinning the write format).
    pub fn preview(&self) -> String {
        self.colors.iter().map(Color::to_hex).collect::<Vec<_>>().join(" ")
    }

    /// Paint the cursor LED with the selected swatch.
    fn paint(&mut self) {
        self.colors[self.cursor] = PALETTE[self.palette].1;
    }

    /// Paint every LED with the selected swatch.
    fn paint_all(&mut self) {
        let c = PALETTE[self.palette].1;
        for led in &mut self.colors {
            *led = c;
        }
    }

    /// Paint the cursor LED and its mirror-pair partner (pairs 1-10, 2-9,
    /// 3-8, 4-7, 5-6), matching the strip's mirrored-direction model.
    fn paint_pair(&mut self) {
        self.paint();
        let mirror = lightsync::mirror_index(self.cursor);
        if mirror < self.colors.len() {
            self.colors[mirror] = PALETTE[self.palette].1;
        }
    }

    /// One key while the picker is open. The hex entry consumes
    /// everything while active; Esc there backs out one level only.
    pub fn on_key(&mut self, key: crossterm::event::KeyCode) -> PickerOutcome {
        use crossterm::event::KeyCode::*;
        if let Some(draft) = self.hex.as_mut() {
            match key {
                Enter => {
                    if let Ok(c) = Color::from_hex(draft) {
                        self.colors[self.cursor] = c;
                        self.hex = None;
                    }
                    // A non-6-hex-digit draft stays open for correction.
                }
                Esc => self.hex = None,
                Backspace => {
                    draft.pop();
                }
                Char(c) if c.is_ascii_hexdigit() && draft.chars().count() < 6 => {
                    draft.push(c.to_ascii_lowercase());
                }
                _ => {}
            }
            return PickerOutcome::Open;
        }
        let n = self.colors.len();
        match key {
            Tab => {
                self.focus = match self.focus {
                    PickerFocus::Leds => PickerFocus::Palette,
                    PickerFocus::Palette => PickerFocus::Leds,
                };
            }
            Left if self.focus == PickerFocus::Leds => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            Right if self.focus == PickerFocus::Leds => {
                self.cursor = (self.cursor + 1).min(n - 1);
            }
            Home if self.focus == PickerFocus::Leds => self.cursor = 0,
            End if self.focus == PickerFocus::Leds => self.cursor = n - 1,
            Left if self.focus == PickerFocus::Palette => {
                self.palette = self.palette.saturating_sub(1);
            }
            Right if self.focus == PickerFocus::Palette => {
                self.palette = (self.palette + 1).min(PALETTE.len() - 1);
            }
            // Up/Down hop grid rows without changing column; the edge
            // rows hold still (no wrap, no column drift).
            Up if self.focus == PickerFocus::Palette => {
                if self.palette >= PALETTE_COLS {
                    self.palette -= PALETTE_COLS;
                }
            }
            Down if self.focus == PickerFocus::Palette => {
                if self.palette + PALETTE_COLS < PALETTE.len() {
                    self.palette += PALETTE_COLS;
                }
            }
            Enter => self.paint(),
            Char('a') => self.paint_all(),
            Char('p') => self.paint_pair(),
            Char('x') => self.hex = Some(self.colors[self.cursor].to_hex()),
            Char('w') => return PickerOutcome::Commit,
            Esc => return PickerOutcome::Cancel,
            _ => {}
        }
        PickerOutcome::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode::*;

    fn picker() -> ColorPicker {
        let colors = vec![Color { r: 0, g: 0, b: 0 }; 10];
        ColorPicker::from_value(&Value::Rgb(colors)).unwrap()
    }

    #[test]
    fn seeds_only_from_an_rgb_value() {
        assert!(ColorPicker::from_value(&Value::Text("x".into())).is_none());
        assert!(ColorPicker::from_value(&Value::Rgb(vec![])).is_none());
        let p = picker();
        assert_eq!(p.cursor, 0);
        assert_eq!(p.focus, PickerFocus::Leds);
    }

    #[test]
    fn led_cursor_moves_and_clamps_with_home_and_end() {
        let mut p = picker();
        assert_eq!(p.on_key(Left), PickerOutcome::Open);
        assert_eq!(p.cursor, 0, "clamps at the first LED");
        p.on_key(Right);
        assert_eq!(p.cursor, 1);
        p.on_key(End);
        assert_eq!(p.cursor, 9);
        p.on_key(Right);
        assert_eq!(p.cursor, 9, "clamps at the last LED");
        p.on_key(Home);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn tab_moves_the_arrows_to_the_palette_grid() {
        let mut p = picker();
        p.on_key(Tab);
        assert_eq!(p.focus, PickerFocus::Palette);
        p.on_key(Right);
        p.on_key(Right);
        assert_eq!(p.palette, 2, "Right walks the swatches");
        p.on_key(Down);
        assert_eq!(p.palette, 2 + PALETTE_COLS, "Down drops one grid row");
        p.on_key(Up);
        assert_eq!(p.palette, 2);
        p.on_key(Up);
        assert_eq!(p.palette, 2, "clamps at the top row");
        let led = p.cursor;
        p.on_key(Left);
        assert_eq!(p.cursor, led, "the LED cursor holds still meanwhile");
        p.on_key(Tab);
        assert_eq!(p.focus, PickerFocus::Leds);
    }

    #[test]
    fn enter_paints_the_cursor_led_with_the_selected_swatch() {
        let mut p = picker();
        p.on_key(Tab);
        p.on_key(Right);
        p.on_key(Right); // Red
        p.on_key(Tab);
        p.on_key(Right); // LED 2
        p.on_key(Enter);
        assert_eq!(p.colors[1].to_hex(), "ff0000");
        assert_eq!(p.colors[0].to_hex(), "000000", "only the cursor LED");
    }

    #[test]
    fn a_paints_all_and_p_paints_the_mirror_pair() {
        let mut p = picker();
        p.palette = 2; // Red
        p.cursor = 2; // LED 3
        p.on_key(Char('p'));
        assert_eq!(p.colors[2].to_hex(), "ff0000");
        assert_eq!(p.colors[7].to_hex(), "ff0000", "LED 8 is LED 3's mirror");
        assert_eq!(p.colors[0].to_hex(), "000000");
        p.palette = 10; // Blue
        p.on_key(Char('a'));
        assert!(p.colors.iter().all(|c| c.to_hex() == "0000ff"), "a paints all 10");
    }

    #[test]
    fn hex_entry_edits_one_led_and_esc_backs_out_one_level() {
        let mut p = picker();
        p.cursor = 4;
        p.on_key(Char('x'));
        assert_eq!(p.hex.as_deref(), Some("000000"), "seeded with the LED's value");
        for _ in 0..6 {
            p.on_key(Backspace);
        }
        for c in "1A2b3C".chars() {
            p.on_key(Char(c));
        }
        assert_eq!(p.hex.as_deref(), Some("1a2b3c"), "hex digits, lowercased, max 6");
        p.on_key(Char('z'));
        assert_eq!(p.hex.as_deref(), Some("1a2b3c"), "non-hex input is refused");
        assert_eq!(p.on_key(Enter), PickerOutcome::Open);
        assert!(p.hex.is_none());
        assert_eq!(p.colors[4].to_hex(), "1a2b3c");
        // Esc inside the entry closes the entry, not the picker.
        p.on_key(Char('x'));
        assert_eq!(p.on_key(Esc), PickerOutcome::Open);
        assert!(p.hex.is_none());
        // A short draft never applies; Enter keeps it open to fix.
        p.on_key(Char('x'));
        for _ in 0..6 {
            p.on_key(Backspace);
        }
        p.on_key(Char('f'));
        p.on_key(Enter);
        assert_eq!(p.hex.as_deref(), Some("f"), "an invalid draft stays for correction");
        assert_eq!(p.colors[4].to_hex(), "1a2b3c", "nothing applied");
    }

    #[test]
    fn w_commits_and_esc_cancels_with_the_write_format_pinned() {
        let mut p = picker();
        p.palette = 2; // Red
        p.on_key(Enter);
        assert_eq!(
            p.preview(),
            "ff0000 000000 000000 000000 000000 000000 000000 000000 000000 000000",
            "the preview is exactly the sysfs write format"
        );
        assert_eq!(p.on_key(Char('w')), PickerOutcome::Commit);
        assert_eq!(p.on_key(Esc), PickerOutcome::Cancel);
    }
}
