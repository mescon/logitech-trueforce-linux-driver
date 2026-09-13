// SPDX-License-Identifier: GPL-2.0-only
use logi_wheel_core::{Error, Kind, Value};

pub struct EditState {
    pub attr: &'static str,
    pub kind: Kind,
    pub draft: Value,
    pub buffer: String,
    /// Which slot the edit targets (1-based). Only meaningful for
    /// `Kind::SlotText`, where bumping picks the slot and the buffer holds
    /// that slot's new name.
    pub slot: u8,
}

impl EditState {
    pub fn start(attr: &'static str, kind: Kind, current: &Value) -> EditState {
        // Text-mode kinds (free text, RGB strip, curve) are edited as their raw
        // sysfs string; seed the buffer with the current value's encoding.
        let buffer = match kind {
            Kind::TextField { .. } => match current {
                Value::Text(s) => s.clone(),
                _ => String::new(),
            },
            Kind::RgbStrip { .. } | Kind::Curve => kind.format(current).unwrap_or_default(),
            // Seed with slot 1's existing name so a rename starts from what is
            // on the wheel rather than a blank field.
            Kind::SlotText { .. } => slot_name(current, 1),
            _ => String::new(),
        };
        EditState { attr, kind, draft: current.clone(), buffer, slot: 1 }
    }

    fn is_text_mode(&self) -> bool {
        matches!(
            self.kind,
            Kind::TextField { .. } | Kind::RgbStrip { .. } | Kind::Curve | Kind::SlotText { .. }
        )
    }

    pub fn bump(&mut self, d: i32) {
        // SlotText: bumping moves between slots, reloading each slot's current
        // name into the buffer, rather than changing the draft value.
        if let Kind::SlotText { slots, .. } = self.kind {
            let n = slots as i32;
            self.slot = ((self.slot as i32 - 1 + d).rem_euclid(n) + 1) as u8;
            self.buffer = slot_name(&self.draft, self.slot);
            return;
        }
        self.draft = match (self.kind, &self.draft) {
            (Kind::Percent | Kind::ScaledPercent { .. }, Value::Percent(n)) => {
                Value::Percent((*n as i32 + d).clamp(0, 100) as u8)
            }
            (Kind::IntRange { min, max, step, .. }, Value::Int(n)) => {
                Value::Int((*n + d * step).clamp(min, max))
            }
            (Kind::Enum(vs), Value::Enum(n)) => {
                let len = vs.len() as i32;
                Value::Enum((*n as i32 + d).rem_euclid(len) as u8)
            }
            (Kind::Toggle { .. }, Value::Bool(b)) => Value::Bool(!*b),
            // A pedal deadzone. This arm was missing entirely, and Pair is
            // not a text mode either, so the editor opened, Left/Right did
            // nothing, typing did nothing, and Enter wrote the value back
            // unchanged and reported success. `slot` picks the half (1 =
            // lower, 2 = upper), the same field SlotText uses to say which
            // of several things a keypress applies to.
            //
            // Clamped against the other half rather than rejected: the
            // driver's constraint is lower + upper <= 99, and the curve
            // editor already clamps for exactly this, so a user leaning on
            // an arrow key stops at the limit instead of being told off.
            (Kind::Pair { .. }, Value::Pair(lo, hi)) => {
                let (lo, hi) = (*lo, *hi);
                if self.slot == 2 {
                    Value::Pair(lo, ((hi as i32 + d).clamp(0, 99 - lo as i32)) as u8)
                } else {
                    Value::Pair(((lo as i32 + d).clamp(0, 99 - hi as i32)) as u8, hi)
                }
            }
            (_, v) => v.clone(),
        };
    }

    /// Move between the halves of a [`Kind::Pair`] (lower <-> upper).
    /// No-op for every other kind, so the caller can bind it globally.
    pub fn select_part(&mut self, d: i32) {
        if matches!(self.kind, Kind::Pair { .. }) {
            self.slot = if (self.slot as i32 + d).rem_euclid(2) == 0 { 2 } else { 1 };
        }
    }

    pub fn push_char(&mut self, c: char) {
        match self.kind {
            Kind::TextField { max_len } | Kind::SlotText { max_len, .. } => {
                if self.buffer.chars().count() < max_len {
                    self.buffer.push(c);
                }
            }
            Kind::RgbStrip { .. } | Kind::Curve => self.buffer.push(c),
            _ => {}
        }
    }

    pub fn backspace(&mut self) {
        if self.is_text_mode() {
            self.buffer.pop();
        }
    }

    pub fn commit_value(&self) -> Result<Value, Error> {
        // SlotText writes one slot; the buffer is that slot's new name, so it
        // is not parsed as a value (parse reads the whole-list form).
        if let Kind::SlotText { .. } = self.kind {
            let v = Value::SlotName { slot: self.slot, name: self.buffer.clone() };
            self.kind.validate(&v)?;
            return Ok(v);
        }
        if self.is_text_mode() {
            // Parse the raw buffer (validates encoding/length).
            self.kind.parse(&self.buffer)
        } else {
            self.kind.validate(&self.draft)?;
            Ok(self.draft.clone())
        }
    }

    /// What to show for the row currently being edited.
    pub fn display(&self) -> String {
        if let Kind::SlotText { .. } = self.kind {
            return format!("{}: {}_", self.slot, self.buffer);
        }
        // Mark the half the arrows will move, or the editor gives no clue
        // which of the two numbers is about to change.
        if let (Kind::Pair { .. }, Value::Pair(lo, hi)) = (self.kind, &self.draft) {
            return if self.slot == 2 {
                format!("{lo}% / [{hi}%]")
            } else {
                format!("[{lo}%] / {hi}%")
            };
        }
        if self.is_text_mode() {
            format!("{}_", self.buffer)
        } else {
            self.kind.display(&self.draft)
        }
    }
}

/// The current name of `slot` (1-based) from a `SlotNames` value, or empty.
fn slot_name(v: &Value, slot: u8) -> String {
    match v {
        Value::SlotNames(names) => {
            names.get(slot.saturating_sub(1) as usize).cloned().unwrap_or_default()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_wheel_core::{Kind, Value};

    #[test]
    fn percent_bump_clamps() {
        let mut e = EditState::start("wheel_strength", Kind::Percent, &Value::Percent(99));
        e.bump(5);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(100));
        e.bump(-200);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(0));
    }

    #[test]
    fn intrange_bump_respects_step_and_bounds() {
        let k = Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" };
        let mut e = EditState::start("wheel_range", k, &Value::Int(900));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Int(910));
    }

    #[test]
    fn enum_bump_wraps() {
        let k = Kind::Enum(&["kf", "tf"]);
        let mut e = EditState::start("wheel_texture_route", k, &Value::Enum(1));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Enum(0));
    }

    #[test]
    fn text_edit_buffer() {
        let mut e = EditState::start("wheel_led_slot_name", Kind::TextField { max_len: 8 }, &Value::Text("RACE".into()));
        e.push_char('R');
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACER".into()));
        e.backspace();
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACE".into()));
    }

    #[test]
    fn rgb_edited_as_raw_string() {
        let ten = "000000 000000 000000 000000 000000 000000 000000 000000 000000 000000";
        let start = Kind::RgbStrip { leds: 10 }.parse(ten).unwrap();
        let mut e = EditState::start("wheel_led_colors", Kind::RgbStrip { leds: 10 }, &start);
        e.backspace();
        e.push_char('f');
        match e.commit_value().unwrap() {
            Value::Rgb(cs) => assert_eq!(cs[9].b, 0x0f),
            _ => panic!("not rgb"),
        }
    }

    #[test]
    fn curve_edited_as_raw_string() {
        let mut e = EditState::start("wheel_response_curve", Kind::Curve, &Value::Curve(vec![]));
        for _ in 0.."reset".len() {
            e.backspace();
        }
        for c in "0:0 65535:65535".chars() {
            e.push_char(c);
        }
        assert_eq!(e.commit_value().unwrap(), Value::Curve(vec![(0, 0), (65535, 65535)]));
    }
}

#[cfg(test)]
mod slot_text_tests {
    use super::*;
    use logi_wheel_core::{Kind, Value};

    const K: Kind = Kind::SlotText { slots: 5, max_len: 9 };

    fn current() -> Value {
        Value::SlotNames(vec![
            "QZX7".into(),
            "GT7".into(),
            "PROFILE 3".into(),
            "PROFILE 4".into(),
            "TEST".into(),
        ])
    }

    #[test]
    fn starts_on_slot_1_seeded_with_its_current_name() {
        let e = EditState::start("wheel_profile_names", K, &current());
        assert_eq!(e.slot, 1);
        assert_eq!(e.buffer, "QZX7");
    }

    #[test]
    fn bump_picks_the_slot_and_reloads_its_name() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        e.bump(1);
        assert_eq!((e.slot, e.buffer.as_str()), (2, "GT7"));
        e.bump(1);
        assert_eq!((e.slot, e.buffer.as_str()), (3, "PROFILE 3"));
        e.bump(-1);
        assert_eq!((e.slot, e.buffer.as_str()), (2, "GT7"));
    }

    #[test]
    fn slot_wraps_around_both_ends() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        e.bump(-1);
        assert_eq!(e.slot, 5, "below slot 1 wraps to the last slot");
        e.bump(1);
        assert_eq!(e.slot, 1);
    }

    #[test]
    fn typing_a_name_and_committing_writes_that_slot() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        e.bump(2); // slot 3
        for _ in 0..("PROFILE 3".len()) {
            e.backspace();
        }
        for c in "Race".chars() {
            e.push_char(c);
        }
        let v = e.commit_value().unwrap();
        assert_eq!(v, Value::SlotName { slot: 3, name: "Race".into() });
        // and that value encodes to what the driver's store expects
        assert_eq!(K.format(&v).unwrap(), "3:Race");
    }

    #[test]
    fn name_is_capped_at_the_drivers_limit() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        for _ in 0..4 {
            e.backspace();
        }
        for c in "ABCDEFGHIJKLMNOPQRST".chars() {
            e.push_char(c);
        }
        assert_eq!(e.buffer.chars().count(), 9, "buffer stops at the wheel's 9-char limit");
        assert!(e.commit_value().is_ok());
    }

    #[test]
    fn an_emptied_name_is_rejected_rather_than_written() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        for _ in 0..4 {
            e.backspace();
        }
        assert!(e.buffer.is_empty());
        assert!(e.commit_value().is_err(), "the driver rejects a zero-length name");
    }

    #[test]
    fn display_shows_the_slot_being_renamed() {
        let mut e = EditState::start("wheel_profile_names", K, &current());
        e.bump(1);
        assert_eq!(e.display(), "2: GT7_");
    }
}

#[cfg(test)]
mod pair_tests {
    use super::*;

    fn dz() -> EditState {
        EditState::start("wheel_throttle_deadzone", Kind::Pair { max: 99 }, &Value::Pair(10, 5))
    }

    /// The bug: Kind::Pair had no bump arm and is not a text mode, so the
    /// editor opened, every key did nothing, and Enter reported success
    /// having changed the value not at all.
    #[test]
    fn arrows_move_the_selected_half() {
        let mut e = dz();
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Pair(11, 5), "lower is selected first");

        e.select_part(1);
        e.bump(-1);
        assert_eq!(e.commit_value().unwrap(), Value::Pair(11, 4), "now the upper half");

        e.select_part(1);
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Pair(12, 4), "and back to the lower");
    }

    /// The driver rejects lower + upper > 99. The curve editor clamps for
    /// the same constraint, so this does too: holding an arrow stops at the
    /// limit rather than producing a value the write will refuse.
    #[test]
    fn the_pair_cannot_be_pushed_past_the_drivers_limit() {
        let mut e =
            EditState::start("wheel_brake_deadzone", Kind::Pair { max: 99 }, &Value::Pair(60, 39));
        for _ in 0..20 {
            e.bump(1);
        }
        let Value::Pair(lo, hi) = e.commit_value().unwrap() else { panic!("not a pair") };
        assert_eq!((lo, hi), (60, 39), "already at the limit, so nothing moves");

        e.select_part(1);
        for _ in 0..20 {
            e.bump(-1);
        }
        for _ in 0..20 {
            e.bump(1);
        }
        let Value::Pair(lo, hi) = e.commit_value().unwrap() else { panic!("not a pair") };
        assert!(lo as u16 + hi as u16 <= 99, "{lo} + {hi} exceeds the driver's limit");
    }

    /// The display has to say which number the arrows will move.
    #[test]
    fn the_selected_half_is_visible() {
        let mut e = dz();
        assert_eq!(e.display(), "[10%] / 5%");
        e.select_part(1);
        assert_eq!(e.display(), "10% / [5%]");
    }
}
