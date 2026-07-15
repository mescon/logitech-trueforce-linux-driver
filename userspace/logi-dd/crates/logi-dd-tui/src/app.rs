use crate::edit;
use logi_dd_core::setting::Access;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Mode, Value, REGISTRY};
use std::collections::BTreeMap;

pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub value: Result<Value, Error>,
    pub available: bool,
}

pub struct App<S: SysfsIo> {
    pub device: Device<S>,
    pub cat_idx: usize,
    pub row_idx: usize,
    pub rows: Vec<Row>,
    pub status: String,
    pub edit: Option<edit::EditState>,
    pub quit: bool,
}

impl<S: SysfsIo> App<S> {
    pub fn new(device: Device<S>) -> Self {
        let mut a = App {
            device,
            cat_idx: 0,
            row_idx: 0,
            rows: Vec::new(),
            status: String::new(),
            edit: None,
            quit: false,
        };
        a.reload();
        a
    }

    pub fn category(&self) -> Category {
        Category::ALL[self.cat_idx]
    }

    pub fn reload(&mut self) {
        let cat = self.category();
        self.rows = REGISTRY
            .iter()
            .filter(|s| s.category == cat)
            .map(|s| Row {
                attr: s.attr,
                label: s.label,
                available: self.device.available(s.attr),
                value: self.device.read(s.attr),
            })
            .collect();
        if self.row_idx >= self.rows.len() {
            self.row_idx = self.rows.len().saturating_sub(1);
        }
    }

    pub fn move_cat(&mut self, d: i32) {
        let n = Category::ALL.len() as i32;
        self.cat_idx = ((self.cat_idx as i32 + d).rem_euclid(n)) as usize;
        self.row_idx = 0;
        self.reload();
    }

    pub fn move_row(&mut self, d: i32) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i32;
        self.row_idx = ((self.row_idx as i32 + d).clamp(0, n - 1)) as usize;
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.row_idx)
    }

    pub fn begin_edit(&mut self) {
        let (attr, label) = match self.selected() {
            Some(row) => (row.attr, row.label),
            None => return,
        };
        let Some(spec) = Device::<S>::spec(attr) else { return };
        if spec.access == Access::ReadOnly {
            return;
        }
        if spec.access == Access::Action {
            self.status = match self.device.write(attr, &Value::Trigger) {
                Ok(()) => format!("{label}: done"),
                Err(e) => format!("{label}: {e}"),
            };
            self.reload();
            return;
        }
        let cur = match self.rows.get(self.row_idx).map(|r| &r.value) {
            Some(Ok(v)) => v.clone(),
            _ => {
                self.status = "cannot edit (value unreadable)".into();
                return;
            }
        };
        self.edit = Some(edit::EditState::start(spec.attr, spec.kind, &cur));
    }

    /// Switch the wheel between desktop and onboard mode (the `d` shortcut).
    pub fn toggle_mode(&mut self) {
        let (idx, name) = match self.device.current_mode() {
            Ok(Mode::Desktop) => (1u8, "onboard"),
            _ => (0u8, "desktop"),
        };
        self.status = match self.device.write("wheel_mode", &Value::Enum(idx)) {
            Ok(()) => format!("switched to {name} mode"),
            Err(e) => format!("mode switch: {e}"),
        };
        self.reload();
    }

    /// The onboard slot names, parsed from `wheel_profile_names` (one
    /// `"N: name"` line per slot), keyed by slot number. Empty if the wheel
    /// does not expose them.
    pub fn profile_names(&self) -> BTreeMap<u8, String> {
        let mut m = BTreeMap::new();
        if let Ok(Value::SlotNames(names)) = self.device.read("wheel_profile_names") {
            for (i, name) in names.iter().enumerate() {
                if !name.is_empty() {
                    m.insert(i as u8 + 1, name.clone());
                }
            }
        }
        m
    }

    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let attr = e.attr;
        let label = Device::<S>::spec(attr).map(|s| s.label).unwrap_or(attr);
        match e.commit_value().and_then(|v| self.device.write(attr, &v)) {
            Ok(()) => {
                self.status = format!("{label} set");
            }
            Err(Error::WrongMode { needed }) => {
                let m = if needed == Mode::Desktop { "desktop" } else { "onboard" };
                self.status = format!("needs {m} mode: press 'd' to toggle, then retry");
            }
            Err(err) => {
                self.status = format!("{label}: {err}");
            }
        }
        self.reload();
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode::*;
        if let Some(ed) = self.edit.as_mut() {
            match key {
                Enter => self.commit_edit(),
                Esc => self.edit = None,
                Left => ed.bump(-1),
                Right => ed.bump(1),
                Backspace => ed.backspace(),
                Char(c) => ed.push_char(c),
                _ => {}
            }
            return;
        }
        match key {
            Char('q') => self.quit = true,
            Char('r') => self.reload(),
            Char('d') => self.toggle_mode(),
            Up => self.move_row(-1),
            Down => self.move_row(1),
            Left => self.move_cat(-1),
            Right => self.move_cat(1),
            Enter => self.begin_edit(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::sysfs::FakeSysfs;

    fn app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_strength", "62");
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.reload();
        a
    }

    #[test]
    fn rows_follow_selected_category() {
        let a = app();
        // first category is Ffb; wheel_strength should be a row
        assert!(a.rows.iter().any(|r| r.attr == "wheel_strength"));
        // absent attrs are marked unavailable, not dropped
        let s = a.rows.iter().find(|r| r.attr == "wheel_ffb_filter").unwrap();
        assert!(!s.available);
    }

    #[test]
    fn move_row_clamps() {
        let mut a = app();
        a.row_idx = 0;
        a.move_row(-1);
        assert_eq!(a.row_idx, 0);
    }

    #[test]
    fn edit_commit_writes_and_reloads() {
        use crossterm::event::KeyCode;
        let mut a = app();
        // navigate to wheel_strength row
        a.cat_idx = 0;
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_strength").unwrap();
        a.on_key(KeyCode::Enter); // begin edit
        assert!(a.edit.is_some());
        a.on_key(KeyCode::Right); // bump +1
        a.on_key(KeyCode::Enter); // commit
        assert!(a.edit.is_none());
        assert_eq!(a.device.read("wheel_strength").unwrap(), logi_dd_core::Value::Percent(63));
    }

    #[test]
    fn wrong_mode_sets_status_prompt() {
        use crossterm::event::KeyCode;
        let fs = logi_dd_core::sysfs::FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_sensitivity", "50");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Sensitivity).unwrap();
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_sensitivity").unwrap();
        a.on_key(KeyCode::Enter);
        a.on_key(KeyCode::Left);
        a.on_key(KeyCode::Enter); // commit -> WrongMode
        assert!(a.status.to_lowercase().contains("desktop"));
    }

    #[test]
    fn d_toggles_mode_both_ways() {
        use crossterm::event::KeyCode;
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.on_key(KeyCode::Char('d'));
        assert_eq!(a.device.current_mode().unwrap(), Mode::Onboard);
        a.on_key(KeyCode::Char('d'));
        assert_eq!(a.device.current_mode().unwrap(), Mode::Desktop);
    }

    #[test]
    fn profile_names_parsed_by_slot() {
        let fs = FakeSysfs::new();
        fs.set("wheel_profile_names", "1: AC EVO\n2: GT7\n3: PROFILE 3");
        let a = App::new(logi_dd_core::Device::with_io(fs));
        let names = a.profile_names();
        assert_eq!(names.get(&1).map(String::as_str), Some("AC EVO"));
        assert_eq!(names.get(&2).map(String::as_str), Some("GT7"));
        assert_eq!(names.get(&3).map(String::as_str), Some("PROFILE 3"));
    }
}
