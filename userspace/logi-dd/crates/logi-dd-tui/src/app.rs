use crate::curve_editor::CurveEditor;
use crate::edit;
use logi_dd_core::setting::Access;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Kind, Mode, Value, REGISTRY};
use std::collections::BTreeMap;

pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub value: Result<Value, Error>,
    pub available: bool,
}

/// The index of the synthetic "Setup" sidebar entry, one past the last real
/// device category. It is not part of `logi_dd_core::Category`: Setup shows
/// the game helpers (logi-ffb, the TrueForce SDK shim), not a device
/// setting, so `cat_idx` reaching this value means "show the Setup body"
/// rather than "look up `Category::ALL[cat_idx]`".
pub const SETUP_INDEX: usize = Category::ALL.len();

/// Whether `bin` resolves on `$PATH`: a plain directory scan rather than a
/// subprocess spawn, so a missing binary costs nothing at startup. Good
/// enough for a presence hint; the actual install/uninstall run still goes
/// through `std::process::Command`, which does its own `$PATH` lookup.
fn found_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Resolve the TrueForce SDK shim installer's binary name: prefer the
/// packaged `logitech-trueforce-install-shim`, falling back to
/// `install-tf-shim.sh` (a dev checkout's `tools/` script, also expected on
/// `PATH` there). `None` means neither was found.
fn resolve_shim_binary() -> Option<&'static str> {
    ["logitech-trueforce-install-shim", "install-tf-shim.sh"].into_iter().find(|bin| found_on_path(bin))
}

pub struct App<S: SysfsIo> {
    pub device: Device<S>,
    pub cat_idx: usize,
    pub row_idx: usize,
    pub rows: Vec<Row>,
    pub status: String,
    pub edit: Option<edit::EditState>,
    /// The modal curve editor, active while shaping a `Kind::Curve` attribute.
    pub curve_edit: Option<CurveEditor>,
    pub quit: bool,
    /// Whether `logi-ffb` was found on `PATH` at startup (Setup body status).
    pub ffb_found: bool,
    /// The TrueForce SDK shim installer's resolved binary name, or `None` if
    /// neither candidate name was found on `PATH` at startup.
    pub shim_binary: Option<&'static str>,
    /// A shim run queued by `on_key` for the main loop to execute:
    /// `(installer arg, verb for the status line)`. The run blocks (an
    /// `--all-steam` Proton-prefix scan can take a while), so instead of
    /// running inside the key handler, where the "running..." status could
    /// never be drawn first, the loop takes this via `take_pending_shim`,
    /// draws once, then calls `run_shim`.
    pending_shim: Option<(&'static str, &'static str)>,
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
            curve_edit: None,
            quit: false,
            ffb_found: found_on_path("logi-ffb"),
            shim_binary: resolve_shim_binary(),
            pending_shim: None,
        };
        a.reload();
        a
    }

    pub fn category(&self) -> Category {
        Category::ALL[self.cat_idx]
    }

    /// Whether the synthetic "Setup" sidebar entry is selected.
    pub fn is_setup(&self) -> bool {
        self.cat_idx == SETUP_INDEX
    }

    pub fn reload(&mut self) {
        if self.is_setup() {
            self.rows.clear();
            return;
        }
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
        // +1 for the trailing Setup entry, one past the last real category.
        let n = (Category::ALL.len() + 1) as i32;
        self.cat_idx = ((self.cat_idx as i32 + d).rem_euclid(n)) as usize;
        self.row_idx = 0;
        self.reload();
    }

    /// Take the shim run the last key press queued, if any; see
    /// `pending_shim`.
    pub fn take_pending_shim(&mut self) -> Option<(&'static str, &'static str)> {
        self.pending_shim.take()
    }

    /// Run the TrueForce SDK shim installer with `arg` (`--all-steam` or
    /// `--uninstall`), blocking: the TUI's event loop is synchronous, so
    /// there is no worker thread to hand this off to. The main loop calls
    /// this via the `pending_shim` queue so a "running..." status gets
    /// drawn first (an `--all-steam` Proton-prefix scan can take a while).
    /// Never sudo. A missing binary or a spawn failure lands in the status
    /// line instead of taking the TUI down.
    pub fn run_shim(&mut self, arg: &'static str, verb: &str) {
        let Some(bin) = self.shim_binary else {
            self.status = "shim: installer not found on PATH".to_string();
            return;
        };
        match std::process::Command::new(bin).arg(arg).output() {
            Ok(out) if out.status.success() => {
                self.status = format!("shim {verb}: ok");
            }
            Ok(out) => {
                let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                let last = combined
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("failed")
                    .to_string();
                self.status = format!("shim {verb}: {last}");
            }
            Err(e) => {
                self.status = format!("shim {verb}: failed to run {bin}: {e}");
            }
        }
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
        // Curves get the modal point-list editor; everything else the inline
        // field editor.
        if matches!(spec.kind, Kind::Curve) {
            self.curve_edit = Some(CurveEditor::from_value(spec.attr, &cur));
            return;
        }
        self.edit = Some(edit::EditState::start(spec.attr, spec.kind, &cur));
    }

    pub fn commit_curve_edit(&mut self) {
        let Some(ed) = self.curve_edit.take() else { return };
        let attr = ed.attr;
        let label = Device::<S>::spec(attr).map(|s| s.label).unwrap_or(attr);
        match self.device.write(attr, &ed.to_value()) {
            Ok(()) => self.status = format!("{label} set ({} points)", ed.point_count()),
            Err(Error::WrongMode { needed }) => {
                let m = if needed == Mode::Desktop { "desktop" } else { "onboard" };
                self.status = format!("needs {m} mode: press 'd' to toggle, then retry");
            }
            Err(err) => self.status = format!("{label}: {err}"),
        }
        self.reload();
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
        if let Some(ce) = self.curve_edit.as_mut() {
            match key {
                Enter => self.commit_curve_edit(),
                Esc => self.curve_edit = None,
                Up => ce.prev_field(),
                Down => ce.next_field(),
                Left => ce.adjust(-1),
                Right => ce.adjust(1),
                Char('+') => ce.add_point(),
                Char('-') => ce.delete_point(),
                _ => {}
            }
            return;
        }
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
        if self.is_setup() {
            match key {
                Char('q') => self.quit = true,
                Left => self.move_cat(-1),
                Right => self.move_cat(1),
                // Queued, not run here: the main loop draws a "running..."
                // status line before the blocking run (see `pending_shim`).
                Char('i') => self.pending_shim = Some(("--all-steam", "install")),
                Char('u') => self.pending_shim = Some(("--uninstall", "uninstall")),
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
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Steering).unwrap();
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

    fn pedal_app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_throttle_curve", "0/64 points loaded (0 = built-in curve)");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Pedals).unwrap();
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_throttle_curve").unwrap();
        a
    }

    #[test]
    fn curve_row_opens_modal_not_inline_editor() {
        use crossterm::event::KeyCode;
        let mut a = pedal_app();
        a.on_key(KeyCode::Enter);
        assert!(a.curve_edit.is_some(), "curve opens the modal editor");
        assert!(a.edit.is_none(), "not the inline field editor");
    }

    #[test]
    fn curve_editor_commit_writes_a_multipoint_curve() {
        use crossterm::event::KeyCode;
        let mut a = pedal_app();
        a.on_key(KeyCode::Enter); // open
        a.on_key(KeyCode::Char('+')); // add a middle point (now 3 points)
        a.on_key(KeyCode::Down); // move to Output field
        a.on_key(KeyCode::Right); // bend the point
        a.on_key(KeyCode::Enter); // save
        assert!(a.curve_edit.is_none());
        assert!(a.status.contains("set"), "status: {}", a.status);
        // the stored curve is a real multi-point list ending at full scale
        match a.device.read("wheel_throttle_curve").unwrap() {
            Value::Curve(pts) => {
                assert!(pts.len() >= 3, "points: {pts:?}");
                assert_eq!(*pts.last().unwrap(), (65535, 65535));
            }
            v => panic!("expected a curve, got {v:?}"),
        }
    }

    #[test]
    fn curve_editor_esc_cancels_without_writing() {
        use crossterm::event::KeyCode;
        let mut a = pedal_app();
        a.on_key(KeyCode::Enter);
        a.on_key(KeyCode::Char('+'));
        a.on_key(KeyCode::Esc);
        assert!(a.curve_edit.is_none());
        // nothing was written: the store still reads built-in (empty curve)
        assert_eq!(
            a.device.read("wheel_throttle_curve").unwrap(),
            Value::Curve(vec![])
        );
    }

    #[test]
    fn move_cat_reaches_and_leaves_setup() {
        let mut a = app();
        // step through every real category, landing on Setup right after
        // the last one.
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup(), "cat_idx {} should be the Setup entry", a.cat_idx);
        assert!(a.rows.is_empty(), "Setup has no settings rows");
        // one more step wraps back around to the first real category.
        a.move_cat(1);
        assert!(!a.is_setup());
        assert_eq!(a.cat_idx, 0);
        // stepping backward from the first category reaches Setup too.
        a.move_cat(-1);
        assert!(a.is_setup());
    }

    #[test]
    fn setup_view_ignores_row_and_edit_keys() {
        use crossterm::event::KeyCode;
        let mut a = app();
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup());
        a.on_key(KeyCode::Enter); // no rows to edit; must not open an editor
        assert!(a.edit.is_none());
        assert!(a.curve_edit.is_none());
    }

    #[test]
    fn setup_keys_queue_the_shim_run_instead_of_running_it() {
        use crossterm::event::KeyCode;
        let mut a = app();
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup());
        a.on_key(KeyCode::Char('i'));
        assert_eq!(a.take_pending_shim(), Some(("--all-steam", "install")));
        // Taken once; a second take finds nothing queued.
        assert_eq!(a.take_pending_shim(), None);
        a.on_key(KeyCode::Char('u'));
        assert_eq!(a.take_pending_shim(), Some(("--uninstall", "uninstall")));
    }

    #[test]
    fn run_shim_reports_missing_binary_without_spawning() {
        let mut a = app();
        a.shim_binary = None;
        a.run_shim("--all-steam", "install");
        assert!(a.status.contains("not found"), "status: {}", a.status);
    }

    #[test]
    fn run_shim_reports_success() {
        let mut a = app();
        a.shim_binary = Some("true"); // exists on PATH, exits 0, ignores args
        a.run_shim("--all-steam", "install");
        assert_eq!(a.status, "shim install: ok");
    }

    #[test]
    fn run_shim_reports_failure_without_crashing() {
        let mut a = app();
        a.shim_binary = Some("false"); // exists on PATH, exits non-zero
        a.run_shim("--uninstall", "uninstall");
        assert!(a.status.starts_with("shim uninstall:"), "status: {}", a.status);
        assert_ne!(a.status, "shim uninstall: ok");
    }

    #[test]
    fn run_shim_reports_spawn_failure_without_crashing() {
        let mut a = app();
        a.shim_binary = Some("this-binary-does-not-exist-anywhere");
        a.run_shim("--all-steam", "install");
        assert!(a.status.contains("failed to run"), "status: {}", a.status);
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
