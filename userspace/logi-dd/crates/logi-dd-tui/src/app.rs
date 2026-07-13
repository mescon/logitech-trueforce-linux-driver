use crate::edit;
use logi_dd_core::setting::Access;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Value, REGISTRY};

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
        let Some(row) = self.selected() else { return };
        let Some(spec) = Device::<S>::spec(row.attr) else { return };
        if spec.access == Access::ReadOnly {
            return;
        }
        if spec.access == Access::Action {
            match self.device.write(row.attr, &Value::Trigger) {
                Ok(()) => self.status = format!("{}: done", row.label),
                Err(e) => self.status = format!("{}: {e}", row.label),
            }
            return;
        }
        let Ok(cur) = &row.value else {
            self.status = "cannot edit (value unreadable)".into();
            return;
        };
        self.edit = Some(edit::EditState::start(spec.attr, spec.kind, cur));
    }

    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let attr = e.attr;
        let label = Device::<S>::spec(attr).map(|s| s.label).unwrap_or(attr);
        match e.commit_value().and_then(|v| self.device.write(attr, &v)) {
            Ok(()) => {
                self.status = format!("{label} set");
            }
            Err(Error::WrongMode { needed: logi_dd_core::Mode::Desktop }) => {
                self.status = "needs desktop mode: press 'd' to switch, then retry".into();
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
            Char('d') => {
                self.status = match self.device.ensure_desktop_mode() {
                    Ok(()) => "switched to desktop mode".into(),
                    Err(e) => format!("mode switch: {e}"),
                };
                self.reload();
            }
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
}
