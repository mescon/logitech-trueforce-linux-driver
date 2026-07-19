use crate::curve_editor::CurveEditor;
use crate::edit;
use crate::wheel_test::{SimKind, TestView};
use logi_dd_core::setting::Access;
use logi_dd_core::shaping::{self, ShapingRole};
use logi_dd_core::lightsync;
use logi_dd_core::steam::{self, SteamGame};
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Kind, Mode, Value, REGISTRY};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub value: Result<Value, Error>,
    pub available: bool,
}

/// The LIGHTSYNC effect selector's modal state: left/right cycles `index`
/// through `labels` (the entries `lightsync::dropdown_labels` builds: the
/// 4 sweeps, the 5 custom slots, plus the raw current effect only while it
/// is outside 1-5), Enter commits it (one or two device writes; see
/// `commit_effect_edit`), Esc discards. A separate little state from
/// `edit::EditState` because the selector's index is a position in a
/// dynamic label list, not a value any registry `Kind` can bump.
pub struct EffectEdit {
    pub index: usize,
    pub labels: Vec<String>,
}

/// The index of the synthetic "Setup" sidebar entry, one past the last real
/// device category. It is not part of `logi_dd_core::Category`: Setup shows
/// the game helpers (logi-ffb, the TrueForce SDK shim), not a device
/// setting, so `cat_idx` reaching this value means "show the Setup body"
/// rather than "look up `Category::ALL[cat_idx]`".
pub const SETUP_INDEX: usize = Category::ALL.len();

/// The index of the synthetic "Test" sidebar entry, right after Setup.
/// Also not a `Category`: it shows the live input tester (steering,
/// buttons, pedals) and the guarded force simulations, reading the
/// wheel's evdev node rather than sysfs.
pub const TEST_INDEX: usize = Category::ALL.len() + 1;

/// Resolve the SDK folder the Setup view starts with:
/// `$LOGITECH_TRUEFORCE_SDK_DIR` when set, else
/// `~/.local/share/logitech-trueforce/sdk` (the installer script's own
/// default). Editable in the view (the `s` key); whatever it holds is
/// passed as `--sdk-dir` to every install run.
fn resolve_sdk_dir() -> String {
    if let Some(dir) = std::env::var_os("LOGITECH_TRUEFORCE_SDK_DIR") {
        if !dir.is_empty() {
            return dir.to_string_lossy().into_owned();
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".local/share/logitech-trueforce/sdk").to_string_lossy().into_owned()
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
    /// The LIGHTSYNC effect selector, active while the Effect row is being
    /// cycled.
    pub effect_edit: Option<EffectEdit>,
    pub quit: bool,
    /// `logi-ffb`'s resolved path (`PATH`, else next to this executable;
    /// see `logi_dd_core::helpers`), or `None` if it was not found at
    /// startup. The Setup body shows the path.
    pub ffb_path: Option<PathBuf>,
    /// The TrueForce SDK shim installer's resolved path (`PATH`, else the
    /// checkout's `tools/install-tf-shim.sh` above this executable), or
    /// `None` if it was not found at startup.
    pub shim_binary: Option<PathBuf>,
    /// The per-axis shaping view toggles: pure per-session view state
    /// (never persisted, never a sysfs write). While an axis's toggle is
    /// off (simple, the default) its block shows the sensitivity row and
    /// hides the curve row; while on, the other way around. Deadzones show
    /// either way. Each axis is rendered as a synthetic toggle row
    /// (`shaping::toggle_attr`) heading its block on the Steering/Pedals
    /// pages; Enter on that row (or 'a' anywhere in the axis's block)
    /// flips it.
    pub shaping_toggles: shaping::AxisToggles,
    /// The SDK folder every per-game install passes as `--sdk-dir`; see
    /// `resolve_sdk_dir` for the startup value, `s` in the Setup view to
    /// edit it.
    pub sdk_dir: String,
    /// Whether `sdk_dir` holds the marker DLL (`steam::sdk_dir_valid`),
    /// re-checked whenever the dir changes.
    pub sdk_valid: bool,
    /// The SDK-dir line editor's draft, `Some` while the Setup view's `s`
    /// edit is active: type to append, Backspace to erase, Enter commits
    /// (re-checking `sdk_valid`), Esc discards.
    pub sdk_edit: Option<String>,
    /// The installed Proton games the Setup view lists, `scan_games`'s
    /// last result; `game_idx` is the selected row.
    pub games: Vec<SteamGame>,
    pub game_idx: usize,
    /// Whether `scan_games` ran at least once, so the view can tell "no
    /// Steam games found" apart from "not scanned yet" (the scan is lazy:
    /// it first runs when the Setup view is entered).
    pub games_scanned: bool,
    /// The Test view's whole state (discovery, live monitor, sims); see
    /// `wheel_test::TestView`.
    pub test: TestView,
    /// A shim run queued by `on_key` for the main loop to execute:
    /// `(installer args, verb for the status line)`. The run blocks, so
    /// instead of running inside the key handler, where the "running..."
    /// status could never be drawn first, the loop takes this via
    /// `take_pending_shim`, draws once, then calls `run_shim`.
    pending_shim: Option<(Vec<String>, &'static str)>,
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
            effect_edit: None,
            quit: false,
            shaping_toggles: shaping::AxisToggles::default(),
            ffb_path: logi_dd_core::helpers::ffb_path(),
            shim_binary: logi_dd_core::helpers::installer_path(),
            sdk_dir: resolve_sdk_dir(),
            sdk_valid: false,
            sdk_edit: None,
            games: Vec::new(),
            game_idx: 0,
            games_scanned: false,
            test: TestView::default(),
            pending_shim: None,
        };
        a.sdk_valid = steam::sdk_dir_valid(Path::new(&a.sdk_dir));
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

    /// Whether the synthetic "Test" sidebar entry is selected.
    pub fn is_test(&self) -> bool {
        self.cat_idx == TEST_INDEX
    }

    /// Whether the main loop should poll with a short timeout and tick
    /// the evdev monitor instead of blocking on the next key.
    pub fn test_polling(&self) -> bool {
        self.is_test() && self.test.monitoring()
    }

    /// The wheel's configured rotation range for the Test view's degree
    /// conversion; 900 when unreadable.
    fn wheel_range(&self) -> u32 {
        match self.device.read("wheel_range") {
            Ok(Value::Int(n)) if n > 0 => n as u32,
            _ => 900,
        }
    }

    pub fn reload(&mut self) {
        if self.is_setup() || self.is_test() {
            self.rows.clear();
            return;
        }
        let cat = self.category();
        self.rows = if cat == Category::Leds {
            self.lightsync_rows()
        } else {
            let rows = REGISTRY
                .iter()
                .filter(|s| s.category == cat)
                .map(|s| Row {
                    attr: s.attr,
                    label: s.label,
                    available: self.device.available(s.attr),
                    value: self.device.read(s.attr),
                })
                .collect();
            self.shaping_rows(rows)
        };
        if self.row_idx >= self.rows.len() {
            self.row_idx = self.rows.len().saturating_sub(1);
        }
    }

    /// Compose a category's rows for the per-axis shaping toggles: when any
    /// row is a shaping generator (a sensitivity or a curve; see
    /// `shaping::role`), insert each axis's synthetic toggle row right
    /// before that axis's first row (its block heading) and keep only the
    /// rows `shaping::visible` allows for the current toggles (an axis on
    /// sensitivity hides its curve, an axis on the curve hides its
    /// sensitivity, deadzones and everything else stay). Rows for a
    /// category with no shaping generators pass through untouched, so
    /// `reload` can call this unconditionally, matching the GUI's
    /// `compose_shaping`.
    fn shaping_rows(&self, rows: Vec<Row>) -> Vec<Row> {
        if !rows.iter().any(|r| shaping::role(r.attr) != ShapingRole::Neutral) {
            return rows;
        }
        let mut out = Vec::with_capacity(rows.len() + shaping::Axis::ALL.len());
        let mut headed: Vec<shaping::Axis> = Vec::new();
        for row in rows {
            if let Some(ax) = shaping::axis(row.attr) {
                if !headed.contains(&ax) {
                    headed.push(ax);
                    out.push(Row {
                        attr: shaping::toggle_attr(ax),
                        label: shaping::toggle_label(ax),
                        available: true,
                        value: Ok(Value::Bool(self.shaping_toggles.get(ax))),
                    });
                }
            }
            if shaping::visible(row.attr, self.shaping_toggles) {
                out.push(row);
            }
        }
        out
    }

    /// Whether the on-screen category carries any shaping toggle rows
    /// (Steering and Pedals do).
    pub fn has_shaping_toggle(&self) -> bool {
        self.rows.iter().any(|r| shaping::toggle_axis(r.attr).is_some())
    }

    /// Flip one axis's shaping view toggle and re-compose the page. Pure
    /// view state: nothing is written to the device.
    pub fn toggle_shaping(&mut self, axis: shaping::Axis) {
        self.shaping_toggles.toggle(axis);
        self.status = format!(
            "{}: {}",
            shaping::toggle_label(axis),
            if self.shaping_toggles.get(axis) { "curve editor" } else { "sensitivity" }
        );
        self.reload();
    }

    /// Flip the shaping toggle for the axis the selected row belongs to
    /// (the 'a' shortcut): works on an axis's toggle row and on any of its
    /// sensitivity/deadzone/curve rows; a row with no axis does nothing.
    pub fn toggle_selected_axis(&mut self) {
        let Some(axis) = self
            .selected()
            .and_then(|r| shaping::toggle_axis(r.attr).or_else(|| shaping::axis(r.attr)))
        else {
            return;
        };
        self.toggle_shaping(axis);
    }

    fn led_row(&self, attr: &'static str, label: &'static str) -> Row {
        Row { attr, label, available: self.device.available(attr), value: self.device.read(attr) }
    }

    /// The composed LIGHTSYNC page: Effect (the 13-entry selector), the
    /// global Brightness, then, only while the custom effect (5) is
    /// active, the active slot's fields as an indented group, and the
    /// G PRO rev lights last. The slot-scoped registry rows stop being
    /// top-level rows; `wheel_led_slot` itself has no row at all (the
    /// selector's CUSTOM entries pick the slot).
    fn lightsync_rows(&self) -> Vec<Row> {
        let mut rows = vec![
            self.led_row("wheel_led_effect", "Effect"),
            self.led_row("wheel_led_brightness", "Brightness"),
        ];
        if matches!(self.device.read("wheel_led_effect"), Ok(Value::Int(5))) {
            rows.push(self.led_row("wheel_led_slot_name", "  Slot name"));
            rows.push(self.led_row("wheel_led_slot_brightness", "  Slot brightness"));
            rows.push(self.led_row("wheel_led_direction", "  Direction"));
            rows.push(self.led_row("wheel_led_colors", "  Colors"));
            rows.push(self.led_row("wheel_led_apply", "  Apply slot"));
        }
        rows.push(self.led_row("wheel_rev_level", "Rev lights (G PRO)"));
        rows
    }

    /// The per-slot names the effect selector's CUSTOM labels show. Only
    /// the ACTIVE slot's name is readable (`wheel_led_slot_name` reads the
    /// slot `wheel_led_slot` points at), so every other entry stays empty
    /// and falls back to the plain "CUSTOM N" label.
    fn led_slot_names(&self) -> Vec<String> {
        let mut names = vec![String::new(); lightsync::CUSTOM_SLOTS];
        if let (Ok(Value::Int(slot)), Ok(Value::Text(name))) =
            (self.device.read("wheel_led_slot"), self.device.read("wheel_led_slot_name"))
        {
            if let Some(entry) = usize::try_from(slot).ok().and_then(|i| names.get_mut(i)) {
                *entry = name;
            }
        }
        names
    }

    /// The selector label for the device's current effect (+ slot when the
    /// custom effect is active); what the Effect row shows at rest.
    pub fn lightsync_effect_label(&self) -> String {
        let effect = match self.device.read("wheel_led_effect") {
            Ok(Value::Int(n)) => n.clamp(0, u8::MAX as i32) as u8,
            _ => return "?".to_string(),
        };
        let slot = match self.device.read("wheel_led_slot") {
            Ok(Value::Int(n)) => n.clamp(0, lightsync::CUSTOM_SLOTS as i32 - 1) as u8,
            _ => 0,
        };
        let labels = lightsync::dropdown_labels(&self.led_slot_names(), effect);
        labels
            .into_iter()
            .nth(lightsync::selection_index(effect, slot))
            .unwrap_or_else(|| "?".to_string())
    }

    pub fn move_cat(&mut self, d: i32) {
        // +2 for the trailing Setup and Test entries, past the last real
        // category.
        let n = (Category::ALL.len() + 2) as i32;
        self.cat_idx = ((self.cat_idx as i32 + d).rem_euclid(n)) as usize;
        self.row_idx = 0;
        // First visit to the Setup view scans the Steam libraries; later
        // visits keep the last scan (r rescans on demand).
        if self.is_setup() && !self.games_scanned {
            self.scan_games();
        }
        // Every entry to the Test view re-runs the (cheap) evdev
        // discovery; leaving it always stops the monitor loop so no fd
        // stays open (and no polling keeps running) behind other views.
        if self.is_test() {
            let range = self.wheel_range();
            self.test.rescan(range);
        } else {
            self.test.stop_monitor();
        }
        self.reload();
    }

    /// Rescan the Steam libraries for installed Proton games (the Setup
    /// view's list). Blocking, like every other fs access in this
    /// synchronous TUI; a scan is a handful of small file reads.
    pub fn scan_games(&mut self) {
        self.games = match std::env::var_os("HOME") {
            Some(home) => steam::installed_games(&steam::library_roots(Path::new(&home))),
            None => Vec::new(),
        };
        self.games_scanned = true;
        if self.game_idx >= self.games.len() {
            self.game_idx = self.games.len().saturating_sub(1);
        }
    }

    /// The Setup view's selected game, if the list has any.
    pub fn selected_game(&self) -> Option<&SteamGame> {
        self.games.get(self.game_idx)
    }

    /// Take the shim run the last key press queued, if any; see
    /// `pending_shim`.
    pub fn take_pending_shim(&mut self) -> Option<(Vec<String>, &'static str)> {
        self.pending_shim.take()
    }

    /// Run the TrueForce SDK shim installer with `args` (a per-game
    /// `--prefix <pfx> --sdk-dir <dir>` install or `--uninstall-prefix
    /// <pfx>` remove), blocking: the TUI's event loop is synchronous, so
    /// there is no worker thread to hand this off to. The main loop calls
    /// this via the `pending_shim` queue so a "running..." status gets
    /// drawn first, then rescans the games list so the row's shim status
    /// updates. Never sudo. A missing binary or a spawn failure lands in
    /// the status line instead of taking the TUI down.
    pub fn run_shim(&mut self, args: &[String], verb: &str) {
        let Some(bin) = self.shim_binary.clone() else {
            self.status = "shim: installer not found (PATH or the repo's tools/)".to_string();
            return;
        };
        match std::process::Command::new(&bin).args(args).output() {
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
                self.status = format!("shim {verb}: failed to run {}: {e}", bin.display());
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
        // A per-axis shaping row is view state, not a device attribute:
        // Enter flips it in place (same as the 'a' shortcut), no editor.
        if let Some(axis) = shaping::toggle_axis(attr) {
            self.toggle_shaping(axis);
            return;
        }
        let Some(spec) = Device::<S>::spec(attr) else { return };
        if spec.access == Access::ReadOnly {
            return;
        }
        // The LIGHTSYNC Effect row cycles the 13 selector entries, not the
        // raw 1-9 value; it gets its own little modal (see `EffectEdit`).
        if attr == "wheel_led_effect" {
            let effect = match self.device.read(attr) {
                Ok(Value::Int(n)) => n.clamp(0, u8::MAX as i32) as u8,
                _ => {
                    self.status = "cannot edit (value unreadable)".into();
                    return;
                }
            };
            let slot = match self.device.read("wheel_led_slot") {
                Ok(Value::Int(n)) => n.clamp(0, lightsync::CUSTOM_SLOTS as i32 - 1) as u8,
                _ => 0,
            };
            self.effect_edit = Some(EffectEdit {
                index: lightsync::selection_index(effect, slot),
                labels: lightsync::dropdown_labels(&self.led_slot_names(), effect),
            });
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

    /// Commit the effect selector: a sweep/numbered entry writes
    /// `wheel_led_effect` directly; a CUSTOM entry writes `wheel_led_slot`
    /// first and then `wheel_led_effect = 5` (the driver re-applies the
    /// slot's stored config on that transition), the same two-write order
    /// the GUI uses.
    pub fn commit_effect_edit(&mut self) {
        let Some(fe) = self.effect_edit.take() else { return };
        // The raw current effect only matters when the trailing raw entry
        // (shown while the device reports an effect outside 1-5) is
        // re-picked: that entry commits the same value back.
        let current = match self.device.read("wheel_led_effect") {
            Ok(Value::Int(n)) => n.clamp(0, i32::from(u8::MAX)) as u8,
            _ => 1,
        };
        let result = match lightsync::index_selection(fe.index, current) {
            lightsync::Selection::Effect(e) => {
                self.device.write("wheel_led_effect", &Value::Int(i32::from(e)))
            }
            lightsync::Selection::Custom(slot) => self
                .device
                .write("wheel_led_slot", &Value::Int(i32::from(slot)))
                .and_then(|()| self.device.write("wheel_led_effect", &Value::Int(5))),
        };
        self.status = match result {
            Ok(()) => {
                let label = fe.labels.get(fe.index).map(String::as_str).unwrap_or("?");
                format!("Effect set: {label}")
            }
            Err(e) => format!("Effect: {e}"),
        };
        self.reload();
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

    /// Make a `wheel_led_colors` write match a mirrored direction: when
    /// the slot's direction is inside-out/outside-in the wheel plays the
    /// 10 LEDs as 5 pairs, so the left half is mirrored onto the right
    /// (left wins) before the write. Any other value, attr or direction
    /// passes through untouched.
    fn mirror_colors_if_needed(&self, attr: &str, v: Value) -> Value {
        if attr != "wheel_led_colors" {
            return v;
        }
        let direction = match self.device.read("wheel_led_direction") {
            Ok(Value::Enum(d)) => d,
            _ => return v,
        };
        match v {
            Value::Rgb(mut colors) if lightsync::mirrored(direction) => {
                lightsync::mirror_left_half(&mut colors);
                Value::Rgb(colors)
            }
            v => v,
        }
    }

    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let attr = e.attr;
        let label = Device::<S>::spec(attr).map(|s| s.label).unwrap_or(attr);
        match e
            .commit_value()
            .map(|v| self.mirror_colors_if_needed(attr, v))
            .and_then(|v| self.device.write(attr, &v))
        {
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

    /// Start or stop the Test view's live monitor (the Enter toggle).
    fn toggle_test_monitor(&mut self) {
        if self.test.monitoring() {
            self.test.stop_monitor();
            self.status = "test: monitoring stopped".to_string();
            return;
        }
        if self.test.dev.is_none() {
            self.status = "test: no wheel found (r to rescan)".to_string();
            return;
        }
        self.status = if self.test.start_monitor() {
            "test: monitoring live input (Enter stops)".to_string()
        } else {
            format!("test: {}", self.test.open_error.as_deref().unwrap_or("cannot open device"))
        };
    }

    /// Arm a simulation: everything is gated behind the y/n confirmation
    /// the status line shows; see `on_key`'s Test branch.
    fn request_sim(&mut self, kind: SimKind) {
        if self.test.dev.is_none() {
            self.status = "test: no wheel found (r to rescan)".to_string();
            return;
        }
        if self.test.sim_running() {
            self.status = "test: a simulation is already playing".to_string();
            return;
        }
        self.test.confirm = Some(kind);
        self.status = format!(
            "{}: the wheel WILL move. Keep hands and objects clear of the rim. y continues, any other key cancels",
            kind.label()
        );
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
        if let Some(fe) = self.effect_edit.as_mut() {
            let n = fe.labels.len().max(1);
            match key {
                Enter => self.commit_effect_edit(),
                Esc => self.effect_edit = None,
                Left => fe.index = (fe.index + n - 1) % n,
                Right => fe.index = (fe.index + 1) % n,
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
        if self.is_test() {
            // A pending sim confirmation swallows the next key: only 'y'
            // fires the effect, anything else cancels. Nothing ever plays
            // without this explicit step.
            if let Some(kind) = self.test.confirm.take() {
                self.status = if matches!(key, Char('y') | Char('Y')) {
                    self.test.spawn_sim(kind)
                } else {
                    "test: simulation cancelled".to_string()
                };
                return;
            }
            match key {
                Char('q') => self.quit = true,
                Left => self.move_cat(-1),
                Right => self.move_cat(1),
                Enter => self.toggle_test_monitor(),
                Char('r') => {
                    let range = self.wheel_range();
                    self.test.rescan(range);
                    self.status = match &self.test.dev {
                        Some(d) => format!("test: found {} ({})", d.name, d.event_path),
                        None => "test: no wheel found".to_string(),
                    };
                }
                Char('f') => self.request_sim(SimKind::ConstantForce),
                Char('t') => self.request_sim(SimKind::Texture),
                _ => {}
            }
            return;
        }
        if self.is_setup() {
            // The SDK-dir line editor swallows every key while active, so
            // typing a path cannot trigger the view's shortcuts.
            if let Some(draft) = self.sdk_edit.as_mut() {
                match key {
                    Enter => {
                        self.sdk_dir = self.sdk_edit.take().unwrap_or_default();
                        self.sdk_valid = steam::sdk_dir_valid(Path::new(&self.sdk_dir));
                        self.status = if self.sdk_valid {
                            "SDK folder set (DLLs found)".to_string()
                        } else {
                            format!("SDK folder set, but no trueforce_sdk_x64.dll under {}/Logi/...", self.sdk_dir)
                        };
                    }
                    Esc => self.sdk_edit = None,
                    Backspace => {
                        draft.pop();
                    }
                    Char(c) => draft.push(c),
                    _ => {}
                }
                return;
            }
            match key {
                Char('q') => self.quit = true,
                Left => self.move_cat(-1),
                Right => self.move_cat(1),
                Up => self.game_idx = self.game_idx.saturating_sub(1),
                Down => {
                    if self.game_idx + 1 < self.games.len() {
                        self.game_idx += 1;
                    }
                }
                Char('r') => {
                    self.scan_games();
                    self.status = format!("rescanned: {} Proton game(s)", self.games.len());
                }
                Char('s') => self.sdk_edit = Some(self.sdk_dir.clone()),
                // Queued, not run here: the main loop draws a "running..."
                // status line before the blocking run (see `pending_shim`).
                Char('i') => match self.selected_game() {
                    Some(game) => {
                        let pfx = game.prefix.to_string_lossy().into_owned();
                        let args = vec![
                            "--prefix".to_string(),
                            pfx,
                            "--sdk-dir".to_string(),
                            self.sdk_dir.clone(),
                        ];
                        self.pending_shim = Some((args, "install"));
                    }
                    None => self.status = "shim install: no game selected".to_string(),
                },
                Char('u') => match self.selected_game() {
                    Some(game) => {
                        let pfx = game.prefix.to_string_lossy().into_owned();
                        self.pending_shim = Some((vec!["--uninstall-prefix".to_string(), pfx], "uninstall"));
                    }
                    None => self.status = "shim uninstall: no game selected".to_string(),
                },
                _ => {}
            }
            return;
        }
        match key {
            Char('q') => self.quit = true,
            Char('r') => self.reload(),
            Char('d') => self.toggle_mode(),
            // Only meaningful on a row that belongs to a shaping axis (a
            // toggle row or one of the axis's own rows); a plain typo
            // elsewhere does nothing.
            Char('a') => self.toggle_selected_axis(),
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
        // A curve row only shows while its axis's shaping toggle is on (the
        // default is the simple sensitivity view).
        a.shaping_toggles.set(shaping::Axis::Throttle, true);
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
    fn move_cat_reaches_and_leaves_setup_and_test() {
        let mut a = app();
        // step through every real category, landing on Setup right after
        // the last one.
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup(), "cat_idx {} should be the Setup entry", a.cat_idx);
        assert!(a.rows.is_empty(), "Setup has no settings rows");
        // then the Test entry, then a wrap back to the first category.
        a.move_cat(1);
        assert!(a.is_test(), "cat_idx {} should be the Test entry", a.cat_idx);
        assert!(a.rows.is_empty(), "Test has no settings rows");
        assert!(a.test.scanned, "entering the Test view runs discovery");
        a.move_cat(1);
        assert!(!a.is_setup());
        assert!(!a.is_test());
        assert_eq!(a.cat_idx, 0);
        // stepping backward from the first category reaches Test too.
        a.move_cat(-1);
        assert!(a.is_test());
    }

    /// An app parked on the Test view (no real wheel is asserted on; the
    /// discovery the entry runs is overwritten per test).
    fn test_view_app() -> App<FakeSysfs> {
        let mut a = app();
        for _ in 0..Category::ALL.len() + 1 {
            a.move_cat(1);
        }
        assert!(a.is_test());
        a
    }

    #[test]
    fn test_sim_keys_without_a_wheel_report_instead_of_arming() {
        use crossterm::event::KeyCode;
        let mut a = test_view_app();
        a.test.dev = None;
        a.on_key(KeyCode::Char('f'));
        assert!(a.test.confirm.is_none(), "nothing armed without a wheel");
        assert!(a.status.contains("no wheel"), "status: {}", a.status);
        a.on_key(KeyCode::Char('t'));
        assert!(a.test.confirm.is_none());
    }

    #[test]
    fn test_sim_keys_arm_a_confirm_and_anything_but_y_cancels() {
        use crate::wheel_test::SimKind;
        use crossterm::event::KeyCode;
        let mut a = test_view_app();
        a.test.dev = Some(logi_dd_core::evtest::WheelInput {
            event_path: "/nonexistent/event99".to_string(),
            name: "Logitech RS50 Base".to_string(),
        });
        a.on_key(KeyCode::Char('f'));
        assert_eq!(a.test.confirm, Some(SimKind::ConstantForce));
        assert!(a.status.contains("WILL move"), "safety text shown: {}", a.status);
        a.on_key(KeyCode::Char('n'));
        assert!(a.test.confirm.is_none());
        assert!(a.status.contains("cancelled"), "status: {}", a.status);
        assert!(!a.test.sim_running(), "nothing played");
        // 't' arms the texture sim; Esc cancels it too.
        a.on_key(KeyCode::Char('t'));
        assert_eq!(a.test.confirm, Some(SimKind::Texture));
        a.on_key(KeyCode::Esc);
        assert!(a.test.confirm.is_none());
        assert!(!a.test.sim_running());
    }

    #[test]
    fn test_enter_without_a_wheel_does_not_start_monitoring() {
        use crossterm::event::KeyCode;
        let mut a = test_view_app();
        a.test.dev = None;
        a.on_key(KeyCode::Enter);
        assert!(!a.test.monitoring());
        assert!(a.status.contains("no wheel"), "status: {}", a.status);
        assert!(!a.test_polling());
    }

    #[test]
    fn leaving_the_test_view_stops_the_monitor() {
        let mut a = test_view_app();
        // Simulate a live monitor without a device by checking the flag
        // path only: start_monitor on a missing dev is a no-op, so drive
        // the state through rescan + move_cat instead.
        a.move_cat(1);
        assert!(!a.is_test());
        assert!(!a.test.monitoring(), "monitor never survives leaving the view");
        assert!(!a.test_polling());
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

    /// A Setup-view app with two fake games in the list (no real Steam
    /// scan results are asserted on; the scan the view entry triggers is
    /// simply overwritten).
    fn setup_app() -> App<FakeSysfs> {
        let mut a = app();
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup());
        a.games = vec![
            SteamGame {
                appid: 100,
                name: "ACC".to_string(),
                prefix: PathBuf::from("/lib/steamapps/compatdata/100/pfx"),
                shim_installed: false,
            },
            SteamGame {
                appid: 400,
                name: "LMU".to_string(),
                prefix: PathBuf::from("/lib/steamapps/compatdata/400/pfx"),
                shim_installed: true,
            },
        ];
        a.game_idx = 0;
        a.games_scanned = true;
        a
    }

    #[test]
    fn setup_keys_queue_a_per_game_shim_run_instead_of_running_it() {
        use crossterm::event::KeyCode;
        let mut a = setup_app();
        a.sdk_dir = "/sdk".to_string();
        a.on_key(KeyCode::Char('i'));
        assert_eq!(
            a.take_pending_shim(),
            Some((
                vec![
                    "--prefix".to_string(),
                    "/lib/steamapps/compatdata/100/pfx".to_string(),
                    "--sdk-dir".to_string(),
                    "/sdk".to_string(),
                ],
                "install"
            ))
        );
        // Taken once; a second take finds nothing queued.
        assert_eq!(a.take_pending_shim(), None);
        a.on_key(KeyCode::Down); // select the second game
        a.on_key(KeyCode::Char('u'));
        assert_eq!(
            a.take_pending_shim(),
            Some((
                vec!["--uninstall-prefix".to_string(), "/lib/steamapps/compatdata/400/pfx".to_string()],
                "uninstall"
            ))
        );
    }

    #[test]
    fn setup_game_selection_clamps_at_both_ends() {
        use crossterm::event::KeyCode;
        let mut a = setup_app();
        a.on_key(KeyCode::Up);
        assert_eq!(a.game_idx, 0, "up from the first game stays");
        a.on_key(KeyCode::Down);
        assert_eq!(a.game_idx, 1);
        a.on_key(KeyCode::Down);
        assert_eq!(a.game_idx, 1, "down from the last game stays");
    }

    #[test]
    fn setup_with_no_games_reports_instead_of_queueing() {
        use crossterm::event::KeyCode;
        let mut a = setup_app();
        a.games.clear();
        a.game_idx = 0;
        a.on_key(KeyCode::Char('i'));
        assert_eq!(a.take_pending_shim(), None);
        assert!(a.status.contains("no game selected"), "status: {}", a.status);
    }

    #[test]
    fn sdk_dir_edit_commits_on_enter_and_discards_on_esc() {
        use crossterm::event::KeyCode;
        let mut a = setup_app();
        a.sdk_dir = "/old".to_string();
        a.on_key(KeyCode::Char('s'));
        assert_eq!(a.sdk_edit.as_deref(), Some("/old"));
        // While editing, view shortcuts are plain characters.
        for _ in 0.."/old".len() {
            a.on_key(KeyCode::Backspace);
        }
        for c in "/new".chars() {
            a.on_key(KeyCode::Char(c));
        }
        a.on_key(KeyCode::Enter);
        assert_eq!(a.sdk_dir, "/new");
        assert!(a.sdk_edit.is_none());
        assert!(!a.sdk_valid, "no marker DLL under /new");
        a.on_key(KeyCode::Char('s'));
        a.on_key(KeyCode::Char('x'));
        a.on_key(KeyCode::Esc);
        assert_eq!(a.sdk_dir, "/new", "Esc keeps the committed dir");
        assert!(a.sdk_edit.is_none());
    }

    #[test]
    fn run_shim_reports_missing_binary_without_spawning() {
        let mut a = app();
        a.shim_binary = None;
        a.run_shim(&["--prefix".to_string(), "/x".to_string()], "install");
        assert!(a.status.contains("not found"), "status: {}", a.status);
    }

    #[test]
    fn run_shim_reports_success() {
        let mut a = app();
        a.shim_binary = Some(PathBuf::from("true")); // exists on PATH, exits 0, ignores args
        a.run_shim(&["--prefix".to_string(), "/x".to_string()], "install");
        assert_eq!(a.status, "shim install: ok");
    }

    #[test]
    fn run_shim_reports_failure_without_crashing() {
        let mut a = app();
        a.shim_binary = Some(PathBuf::from("false")); // exists on PATH, exits non-zero
        a.run_shim(&["--uninstall-prefix".to_string(), "/x".to_string()], "uninstall");
        assert!(a.status.starts_with("shim uninstall:"), "status: {}", a.status);
        assert_ne!(a.status, "shim uninstall: ok");
    }

    #[test]
    fn run_shim_reports_spawn_failure_without_crashing() {
        let mut a = app();
        a.shim_binary = Some(PathBuf::from("this-binary-does-not-exist-anywhere"));
        a.run_shim(&["--prefix".to_string(), "/x".to_string()], "install");
        assert!(a.status.contains("failed to run"), "status: {}", a.status);
    }

    // --- LIGHTSYNC page ---

    const TEN_COLORS: &str =
        "ff0000 00ff00 0000ff 111111 222222 000000 000000 000000 000000 000000";

    fn leds_app(effect: &str, direction: &str) -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", effect);
        fs.set("wheel_led_slot", "0");
        fs.set("wheel_led_brightness", "80");
        fs.set("wheel_led_slot_name", "RACE");
        fs.set("wheel_led_slot_brightness", "70");
        fs.set("wheel_led_direction", direction);
        fs.set("wheel_led_colors", TEN_COLORS);
        fs.set("wheel_rev_level", "0");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Leds).unwrap();
        a.reload();
        a
    }

    #[test]
    fn lightsync_page_hides_the_slot_group_for_builtin_effects() {
        let a = leds_app("3", "0");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert_eq!(attrs, vec!["wheel_led_effect", "wheel_led_brightness", "wheel_rev_level"]);
    }

    #[test]
    fn lightsync_page_shows_the_indented_slot_group_for_the_custom_effect() {
        let a = leds_app("5", "0");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert_eq!(
            attrs,
            vec![
                "wheel_led_effect",
                "wheel_led_brightness",
                "wheel_led_slot_name",
                "wheel_led_slot_brightness",
                "wheel_led_direction",
                "wheel_led_colors",
                "wheel_led_apply",
                "wheel_rev_level",
            ]
        );
        let name_row = a.rows.iter().find(|r| r.attr == "wheel_led_slot_name").unwrap();
        assert!(name_row.label.starts_with("  "), "slot rows are indented: {:?}", name_row.label);
    }

    #[test]
    fn effect_row_opens_the_selector_with_the_current_entry() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("5", "0");
        a.row_idx = 0;
        a.on_key(KeyCode::Enter);
        let fe = a.effect_edit.as_ref().expect("Enter opens the effect selector");
        assert_eq!(fe.labels.len(), 9, "4 sweeps + 5 custom slots, no unlabeled effects");
        assert_eq!(fe.index, 4, "effect 5 + slot 0 = CUSTOM 1");
        assert_eq!(fe.labels[4], "CUSTOM 1: RACE", "the active slot's name is shown");
        assert!(a.edit.is_none(), "not the inline field editor");
    }

    #[test]
    fn effect_selector_cycles_and_wraps_both_ways() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("1", "0");
        a.row_idx = 0;
        a.on_key(KeyCode::Enter);
        assert_eq!(a.effect_edit.as_ref().unwrap().index, 0);
        a.on_key(KeyCode::Left);
        assert_eq!(a.effect_edit.as_ref().unwrap().index, 8, "left from the first entry wraps");
        a.on_key(KeyCode::Right);
        assert_eq!(a.effect_edit.as_ref().unwrap().index, 0);
        a.on_key(KeyCode::Esc);
        assert!(a.effect_edit.is_none(), "Esc discards without writing");
        assert_eq!(a.device.read("wheel_led_effect").unwrap(), Value::Int(1));
    }

    #[test]
    fn effect_selector_shows_a_raw_entry_only_for_an_out_of_range_effect() {
        use crossterm::event::KeyCode;
        // The driver accepts 1-9; a device reporting 7 gets a trailing
        // "Effect 7" entry so the selector reflects the real state, and
        // committing it re-writes the same value.
        let mut a = leds_app("7", "0");
        a.row_idx = 0;
        assert_eq!(a.lightsync_effect_label(), "Effect 7");
        a.on_key(KeyCode::Enter);
        {
            let fe = a.effect_edit.as_ref().unwrap();
            assert_eq!(fe.labels.len(), 10);
            assert_eq!(fe.index, 9, "the raw entry is selected");
            assert_eq!(fe.labels[9], "Effect 7");
        }
        a.on_key(KeyCode::Enter); // commit unchanged
        assert_eq!(a.device.read("wheel_led_effect").unwrap(), Value::Int(7));
    }

    #[test]
    fn effect_selector_custom_commit_writes_slot_then_effect() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("1", "0");
        a.row_idx = 0;
        a.on_key(KeyCode::Enter);
        for _ in 0..6 {
            a.on_key(KeyCode::Right); // index 6 = CUSTOM 3
        }
        a.on_key(KeyCode::Enter);
        assert!(a.effect_edit.is_none());
        assert_eq!(a.device.read("wheel_led_slot").unwrap(), Value::Int(2));
        assert_eq!(a.device.read("wheel_led_effect").unwrap(), Value::Int(5));
        // the slot group appears after the reload the commit triggers
        assert!(a.rows.iter().any(|r| r.attr == "wheel_led_colors"));
    }

    #[test]
    fn effect_selector_sweep_commit_writes_only_the_effect() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("1", "0");
        a.row_idx = 0;
        a.on_key(KeyCode::Enter);
        a.on_key(KeyCode::Right); // index 1 = Outside in = effect 2
        a.on_key(KeyCode::Enter);
        assert_eq!(a.device.read("wheel_led_effect").unwrap(), Value::Int(2));
        assert_eq!(a.device.read("wheel_led_slot").unwrap(), Value::Int(0), "slot untouched");
    }

    #[test]
    fn colors_commit_mirrors_the_left_half_when_the_direction_mirrors() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("5", "2"); // inside-out
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_led_colors").unwrap();
        a.on_key(KeyCode::Enter); // open the raw color entry
        a.on_key(KeyCode::Enter); // commit unchanged
        match a.device.read("wheel_led_colors").unwrap() {
            Value::Rgb(cs) => {
                for i in 0..5 {
                    assert_eq!(cs[9 - i], cs[i], "LED {} mirrors LED {}", 10 - i, i + 1);
                }
                assert_eq!(cs[0].to_hex(), "ff0000", "the left half is untouched");
                assert_eq!(cs[4].to_hex(), "222222");
            }
            v => panic!("expected colors, got {v:?}"),
        }
    }

    #[test]
    fn colors_commit_stays_untouched_for_a_sweep_direction() {
        use crossterm::event::KeyCode;
        let mut a = leds_app("5", "0"); // left to right
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_led_colors").unwrap();
        a.on_key(KeyCode::Enter);
        a.on_key(KeyCode::Enter);
        match a.device.read("wheel_led_colors").unwrap() {
            Value::Rgb(cs) => {
                assert_eq!(cs[9].to_hex(), "000000", "no mirroring for a sweep");
                assert_eq!(cs[0].to_hex(), "ff0000");
            }
            v => panic!("expected colors, got {v:?}"),
        }
    }

    #[test]
    fn effect_label_names_the_active_custom_slot() {
        let a = leds_app("5", "0");
        assert_eq!(a.lightsync_effect_label(), "CUSTOM 1: RACE");
        let b = leds_app("4", "0");
        assert_eq!(b.lightsync_effect_label(), "Left to right");
    }

    // --- advanced shaping toggle ---

    fn steering_app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_range", "900");
        fs.set("wheel_sensitivity", "50");
        fs.set("wheel_response_curve", "reset");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Steering).unwrap();
        a.reload();
        a
    }

    #[test]
    fn shaping_toggles_head_each_axis_block_on_steering_and_pedals_only() {
        use shaping::Axis;
        let a = steering_app();
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        // The steering toggle heads the axis block (right before the
        // sensitivity row), not the whole page.
        let toggle = attrs.iter().position(|a| *a == shaping::toggle_attr(Axis::Steering)).unwrap();
        assert_eq!(attrs[toggle + 1], "wheel_sensitivity");
        let mut p = steering_app();
        p.cat_idx = Category::ALL.iter().position(|c| *c == Category::Pedals).unwrap();
        p.reload();
        let pattrs: Vec<&str> = p.rows.iter().map(|r| r.attr).collect();
        for ax in [Axis::Throttle, Axis::Brake, Axis::Clutch, Axis::Handbrake] {
            assert!(pattrs.contains(&shaping::toggle_attr(ax)), "missing {ax:?} toggle");
        }
        let ffb = app(); // first category, Ffb
        assert!(!ffb.has_shaping_toggle(), "Ffb has no shaping generators");
    }

    #[test]
    fn simple_mode_shows_sensitivity_and_hides_curves() {
        let a = steering_app();
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(attrs.contains(&"wheel_sensitivity"));
        assert!(!attrs.contains(&"wheel_response_curve"));
    }

    #[test]
    fn enter_on_a_toggle_row_switches_its_axis_without_a_device_write() {
        use crossterm::event::KeyCode;
        let mut a = steering_app();
        a.row_idx =
            a.rows.iter().position(|r| r.attr == shaping::toggle_attr(shaping::Axis::Steering)).unwrap();
        a.on_key(KeyCode::Enter);
        assert!(a.shaping_toggles.get(shaping::Axis::Steering));
        assert!(a.edit.is_none(), "no inline editor opens");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(attrs.contains(&"wheel_response_curve"));
        assert!(!attrs.contains(&"wheel_sensitivity"));
        // Pure view state: the reserved attr never reaches the device.
        assert!(a.device.read(shaping::toggle_attr(shaping::Axis::Steering)).is_err());
    }

    #[test]
    fn a_key_toggles_the_selected_rows_axis_and_back() {
        use crossterm::event::KeyCode;
        let mut a = steering_app();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_sensitivity").unwrap();
        a.on_key(KeyCode::Char('a'));
        assert!(a.shaping_toggles.get(shaping::Axis::Steering));
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_response_curve").unwrap();
        a.on_key(KeyCode::Char('a'));
        assert!(!a.shaping_toggles.get(shaping::Axis::Steering));
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(attrs.contains(&"wheel_sensitivity"), "back to the simple view");
    }

    #[test]
    fn a_key_does_nothing_on_a_row_without_an_axis() {
        use crossterm::event::KeyCode;
        let mut a = app(); // Ffb
        let before: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        a.on_key(KeyCode::Char('a'));
        assert_eq!(a.shaping_toggles, shaping::AxisToggles::default());
        let after: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn pedal_axes_toggle_independently_and_keep_deadzones() {
        let mut a = steering_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Pedals).unwrap();
        a.reload();
        let simple: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(simple.contains(&"wheel_throttle_deadzone"));
        assert!(simple.contains(&"wheel_throttle_sensitivity"));
        assert!(!simple.contains(&"wheel_throttle_curve"));
        // Brake to the curve view; the throttle stays on sensitivity.
        a.toggle_shaping(shaping::Axis::Brake);
        let mixed: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(mixed.contains(&"wheel_brake_curve"));
        assert!(!mixed.contains(&"wheel_brake_sensitivity"));
        assert!(mixed.contains(&"wheel_brake_deadzone"));
        assert!(mixed.contains(&"wheel_throttle_sensitivity"));
        assert!(!mixed.contains(&"wheel_throttle_curve"));
        // Every axis on the curve view: no sensitivities remain.
        for ax in [shaping::Axis::Throttle, shaping::Axis::Clutch, shaping::Axis::Handbrake] {
            a.toggle_shaping(ax);
        }
        let curves: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(curves.contains(&"wheel_throttle_curve"));
        assert!(curves.contains(&"wheel_handbrake_curve"));
        assert!(curves.contains(&"wheel_clutch_deadzone"));
        assert!(!curves.iter().any(|a| a.ends_with("_sensitivity")));
    }

    #[test]
    fn shaping_state_survives_a_category_round_trip() {
        let mut a = steering_app();
        a.toggle_shaping(shaping::Axis::Steering);
        assert!(a.shaping_toggles.get(shaping::Axis::Steering));
        a.move_cat(-1); // away to Ffb
        a.move_cat(1); // and back to Steering
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr).collect();
        assert!(attrs.contains(&"wheel_response_curve"), "still the curve view after navigation");
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
