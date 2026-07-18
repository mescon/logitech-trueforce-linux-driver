//! Converts a `viewmodel::Row` into the Slint-generated `SettingRow` struct
//! and builds the `ModelRc<SettingRow>` a `SettingsList` renders.
//!
//! Kept separate from `viewmodel` so that module stays Slint-free; this one
//! is the only place that knows about both worlds.

use logi_dd_core::curve::{Curve, FULL};
use logi_dd_core::{Access, Category, Color, DeviceInfo, Error, Kind, Mode, Value, REGISTRY};

use crate::viewmodel::Row;
use crate::{CurvePoint, LedColor, SettingRow};

// Stable `SettingRow.kind` tag numbering; keep in sync with the doc comment
// on `SettingRow` and the per-kind branches in `ui/widgets.slint`.
pub const KIND_PERCENT: i32 = 0;
pub const KIND_INT_RANGE: i32 = 1;
pub const KIND_ENUM: i32 = 2;
pub const KIND_TOGGLE: i32 = 3;
pub const KIND_TEXT: i32 = 4;
pub const KIND_ACTION: i32 = 5;
/// Everything without a live editor yet (deadzone pair, slot text) and any
/// read-only attribute: rendered as a plain value.
pub const KIND_READONLY: i32 = 6;
/// A curve row: rendered as a button that opens the curve editor overlay.
pub const KIND_CURVE: i32 = 7;
/// An RGB strip row: rendered as a button that opens the RGB strip editor
/// overlay.
pub const KIND_RGB: i32 = 8;

fn is_read_only(attr: &str) -> bool {
    REGISTRY.iter().any(|s| s.attr == attr && s.access == Access::ReadOnly)
}

fn kind_tag(attr: &str, kind: &Kind) -> i32 {
    if is_read_only(attr) {
        return KIND_READONLY;
    }
    match kind {
        Kind::Percent => KIND_PERCENT,
        Kind::IntRange { .. } => KIND_INT_RANGE,
        Kind::Enum(_) => KIND_ENUM,
        Kind::Toggle { .. } => KIND_TOGGLE,
        Kind::TextField { .. } => KIND_TEXT,
        Kind::Action => KIND_ACTION,
        Kind::Curve => KIND_CURVE,
        Kind::RgbStrip { .. } => KIND_RGB,
        Kind::Pair { .. } | Kind::SlotText { .. } => KIND_READONLY,
    }
}

/// Convert one view-model `Row` into the `SettingRow` the UI renders.
pub fn to_setting_row(row: &Row) -> SettingRow {
    let kind = *row.kind;
    let tag = kind_tag(row.attr, &kind);
    let display = row.value.as_ref().map(|v| kind.display(v)).unwrap_or_default();

    let (min, max, step, unit) = match kind {
        Kind::Percent => (0, 100, 1, "%"),
        Kind::IntRange { min, max, step, unit } => (min, max, step, unit),
        _ => (0, 0, 0, ""),
    };

    let choices: Vec<slint::SharedString> = match kind {
        Kind::Enum(variants) => variants.iter().map(|s| slint::SharedString::from(*s)).collect(),
        _ => Vec::new(),
    };

    let int_value = match (&kind, &row.value) {
        (Kind::Percent, Some(Value::Percent(n))) => i32::from(*n),
        (Kind::IntRange { .. }, Some(Value::Int(n))) => *n,
        (Kind::Enum(_), Some(Value::Enum(n))) => i32::from(*n),
        _ => 0,
    };

    let bool_value = matches!(row.value, Some(Value::Bool(true)));

    let text_value = match &row.value {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    };

    // Say *which* mode the greyed-out control needs, not just that it is
    // greyed out.
    let help = if row.mode_ok {
        row.help.to_string()
    } else if row.mode_req_desktop_only() {
        format!("{} Needs desktop mode.", row.help)
    } else if row.mode_req_onboard_only() {
        format!("{} Needs onboard mode.", row.help)
    } else {
        row.help.to_string()
    };

    SettingRow {
        attr: row.attr.into(),
        label: row.label.into(),
        help: help.into(),
        kind: tag,
        int_value,
        bool_value,
        text_value: text_value.into(),
        display: display.into(),
        choices: slint::ModelRc::new(slint::VecModel::from(choices)),
        min,
        max,
        step,
        unit: unit.into(),
        available: row.available,
        mode_ok: row.mode_ok,
        error: slint::SharedString::new(),
    }
}

/// Build the `ModelRc<SettingRow>` for a whole category's rows. `edit_error`
/// is `(attr, message)` for a failed edit: it is attached to the one row
/// that failed so the list shows an inline error next to the offending
/// control, while every other row reverts to its freshly re-read value.
pub fn rows_model(rows: &[Row], edit_error: Option<(&str, &str)>) -> slint::ModelRc<SettingRow> {
    let items: Vec<SettingRow> = rows
        .iter()
        .map(|row| {
            let mut sr = to_setting_row(row);
            if let Some((attr, message)) = edit_error {
                if row.attr == attr {
                    sr.error = message.into();
                }
            }
            sr
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// Sidebar labels for `Category::ALL`, in the same order the index the
/// sidebar reports (`select-category(index)`) is resolved against.
pub fn category_labels_model() -> slint::ModelRc<slint::SharedString> {
    let labels: Vec<slint::SharedString> =
        Category::ALL.iter().map(|c| slint::SharedString::from(c.label())).collect();
    slint::ModelRc::new(slint::VecModel::from(labels))
}

/// Resolve a sidebar row index (as the Slint `select-category` callback
/// hands it back) to the `Category` it represents. Slint only ever reports
/// an index the `for` loop actually produced, but a negative or
/// past-the-end value is clamped rather than indexed into a panic.
pub fn category_at(index: i32) -> Category {
    let last = Category::ALL.len() as i32 - 1;
    Category::ALL[index.clamp(0, last) as usize]
}

/// The inverse of `category_at`: which sidebar row index highlights
/// `category`.
pub fn index_of(category: Category) -> i32 {
    Category::ALL.iter().position(|c| *c == category).unwrap_or(0) as i32
}

/// Pull the header's fields out of a `DeviceInfo`: serial, firmware, and
/// whether the wheel is currently in onboard mode (the mode toggle's
/// `mode-onboard` property).
pub fn header_fields(info: &DeviceInfo) -> (String, String, bool) {
    (info.serial.clone(), info.firmware.clone(), matches!(info.mode, Mode::Onboard))
}

// --- curve editor <-> Curve conversions ---
//
// The curve editor (`ui/curve_editor.slint`) never touches a raw `(u16,
// u16)` curve point: it only ever draws and drags plain 0..1 screen
// fractions. `x` is always the input fraction (0 = min input, 1 = max
// input); `y` is the *screen* fraction (0 = top/full output, 1 =
// bottom/zero output), so a higher output value plots nearer the top. These
// helpers are the only place that conversion happens, in both directions.

/// Convert one `(input, output)` curve sample (each 0..=FULL) into the
/// editor's screen-fraction space.
fn to_screen_frac(input: u16, output: u16) -> (f32, f32) {
    let x = f32::from(input) / f32::from(FULL);
    let y = 1.0 - f32::from(output) / f32::from(FULL);
    (x, y)
}

/// Convert one screen fraction back to a `0..=FULL` curve value, clamping
/// anything a drag pushed outside `0.0..=1.0`.
fn from_frac(f: f32) -> u16 {
    (f.clamp(0.0, 1.0) * f32::from(FULL)).round() as u16
}

/// Build the SVG-style path-commands string `curve_editor.slint`'s `Path`
/// draws: one `M`/`L` command per `curve.compose()` sample, in screen-
/// fraction space (the `Path` itself uses a `1x1` viewbox, so these
/// fractions are exactly its coordinate system).
pub fn curve_plot_commands(curve: &Curve) -> String {
    let mut out = String::new();
    for (i, &(input, output)) in curve.compose().iter().enumerate() {
        let (x, y) = to_screen_frac(input, output);
        let op = if i == 0 { 'M' } else { 'L' };
        out.push_str(&format!("{op} {x:.6} {y:.6} "));
    }
    out.trim_end().to_string()
}

/// Convert `curve.points()` (the draggable control points, not the composed
/// plot) into the editor's `CurvePoint` list, same screen-fraction space as
/// `curve_plot_commands`.
pub fn curve_control_points(curve: &Curve) -> slint::ModelRc<CurvePoint> {
    let points: Vec<CurvePoint> = curve
        .points()
        .iter()
        .map(|&(input, output)| {
            let (x, y) = to_screen_frac(input, output);
            CurvePoint { x, y }
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(points))
}

/// Apply the editor's `move-point` callback (a control point index plus its
/// new screen-fraction position) to `curve`. `Curve::move_point` itself
/// clamps the input/output against the point's neighbours and leaves the
/// endpoints untouched, so this only needs to undo the screen-fraction
/// conversion before delegating.
pub fn apply_move_point(curve: &mut Curve, index: usize, x_frac: f32, y_frac: f32) {
    let input = from_frac(x_frac);
    let output = from_frac(1.0 - y_frac.clamp(0.0, 1.0));
    curve.move_point(index, input, output);
}

/// Apply the editor's `add-point` callback (an input screen-fraction) to
/// `curve`.
pub fn apply_add_point(curve: &mut Curve, x_frac: f32) {
    curve.add_point(from_frac(x_frac));
}

/// Apply the lower-deadzone slider's raw `int` (the Slint `Slider` reports a
/// `float` rounded to a plain `int` in `curve_editor.slint`) to `curve`,
/// clamping into the `u8` range `Curve::set_lower_deadzone` itself expects.
pub fn apply_lower_deadzone(curve: &mut Curve, v: i32) {
    curve.set_lower_deadzone(v.clamp(0, 99) as u8);
}

/// The upper-deadzone counterpart of `apply_lower_deadzone`.
pub fn apply_upper_deadzone(curve: &mut Curve, v: i32) {
    curve.set_upper_deadzone(v.clamp(0, 99) as u8);
}

// --- RGB strip editor <-> Vec<Color> conversions ---
//
// `ui/rgb_strip.slint` never touches `logi_dd_core::Color`: each swatch is
// plain 0..255 channel ints (`LedColor`) so the component can paint its
// background with Slint's builtin `rgb()` function. These helpers are the
// only place that conversion happens, in both directions, same pattern as
// the curve editor's screen-fraction helpers above.

fn clamp_channel(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Convert `colors` into the swatch list `rgb_strip.slint` renders.
pub fn rgb_leds_model(colors: &[Color]) -> slint::ModelRc<LedColor> {
    let items: Vec<LedColor> =
        colors.iter().map(|c| LedColor { r: i32::from(c.r), g: i32::from(c.g), b: i32::from(c.b) }).collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// `attr`'s default color list: every LED black, at that setting's own
/// `Kind::RgbStrip::leds` count. Seeds the editor when no live value has
/// been read yet (row unavailable on this wheel, or not seen since the app
/// started). Any other attr (should not happen; only an `RgbStrip`-kind
/// row's "Edit colors" button opens this editor) yields an empty list.
pub fn default_rgb(attr: &str) -> Vec<Color> {
    match REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.kind) {
        Some(Kind::RgbStrip { leds }) => vec![Color { r: 0, g: 0, b: 0 }; leds],
        _ => Vec::new(),
    }
}

/// Apply the picker's per-channel sliders (`set-color(index, r, g, b)`) to
/// `colors`, clamping each channel into `u8` (the sliders are already
/// `0..255`, but a defensive clamp costs nothing). An out-of-range `index`
/// (should not happen; every caller derives it from `colors`' own length,
/// same as the curve editor's control-point indices) is a no-op rather than
/// a panic.
pub fn apply_set_color(colors: &mut [Color], index: usize, r: i32, g: i32, b: i32) {
    if let Some(c) = colors.get_mut(index) {
        *c = Color { r: clamp_channel(r), g: clamp_channel(g), b: clamp_channel(b) };
    }
}

/// Apply the hex field (`set-hex(index, text)`) to `colors`. A bad hex
/// string leaves `colors` untouched rather than partially applying it.
pub fn apply_set_hex(colors: &mut [Color], index: usize, hex: &str) -> Result<(), Error> {
    let c = Color::from_hex(hex)?;
    if let Some(slot) = colors.get_mut(index) {
        *slot = c;
    }
    Ok(())
}

/// Apply "apply to all": every LED becomes `(r, g, b)`.
pub fn apply_to_all(colors: &mut [Color], r: i32, g: i32, b: i32) {
    let c = Color { r: clamp_channel(r), g: clamp_channel(g), b: clamp_channel(b) };
    for slot in colors.iter_mut() {
        *slot = c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::sysfs::FakeSysfs;
    use logi_dd_core::{Category, Device};
    use slint::Model as _;

    fn row_for(attr: &str) -> Row {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        fs.set("wheel_ffb_filter", "5");
        fs.set("wheel_ffb_filter_auto", "1");
        fs.set("wheel_texture_route", "tf");
        fs.set("wheel_serial", "ABC123");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        // Ffb + Info both have a row per attr under test here.
        vm.rows_for(Category::Ffb)
            .into_iter()
            .chain(vm.rows_for(Category::Info))
            .find(|r| r.attr == attr)
            .unwrap_or_else(|| panic!("no row for {attr}"))
    }

    #[test]
    fn percent_row_maps_to_slider_tag_with_unit_and_value() {
        let sr = to_setting_row(&row_for("wheel_strength"));
        assert_eq!(sr.kind, KIND_PERCENT);
        assert_eq!(sr.int_value, 80);
        assert_eq!(sr.min, 0);
        assert_eq!(sr.max, 100);
        assert_eq!(sr.unit, "%");
        assert_eq!(sr.display, "80%");
    }

    #[test]
    fn intrange_row_maps_to_slider_tag_with_kind_bounds() {
        let sr = to_setting_row(&row_for("wheel_ffb_filter"));
        assert_eq!(sr.kind, KIND_INT_RANGE);
        assert_eq!(sr.int_value, 5);
        assert_eq!(sr.min, 1);
        assert_eq!(sr.max, 15);
    }

    #[test]
    fn enum_row_maps_to_choices_and_current_index() {
        let sr = to_setting_row(&row_for("wheel_texture_route"));
        assert_eq!(sr.kind, KIND_ENUM);
        assert_eq!(sr.int_value, 1); // "tf" is index 1
        let choices: Vec<String> = sr.choices.iter().map(|s| s.to_string()).collect();
        assert_eq!(choices, vec!["kf", "tf"]);
    }

    #[test]
    fn toggle_row_maps_to_bool_value() {
        let sr = to_setting_row(&row_for("wheel_ffb_filter_auto"));
        assert_eq!(sr.kind, KIND_TOGGLE);
        assert!(sr.bool_value);
    }

    #[test]
    fn readonly_textfield_row_maps_to_readonly_tag() {
        // wheel_serial is TextField but ReadOnly access, so it renders as
        // a plain value, not an editable LineEdit.
        let sr = to_setting_row(&row_for("wheel_serial"));
        assert_eq!(sr.kind, KIND_READONLY);
        assert_eq!(sr.display, "ABC123");
    }

    #[test]
    fn unavailable_row_carries_that_through() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        // wheel_trueforce left absent -> unavailable on this (fake) wheel.
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm.rows_for(Category::Ffb).into_iter().find(|r| r.attr == "wheel_trueforce").unwrap();
        let sr = to_setting_row(&row);
        assert!(!sr.available);
        assert_eq!(sr.display, "");
    }

    #[test]
    fn rows_model_attaches_the_error_to_only_the_matching_row() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        fs.set("wheel_damping", "20");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let rows = vm.rows_for(Category::Ffb);

        let model = rows_model(&rows, Some(("wheel_strength", "value out of range")));
        let items: Vec<SettingRow> = model.iter().collect();

        let strength = items.iter().find(|r| r.attr == "wheel_strength").unwrap();
        assert_eq!(strength.error, "value out of range");

        let damping = items.iter().find(|r| r.attr == "wheel_damping").unwrap();
        assert_eq!(damping.error, "");
    }

    #[test]
    fn rows_model_with_no_error_leaves_every_row_clean() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let rows = vm.rows_for(Category::Ffb);

        let model = rows_model(&rows, None);
        let items: Vec<SettingRow> = model.iter().collect();
        assert!(items.iter().all(|r| r.error.is_empty()));
    }

    #[test]
    fn device_spec_lookup_agrees_with_registry_read_only_flag() {
        // Sanity check for is_read_only(), which drives the readonly tag.
        let spec = Device::<FakeSysfs>::spec("wheel_serial").unwrap();
        assert_eq!(spec.access, Access::ReadOnly);
    }

    #[test]
    fn text_row_maps_to_text_tag_with_text_value() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_slot_name", "MYSLOT");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm.rows_for(Category::Leds)
            .into_iter()
            .find(|r| r.attr == "wheel_led_slot_name")
            .expect("no row for wheel_led_slot_name");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_TEXT);
        assert_eq!(sr.text_value, "MYSLOT");
    }

    #[test]
    fn action_row_maps_to_action_tag_with_trigger_display() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_apply", "1");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm.rows_for(Category::Leds)
            .into_iter()
            .find(|r| r.attr == "wheel_led_apply")
            .expect("no row for wheel_led_apply");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_ACTION);
        assert_eq!(sr.display, "[trigger]");
    }

    #[test]
    fn category_labels_model_has_one_label_per_category_in_order() {
        let model = category_labels_model();
        let labels: Vec<String> = model.iter().map(|s| s.to_string()).collect();
        let expected: Vec<String> = Category::ALL.iter().map(|c| c.label().to_string()).collect();
        assert_eq!(labels, expected);
    }

    #[test]
    fn category_at_and_index_of_round_trip_for_every_category() {
        for cat in Category::ALL {
            let idx = index_of(*cat);
            assert_eq!(category_at(idx), *cat);
        }
    }

    #[test]
    fn category_at_clamps_out_of_range_indices() {
        assert_eq!(category_at(-1), Category::ALL[0]);
        assert_eq!(category_at(9999), *Category::ALL.last().unwrap());
    }

    #[test]
    fn header_fields_reads_serial_firmware_and_onboard_flag() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_serial", "ABC123");
        fs.set("wheel_firmware", "1.2.3");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let info = vm.info().unwrap();

        let (serial, firmware, onboard) = header_fields(&info);
        assert_eq!(serial, "ABC123");
        assert_eq!(firmware, "1.2.3");
        assert!(onboard);
    }

    #[test]
    fn header_fields_reports_desktop_as_not_onboard() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "X");
        fs.set("wheel_firmware", "1.0");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let info = vm.info().unwrap();

        let (_, _, onboard) = header_fields(&info);
        assert!(!onboard);
    }

    #[test]
    fn curve_row_maps_to_curve_tag() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_response_curve", "reset");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Steering)
            .into_iter()
            .find(|r| r.attr == "wheel_response_curve")
            .expect("no row for wheel_response_curve");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_CURVE);
        assert_eq!(sr.display, "built-in");
    }

    // --- curve editor <-> Curve conversions ---

    fn linear_curve() -> Curve {
        Curve::from_value("wheel_response_curve", &Value::Curve(vec![]))
    }

    #[test]
    fn plot_commands_trace_the_composed_curve_in_screen_fractions() {
        let c = linear_curve();
        // A plain linear curve composes to exactly its two endpoints:
        // (0, 0) plots at the bottom-left (y=1, full output at the top);
        // (FULL, FULL) plots at the top-right (y=0).
        let commands = curve_plot_commands(&c);
        assert_eq!(commands, "M 0.000000 1.000000 L 1.000000 0.000000");
    }

    #[test]
    fn plot_commands_reflect_a_bent_curve() {
        let mut c = linear_curve();
        c.add_point(FULL / 2); // (32767, 32767) by linear interpolation
        c.move_point(1, FULL / 2, FULL); // bend the midpoint to full output
        let commands = curve_plot_commands(&c);
        // Three samples now: (0,0), (~32767, FULL), (FULL, FULL). The bent
        // midpoint's output is full, so it plots at the top (y=0).
        assert!(commands.starts_with("M 0.000000 1.000000 L "));
        assert!(commands.contains(" 0.000000 L 1.000000 0.000000"));
    }

    #[test]
    fn control_points_mirror_curve_points_in_screen_fractions() {
        let mut c = linear_curve();
        c.add_point(FULL / 2);
        let points: Vec<CurvePoint> = {
            use slint::Model as _;
            curve_control_points(&c).iter().collect()
        };
        assert_eq!(points.len(), 3);
        assert_eq!((points[0].x, points[0].y), (0.0, 1.0));
        assert_eq!((points[2].x, points[2].y), (1.0, 0.0));
        // The midpoint sits at (~0.5, ~0.5) in screen fractions too, since
        // its output is half of full scale.
        assert!((points[1].x - 0.5).abs() < 0.01);
        assert!((points[1].y - 0.5).abs() < 0.01);
    }

    #[test]
    fn apply_move_point_undoes_the_screen_fraction_flip() {
        let mut c = linear_curve();
        c.add_point(FULL / 2); // index 1, the only movable point
        // Drag to 25% input, 25% *screen* fraction (near the top => a high
        // output), matching what a `moved` callback with those fractions
        // would report.
        apply_move_point(&mut c, 1, 0.25, 0.25);

        // The same edit performed directly against `Curve`'s own API (no
        // screen-fraction conversion) is the ground truth: 25% input, and a
        // screen y of 0.25 means 75% output.
        let mut expected = linear_curve();
        expected.add_point(FULL / 2);
        expected.move_point(1, from_frac(0.25), from_frac(0.75));

        assert_eq!(c.points(), expected.points());
    }

    #[test]
    fn apply_add_point_inserts_at_the_input_fraction() {
        let mut c = linear_curve();
        apply_add_point(&mut c, 0.5);

        let mut expected = linear_curve();
        expected.add_point(from_frac(0.5));

        assert_eq!(c.points(), expected.points());
    }

    #[test]
    fn apply_deadzones_clamp_and_forward_to_curve() {
        let mut c = linear_curve();
        apply_lower_deadzone(&mut c, 20);
        apply_upper_deadzone(&mut c, 15);
        assert_eq!(c.lower_deadzone(), 20);
        assert_eq!(c.upper_deadzone(), 15);

        // Out-of-range slider input (should not happen from the `0..99`
        // Slint `Slider`, but a defensive clamp costs nothing) never panics
        // and never exceeds the u8 range `Curve` itself expects. `Curve`
        // also keeps the pair summing to at most 99, so with upper still at
        // 15 the lower deadzone tops out at 84, not 99.
        apply_lower_deadzone(&mut c, 500);
        assert_eq!(c.lower_deadzone(), 84);
    }

    #[test]
    fn commit_produces_the_curves_own_value_via_to_value() {
        let mut c = linear_curve();
        apply_add_point(&mut c, 0.5);
        apply_move_point(&mut c, 1, 0.5, 0.2); // screen y=0.2 => 80% output
        apply_lower_deadzone(&mut c, 10);
        apply_upper_deadzone(&mut c, 5);

        // Committing writes `curve.to_value()`; confirm it is exactly the
        // composed curve `Curve::to_value` documents, not some other shape.
        assert_eq!(c.to_value(), Value::Curve(c.compose()));
        match c.to_value() {
            Value::Curve(pts) => {
                assert_eq!(pts.first(), Some(&(0, 0)));
                assert_eq!(pts.last(), Some(&(FULL, FULL)));
            }
            other => panic!("expected Value::Curve, got {other:?}"),
        }
    }

    // --- RGB strip editor <-> Vec<Color> conversions ---

    #[test]
    fn rgb_row_maps_to_rgb_tag() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let ten = "ff0000 00ff00 0000ff 000000 000000 000000 000000 000000 000000 000000";
        fs.set("wheel_led_colors", ten);
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Leds)
            .into_iter()
            .find(|r| r.attr == "wheel_led_colors")
            .expect("no row for wheel_led_colors");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_RGB);
        assert_eq!(sr.display, "10 LEDs");
    }

    #[test]
    fn rgb_leds_model_round_trips_colors() {
        let colors = vec![
            Color { r: 0xff, g: 0x00, b: 0x80 },
            Color { r: 0x01, g: 0x02, b: 0x03 },
        ];
        let model = rgb_leds_model(&colors);
        let items: Vec<LedColor> = model.iter().collect();
        assert_eq!(items.len(), colors.len());
        let back: Vec<Color> =
            items.iter().map(|l| Color { r: l.r as u8, g: l.g as u8, b: l.b as u8 }).collect();
        assert_eq!(back, colors);
    }

    #[test]
    fn default_rgb_is_all_black_at_the_registry_led_count() {
        let colors = default_rgb("wheel_led_colors");
        assert_eq!(colors.len(), 10);
        assert!(colors.iter().all(|c| *c == Color { r: 0, g: 0, b: 0 }));
    }

    #[test]
    fn default_rgb_for_an_unknown_attr_is_empty() {
        assert!(default_rgb("not_a_real_attr").is_empty());
    }

    #[test]
    fn apply_set_color_updates_only_the_indexed_led() {
        let mut colors = vec![Color { r: 0, g: 0, b: 0 }; 3];
        apply_set_color(&mut colors, 1, 10, 20, 30);
        assert_eq!(colors[1], Color { r: 10, g: 20, b: 30 });
        assert_eq!(colors[0], Color { r: 0, g: 0, b: 0 });
        assert_eq!(colors[2], Color { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn apply_set_color_clamps_out_of_range_channels() {
        let mut colors = vec![Color { r: 0, g: 0, b: 0 }];
        apply_set_color(&mut colors, 0, 300, -5, 999);
        assert_eq!(colors[0], Color { r: 255, g: 0, b: 255 });
    }

    #[test]
    fn apply_set_color_out_of_range_index_is_a_no_op() {
        let mut colors = vec![Color { r: 1, g: 2, b: 3 }];
        apply_set_color(&mut colors, 5, 9, 9, 9);
        assert_eq!(colors[0], Color { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn apply_set_hex_parses_and_replaces_the_indexed_led() {
        let mut colors = vec![Color { r: 0, g: 0, b: 0 }; 2];
        apply_set_hex(&mut colors, 1, "ff8000").unwrap();
        assert_eq!(colors[0], Color { r: 0, g: 0, b: 0 });
        assert_eq!(colors[1], Color { r: 0xff, g: 0x80, b: 0x00 });
    }

    #[test]
    fn apply_set_hex_rejects_bad_hex_and_leaves_colors_untouched() {
        let mut colors = vec![Color { r: 1, g: 2, b: 3 }];
        let result = apply_set_hex(&mut colors, 0, "zzzzzz");
        assert!(result.is_err());
        assert_eq!(colors[0], Color { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn apply_to_all_sets_every_led_to_the_same_color() {
        let mut colors = vec![Color { r: 1, g: 1, b: 1 }; 4];
        apply_to_all(&mut colors, 5, 6, 7);
        assert!(colors.iter().all(|c| *c == Color { r: 5, g: 6, b: 7 }));
    }
}
