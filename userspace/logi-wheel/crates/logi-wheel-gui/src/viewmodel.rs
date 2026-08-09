//! Pure Rust view-model over `logi-wheel-core`: turns `REGISTRY` into the rows a
//! GUI renders, and converts widget input back into `Value`s written to the
//! device. No Slint dependency here, so this is fully unit-testable with
//! `FakeSysfs` and no display.
//!
//! The window (`worker`/`main`) wires up `rows_for`/`edit`/`info`/`set_mode`
//! for every category now, plus the curve editor's `WidgetInput::Curve`, the
//! RGB strip editor's `WidgetInput::Rgb`, the slot-text editor's
//! `WidgetInput::SlotText`, and the pedal deadzone pair's
//! `WidgetInput::PairLower`/`PairUpper`. `mode`/`refresh`/`device_read` are still ahead of any
//! live widget: that is a later task's job. They are marked
//! `#[allow(dead_code)]` individually rather than blanket-silencing the
//! whole module.

use logi_wheel_core::curve::Curve;
use logi_wheel_core::onboard::{self, OnboardEditor, OnboardError};
use logi_wheel_core::profiles;
use logi_wheel_core::sysfs::SysfsIo;
use logi_wheel_core::{Category, Color, Device, DeviceInfo, Error, Kind, Mode, ModeReq, Value};
use std::cell::RefCell;
use std::path::PathBuf;

/// Raw input from a widget, converted to a `Value` per the target setting's
/// `Kind` in `ViewModel::edit`.
#[derive(Debug, Clone)]
pub enum WidgetInput {
    Slider(i64),
    Choice(usize),
    Switch(bool),
    Text(String),
    /// A single onboard slot's new name (1-based `slot`); `Kind::SlotText`
    /// converts this to a single-slot `Value::SlotName` write.
    SlotText { slot: u8, text: String },
    /// A pedal/handbrake deadzone's new lower half, in percent. The upper
    /// half is read fresh from the device at edit time (see
    /// `ViewModel::edit`), so the widget only ever reports the side the
    /// user actually touched.
    PairLower(u8),
    /// The upper-half counterpart of `PairLower`.
    PairUpper(u8),
    Curve(Curve),
    Rgb(Vec<Color>),
    Trigger,
}

/// One rendered row: everything a GUI needs to draw a single setting.
pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: &'static Kind,
    pub value: Option<Value>,
    pub available: bool,
    pub mode_ok: bool,
    mode_req: ModeReq,
}

impl Row {
    pub fn mode_req_desktop_only(&self) -> bool {
        matches!(self.mode_req, ModeReq::DesktopOnly)
    }

    pub fn mode_req_onboard_only(&self) -> bool {
        matches!(self.mode_req, ModeReq::OnboardOnly)
    }
}

/// Wraps a `Device` and adapts `logi-wheel-core`'s registry/value model to what
/// a GUI widget tree renders and edits.
pub struct ViewModel<S: SysfsIo> {
    device: Device<S>,
    /// Where the computer-side profile store lives; resolved once from the
    /// environment (`profiles::default_dir`), overridable for tests.
    profiles_dir: PathBuf,
    /// The in-progress "edit onboard slot" flow, `None` outside it. Interior
    /// mutability because the worker's `handle` only ever borrows a
    /// `ViewModel` shared (see `worker::handle`'s `vm: &ViewModel<S>`).
    onboard: RefCell<Option<OnboardEditor>>,
}

impl<S: SysfsIo> ViewModel<S> {
    // The only production entry point is `new(Device::discover())`
    // (`worker::Worker::spawn`); this constructor exists for tests, which
    // hand it a `FakeSysfs`.
    #[allow(dead_code)]
    pub fn with_io(io: S) -> ViewModel<S> {
        ViewModel::new(Device::with_io(io))
    }

    pub fn new(device: Device<S>) -> ViewModel<S> {
        ViewModel { device, profiles_dir: profiles::default_dir(), onboard: RefCell::new(None) }
    }

    /// Point the computer-side profile store somewhere else (tests only;
    /// production always uses `profiles::default_dir`).
    #[allow(dead_code)]
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
        self.profiles_dir = dir;
    }

    /// Rows for one category, in registry order. `mode_ok` is computed
    /// against a single read of the device's current mode. Reads from
    /// `self.device.settings()`, not the bare `REGISTRY` constant, so a
    /// connected G923 only ever shows its own four classic settings rather
    /// than every DD wheel row marked unavailable (a different device
    /// model, not "DD with everything missing").
    pub fn rows_for(&self, cat: Category) -> Vec<Row> {
        let mode = self.device.current_mode().ok();
        self.device
            .settings()
            .iter()
            .filter(|spec| spec.category == cat)
            .map(|spec| {
                // `read_supported` collapses both "file missing" and "file
                // present but the wheel/firmware says EOPNOTSUPP" (e.g. an
                // RS50's pedal-curve/sensitivity attrs, which exist as
                // files but have no feature behind them on that MCU) to
                // `Ok(None)`, so both present the same way here: an
                // unavailable, greyed-out row rather than one that looks
                // live at a fake 0. Any other read error (permissions, a
                // transient I/O failure) is not "unsupported" and must not
                // be presented that way, so it keeps today's behavior: the
                // row stays available with no value.
                let (available, value) = match self.device.read_supported(spec.attr) {
                    Ok(Some(v)) => (true, Some(v)),
                    Ok(None) => (false, None),
                    Err(_) => (true, None),
                };
                let mode_ok = match spec.mode_req {
                    ModeReq::Any => true,
                    ModeReq::DesktopOnly => mode == Some(Mode::Desktop),
                    ModeReq::OnboardOnly => mode == Some(Mode::Onboard),
                };
                Row {
                    attr: spec.attr,
                    label: spec.label,
                    help: spec.help,
                    kind: &spec.kind,
                    value,
                    available,
                    mode_ok,
                    mode_req: spec.mode_req,
                }
            })
            .collect()
    }

    /// Convert `input` to a `Value` per `attr`'s `Kind` and write it through
    /// `Device::write` (which validates and mode-gates it).
    ///
    /// Pair (deadzone) edits are a read-modify-write: the widget reports
    /// only the half the user touched, and the untouched half comes from a
    /// fresh device read here. Trusting the UI's row snapshot for the other
    /// half instead would let two quick edits clobber each other: the
    /// second edit's snapshot predates the first edit's round-trip, so it
    /// would silently rewrite the first half back to its old value.
    pub fn edit(&self, attr: &str, input: WidgetInput) -> Result<(), Error> {
        let spec = Device::<S>::spec(attr).ok_or(Error::Invalid)?;
        let value = match input {
            WidgetInput::PairLower(lo) => match (spec.kind, self.device.read(attr)?) {
                (Kind::Pair { .. }, Value::Pair(_, hi)) => Value::Pair(lo, hi),
                _ => return Err(Error::Invalid),
            },
            WidgetInput::PairUpper(hi) => match (spec.kind, self.device.read(attr)?) {
                (Kind::Pair { .. }, Value::Pair(lo, _)) => Value::Pair(lo, hi),
                _ => return Err(Error::Invalid),
            },
            other => to_value(spec.kind, other)?,
        };
        self.device.write(attr, &value)
    }

    /// The header's device-identity panel: serial, firmware, current mode,
    /// and which wheel model this is (for the Info/Testing page's photo).
    pub fn info(&self) -> Result<DeviceInfo, Error> {
        self.device.info()
    }

    /// Best-effort HID++ firmware string for a classic (G923) wheel; see
    /// `Device::classic_firmware`. A real, timed USB round trip - the
    /// worker calls this only at explicit refresh points (page load,
    /// rescan, mode/profile changes), never per frame.
    pub fn classic_firmware(&self) -> Option<String> {
        self.device.classic_firmware()
    }

    // Not called yet: nothing reads the mode outside of `rows_for`'s own
    // per-row gating until the mode-switch control is wired.
    #[allow(dead_code)]
    pub fn mode(&self) -> Result<Mode, Error> {
        self.device.current_mode()
    }

    pub fn set_mode(&self, m: Mode) -> Result<(), Error> {
        match m {
            Mode::Desktop => self.device.ensure_desktop_mode(),
            Mode::Onboard => self.device.write("wheel_mode", &Value::Enum(1)),
        }
    }

    /// What the worker's drift watcher compares between idle polls: the
    /// active onboard profile slot (`None` on a wheel that does not expose
    /// `wheel_profile`, which is then simply never watched) and the current
    /// mode. An `Err` means an attribute that was there cannot be read any
    /// more, i.e. the wheel is gone.
    pub fn drift_snapshot(&self) -> Result<(Option<Value>, Mode), Error> {
        let profile = if self.device.available("wheel_profile") {
            Some(self.device.read("wheel_profile")?)
        } else {
            None
        };
        Ok((profile, self.device.current_mode()?))
    }

    /// The computer-side profile store's saved names, sorted.
    pub fn profile_list(&self) -> Vec<String> {
        profiles::list_in(&self.profiles_dir)
    }

    /// Snapshot the wheel's current settings as computer profile `name`.
    pub fn profile_save(&self, name: &str) -> Result<(), Error> {
        profiles::save_in(&self.profiles_dir, name, &self.device)
    }

    /// Replay computer profile `name` onto the wheel; per-attr failures
    /// come back as `(attr, message)` pairs (see `profiles::apply_in`).
    pub fn profile_apply(&self, name: &str) -> Result<Vec<(String, String)>, Error> {
        profiles::apply_in(&self.profiles_dir, name, &self.device)
    }

    /// Delete computer profile `name`.
    pub fn profile_delete(&self, name: &str) -> Result<(), Error> {
        profiles::delete_in(&self.profiles_dir, name)
    }

    /// Rows are read live from the device on every `rows_for` call, so there
    /// is no cache to invalidate; kept as a hook for callers that expect one.
    #[allow(dead_code)]
    pub fn refresh(&self) {}

    /// Read a raw attribute back through the wrapped device. Used by the
    /// worker's LIGHTSYNC try-on-wheel run (to remember the state it must
    /// restore) and by tests.
    /// What this wheel can do, for the Setup page's per-game advice.
    /// Read from the managed device rather than rediscovered, so the advice
    /// follows the wheel the picker is on.
    pub fn wheel_caps(&self) -> logi_wheel_core::games::WheelCaps {
        self.device.wheel_caps()
    }

    pub fn device_read(&self, attr: &str) -> Result<Value, Error> {
        self.device.read(attr)
    }

    // --- "Edit onboard slot" flow: see `logi_wheel_core::onboard` ---

    /// Whether the flow is active (a slot has been picked and is being
    /// edited). The Profiles page swaps to `onboard_rows` while this is
    /// true.
    pub fn onboard_active(&self) -> bool {
        self.onboard.borrow().is_some()
    }

    /// The slot being authored (1-5), or 0 outside the flow.
    pub fn onboard_slot(&self) -> u8 {
        self.onboard.borrow().as_ref().map_or(0, OnboardEditor::slot)
    }

    /// The 5 onboard slots' names (index 0 = slot 1); a blank entry where
    /// the wheel has none or the read failed. Not wired to a picker label
    /// yet (the GUI's slot picker shows plain "Slot N" buttons, matching
    /// `onboard_begin`'s own numbering); kept for a future enhancement and
    /// exercised directly by tests in the meantime.
    #[allow(dead_code)]
    pub fn onboard_slot_names(&self) -> Vec<String> {
        match self.device.read("wheel_profile_names") {
            Ok(Value::SlotNames(names)) => names,
            _ => vec![String::new(); 5],
        }
    }

    /// The active slot's own name, or "" outside the flow.
    pub fn onboard_slot_name(&self) -> String {
        let slot = self.onboard_slot();
        if slot == 0 {
            return String::new();
        }
        onboard::slot_name(&self.device, slot).unwrap_or_default()
    }

    /// Begin authoring `slot`: switch+verify, snapshot. See
    /// `OnboardEditor::begin`. Replaces any flow already in progress.
    pub fn onboard_begin(&self, slot: u8) -> Result<(), OnboardError> {
        let editor = OnboardEditor::begin(&self.device, slot)?;
        *self.onboard.borrow_mut() = Some(editor);
        Ok(())
    }

    /// Rows for the active slot: every `onboard::ONBOARD_ATTRS` entry this
    /// wheel exposes, read live (fresh after `onboard_begin`'s switch).
    /// Empty outside the flow.
    pub fn onboard_rows(&self) -> Vec<Row> {
        if self.onboard.borrow().is_none() {
            return Vec::new();
        }
        self.device
            .settings()
            .iter()
            .filter(|spec| onboard::ONBOARD_ATTRS.contains(&spec.attr))
            .map(|spec| {
                let (available, value) = match self.device.read_supported(spec.attr) {
                    Ok(Some(v)) => (true, Some(v)),
                    Ok(None) => (false, None),
                    Err(_) => (true, None),
                };
                Row {
                    attr: spec.attr,
                    label: spec.label,
                    help: spec.help,
                    kind: &spec.kind,
                    value,
                    available,
                    // The flow only ever runs with the wheel in onboard
                    // mode on the slot it just switched to; every editable
                    // attr here is `ModeReq::Any` or `OnboardOnly`
                    // (`wheel_brake_force`), both satisfied by construction.
                    mode_ok: true,
                    mode_req: spec.mode_req,
                }
            })
            .collect()
    }

    /// Write one attr to the active slot; see `OnboardEditor::set`.
    pub fn onboard_set(&self, attr: &str, input: WidgetInput) -> Result<(), OnboardError> {
        let guard = self.onboard.borrow();
        let editor = guard.as_ref().ok_or(Error::Invalid)?;
        let spec = Device::<S>::spec(attr).ok_or(Error::Invalid)?;
        let value = to_value(spec.kind, input)?;
        editor.set(&self.device, attr, &value)
    }

    /// Rename the active slot; see `OnboardEditor::set_name`.
    pub fn onboard_set_name(&self, name: &str) -> Result<(), OnboardError> {
        let guard = self.onboard.borrow();
        let editor = guard.as_ref().ok_or(Error::Invalid)?;
        editor.set_name(&self.device, name)
    }

    /// Replay the snapshot taken at `onboard_begin`; see
    /// `OnboardEditor::revert`.
    pub fn onboard_revert(&self) -> Result<Vec<(String, String)>, OnboardError> {
        let guard = self.onboard.borrow();
        let editor = guard.as_ref().ok_or(Error::Invalid)?;
        editor.revert(&self.device)
    }

    /// Copy saved computer profile `name` into the active slot; see
    /// `OnboardEditor::copy_from_computer_profile`.
    pub fn onboard_copy_from_profile(&self, name: &str) -> Result<Vec<(String, String)>, OnboardError> {
        let guard = self.onboard.borrow();
        let editor = guard.as_ref().ok_or(Error::Invalid)?;
        editor.copy_from_computer_profile(&self.device, &self.profiles_dir, name)
    }

    /// Leave the flow; see `OnboardEditor::finish`.
    pub fn onboard_exit(&self, restore_previous: bool) -> Result<(), OnboardError> {
        let Some(editor) = self.onboard.borrow_mut().take() else { return Ok(()) };
        editor.finish(&self.device, restore_previous)
    }
}

/// Convert a widget's raw input into the `Value` its setting's `Kind` needs,
/// per the spec's own union of widget-shape and kind.
fn to_value(kind: Kind, input: WidgetInput) -> Result<Value, Error> {
    match (kind, input) {
        (Kind::Percent | Kind::ScaledPercent { .. }, WidgetInput::Slider(n)) => {
            Ok(Value::Percent(u8::try_from(n).map_err(|_| Error::Invalid)?))
        }
        (Kind::IntRange { .. }, WidgetInput::Slider(n)) => Ok(Value::Int(i32::try_from(n).map_err(|_| Error::Invalid)?)),
        (Kind::Enum(_), WidgetInput::Choice(i)) => Ok(Value::Enum(i as u8)),
        (Kind::Toggle { .. }, WidgetInput::Switch(b)) => Ok(Value::Bool(b)),
        (Kind::TextField { .. }, WidgetInput::Text(s)) => Ok(Value::Text(s)),
        (Kind::SlotText { .. }, WidgetInput::SlotText { slot, text }) => {
            Ok(Value::SlotName { slot, name: text })
        }
        (Kind::RgbStrip { .. }, WidgetInput::Rgb(cs)) => Ok(Value::Rgb(cs)),
        (Kind::Curve, WidgetInput::Curve(c)) => Ok(c.to_value()),
        (Kind::Action, WidgetInput::Trigger) => Ok(Value::Trigger),
        _ => Err(Error::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_wheel_core::sysfs::FakeSysfs;

    fn vm() -> ViewModel<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80"); // Percent
        fs.set("wheel_combined_pedals", "0"); // Toggle-ish
        ViewModel::with_io(fs)
    }

    #[test]
    fn rows_for_a_category_come_from_the_registry() {
        let rows = vm().rows_for(Category::Ffb);
        assert!(rows.iter().any(|r| r.attr == "wheel_strength" && r.label == "FFB strength"));
    }

    #[test]
    fn rows_for_marks_an_attr_unavailable_when_the_wheel_says_unsupported() {
        // The RS50 bug this guards against: the pedal-curve/sensitivity
        // sysfs files exist, but the pedal MCU has no such feature, so
        // every read answers EOPNOTSUPP. Before this row also checked the
        // read outcome, `available` came only from the file's existence,
        // so the GUI rendered these as live, editable controls sitting at
        // a fake 0.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set_read_errno("wheel_throttle_sensitivity", 95); // EOPNOTSUPP
        let vm = ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Pedals)
            .into_iter()
            .find(|r| r.attr == "wheel_throttle_sensitivity")
            .unwrap();
        assert!(!row.available, "must present as unavailable, not a live 0");
        assert!(row.value.is_none());
    }

    #[test]
    fn rows_for_keeps_a_permission_error_available_not_unsupported() {
        // A non-EOPNOTSUPP read failure (permissions, a transient I/O
        // error) is a different problem than "this feature doesn't exist",
        // and must not be relabeled "Unavailable": it keeps today's
        // behavior of an available row with no value.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set_read_errno("wheel_throttle_sensitivity", 13); // EACCES
        let vm = ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Pedals)
            .into_iter()
            .find(|r| r.attr == "wheel_throttle_sensitivity")
            .unwrap();
        assert!(row.available, "a permission error is not treated as unsupported");
        assert!(row.value.is_none());
    }

    fn g923_vm() -> ViewModel<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        fs.set("gain", "0");
        fs.set("autocenter", "0");
        fs.set("combine_pedals", "0");
        ViewModel::new(logi_wheel_core::Device::with_io_and_model(fs, logi_wheel_core::WheelModel::G923))
    }

    #[test]
    fn classic_firmware_is_none_without_a_real_hid_dir() {
        // `with_io_and_model` has no real sysfs directory behind it (only
        // `Device::discover` populates that): the HID++ round trip has
        // nothing to anchor to, so this must return `None` rather than
        // panic. Exercised end to end against a live G923 in
        // `logi-wheel-core`'s own `device`/`hidpp` tests.
        assert!(g923_vm().classic_firmware().is_none());
        assert!(vm().classic_firmware().is_none(), "a non-G923 model is always None too");
    }

    #[test]
    fn g923_rows_are_its_own_classic_settings_not_the_dd_wheel_set() {
        let rows = g923_vm().rows_for(Category::Steering);
        assert!(rows.iter().any(|r| r.attr == "range"));
        assert!(!rows.iter().any(|r| r.attr == "wheel_range"));

        let ffb = g923_vm().rows_for(Category::Ffb);
        let attrs: Vec<&str> = ffb.iter().map(|r| r.attr).collect();
        assert_eq!(attrs, vec!["gain", "autocenter"]);

        // Categories the classic engine has nothing in stay empty, not
        // "every DD row marked unavailable".
        assert!(g923_vm().rows_for(Category::Leds).is_empty());
        assert!(g923_vm().rows_for(Category::Profiles).is_empty());
        assert!(g923_vm().rows_for(Category::Info).is_empty());
    }

    #[test]
    fn g923_edit_writes_its_classic_attrs() {
        let vm = g923_vm();
        vm.edit("range", WidgetInput::Slider(540)).unwrap();
        assert_eq!(vm.device_read("range").unwrap(), Value::Int(540));
        vm.edit("combine_pedals", WidgetInput::Choice(2)).unwrap();
        assert_eq!(vm.device_read("combine_pedals").unwrap(), Value::Enum(2));
        // Out-of-range writes are rejected the same way as the DD registry.
        assert!(vm.edit("range", WidgetInput::Slider(39)).is_err());
    }

    #[test]
    fn slider_edit_writes_the_percent_value() {
        let vm = vm();
        vm.edit("wheel_strength", WidgetInput::Slider(55)).unwrap();
        assert_eq!(vm.device_read("wheel_strength").unwrap(), Value::Percent(55));
    }

    #[test]
    fn mode_gated_row_is_flagged_when_in_the_wrong_mode() {
        // a DesktopOnly setting while the device is in onboard mode -> mode_ok false
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        let vm = ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Steering)
            .into_iter()
            .find(|r| r.mode_req_desktop_only())
            .unwrap();
        assert!(!row.mode_ok);
    }

    // --- one conversion test per Kind ---

    #[test]
    fn intrange_edit_writes_int() {
        let vm = vm();
        vm.edit("wheel_range", WidgetInput::Slider(540)).unwrap();
        assert_eq!(vm.device_read("wheel_range").unwrap(), Value::Int(540));
    }

    #[test]
    fn enum_edit_writes_the_variant_word() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_texture_route", "kf");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_texture_route", WidgetInput::Choice(1)).unwrap();
        assert_eq!(vm.device_read("wheel_texture_route").unwrap(), Value::Enum(1));
    }

    #[test]
    fn toggle_edit_writes_bool() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_range_restore", "0");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_range_restore", WidgetInput::Switch(true)).unwrap();
        assert_eq!(vm.device_read("wheel_range_restore").unwrap(), Value::Bool(true));
    }

    #[test]
    fn textfield_edit_writes_text() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_slot_name", "OLD");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_led_slot_name", WidgetInput::Text("RACER".into())).unwrap();
        assert_eq!(vm.device_read("wheel_led_slot_name").unwrap(), Value::Text("RACER".into()));
    }

    #[test]
    fn slot_text_edit_writes_one_slot() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile_names", "1: A\n2: B\n3: C\n4: D\n5: E");
        let vm = ViewModel::with_io(fs);
        vm.edit(
            "wheel_profile_names",
            WidgetInput::SlotText { slot: 2, text: "GT7".into() },
        )
        .unwrap();
        match vm.device_read("wheel_profile_names").unwrap() {
            Value::SlotNames(names) => assert_eq!(names[1], "GT7"),
            other => panic!("expected SlotNames, got {other:?}"),
        }
    }

    #[test]
    fn pair_lower_edit_keeps_the_devices_upper_half() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_throttle_deadzone", "0 10");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_throttle_deadzone", WidgetInput::PairLower(5)).unwrap();
        assert_eq!(vm.device_read("wheel_throttle_deadzone").unwrap(), Value::Pair(5, 10));
    }

    #[test]
    fn pair_upper_edit_keeps_the_devices_lower_half() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_throttle_deadzone", "8 0");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_throttle_deadzone", WidgetInput::PairUpper(12)).unwrap();
        assert_eq!(vm.device_read("wheel_throttle_deadzone").unwrap(), Value::Pair(8, 12));
    }

    #[test]
    fn rapid_pair_edits_preserve_both_halves() {
        // The race the old whole-pair widget contract lost: edit the lower
        // half, then the upper half before any UI round-trip could refresh
        // a row snapshot. Each edit only carries the touched side, and the
        // untouched side is read fresh from the device, so both edits land.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_throttle_deadzone", "0 0");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_throttle_deadzone", WidgetInput::PairLower(10)).unwrap();
        vm.edit("wheel_throttle_deadzone", WidgetInput::PairUpper(5)).unwrap();
        assert_eq!(vm.device_read("wheel_throttle_deadzone").unwrap(), Value::Pair(10, 5));
    }

    #[test]
    fn pair_input_on_a_non_pair_attr_errors() {
        let vm = vm();
        let result = vm.edit("wheel_strength", WidgetInput::PairLower(5));
        assert!(result.is_err(), "expected Err for a pair input on a non-pair attr");
    }

    #[test]
    fn rgb_edit_writes_the_strip() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let ten = "000000 000000 000000 000000 000000 000000 000000 000000 000000 000000";
        fs.set("wheel_led_colors", ten);
        let vm = ViewModel::with_io(fs);
        let colors: Vec<Color> = (0..10).map(|_| Color { r: 0xff, g: 0x00, b: 0x80 }).collect();
        vm.edit("wheel_led_colors", WidgetInput::Rgb(colors.clone())).unwrap();
        assert_eq!(vm.device_read("wheel_led_colors").unwrap(), Value::Rgb(colors));
    }

    #[test]
    fn curve_edit_writes_the_composed_points() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_response_curve", "reset");
        let vm = ViewModel::with_io(fs);
        let curve = Curve::from_value("wheel_response_curve", &Value::Curve(vec![]));
        let expected = curve.to_value();
        vm.edit("wheel_response_curve", WidgetInput::Curve(curve)).unwrap();
        assert_eq!(vm.device_read("wheel_response_curve").unwrap(), expected);
    }

    #[test]
    fn action_edit_writes_the_trigger() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let vm = ViewModel::with_io(fs);
        vm.edit("wheel_led_apply", WidgetInput::Trigger).unwrap();
        // Action attrs read back as a synthetic trigger, not the raw sysfs value.
        assert_eq!(vm.device_read("wheel_led_apply").unwrap(), Value::Trigger);
    }

    #[test]
    fn slider_out_of_range_errors_instead_of_wrapping() {
        let vm = vm();
        let result = vm.edit("wheel_strength", WidgetInput::Slider(300));
        assert!(result.is_err(), "expected Err for out-of-range slider input");
    }

    #[test]
    fn mismatched_widget_for_kind_errors() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_texture_route", "kf");
        let vm = ViewModel::with_io(fs);
        let result = vm.edit("wheel_texture_route", WidgetInput::Slider(1));
        assert!(result.is_err(), "expected Err for mismatched widget type");
    }

    // --- "Edit onboard slot" flow ---

    fn onboard_vm() -> ViewModel<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_profile_names", "1: AC EVO\n2: GT7\n3: PROFILE 3\n4: PROFILE 4\n5: PROFILE 5");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_brake_force", "60"); // OnboardOnly: exercises begin()'s wheel_mode fixup
        ViewModel::with_io(fs)
    }

    #[test]
    fn onboard_begin_activates_the_slot_and_populates_rows() {
        let vm = onboard_vm();
        assert!(!vm.onboard_active());
        vm.onboard_begin(3).unwrap();
        assert!(vm.onboard_active());
        assert_eq!(vm.onboard_slot(), 3);
        assert_eq!(vm.device_read("wheel_profile").unwrap(), Value::Int(3));
        let rows = vm.onboard_rows();
        assert!(rows.iter().any(|r| r.attr == "wheel_range"));
        assert!(rows.iter().any(|r| r.attr == "wheel_strength"));
        // Never a full-registry row that is not part of an onboard slot.
        assert!(!rows.iter().any(|r| r.attr == "wheel_sensitivity"));
    }

    #[test]
    fn onboard_rows_are_empty_outside_the_flow() {
        assert!(onboard_vm().onboard_rows().is_empty());
    }

    #[test]
    fn onboard_set_writes_the_attr() {
        let vm = onboard_vm();
        vm.onboard_begin(2).unwrap();
        vm.onboard_set("wheel_strength", WidgetInput::Slider(42)).unwrap();
        assert_eq!(vm.device_read("wheel_strength").unwrap(), Value::Percent(42));
    }

    #[test]
    fn onboard_set_refuses_once_the_slot_changed_underneath_it() {
        // `Rc<FakeSysfs>` so the test keeps its own handle to mutate the
        // fake wheel behind the `ViewModel`'s back (what the wheel's own
        // OLED profile switch looks like from here).
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        let vm = ViewModel::with_io(fs.clone());
        vm.onboard_begin(2).unwrap();
        fs.set("wheel_profile", "5"); // simulate an OLED switch mid-flow
        let err = vm.onboard_set("wheel_strength", WidgetInput::Slider(42)).unwrap_err();
        assert!(matches!(err, OnboardError::SlotChanged { expected: 2, actual: 5 }));
        assert_ne!(fs.read("wheel_strength").unwrap().trim(), "42", "nothing was written");
    }

    #[test]
    fn onboard_set_name_renames_the_active_slot() {
        let vm = onboard_vm();
        vm.onboard_begin(3).unwrap();
        vm.onboard_set_name("Race nite").unwrap();
        assert_eq!(vm.onboard_slot_name(), "Race nite");
    }

    #[test]
    fn onboard_revert_replays_the_snapshot() {
        let vm = onboard_vm();
        vm.onboard_begin(1).unwrap();
        vm.onboard_set("wheel_range", WidgetInput::Slider(360)).unwrap();
        let errors = vm.onboard_revert().unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(vm.device_read("wheel_range").unwrap(), Value::Int(900));
    }

    #[test]
    fn onboard_copy_from_profile_maps_only_onboard_attrs() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-gui-onboard-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("race.profile"),
            "# logi-wheel profile\nwheel_range=540\nwheel_sensitivity=70\nwheel_led_effect=3\n",
        )
        .unwrap();

        let mut vm = onboard_vm();
        vm.set_profiles_dir(dir.clone());
        vm.onboard_begin(1).unwrap();
        let errors = vm.onboard_copy_from_profile("race").unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(vm.device_read("wheel_range").unwrap(), Value::Int(540));
        assert_eq!(vm.device_read("wheel_led_effect").unwrap(), Value::Int(3));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn onboard_exit_without_restore_keeps_the_slot_active_and_closes_the_flow() {
        let vm = onboard_vm();
        vm.onboard_begin(4).unwrap();
        vm.onboard_exit(false).unwrap();
        assert!(!vm.onboard_active());
        assert_eq!(vm.device_read("wheel_profile").unwrap(), Value::Int(4));
    }

    #[test]
    fn onboard_exit_with_restore_switches_back() {
        let vm = onboard_vm();
        vm.onboard_begin(4).unwrap();
        vm.onboard_exit(true).unwrap();
        assert!(!vm.onboard_active());
        assert_eq!(vm.device_read("wheel_profile").unwrap(), Value::Int(0));
    }

    #[test]
    fn onboard_actions_outside_the_flow_error_instead_of_panicking() {
        let vm = onboard_vm();
        assert!(vm.onboard_set("wheel_strength", WidgetInput::Slider(1)).is_err());
        assert!(vm.onboard_set_name("x").is_err());
        assert!(vm.onboard_revert().is_err());
        assert!(vm.onboard_copy_from_profile("nope").is_err());
        vm.onboard_exit(true).unwrap(); // a no-op, not an error
    }
}
