use crate::curve_editor::CurveEditor;
use crate::edit;
use crate::wheel_test::{SimKind, TestView};
use logi_dd_core::profiles;
use logi_dd_core::setting::Access;
use logi_dd_core::shaping::{self, ShapingRole};
use logi_dd_core::lightsync;
use logi_dd_core::steam::{self, SteamGame};
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Kind, Mode, Value, REGISTRY};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One rendered settings row. `attr`/`label` are owned strings rather than
/// registry `&'static str`s because the desktop Profiles page also renders
/// synthetic rows for the computer-side profile store, named after the
/// saved profiles.
pub struct Row {
    pub attr: String,
    pub label: String,
    pub value: Result<Value, Error>,
    pub available: bool,
}

/// The prefix the desktop Profiles page's saved-profile rows wear:
/// `profile:<name>`. Not sysfs attributes; `on_key` intercepts them
/// (Enter applies, `d` arms a delete) before any editor could open.
pub const PROFILE_ROW_PREFIX: &str = "profile:";

/// The desktop Profiles page's trailing "Save current as..." row. The
/// dash (no colon) keeps it distinct from every saved-profile row, even
/// one literally named "new".
pub const PROFILE_NEW_ATTR: &str = "profile-new";

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
/// rather than "look up `Category::ALL[cat_idx]`". It is the only synthetic
/// entry: the live input tester lives on the Info category's page.
pub const SETUP_INDEX: usize = Category::ALL.len();

/// Resolve the SDK folder the Setup view starts with:
/// `$LOGITECH_TRUEFORCE_SDK_DIR` when set, else
/// `~/.local/share/logitech-trueforce/sdk` (the installer script's own
/// default). Editable in the view (the `s` key); whatever it holds is
/// passed as `--sdk-dir` to install runs while it validates.
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
    /// The SDK folder per-game installs pass as `--sdk-dir` while it
    /// validates (an invalid one is omitted so the installer's own
    /// lookup runs); see
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
    /// Where the computer-side profile store lives
    /// (`profiles::default_dir()`); overridable in tests.
    pub profiles_dir: PathBuf,
    /// The new-profile name prompt's draft, `Some` while the desktop
    /// Profiles page's `n` (or the Save row's Enter) is active: type to
    /// append, Backspace to erase, Enter saves, Esc discards.
    pub profile_name_edit: Option<String>,
    /// A profile delete waiting for its y/n confirmation (the profile's
    /// name); armed by `d` on a saved-profile row.
    pub profile_delete_confirm: Option<String>,
    /// A shim run queued by `on_key` for the main loop to execute:
    /// `(installer args, verb for the status line)`. The run blocks, so
    /// instead of running inside the key handler, where the "running..."
    /// status could never be drawn first, the loop takes this via
    /// `take_pending_shim`, draws once, then calls `run_shim`.
    pending_shim: Option<(Vec<String>, &'static str)>,
    /// Whether the wrapped device probes as absent (no registry attribute
    /// readable): the shell stays up with empty device categories and a
    /// red header note, Setup and the Info monitor keep working, and `r`
    /// queues a re-discovery. Refreshed by `adopt_device`.
    pub no_wheel: bool,
    /// A re-discovery queued by `r` in the no-wheel state; the main loop
    /// takes it (`take_retry_request`), runs `Device::discover()` (which
    /// only exists for the real sysfs type) and hands any find back via
    /// `adopt_device`.
    retry_requested: bool,
    /// The last `(wheel_profile, wheel_mode)` observation, for external-
    /// change detection: the wheel's physical profile button (or another
    /// tool writing sysfs) changes settings without any key passing through
    /// `on_key`, and `check_drift` (run by the main loop's idle ticks)
    /// reloads the view when this pair moves. Resynced by every `reload`,
    /// so the app's own edits never read as drift.
    last_drift: Option<(Option<Value>, Option<Value>)>,
}

/// Whether `device` looks like a real wheel: at least one registry
/// attribute is present. Mirrors `Device::discover`'s wheel_range probe
/// without insisting on that one attribute (a test fake may expose any
/// subset).
fn wheel_present<S: SysfsIo>(device: &Device<S>) -> bool {
    REGISTRY.iter().any(|s| device.available(s.attr))
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
            profiles_dir: profiles::default_dir(),
            profile_name_edit: None,
            profile_delete_confirm: None,
            pending_shim: None,
            no_wheel: false,
            retry_requested: false,
            last_drift: None,
        };
        a.no_wheel = !wheel_present(&a.device);
        a.sdk_valid = steam::sdk_dir_valid(Path::new(&a.sdk_dir));
        a.reload();
        a
    }

    /// Swap in a freshly discovered device (the `r` retry's success path)
    /// and bring the whole view back to life.
    pub fn adopt_device(&mut self, device: Device<S>) {
        self.device = device;
        self.no_wheel = !wheel_present(&self.device);
        self.status = if self.no_wheel {
            "no wheel found (r to retry)".to_string()
        } else {
            "wheel found".to_string()
        };
        if self.is_info() && !self.no_wheel {
            self.rescan_input();
        }
        self.reload();
    }

    /// Take the queued re-discovery, if any; see `retry_requested`.
    pub fn take_retry_request(&mut self) -> bool {
        std::mem::take(&mut self.retry_requested)
    }

    pub fn category(&self) -> Category {
        Category::ALL[self.cat_idx]
    }

    /// Whether the synthetic "Setup" sidebar entry is selected.
    pub fn is_setup(&self) -> bool {
        self.cat_idx == SETUP_INDEX
    }

    /// Whether the Info category is selected (the page that carries the
    /// live input monitor alongside the identity rows).
    pub fn is_info(&self) -> bool {
        !self.is_setup() && self.category() == Category::Info
    }

    /// Whether the main loop should poll with a short timeout and tick
    /// the evdev monitor instead of blocking on the next key.
    pub fn test_polling(&self) -> bool {
        self.is_info() && self.test.monitoring()
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
        // Every reload resyncs the drift baseline: the values on screen are
        // (about to be) exactly what the device reports now, so only a LATER
        // external write should count as drift.
        self.last_drift = Some(self.drift_observation());
        if self.is_setup() {
            self.rows.clear();
            return;
        }
        // Without a wheel there is nothing to read: every device category
        // (Info included) shows its empty state instead of rows.
        if self.no_wheel {
            self.rows.clear();
            self.row_idx = 0;
            return;
        }
        let cat = self.category();
        self.rows = if cat == Category::Leds {
            self.lightsync_rows()
        } else if cat == Category::Profiles {
            self.profiles_rows()
        } else {
            let rows = self.registry_rows(cat);
            self.shaping_rows(rows)
        };
        if self.row_idx >= self.rows.len() {
            self.row_idx = self.rows.len().saturating_sub(1);
        }
    }

    /// One drift observation: what `wheel_profile`/`wheel_mode` read right
    /// now, an unreadable one as `None` (absent on this wheel, or the wheel
    /// is gone).
    fn drift_observation(&self) -> (Option<Value>, Option<Value>) {
        (self.device.read("wheel_profile").ok(), self.device.read("wheel_mode").ok())
    }

    /// External-change detection, run by the main loop's idle ticks: when
    /// `wheel_profile`/`wheel_mode` moved since the last observation (the
    /// wheel's physical profile button, another tool writing sysfs), reload
    /// the current view exactly like the refresh key would (the header re-
    /// reads the mode on every draw already) and say so in the status line.
    /// Skipped while any editor, prompt or confirmation is open, so a
    /// reload can never yank state from under one; the check right after it
    /// closes still catches up. Returns whether a refresh happened.
    pub fn check_drift(&mut self) -> bool {
        if self.no_wheel
            || self.edit.is_some()
            || self.curve_edit.is_some()
            || self.effect_edit.is_some()
            || self.sdk_edit.is_some()
            || self.profile_name_edit.is_some()
            || self.profile_delete_confirm.is_some()
            || self.test.confirm.is_some()
        {
            return false;
        }
        let seen = self.drift_observation();
        let drifted = self.last_drift.as_ref().is_some_and(|last| *last != seen);
        self.last_drift = Some(seen);
        if !drifted {
            return false;
        }
        // The wheel may have gone away entirely (both reads turned None):
        // re-probe so the shell flips to its no-wheel state, same as
        // `adopt_device`, instead of showing rows that can no longer be read.
        self.no_wheel = !wheel_present(&self.device);
        self.reload();
        self.status = if self.no_wheel {
            "wheel disconnected (r to retry)".to_string()
        } else {
            "profile/mode changed on the wheel; view refreshed".to_string()
        };
        true
    }

    /// One `Row` per registry spec of `cat`, in registry order.
    fn registry_rows(&self, cat: Category) -> Vec<Row> {
        REGISTRY
            .iter()
            .filter(|s| s.category == cat)
            .map(|s| Row {
                attr: s.attr.to_string(),
                label: s.label.to_string(),
                available: self.device.available(s.attr),
                value: self.device.read(s.attr),
            })
            .collect()
    }

    /// The mode-coupled Profiles page. Onboard: the registry rows (Mode,
    /// the onboard slot picker, the rename editor). Desktop: the Mode row
    /// followed by the computer-side profile store (one row per saved
    /// profile: Enter applies, `d` deletes) and the "Save current as..."
    /// row (`n` or Enter opens the name prompt).
    fn profiles_rows(&self) -> Vec<Row> {
        let all = self.registry_rows(Category::Profiles);
        if matches!(self.device.current_mode(), Ok(Mode::Onboard)) {
            return all;
        }
        let mut rows: Vec<Row> = all.into_iter().filter(|r| r.attr == "wheel_mode").collect();
        for name in profiles::list_in(&self.profiles_dir) {
            rows.push(Row {
                attr: format!("{PROFILE_ROW_PREFIX}{name}"),
                label: name,
                available: true,
                value: Ok(Value::Text("Enter applies   d deletes".into())),
            });
        }
        rows.push(Row {
            attr: PROFILE_NEW_ATTR.to_string(),
            label: "Save current as...".to_string(),
            available: true,
            value: Ok(Value::Text("Enter or n names a new profile".into())),
        });
        rows
    }

    /// The saved profile the selected row stands for, if any.
    fn selected_profile_name(&self) -> Option<String> {
        self.selected().and_then(|r| r.attr.strip_prefix(PROFILE_ROW_PREFIX)).map(str::to_string)
    }

    /// Snapshot the wheel's settings as computer profile `name` (the name
    /// prompt's Enter).
    fn save_profile(&mut self, name: &str) {
        self.status = match profiles::save_in(&self.profiles_dir, name, &self.device) {
            Ok(()) => format!("profile '{}' saved", name.trim()),
            Err(e) => format!("profile save: {e}"),
        };
        self.reload();
    }

    /// Replay computer profile `name` onto the wheel (Enter on its row).
    fn apply_profile(&mut self, name: &str) {
        self.status = match profiles::apply_in(&self.profiles_dir, name, &self.device) {
            Ok(errors) if errors.is_empty() => format!("profile '{name}' applied"),
            Ok(errors) => {
                let (attr, msg) = &errors[0];
                format!(
                    "profile '{name}' applied, {} setting(s) failed, first: {attr}: {msg}",
                    errors.len()
                )
            }
            Err(e) => format!("profile '{name}': {e}"),
        };
        self.reload();
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
        if !rows.iter().any(|r| shaping::role(&r.attr) != ShapingRole::Neutral) {
            return rows;
        }
        let mut out = Vec::with_capacity(rows.len() + shaping::Axis::ALL.len());
        let mut headed: Vec<shaping::Axis> = Vec::new();
        for row in rows {
            if let Some(ax) = shaping::axis(&row.attr) {
                if !headed.contains(&ax) {
                    headed.push(ax);
                    out.push(Row {
                        attr: shaping::toggle_attr(ax).to_string(),
                        label: shaping::toggle_label(ax).to_string(),
                        available: true,
                        value: Ok(Value::Bool(self.shaping_toggles.get(ax))),
                    });
                }
            }
            if shaping::visible(&row.attr, self.shaping_toggles) {
                out.push(row);
            }
        }
        out
    }

    /// Whether the on-screen category carries any shaping toggle rows
    /// (Steering and Pedals do).
    pub fn has_shaping_toggle(&self) -> bool {
        self.rows.iter().any(|r| shaping::toggle_axis(&r.attr).is_some())
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
            .and_then(|r| shaping::toggle_axis(&r.attr).or_else(|| shaping::axis(&r.attr)))
        else {
            return;
        };
        self.toggle_shaping(axis);
    }

    fn led_row(&self, attr: &str, label: &str) -> Row {
        Row {
            attr: attr.to_string(),
            label: label.to_string(),
            available: self.device.available(attr),
            value: self.device.read(attr),
        }
    }

    /// The composed LIGHTSYNC page: Effect (the composed selector), the
    /// global Brightness, then, only while the custom effect (5) is
    /// active, the active slot's fields as an indented group. The
    /// slot-scoped registry rows stop being top-level rows;
    /// `wheel_led_slot` itself has no row at all (the selector's CUSTOM
    /// entries pick the slot). The G PRO rev lights live on the Steering
    /// page (they sit on the steering rim), not here.
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
        // +1 for the trailing Setup entry, past the last real category.
        let n = (Category::ALL.len() + 1) as i32;
        self.cat_idx = ((self.cat_idx as i32 + d).rem_euclid(n)) as usize;
        self.row_idx = 0;
        // First visit to the Setup view scans the Steam libraries; later
        // visits keep the last scan (r rescans on demand).
        if self.is_setup() && !self.games_scanned {
            self.scan_games();
        }
        // Every entry to the Info view re-runs the (cheap) evdev discovery
        // and auto-starts the live monitor when a wheel input is found;
        // leaving it always stops the monitor loop so no fd stays open
        // (and no polling keeps running) behind other views.
        if self.is_info() {
            self.rescan_input();
        } else {
            self.test.stop_monitor();
        }
        self.reload();
    }

    /// (Re-)discover the wheel's evdev node for the Info view's monitor
    /// and start monitoring right away when one is found (the monitor is
    /// not toggled by hand; it runs whenever the Info view is open and a
    /// wheel input exists).
    pub fn rescan_input(&mut self) {
        let range = self.wheel_range();
        self.test.rescan(range);
        if self.test.dev.is_some() {
            self.test.start_monitor();
        }
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
    /// install, `--prefix <pfx>` plus `--sdk-dir <dir>` when the folder
    /// validates, or an `--uninstall-prefix <pfx>` remove), blocking: the TUI's event loop is synchronous, so
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
            Some(row) => (row.attr.clone(), row.label.clone()),
            None => return,
        };
        // A per-axis shaping row is view state, not a device attribute:
        // Enter flips it in place (same as the 'a' shortcut), no editor.
        if let Some(axis) = shaping::toggle_axis(&attr) {
            self.toggle_shaping(axis);
            return;
        }
        let Some(spec) = Device::<S>::spec(&attr) else { return };
        if spec.access == Access::ReadOnly {
            return;
        }
        // The LIGHTSYNC Effect row cycles the 13 selector entries, not the
        // raw 1-9 value; it gets its own little modal (see `EffectEdit`).
        if attr == "wheel_led_effect" {
            let effect = match self.device.read(&attr) {
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
            self.status = match self.device.write(&attr, &Value::Trigger) {
                Ok(()) => format!("{label}: done"),
                Err(e) => format!("{label}: {e}"),
            };
            self.reload();
            return;
        }
        // The onboard profile picker: only slots 1-5 (profile 0 is the
        // desktop state, and this row only shows in onboard mode), so the
        // editor bumps inside that range regardless of the registry's
        // wider 0-5 kind.
        if attr == "wheel_profile" {
            let cur = match self.rows.get(self.row_idx).map(|r| &r.value) {
                Some(Ok(Value::Int(n))) => Value::Int((*n).max(1)),
                _ => Value::Int(1),
            };
            let kind = Kind::IntRange { min: 1, max: 5, step: 1, unit: "" };
            self.edit = Some(edit::EditState::start(spec.attr, kind, &cur));
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
        if self.is_info() {
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
                Up => self.move_row(-1),
                Down => self.move_row(1),
                Char('d') => self.toggle_mode(),
                // The Info view's r rescans both sides: the identity rows
                // (sysfs; a missing wheel queues a re-discovery) and the
                // monitor's input device (evdev).
                Char('r') => {
                    if self.no_wheel {
                        self.retry_requested = true;
                    }
                    self.rescan_input();
                    self.reload();
                    self.status = match &self.test.dev {
                        Some(d) => format!("input: found {} ({})", d.name, d.event_path),
                        None => "input: no wheel found (r to rescan)".to_string(),
                    };
                }
                Char('f') => self.request_sim(SimKind::ConstantForce),
                Char('t') => self.request_sim(SimKind::Texture),
                // Stop the playing sim; a no-op while nothing plays.
                Char('s') if self.test.stop_sim() => {
                    self.status = "test: simulation stopped".to_string();
                }
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
                            "SDK folder set; no DLLs there, so installs will use the installer's own lookup (repo sdk/ or $LOGITECH_TRUEFORCE_SDK_DIR)".to_string()
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
                        // --sdk-dir only when the folder validates; an
                        // invalid one must not override the installer's
                        // own resolution (env var, repo sdk/, XDG
                        // default).
                        let args = steam::shim_install_args(&pfx, Path::new(&self.sdk_dir));
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
        // A pending profile delete swallows the next key: only 'y'
        // deletes, anything else cancels.
        if let Some(name) = self.profile_delete_confirm.take() {
            if matches!(key, Char('y') | Char('Y')) {
                self.status = match profiles::delete_in(&self.profiles_dir, &name) {
                    Ok(()) => format!("profile '{name}' deleted"),
                    Err(e) => format!("profile '{name}': {e}"),
                };
                self.reload();
            } else {
                self.status = "delete cancelled".to_string();
            }
            return;
        }
        // The new-profile name prompt swallows every key while active, so
        // typing a name cannot trigger the view's shortcuts.
        if self.profile_name_edit.is_some() {
            match key {
                Enter => {
                    let name = self.profile_name_edit.take().unwrap_or_default();
                    self.save_profile(&name);
                }
                Esc => self.profile_name_edit = None,
                Backspace => {
                    if let Some(draft) = self.profile_name_edit.as_mut() {
                        draft.pop();
                    }
                }
                Char(c) => {
                    if let Some(draft) = self.profile_name_edit.as_mut() {
                        draft.push(c);
                    }
                }
                _ => {}
            }
            return;
        }
        match key {
            Char('q') => self.quit = true,
            // Without a wheel, r queues a re-discovery instead of a
            // (pointless) reload; the main loop runs it.
            Char('r') => {
                if self.no_wheel {
                    self.retry_requested = true;
                    self.status = "retrying wheel discovery...".to_string();
                } else {
                    self.reload();
                }
            }
            // On a saved-profile row 'd' arms its delete; anywhere else it
            // keeps its usual desktop/onboard meaning.
            Char('d') => match self.selected_profile_name() {
                Some(name) => {
                    self.status =
                        format!("delete profile '{name}'? y deletes, any other key cancels");
                    self.profile_delete_confirm = Some(name);
                }
                None => self.toggle_mode(),
            },
            // Only meaningful on a row that belongs to a shaping axis (a
            // toggle row or one of the axis's own rows); a plain typo
            // elsewhere does nothing.
            Char('a') => self.toggle_selected_axis(),
            // Only on the desktop Profiles page (the row exists nowhere
            // else): open the new-profile name prompt.
            Char('n') if self.rows.iter().any(|r| r.attr == PROFILE_NEW_ATTR) => {
                self.profile_name_edit = Some(String::new());
            }
            Up => self.move_row(-1),
            Down => self.move_row(1),
            Left => self.move_cat(-1),
            Right => self.move_cat(1),
            Enter => {
                if let Some(name) = self.selected_profile_name() {
                    self.apply_profile(&name);
                } else if self.selected().is_some_and(|r| r.attr == PROFILE_NEW_ATTR) {
                    self.profile_name_edit = Some(String::new());
                } else {
                    self.begin_edit();
                }
            }
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

    // --- external-change (drift) detection ---

    use std::rc::Rc;

    /// An app plus a second handle to its `FakeSysfs`, so a test can mutate
    /// attributes behind the app's back (what the wheel's physical profile
    /// button looks like from here).
    fn drift_app() -> (Rc<FakeSysfs>, App<Rc<FakeSysfs>>) {
        let fs = Rc::new(FakeSysfs::new());
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "1");
        fs.set("wheel_strength", "62");
        let app = App::new(logi_dd_core::Device::with_io(fs.clone()));
        (fs, app)
    }

    #[test]
    fn check_drift_without_changes_does_nothing() {
        let (_fs, mut a) = drift_app();
        a.status.clear();
        assert!(!a.check_drift());
        assert!(!a.check_drift());
        assert!(a.status.is_empty());
    }

    #[test]
    fn profile_drift_reloads_the_rows_and_reports() {
        let (fs, mut a) = drift_app();
        // The wheel's profile button fired: the slot AND an effective
        // setting move without any key passing through the app.
        fs.set("wheel_profile", "3");
        fs.set("wheel_strength", "30");
        assert!(a.check_drift());
        let strength = a.rows.iter().find(|r| r.attr == "wheel_strength").unwrap();
        assert_eq!(
            strength.value.as_ref().unwrap(),
            &Value::Percent(30),
            "the visible rows must be re-read, not stale"
        );
        assert!(a.status.contains("changed"), "status: {}", a.status);
        // The baseline advanced with the reload: the next tick is quiet.
        assert!(!a.check_drift());
    }

    #[test]
    fn mode_drift_recomposes_the_profiles_page() {
        let (fs, mut a) = drift_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Profiles).unwrap();
        a.reload();
        // Desktop mode composes the computer-side store rows.
        assert!(a.rows.iter().any(|r| r.attr == PROFILE_NEW_ATTR));
        fs.set("wheel_mode", "onboard");
        assert!(a.check_drift());
        // Onboard mode shows the registry rows (slot picker etc.) instead.
        assert!(!a.rows.iter().any(|r| r.attr == PROFILE_NEW_ATTR));
        assert!(a.rows.iter().any(|r| r.attr == "wheel_profile"));
    }

    #[test]
    fn drift_check_is_skipped_while_an_editor_is_open() {
        let (fs, mut a) = drift_app();
        a.profile_name_edit = Some(String::new());
        fs.set("wheel_profile", "4");
        assert!(!a.check_drift(), "a reload must not yank state from under an open prompt");
        a.profile_name_edit = None;
        assert!(a.check_drift(), "the first check after the prompt closes catches up");
    }

    #[test]
    fn own_edits_never_read_as_drift() {
        let (_fs, mut a) = drift_app();
        a.toggle_mode(); // writes wheel_mode and reloads, resyncing the baseline
        assert!(!a.check_drift());
    }

    #[test]
    fn drift_to_an_unreadable_wheel_flips_the_no_wheel_shell() {
        let (fs, mut a) = drift_app();
        fs.set_absent("wheel_range");
        fs.set_absent("wheel_mode");
        fs.set_absent("wheel_profile");
        fs.set_absent("wheel_strength");
        assert!(a.check_drift());
        assert!(a.no_wheel);
        assert!(a.rows.is_empty());
        assert!(a.status.contains("disconnected"), "status: {}", a.status);
        // With no wheel there is nothing to watch; `r` retries discovery.
        assert!(!a.check_drift());
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
    fn move_cat_reaches_and_leaves_setup() {
        let mut a = app();
        // step through every real category, landing on Setup right after
        // the last one.
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup(), "cat_idx {} should be the Setup entry", a.cat_idx);
        assert!(a.rows.is_empty(), "Setup has no settings rows");
        // then a wrap back to the first category.
        a.move_cat(1);
        assert!(!a.is_setup());
        assert_eq!(a.cat_idx, 0);
        // stepping backward from the first category reaches Setup too.
        a.move_cat(-1);
        assert!(a.is_setup());
    }

    /// An app parked on the Info view (no real wheel input is asserted on;
    /// the discovery the entry runs is overwritten per test).
    fn info_view_app() -> App<FakeSysfs> {
        let mut a = app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        assert!(a.is_info());
        a
    }

    #[test]
    fn entering_the_info_view_runs_input_discovery() {
        let mut a = app();
        let info = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        for _ in 0..info {
            a.move_cat(1);
        }
        assert!(a.is_info());
        assert!(a.test.scanned, "entering the Info view runs discovery");
        // The identity rows still load like any category's.
        assert!(a.rows.iter().any(|r| r.attr == "wheel_serial"));
    }

    #[test]
    fn info_sim_keys_without_a_wheel_report_instead_of_arming() {
        use crossterm::event::KeyCode;
        let mut a = info_view_app();
        a.test.dev = None;
        a.on_key(KeyCode::Char('f'));
        assert!(a.test.confirm.is_none(), "nothing armed without a wheel");
        assert!(a.status.contains("no wheel"), "status: {}", a.status);
        a.on_key(KeyCode::Char('t'));
        assert!(a.test.confirm.is_none());
    }

    #[test]
    fn info_sim_keys_arm_a_confirm_and_anything_but_y_cancels() {
        use crate::wheel_test::SimKind;
        use crossterm::event::KeyCode;
        let mut a = info_view_app();
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
    fn leaving_the_info_view_stops_the_monitor() {
        let mut a = info_view_app();
        a.move_cat(1);
        assert!(!a.is_info());
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
        // An SDK folder without the DLLs: --sdk-dir is omitted so the
        // installer's own lookup (repo sdk/, env var, XDG default) runs.
        a.sdk_dir = "/sdk".to_string();
        a.on_key(KeyCode::Char('i'));
        assert_eq!(
            a.take_pending_shim(),
            Some((
                vec!["--prefix".to_string(), "/lib/steamapps/compatdata/100/pfx".to_string()],
                "install"
            ))
        );
        // Taken once; a second take finds nothing queued.
        assert_eq!(a.take_pending_shim(), None);
        // A validated folder (the marker DLL exists) is passed through.
        let sdk = std::env::temp_dir().join(format!("logi-dd-tui-sdk-{}", std::process::id()));
        let marker = sdk.join("Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "dll").unwrap();
        a.sdk_dir = sdk.to_string_lossy().into_owned();
        a.on_key(KeyCode::Char('i'));
        assert_eq!(
            a.take_pending_shim(),
            Some((
                vec![
                    "--prefix".to_string(),
                    "/lib/steamapps/compatdata/100/pfx".to_string(),
                    "--sdk-dir".to_string(),
                    sdk.to_string_lossy().into_owned(),
                ],
                "install"
            ))
        );
        std::fs::remove_dir_all(&sdk).unwrap();
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
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_led_effect", "wheel_led_brightness"]);
    }

    #[test]
    fn lightsync_page_shows_the_indented_slot_group_for_the_custom_effect() {
        let a = leds_app("5", "0");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
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
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        // The steering toggle heads the axis block (right before the
        // sensitivity row), not the whole page.
        let toggle = attrs.iter().position(|a| *a == shaping::toggle_attr(Axis::Steering)).unwrap();
        assert_eq!(attrs[toggle + 1], "wheel_sensitivity");
        let mut p = steering_app();
        p.cat_idx = Category::ALL.iter().position(|c| *c == Category::Pedals).unwrap();
        p.reload();
        let pattrs: Vec<&str> = p.rows.iter().map(|r| r.attr.as_str()).collect();
        for ax in [Axis::Throttle, Axis::Brake, Axis::Clutch, Axis::Handbrake] {
            assert!(pattrs.contains(&shaping::toggle_attr(ax)), "missing {ax:?} toggle");
        }
        let ffb = app(); // first category, Ffb
        assert!(!ffb.has_shaping_toggle(), "Ffb has no shaping generators");
    }

    #[test]
    fn simple_mode_shows_sensitivity_and_hides_curves() {
        let a = steering_app();
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
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
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
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
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert!(attrs.contains(&"wheel_sensitivity"), "back to the simple view");
    }

    #[test]
    fn a_key_does_nothing_on_a_row_without_an_axis() {
        use crossterm::event::KeyCode;
        let mut a = app(); // Ffb
        let before: Vec<String> = a.rows.iter().map(|r| r.attr.clone()).collect();
        a.on_key(KeyCode::Char('a'));
        assert_eq!(a.shaping_toggles, shaping::AxisToggles::default());
        let after: Vec<String> = a.rows.iter().map(|r| r.attr.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn pedal_axes_toggle_independently_and_keep_deadzones() {
        let mut a = steering_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Pedals).unwrap();
        a.reload();
        let simple: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert!(simple.contains(&"wheel_throttle_deadzone"));
        assert!(simple.contains(&"wheel_throttle_sensitivity"));
        assert!(!simple.contains(&"wheel_throttle_curve"));
        // Brake to the curve view; the throttle stays on sensitivity.
        a.toggle_shaping(shaping::Axis::Brake);
        let mixed: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert!(mixed.contains(&"wheel_brake_curve"));
        assert!(!mixed.contains(&"wheel_brake_sensitivity"));
        assert!(mixed.contains(&"wheel_brake_deadzone"));
        assert!(mixed.contains(&"wheel_throttle_sensitivity"));
        assert!(!mixed.contains(&"wheel_throttle_curve"));
        // Every axis on the curve view: no sensitivities remain.
        for ax in [shaping::Axis::Throttle, shaping::Axis::Clutch, shaping::Axis::Handbrake] {
            a.toggle_shaping(ax);
        }
        let curves: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
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
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
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

    // --- mode-coupled Profiles page ---

    /// A fresh, unique temp directory per test for the computer-side
    /// profile store.
    fn profiles_tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "logi-dd-tui-profiles-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn profiles_app(mode: &str) -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", mode);
        fs.set("wheel_strength", "62");
        fs.set("wheel_profile", "2");
        fs.set("wheel_profile_names", "1: AC EVO\n2: GT7");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.profiles_dir = profiles_tempdir();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Profiles).unwrap();
        a.reload();
        a
    }

    #[test]
    fn onboard_profiles_page_shows_the_wheel_slot_rows() {
        let a = profiles_app("onboard");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_mode", "wheel_profile", "wheel_profile_names"]);
    }

    #[test]
    fn desktop_profiles_page_shows_mode_plus_the_computer_store() {
        let a = profiles_app("desktop");
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_mode", PROFILE_NEW_ATTR], "empty store: just Mode + Save");
    }

    #[test]
    fn mode_toggle_recomposes_the_profiles_page() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("desktop");
        a.on_key(KeyCode::Char('d')); // Mode row selected: toggles the mode
        assert_eq!(a.device.current_mode().unwrap(), Mode::Onboard);
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_mode", "wheel_profile", "wheel_profile_names"]);
        a.on_key(KeyCode::Char('d'));
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_mode", PROFILE_NEW_ATTR]);
    }

    #[test]
    fn save_prompt_creates_a_profile_row() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("desktop");
        // Enter on the Save row opens the name prompt.
        a.row_idx = a.rows.iter().position(|r| r.attr == PROFILE_NEW_ATTR).unwrap();
        a.on_key(KeyCode::Enter);
        assert_eq!(a.profile_name_edit.as_deref(), Some(""));
        for c in "race".chars() {
            a.on_key(KeyCode::Char(c));
        }
        a.on_key(KeyCode::Enter);
        assert!(a.profile_name_edit.is_none());
        assert!(a.status.contains("saved"), "status: {}", a.status);
        let attrs: Vec<&str> = a.rows.iter().map(|r| r.attr.as_str()).collect();
        assert_eq!(attrs, vec!["wheel_mode", "profile:race", PROFILE_NEW_ATTR]);
        // The file really is a snapshot of the device.
        let text =
            std::fs::read_to_string(a.profiles_dir.join("race.profile")).unwrap();
        assert!(text.contains("wheel_strength=62"));
    }

    #[test]
    fn n_opens_the_prompt_and_esc_discards_it() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("desktop");
        a.on_key(KeyCode::Char('n'));
        assert!(a.profile_name_edit.is_some());
        a.on_key(KeyCode::Char('x'));
        a.on_key(KeyCode::Esc);
        assert!(a.profile_name_edit.is_none());
        assert!(!a.rows.iter().any(|r| r.attr.starts_with(PROFILE_ROW_PREFIX)), "nothing saved");
        // 'n' outside the desktop Profiles page does nothing.
        let mut b = profiles_app("onboard");
        b.on_key(KeyCode::Char('n'));
        assert!(b.profile_name_edit.is_none());
    }

    #[test]
    fn enter_on_a_saved_profile_applies_it() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("desktop");
        profiles::save_in(&a.profiles_dir, "race", &a.device).unwrap();
        // Drift a setting, then apply the snapshot back.
        a.device.write("wheel_strength", &Value::Percent(10)).unwrap();
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "profile:race").unwrap();
        a.on_key(KeyCode::Enter);
        assert!(a.status.contains("applied"), "status: {}", a.status);
        assert_eq!(a.device.read("wheel_strength").unwrap(), Value::Percent(62));
    }

    #[test]
    fn d_on_a_profile_row_arms_a_delete_confirm() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("desktop");
        profiles::save_in(&a.profiles_dir, "race", &a.device).unwrap();
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "profile:race").unwrap();
        a.on_key(KeyCode::Char('d'));
        assert_eq!(a.profile_delete_confirm.as_deref(), Some("race"));
        assert_eq!(a.device.current_mode().unwrap(), Mode::Desktop, "no mode toggle");
        // Anything but y cancels.
        a.on_key(KeyCode::Char('x'));
        assert!(a.profile_delete_confirm.is_none());
        assert!(a.rows.iter().any(|r| r.attr == "profile:race"), "still saved");
        // y really deletes.
        a.on_key(KeyCode::Char('d'));
        a.on_key(KeyCode::Char('y'));
        assert!(!a.rows.iter().any(|r| r.attr == "profile:race"));
        assert!(a.status.contains("deleted"), "status: {}", a.status);
    }

    // --- the no-wheel state machine ---

    /// An app whose device probes as absent (an empty fake sysfs).
    fn no_wheel_app() -> App<FakeSysfs> {
        let a = App::new(logi_dd_core::Device::with_io(FakeSysfs::new()));
        assert!(a.no_wheel, "an empty sysfs probes as no wheel");
        a
    }

    #[test]
    fn no_wheel_starts_into_the_shell_with_empty_categories() {
        let mut a = no_wheel_app();
        // Every device category shows the empty state (no rows), Setup
        // stays a full page.
        for _ in 0..Category::ALL.len() + 1 {
            assert!(a.rows.is_empty(), "no rows in the no-wheel state (cat {})", a.cat_idx);
            a.move_cat(1);
        }
    }

    #[test]
    fn no_wheel_survives_every_keypress() {
        use crossterm::event::KeyCode::*;
        // Walk every category (Setup and Info included) mashing the whole
        // key map; nothing may panic and nothing may open an editor.
        let mut a = no_wheel_app();
        for _ in 0..Category::ALL.len() + 1 {
            for key in [
                Enter,
                Up,
                Down,
                Char('a'),
                Char('d'),
                Char('f'),
                Char('t'),
                Char('n'),
                Char('i'),
                Char('u'),
                Char('s'),
                Char('y'),
                Esc,
                Backspace,
            ] {
                a.on_key(key);
            }
            // Leave any editor/prompt a key might have opened (Setup's
            // 's' SDK editor), then move on.
            a.on_key(Esc);
            assert!(a.edit.is_none() && a.curve_edit.is_none(), "no editor without rows");
            a.on_key(Right);
        }
        assert!(!a.quit);
    }

    #[test]
    fn no_wheel_r_queues_a_retry_once() {
        use crossterm::event::KeyCode;
        let mut a = no_wheel_app();
        assert!(!a.take_retry_request(), "nothing queued at startup");
        a.on_key(KeyCode::Char('r'));
        assert!(a.take_retry_request());
        assert!(!a.take_retry_request(), "taken once");
        // With a wheel present, r reloads instead of queueing.
        let mut b = app();
        b.on_key(KeyCode::Char('r'));
        assert!(!b.take_retry_request());
    }

    #[test]
    fn info_r_without_a_wheel_also_queues_a_retry() {
        use crossterm::event::KeyCode;
        let mut a = no_wheel_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        assert!(a.is_info());
        a.on_key(KeyCode::Char('r'));
        assert!(a.take_retry_request());
    }

    #[test]
    fn adopt_device_restores_live_behavior() {
        let mut a = no_wheel_app();
        assert!(a.rows.is_empty());
        let fs = FakeSysfs::new();
        fs.set("wheel_strength", "62");
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        a.adopt_device(logi_dd_core::Device::with_io(fs));
        assert!(!a.no_wheel);
        assert!(a.status.contains("wheel found"), "status: {}", a.status);
        assert!(a.rows.iter().any(|r| r.attr == "wheel_strength"), "rows are live again");
    }

    #[test]
    fn adopt_device_with_still_no_wheel_stays_in_the_empty_state() {
        let mut a = no_wheel_app();
        a.adopt_device(logi_dd_core::Device::with_io(FakeSysfs::new()));
        assert!(a.no_wheel);
        assert!(a.rows.is_empty());
        assert!(a.status.contains("no wheel"), "status: {}", a.status);
    }

    #[test]
    fn no_wheel_setup_stays_usable() {
        use crossterm::event::KeyCode;
        let mut a = no_wheel_app();
        for _ in 0..Category::ALL.len() {
            a.move_cat(1);
        }
        assert!(a.is_setup());
        a.games = vec![SteamGame {
            appid: 100,
            name: "ACC".to_string(),
            prefix: PathBuf::from("/lib/steamapps/compatdata/100/pfx"),
            shim_installed: false,
        }];
        a.games_scanned = true;
        a.sdk_dir = "/sdk".to_string();
        a.on_key(KeyCode::Char('i'));
        let (args, verb) = a.take_pending_shim().expect("shim installs work without a wheel");
        assert_eq!(verb, "install");
        assert!(!args.contains(&"--sdk-dir".to_string()), "invalid dir is not passed along");
    }

    #[test]
    fn onboard_profile_picker_bumps_within_1_to_5() {
        use crossterm::event::KeyCode;
        let mut a = profiles_app("onboard");
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_profile").unwrap();
        a.on_key(KeyCode::Enter);
        assert!(a.edit.is_some());
        a.on_key(KeyCode::Left); // 2 -> 1
        a.on_key(KeyCode::Left); // clamps at 1, never 0
        a.on_key(KeyCode::Enter);
        assert_eq!(a.device.read("wheel_profile").unwrap(), Value::Int(1));
        a.on_key(KeyCode::Enter);
        for _ in 0..9 {
            a.on_key(KeyCode::Right); // clamps at 5
        }
        a.on_key(KeyCode::Enter);
        assert_eq!(a.device.read("wheel_profile").unwrap(), Value::Int(5));
    }
}
