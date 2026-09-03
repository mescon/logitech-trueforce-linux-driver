//! The terminal app's screen editor: a modal over the `wheel_oled` row, the
//! sibling of the colour picker. Pick a layout with Left/Right on the top
//! row, move between its fields with Up/Down, type into a field, Enter sends
//! the composed frame, Esc leaves without writing, and `x` on the layout row
//! hands the screen back to the wheel.
//!
//! Composition and validation live in `logi_wheel_core::oled`, shared with
//! the window, so both apps send exactly the string the driver takes.

use crossterm::event::KeyCode;
use logi_wheel_core::oled::{self, ComposeError, Layout};
use logi_wheel_core::Value;

/// What a key did.
#[derive(Debug, PartialEq, Eq)]
pub enum ScreenOutcome {
    /// Still editing.
    Open,
    /// Send this frame to the wheel.
    Commit(String),
    /// Hand the screen back to the wheel's menu.
    Off,
    /// Leave without writing.
    Cancel,
}

#[derive(Debug, Clone)]
pub struct ScreenEditor {
    /// Index into [`oled::LAYOUTS`].
    pub layout: usize,
    /// One staged value per field of the layout, in write order.
    pub values: Vec<String>,
    /// 0 is the layout row; 1..=n are the fields.
    pub focus: usize,
    /// Why the last Enter did not send, if it did not.
    pub error: Option<String>,
}

impl ScreenEditor {
    /// Seed from the wheel's current frame, or the gear-and-speed layout
    /// when the screen is off. Only a text value can be a frame.
    pub fn from_value(v: &Value) -> Option<ScreenEditor> {
        let Value::Text(s) = v else { return None };
        let (layout, values) = match oled::parse(s) {
            Some((l, vals)) => (l.index as usize, vals),
            None => {
                let g = oled::layout('G').expect("G exists");
                (g.index as usize, vec![String::new(); g.fields.len()])
            }
        };
        Some(ScreenEditor { layout, values, focus: 1.min(oled::LAYOUTS[layout].fields.len()), error: None })
    }

    pub fn current(&self) -> &'static Layout {
        &oled::LAYOUTS[self.layout]
    }

    /// The frame Enter would send, or why it cannot.
    pub fn frame(&self) -> Result<String, ComposeError> {
        oled::compose(self.current(), &self.values)
    }

    /// The preview as plain rows, for drawing.
    pub fn preview(&self) -> Vec<String> {
        oled::preview(self.current(), &self.values).into_iter().map(|r| r.text).collect()
    }

    fn set_layout(&mut self, idx: usize) {
        self.layout = idx;
        self.values.resize(self.current().fields.len(), String::new());
        self.focus = self.focus.min(self.current().fields.len());
        self.error = None;
    }

    pub fn on_key(&mut self, key: KeyCode) -> ScreenOutcome {
        let n = self.current().fields.len();
        let count = oled::LAYOUTS.len();
        match key {
            KeyCode::Esc => return ScreenOutcome::Cancel,
            KeyCode::Enter => {
                return match self.frame() {
                    Ok(frame) => ScreenOutcome::Commit(frame),
                    Err(e) => {
                        self.error = Some(e.to_string());
                        ScreenOutcome::Open
                    }
                }
            }
            KeyCode::Up => self.focus = self.focus.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.focus = (self.focus + 1).min(n),
            _ if self.focus == 0 => match key {
                KeyCode::Left => self.set_layout((self.layout + count - 1) % count),
                KeyCode::Right => self.set_layout((self.layout + 1) % count),
                KeyCode::Char('x') => return ScreenOutcome::Off,
                _ => {}
            },
            KeyCode::Backspace => {
                self.values[self.focus - 1].pop();
                self.error = None;
            }
            KeyCode::Char(c) if !c.is_control() && c != '|' => {
                self.values[self.focus - 1].push(c);
                self.error = None;
            }
            _ => {}
        }
        ScreenOutcome::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_the_frame_or_defaults_to_gear_and_speed() {
        let ed = ScreenEditor::from_value(&Value::Text("J|Lap 12|1:42.7".into())).unwrap();
        assert_eq!(ed.current().letter, 'J');
        assert_eq!(ed.values, vec!["Lap 12", "1:42.7", "", ""]);
        let ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        assert_eq!(ed.current().letter, 'G');
        assert_eq!(ed.values.len(), 2);
        assert!(ScreenEditor::from_value(&Value::Int(3)).is_none());
    }

    #[test]
    fn arrows_pick_a_layout_and_typing_fills_the_focused_field() {
        let mut ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        // Focus starts on the first field of G; type the gear, then the value.
        for c in "3".chars() {
            ed.on_key(KeyCode::Char(c));
        }
        ed.on_key(KeyCode::Down);
        for c in "142".chars() {
            ed.on_key(KeyCode::Char(c));
        }
        assert_eq!(ed.on_key(KeyCode::Enter), ScreenOutcome::Commit("G|3|142".into()));

        // Up to the layout row, Right to H, values resized to two fields.
        ed.on_key(KeyCode::Up);
        ed.on_key(KeyCode::Up);
        assert_eq!(ed.focus, 0);
        ed.on_key(KeyCode::Right);
        assert_eq!(ed.current().letter, 'H');
        assert_eq!(ed.values.len(), 2);
        assert_eq!(ed.on_key(KeyCode::Char('x')), ScreenOutcome::Off);
        assert_eq!(ed.on_key(KeyCode::Esc), ScreenOutcome::Cancel);
    }

    #[test]
    fn an_overlong_field_keeps_the_editor_open_with_a_reason() {
        let mut ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        for c in "12".chars() {
            ed.on_key(KeyCode::Char(c));
        }
        assert_eq!(ed.on_key(KeyCode::Enter), ScreenOutcome::Open);
        assert!(ed.error.as_deref().unwrap_or("").contains("wider"), "{:?}", ed.error);
        ed.on_key(KeyCode::Backspace);
        assert!(ed.error.is_none(), "editing clears the reason");
        assert_eq!(ed.on_key(KeyCode::Enter), ScreenOutcome::Commit("G|1".into()));
    }
}
