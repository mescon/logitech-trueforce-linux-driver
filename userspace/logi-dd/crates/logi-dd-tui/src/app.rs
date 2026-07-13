use crate::edit;
use logi_dd_core::setting::Access;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Value, REGISTRY};

pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub value: Result<Value, Error>,
    pub available: bool,
    pub access: Access,
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
                access: s.access,
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
        let mut a = app();
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
}
