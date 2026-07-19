//! Pure Rust view-model over `logi-dd-core`: turns `REGISTRY` into the rows a
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

use logi_dd_core::curve::Curve;
use logi_dd_core::profiles;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Color, Device, DeviceInfo, Error, Kind, Mode, ModeReq, Value, REGISTRY};
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

/// Wraps a `Device` and adapts `logi-dd-core`'s registry/value model to what
/// a GUI widget tree renders and edits.
pub struct ViewModel<S: SysfsIo> {
    device: Device<S>,
    /// Where the computer-side profile store lives; resolved once from the
    /// environment (`profiles::default_dir`), overridable for tests.
    profiles_dir: PathBuf,
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
        ViewModel { device, profiles_dir: profiles::default_dir() }
    }

    /// Point the computer-side profile store somewhere else (tests only;
    /// production always uses `profiles::default_dir`).
    #[allow(dead_code)]
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
        self.profiles_dir = dir;
    }

    /// Rows for one category, in registry order. `mode_ok` is computed
    /// against a single read of the device's current mode.
    pub fn rows_for(&self, cat: Category) -> Vec<Row> {
        let mode = self.device.current_mode().ok();
        REGISTRY
            .iter()
            .filter(|spec| spec.category == cat)
            .map(|spec| {
                let available = self.device.available(spec.attr);
                let value = if available { self.device.read(spec.attr).ok() } else { None };
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

    /// The header's device-identity panel: serial, firmware, current mode.
    pub fn info(&self) -> Result<DeviceInfo, Error> {
        self.device.info()
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

    /// Test/debug hook: read a raw attribute back through the wrapped device.
    #[allow(dead_code)]
    pub fn device_read(&self, attr: &str) -> Result<Value, Error> {
        self.device.read(attr)
    }
}

/// Convert a widget's raw input into the `Value` its setting's `Kind` needs,
/// per the spec's own union of widget-shape and kind.
fn to_value(kind: Kind, input: WidgetInput) -> Result<Value, Error> {
    match (kind, input) {
        (Kind::Percent, WidgetInput::Slider(n)) => Ok(Value::Percent(u8::try_from(n).map_err(|_| Error::Invalid)?)),
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
    use logi_dd_core::sysfs::FakeSysfs;

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
}
