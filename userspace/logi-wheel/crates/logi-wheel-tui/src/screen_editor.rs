//! The terminal app's screen editor: a modal over the `wheel_oled` row, the
//! sibling of the colour picker. Two panes. The presets pane is where most
//! people stop: Up/Down pick one, and the preview follows. Tab moves to the
//! design pane, which is the same design open for changes: Left/Right on
//! its top row pick a layout, Up/Down move between the fields, typing fills
//! the focused field. From anywhere, Enter shows the design on the screen
//! now, and from either pane's non-typing rows `g` makes it the dashboard
//! the simulated-TrueForce daemon shows during games and `x` hands the
//! screen back to the wheel.
//!
//! Composition and validation live in `logi_wheel_core::oled`, shared with
//! the window, so both apps send exactly the string the driver takes.

use crossterm::event::KeyCode;
use logi_wheel_core::oled::{self, ComposeError, Layout, PRESETS};
use logi_wheel_core::Value;

/// What a key did.
#[derive(Debug, PartialEq, Eq)]
pub enum ScreenOutcome {
    /// Still editing.
    Open,
    /// Send this frame to the wheel now (samples in place of placeholders).
    Commit(String),
    /// Make this the daemon's dashboard template and switch the dashboard on.
    UseInGames(String),
    /// Hand the screen back to the wheel's menu.
    Off,
    /// Leave without writing.
    Cancel,
}

/// Where the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The presets pane; `preset` is the highlighted entry.
    Presets,
    /// The design pane's layout row.
    Layout,
    /// The design pane's field `n`, zero-based.
    Field(usize),
}

#[derive(Debug, Clone)]
pub struct ScreenEditor {
    /// Index into [`oled::LAYOUTS`].
    pub layout: usize,
    /// One staged value per field of the layout, in write order.
    pub values: Vec<String>,
    /// The highlighted preset in the presets pane.
    pub preset: usize,
    pub focus: Focus,
    /// Why the last Enter did not send, if it did not.
    pub error: Option<String>,
}

impl ScreenEditor {
    /// Seed from the wheel's current frame, or the first preset when the
    /// screen is off. Only a text value can be a frame.
    pub fn from_value(v: &Value) -> Option<ScreenEditor> {
        let Value::Text(s) = v else { return None };
        let (layout, values) = match oled::parse(s) {
            Some((l, vals)) => (l.index as usize, vals),
            None => {
                let p = &PRESETS[0];
                (p.layout().index as usize, p.values())
            }
        };
        let preset = oled::preset_index(&oled::LAYOUTS[layout], &values).unwrap_or(0);
        Some(ScreenEditor { layout, values, preset, focus: Focus::Presets, error: None })
    }

    pub fn current(&self) -> &'static Layout {
        &oled::LAYOUTS[self.layout]
    }

    /// The preset the design still matches, if any.
    pub fn matches_preset(&self) -> Option<usize> {
        oled::preset_index(self.current(), &self.values)
    }

    /// Whether a field is fed from the game.
    pub fn live(&self) -> bool {
        oled::is_live(&self.values)
    }

    /// The template `g` would set, or why it cannot.
    pub fn template(&self) -> Result<String, ComposeError> {
        oled::compose(self.current(), &self.values)
    }

    /// The frame Enter would send: the design with samples for placeholders.
    pub fn frame(&self) -> Result<String, ComposeError> {
        oled::compose(self.current(), &oled::sample_values(&self.values))
    }

    /// The preview as plain rows, for drawing.
    pub fn preview(&self) -> Vec<String> {
        oled::preview(self.current(), &oled::sample_values(&self.values)).into_iter().map(|r| r.text).collect()
    }

    fn apply_preset(&mut self, idx: usize) {
        self.preset = idx;
        let p = &PRESETS[idx];
        self.layout = p.layout().index as usize;
        self.values = p.values();
        self.error = None;
    }

    fn set_layout(&mut self, idx: usize) {
        self.layout = idx;
        self.values = vec![String::new(); self.current().fields.len()];
        self.error = None;
    }

    pub fn on_key(&mut self, key: KeyCode) -> ScreenOutcome {
        let n = self.current().fields.len();
        let layouts = oled::LAYOUTS.len();
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
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Presets => Focus::Layout,
                    _ => Focus::Presets,
                }
            }
            _ => {}
        }
        match self.focus {
            Focus::Presets => match key {
                KeyCode::Up => self.apply_preset(self.preset.saturating_sub(1)),
                KeyCode::Down => self.apply_preset((self.preset + 1).min(PRESETS.len() - 1)),
                KeyCode::Right => self.focus = Focus::Layout,
                KeyCode::Char('g') => return self.use_in_games(),
                KeyCode::Char('x') => return ScreenOutcome::Off,
                _ => {}
            },
            Focus::Layout => match key {
                KeyCode::Left => self.set_layout((self.layout + layouts - 1) % layouts),
                KeyCode::Right => self.set_layout((self.layout + 1) % layouts),
                KeyCode::Up => self.focus = Focus::Presets,
                KeyCode::Down if n > 0 => self.focus = Focus::Field(0),
                KeyCode::Char('g') => return self.use_in_games(),
                KeyCode::Char('x') => return ScreenOutcome::Off,
                _ => {}
            },
            Focus::Field(i) => match key {
                KeyCode::Up => self.focus = if i == 0 { Focus::Layout } else { Focus::Field(i - 1) },
                KeyCode::Down => self.focus = Focus::Field((i + 1).min(n - 1)),
                KeyCode::Backspace => {
                    self.values[i].pop();
                    self.error = None;
                }
                KeyCode::Char(c) if !c.is_control() && c != '|' => {
                    self.values[i].push(c);
                    self.error = None;
                }
                _ => {}
            },
        }
        ScreenOutcome::Open
    }

    fn use_in_games(&mut self) -> ScreenOutcome {
        match self.template() {
            Ok(t) => ScreenOutcome::UseInGames(t),
            Err(e) => {
                self.error = Some(e.to_string());
                ScreenOutcome::Open
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_the_frame_or_from_the_first_preset() {
        let ed = ScreenEditor::from_value(&Value::Text("J|Lap 12|1:42.7".into())).unwrap();
        assert_eq!(ed.current().letter, 'J');
        assert_eq!(ed.values, vec!["Lap 12", "1:42.7", "", ""]);
        assert_eq!(ed.matches_preset(), None, "a hand-written frame is no preset");
        let ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        assert_eq!(ed.current().letter, 'G');
        assert_eq!(ed.values, vec!["{gear}", "{speed}"]);
        assert_eq!(ed.matches_preset(), Some(0));
        assert_eq!(ed.focus, Focus::Presets);
        assert!(ScreenEditor::from_value(&Value::Int(3)).is_none());
    }

    #[test]
    fn presets_apply_as_they_are_highlighted_and_enter_shows_samples() {
        let mut ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        assert_eq!(ed.preview(), vec!["3 142"], "the preview shows samples, not placeholders");
        ed.on_key(KeyCode::Down);
        assert_eq!(ed.preset, 1);
        assert_eq!(ed.current().letter, PRESETS[1].layout);
        assert!(ed.live());
        assert_eq!(ed.on_key(KeyCode::Enter), ScreenOutcome::Commit("F|3|142".into()));
        assert_eq!(ed.on_key(KeyCode::Char('g')), ScreenOutcome::UseInGames("F|{gear}|{speed}".into()));
        assert_eq!(ed.on_key(KeyCode::Char('x')), ScreenOutcome::Off);
        assert_eq!(ed.on_key(KeyCode::Esc), ScreenOutcome::Cancel);
    }

    #[test]
    fn tab_opens_the_design_for_changes() {
        let mut ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        ed.on_key(KeyCode::Tab);
        assert_eq!(ed.focus, Focus::Layout);
        // Right to H: two empty fields, no longer a preset.
        ed.on_key(KeyCode::Right);
        assert_eq!(ed.current().letter, 'H');
        assert_eq!(ed.values, vec!["", ""]);
        assert_eq!(ed.matches_preset(), None);
        ed.on_key(KeyCode::Down);
        assert_eq!(ed.focus, Focus::Field(0));
        for c in "HELLO".chars() {
            ed.on_key(KeyCode::Char(c));
        }
        ed.on_key(KeyCode::Down);
        for c in "142".chars() {
            ed.on_key(KeyCode::Char(c));
        }
        assert_eq!(ed.on_key(KeyCode::Enter), ScreenOutcome::Commit("H|HELLO|142".into()));
        // A field is text input: g and x type, they do not act.
        ed.on_key(KeyCode::Char('g'));
        assert_eq!(ed.values[1], "142g");
        ed.on_key(KeyCode::Backspace);
        ed.on_key(KeyCode::Tab);
        assert_eq!(ed.focus, Focus::Presets, "Tab comes back to the presets");
    }

    #[test]
    fn an_overlong_field_keeps_the_editor_open_with_a_reason() {
        let mut ed = ScreenEditor::from_value(&Value::Text("off".into())).unwrap();
        // A blank gear-and-speed layout: Right then Left clears the preset's fields.
        for k in [KeyCode::Tab, KeyCode::Right, KeyCode::Left, KeyCode::Down] {
            ed.on_key(k);
        }
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
