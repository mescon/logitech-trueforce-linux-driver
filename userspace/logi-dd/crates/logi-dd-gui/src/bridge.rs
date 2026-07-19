//! Converts a `viewmodel::Row` into the Slint-generated `SettingRow` struct
//! and builds the `ModelRc<SettingRow>` a `SettingsList` renders.
//!
//! Kept separate from `viewmodel` so that module stays Slint-free; this one
//! is the only place that knows about both worlds.

use logi_dd_core::curve::{Curve, FULL};
use logi_dd_core::shaping::{self, ShapingRole};
use logi_dd_core::{lightsync, Access, Category, Color, Error, Kind, Value, REGISTRY};

use crate::viewmodel::Row;
use crate::{CurvePoint, LedColor, SettingRow, SlotNameRow};

// Stable `SettingRow.kind` tag numbering; keep in sync with the doc comment
// on `SettingRow` and the per-kind branches in `ui/widgets.slint`.
pub const KIND_PERCENT: i32 = 0;
pub const KIND_INT_RANGE: i32 = 1;
pub const KIND_ENUM: i32 = 2;
pub const KIND_TOGGLE: i32 = 3;
pub const KIND_TEXT: i32 = 4;
pub const KIND_ACTION: i32 = 5;
/// Any read-only attribute, regardless of its `Kind`: rendered as a plain
/// value.
pub const KIND_READONLY: i32 = 6;
/// A curve row: rendered as a button that opens the curve editor overlay.
pub const KIND_CURVE: i32 = 7;
/// An RGB strip row: rendered as a button that opens the RGB strip editor
/// overlay.
pub const KIND_RGB: i32 = 8;
/// A slot-text row (onboard profile renaming): rendered as a button that
/// opens the slot-text editor overlay.
pub const KIND_SLOTTEXT: i32 = 9;
/// A pedal deadzone pair: rendered as two SpinBoxes (lower/upper).
pub const KIND_PAIR: i32 = 10;
/// The active-profile row (`wheel_profile`), rewritten from its generic
/// `KIND_INT_RANGE` tag once the onboard profile names are known: rendered
/// as a ComboBox of "N: name" labels instead of a number spinner. Only
/// `apply_profile_choices` sets this tag; `kind_tag` never produces it,
/// since the profile names live in a different row (`wheel_profile_names`)
/// this stateless function cannot see.
pub const KIND_PROFILE: i32 = 11;
/// The LIGHTSYNC effect row (`wheel_led_effect`), rewritten from its
/// generic `KIND_INT_RANGE` tag on the composed LIGHTSYNC page: rendered
/// as a ComboBox over `lightsync::dropdown_labels` and committed through
/// the dedicated `edit-light-effect` callback (a CUSTOM pick writes two
/// attrs, so the plain index-commit paths do not fit). Only
/// `apply_lightsync_effect` sets this tag, same convention as
/// `KIND_PROFILE`.
pub const KIND_LIGHT_EFFECT: i32 = 12;
/// The synthetic "Edit slot" row `compose_lightsync` appends: a button
/// (enabled only while a CUSTOM effect is active, via `bool_value`) that
/// opens the slot editor overlay. Never produced by `kind_tag`; the row
/// does not exist in the registry.
pub const KIND_LIGHT_SLOT: i32 = 13;
/// A per-axis shaping toggle row `compose_shaping` inserts: a Switch whose
/// own label reads "Sensitivity" (off) or "Curve" (on). Never produced by
/// `kind_tag`; the row does not exist in the registry (its attr is
/// `shaping::toggle_attr`'s reserved `ui:` name, intercepted in `main.rs`).
pub const KIND_SHAPING: i32 = 14;

/// The synthetic "Edit slot" row's attr. Not a sysfs attribute: it only
/// exists so the row can be found in the model (`update_row`'s
/// find-by-attr) and so its button callback has something to report.
pub const LIGHT_EDIT_SLOT_ATTR: &str = "lightsync_edit_slot";

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
        Kind::SlotText { .. } => KIND_SLOTTEXT,
        Kind::Pair { .. } => KIND_PAIR,
    }
}

/// Convert one view-model `Row` into the `SettingRow` the UI renders.
pub fn to_setting_row(row: &Row) -> SettingRow {
    let kind = *row.kind;
    let tag = kind_tag(row.attr, &kind);
    let mut display = row.value.as_ref().map(|v| kind.display(v)).unwrap_or_default();
    // `Kind::display` collapses newlines into " / " for one-line surfaces
    // (the TUI's list style). The GUI's read-only rows grow with their
    // content, so keep the raw multi-line text instead: the firmware attr
    // then shows base on one line and motor on the next.
    if tag == KIND_READONLY {
        if let Some(Value::Text(s)) = &row.value {
            display = s.clone();
        }
    }

    let (min, max, step, unit) = match kind {
        Kind::Percent => (0, 100, 1, "%"),
        Kind::IntRange { min, max, step, unit } => (min, max, step, unit),
        Kind::Pair { max } => (0, i32::from(max), 1, "%"),
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
        (Kind::Pair { .. }, Some(Value::Pair(lo, _))) => i32::from(*lo),
        _ => 0,
    };

    let int_value2 = match (&kind, &row.value) {
        (Kind::Pair { .. }, Some(Value::Pair(_, hi))) => i32::from(*hi),
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
        int_value2,
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
        // Freshly built rows start at revision 0; `main.rs` bumps this on
        // every in-place push so touched widgets re-assert their display
        // (see `SettingRow`'s doc in `ui/widgets.slint`).
        revision: 0,
    }
}

/// Build the 6 dropdown labels for the `wheel_profile` row: index 0 is
/// always "0: Desktop"; index `i` (1..=5) is "`i`: `name`" using
/// `names[i - 1]` when present and non-empty, else a "Profile `i`"
/// placeholder. `names` is `wheel_profile_names`'s live value (one entry per
/// onboard slot); a short or empty list (unread yet, or no wheel attached)
/// still yields all 6 labels via the placeholder.
pub fn profile_choice_labels(names: &[String]) -> Vec<slint::SharedString> {
    let mut labels = Vec::with_capacity(6);
    labels.push(slint::SharedString::from("0: Desktop"));
    for i in 1..=5usize {
        let name = names.get(i - 1).filter(|n| !n.is_empty());
        let label = match name {
            Some(n) => format!("{i}: {n}"),
            None => format!("{i}: Profile {i}"),
        };
        labels.push(slint::SharedString::from(label));
    }
    labels
}

/// Rewrite `sr` (already built by `to_setting_row` for the `wheel_profile`
/// row) into the profile dropdown: tags it `KIND_PROFILE` and installs
/// `profile_choice_labels(names)` as its choices. `sr.int_value` (the active
/// profile number) is left untouched, so the ComboBox's `current-index`
/// still lands on the right entry. Only meaningful for `wheel_profile`;
/// callers guard on `attr` before calling this.
pub fn apply_profile_choices(sr: &mut SettingRow, names: &[String]) {
    sr.kind = KIND_PROFILE;
    sr.choices = slint::ModelRc::new(slint::VecModel::from(profile_choice_labels(names)));
}

/// The effect-selector labels as Slint strings, in
/// `lightsync::selection_index` order: the 4 sweeps, the 5 custom slots,
/// plus the trailing raw entry only while `effect` is outside 1-5 (see
/// `lightsync::dropdown_labels`). `slot_names` is the per-slot name cache
/// `main.rs` maintains (see its `led_slot_names` doc); missing, empty or
/// default-named entries fall back to the plain "CUSTOM N" labels.
pub fn lightsync_choice_labels(slot_names: &[String], effect: u8) -> Vec<slint::SharedString> {
    lightsync::dropdown_labels(slot_names, effect).into_iter().map(slint::SharedString::from).collect()
}

/// Rewrite `sr` (already built by `to_setting_row` for the
/// `wheel_led_effect` row, so `sr.int_value` is the raw effect value 1-9)
/// into the composed effect selector: tags it `KIND_LIGHT_EFFECT`,
/// installs the labels, and replaces `int_value` with the selector index
/// for the current effect + `slot` (`wheel_led_slot`'s value, which picks
/// the CUSTOM entry when the effect is 5). Only meaningful for
/// `wheel_led_effect`; callers guard on `attr` before calling this, same
/// convention as `apply_profile_choices`.
pub fn apply_lightsync_effect(sr: &mut SettingRow, slot: i32, slot_names: &[String]) {
    let effect = sr.int_value.clamp(0, u8::MAX as i32) as u8;
    let slot = slot.clamp(0, lightsync::CUSTOM_SLOTS as i32 - 1) as u8;
    sr.kind = KIND_LIGHT_EFFECT;
    sr.int_value = lightsync::selection_index(effect, slot) as i32;
    sr.choices = slint::ModelRc::new(slint::VecModel::from(lightsync_choice_labels(slot_names, effect)));
}

/// Record one slot-name read in the per-slot cache the effect selector's
/// CUSTOM labels are built from. `slot` MUST be the slot that was active
/// when `name` was read (`wheel_led_slot_name` only ever reads the ACTIVE
/// slot's name), and only that one entry is written: attributing a name to
/// any other slot is exactly the poisoned-cache bug this guards against.
/// An out-of-range slot is ignored rather than panicking.
pub fn record_led_slot_name(cache: &mut [String], slot: i32, name: &str) {
    if let Some(entry) = usize::try_from(slot).ok().and_then(|i| cache.get_mut(i)) {
        *entry = name.to_string();
    }
}

/// Build the synthetic "Edit slot" row `compose_lightsync` appends.
/// `bool_value` carries the enabled condition (a CUSTOM effect is active);
/// `available`/`mode_ok` are inherited from the effect row so the button
/// greys out alongside it instead of promising an editor a wheel without
/// LIGHTSYNC cannot honor.
fn light_slot_row(effect: i32, available: bool, mode_ok: bool) -> SettingRow {
    SettingRow {
        attr: LIGHT_EDIT_SLOT_ATTR.into(),
        label: "Custom slot".into(),
        help: "A custom slot is a saved lighting preset: colors, direction, name and brightness. Pick a CUSTOM effect above, then edit the active slot here.".into(),
        kind: KIND_LIGHT_SLOT,
        int_value: 0,
        int_value2: 0,
        bool_value: effect == 5,
        text_value: slint::SharedString::new(),
        display: slint::SharedString::new(),
        choices: slint::ModelRc::new(slint::VecModel::<slint::SharedString>::default()),
        min: 0,
        max: 0,
        step: 0,
        unit: slint::SharedString::new(),
        available,
        mode_ok,
        error: slint::SharedString::new(),
        revision: 0,
    }
}

/// Compose the LIGHTSYNC page out of the generic Leds rows: keep Effect
/// (rewritten into the composed selector) and Brightness, append the
/// "Edit slot" button row, and drop the slot-scoped rows (slot, name,
/// colors, direction, slot brightness, apply), which are all edited
/// through the slot editor overlay instead. Rows for any other category
/// (no `wheel_led_effect` present) pass through untouched, so `load_rows`
/// can call this unconditionally. The registry itself is not consulted:
/// everything needed (the raw effect and slot values) is read off the
/// rows, which a whole-category reload always carries together.
pub fn compose_lightsync(items: Vec<SettingRow>, slot_names: &[String]) -> Vec<SettingRow> {
    let Some(effect_row) = items.iter().find(|r| r.attr == "wheel_led_effect") else {
        return items;
    };
    let effect = effect_row.int_value;
    let (available, mode_ok) = (effect_row.available, effect_row.mode_ok);
    let slot = items.iter().find(|r| r.attr == "wheel_led_slot").map(|r| r.int_value).unwrap_or(0);
    let mut out = Vec::with_capacity(3);
    for mut item in items {
        match item.attr.as_str() {
            "wheel_led_effect" => {
                apply_lightsync_effect(&mut item, slot, slot_names);
                out.push(item);
            }
            "wheel_led_brightness" => out.push(item),
            _ => {}
        }
    }
    out.push(light_slot_row(effect, available, mode_ok));
    out
}

/// Build one synthetic per-axis shaping toggle row `compose_shaping`
/// inserts. A `KIND_SHAPING` Switch wearing that axis's reserved
/// `shaping::toggle_attr` attr: it commits through the same `edit-switch`
/// callback every real toggle uses, and `main.rs` intercepts those attrs
/// there to flip its local view state instead of sending a worker request
/// (the toggles are pure view state, never a device write).
fn shaping_toggle_row(axis: shaping::Axis, curve: bool) -> SettingRow {
    SettingRow {
        attr: shaping::toggle_attr(axis).into(),
        label: shaping::toggle_label(axis).into(),
        help: shaping::TOGGLE_HELP.into(),
        kind: KIND_SHAPING,
        int_value: 0,
        int_value2: 0,
        bool_value: curve,
        text_value: slint::SharedString::new(),
        display: slint::SharedString::new(),
        choices: slint::ModelRc::new(slint::VecModel::<slint::SharedString>::default()),
        min: 0,
        max: 0,
        step: 0,
        unit: slint::SharedString::new(),
        available: true,
        mode_ok: true,
        error: slint::SharedString::new(),
        revision: 0,
    }
}

/// Compose a category's rows for the per-axis shaping toggles: when any row
/// is a shaping generator (a sensitivity or a curve; see `shaping::role`),
/// insert each axis's toggle row right before that axis's first row (its
/// block heading) and keep only the rows `shaping::visible` allows for the
/// current toggles (an axis on "Sensitivity" hides its curve, an axis on
/// "Curve" hides its sensitivity, deadzones and everything else stay). Rows
/// for a category with no shaping generators pass through untouched, so
/// `load_rows` can call this unconditionally, same convention as
/// `compose_lightsync`.
pub fn compose_shaping(items: Vec<SettingRow>, toggles: shaping::AxisToggles) -> Vec<SettingRow> {
    if !items.iter().any(|r| shaping::role(r.attr.as_str()) != ShapingRole::Neutral) {
        return items;
    }
    let mut out = Vec::with_capacity(items.len() + shaping::Axis::ALL.len());
    let mut headed: Vec<shaping::Axis> = Vec::new();
    for item in items {
        if let Some(ax) = shaping::axis(item.attr.as_str()) {
            if !headed.contains(&ax) {
                headed.push(ax);
                out.push(shaping_toggle_row(ax, toggles.get(ax)));
            }
        }
        if shaping::visible(item.attr.as_str(), toggles) {
            out.push(item);
        }
    }
    out
}

/// Build the `ModelRc<SettingRow>` for a whole category's rows. `edit_error`
/// is `(attr, message)` for a failed edit: it is attached to the one row
/// that failed so the list shows an inline error next to the offending
/// control, while every other row reverts to its freshly re-read value.
///
/// Not called from `main.rs` any more: a whole-category reload now mutates
/// the persistent rows model in place via `setting_rows`, and a single
/// edit's result via `to_setting_row_with_error`, neither of which needs a
/// fresh `ModelRc`. Kept (and still tested) as the one place this exact
/// "rows plus one attached error" conversion is documented and verified.
#[allow(dead_code)]
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

/// Convert `rows` into a plain `Vec<SettingRow>` (the same mapping
/// `rows_model` does, minus the `ModelRc`/`VecModel` wrapping). Used for a
/// whole-category reload against the persistent rows model `main.rs` keeps
/// alive for the life of the app: that model's contents are mutated in
/// place (`set_row_data`/`set_vec`) rather than the model itself being
/// replaced, so nothing here needs to allocate a fresh `ModelRc`.
pub fn setting_rows(rows: &[Row]) -> Vec<SettingRow> {
    rows.iter().map(to_setting_row).collect()
}

/// Convert one `Row` into the `SettingRow` a single-row model update pushes
/// (`Response::RowUpdated`), with `error` (the edit's failure message, if
/// any) attached directly. Same conversion `to_setting_row` does; there is
/// only the one row here, so there is no attr to match against the way
/// `rows_model`'s `edit_error` does.
pub fn to_setting_row_with_error(row: &Row, error: Option<&str>) -> SettingRow {
    let mut sr = to_setting_row(row);
    if let Some(message) = error {
        sr.error = message.into();
    }
    sr
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
/// plot) into `(x_frac, y_frac)` pairs, same screen-fraction space as
/// `curve_plot_commands`. Shared by `curve_control_points` (the Slint-facing
/// list) and `control_point_fracs` (the Rust-side hit-testing list), so both
/// stay in exact agreement.
fn control_point_frac_pairs(curve: &Curve) -> Vec<(f32, f32)> {
    curve
        .points()
        .iter()
        .map(|&(input, output)| to_screen_frac(input, output))
        .collect()
}

/// Convert `curve.points()` (the draggable control points, not the composed
/// plot) into the editor's `CurvePoint` list, same screen-fraction space as
/// `curve_plot_commands`.
pub fn curve_control_points(curve: &Curve) -> slint::ModelRc<CurvePoint> {
    let points: Vec<CurvePoint> =
        control_point_frac_pairs(curve).into_iter().map(|(x, y)| CurvePoint { x, y }).collect();
    slint::ModelRc::new(slint::VecModel::from(points))
}

/// Same control points as `curve_control_points`, as plain `(x_frac, y_frac)`
/// tuples for Rust-side hit-testing (`grab-point`'s nearest-point search),
/// without going through the Slint model type.
pub fn control_point_fracs(curve: &Curve) -> Vec<(f32, f32)> {
    control_point_frac_pairs(curve)
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

// --- slot-text editor <-> Vec<String> conversions ---
//
// `ui/slot_text.slint` never touches `logi_dd_core::Value`: it renders one
// `SlotNameRow` (1-based slot number plus that slot's name) per entry in the
// `Value::SlotNames` list a `SlotText` row reads back. These helpers are the
// only place that conversion happens, same pattern as `rgb_leds_model` and
// `default_rgb` above.

/// Convert `names` (one entry per onboard slot, index 0 = slot 1) into the
/// slot-name row list the editor renders.
pub fn slot_names_model(names: &[String]) -> slint::ModelRc<SlotNameRow> {
    let items: Vec<SlotNameRow> = names
        .iter()
        .enumerate()
        .map(|(i, name)| SlotNameRow { slot: i as i32 + 1, name: name.clone().into() })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// `attr`'s default slot-name list: `slots` empty strings, at that setting's
/// own `Kind::SlotText::slots` count. Seeds the editor when no live value has
/// been read yet, same fallback role as `default_rgb`. Any other attr
/// (should not happen; only a `SlotText`-kind row's "Edit slot names" button
/// opens this editor) yields an empty list.
pub fn default_slot_names(attr: &str) -> Vec<String> {
    match REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.kind) {
        Some(Kind::SlotText { slots, .. }) => vec![String::new(); slots as usize],
        _ => Vec::new(),
    }
}

/// `attr`'s per-slot name length limit (`Kind::SlotText::max_len`), for the
/// editor's hint text. Any other attr yields 0.
pub fn slot_text_max_len(attr: &str) -> i32 {
    match REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.kind) {
        Some(Kind::SlotText { max_len, .. }) => max_len as i32,
        _ => 0,
    }
}

/// Apply the editor's `set-slot-name(slot, name)` callback (1-based `slot`)
/// to `names`. An out-of-range slot (should not happen; the editor only ever
/// hands back a slot number it rendered from `names` itself) is a no-op
/// rather than a panic, same convention as `apply_set_color`.
pub fn apply_set_slot_name(names: &mut [String], slot: u8, name: &str) {
    if let Some(n) = names.get_mut(slot.saturating_sub(1) as usize) {
        *n = name.to_string();
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
    fn pair_row_maps_to_pair_tag_with_both_values() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_throttle_deadzone", "8 5");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Pedals)
            .into_iter()
            .find(|r| r.attr == "wheel_throttle_deadzone")
            .expect("no row for wheel_throttle_deadzone");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_PAIR);
        assert_eq!(sr.int_value, 8);
        assert_eq!(sr.int_value2, 5);
        assert_eq!(sr.min, 0);
        assert_eq!(sr.max, 99);
        assert_eq!(sr.unit, "%");
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
    fn readonly_multiline_text_keeps_its_line_breaks() {
        // The firmware attr reads back two lines (base and motor); the
        // GUI row must show them as two lines, not "base ... / motor ...".
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_firmware", "base: U1 65.04.B0039\nmotor: SC 02.01.B0042");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row =
            vm.rows_for(Category::Info).into_iter().find(|r| r.attr == "wheel_firmware").unwrap();
        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_READONLY);
        assert_eq!(sr.display, "base: U1 65.04.B0039\nmotor: SC 02.01.B0042");
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
    fn setting_rows_matches_rows_model_contents() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let rows = vm.rows_for(Category::Ffb);

        let plain = setting_rows(&rows);
        let modeled: Vec<SettingRow> = rows_model(&rows, None).iter().collect();
        assert_eq!(plain.len(), modeled.len());
        for (p, m) in plain.iter().zip(modeled.iter()) {
            assert_eq!(p.attr, m.attr);
            assert_eq!(p.display, m.display);
            assert_eq!(p.error, m.error);
        }
    }

    #[test]
    fn to_setting_row_with_error_attaches_the_message() {
        let row = row_for("wheel_strength");
        let sr = to_setting_row_with_error(&row, Some("value out of range"));
        assert_eq!(sr.error, "value out of range");
    }

    #[test]
    fn to_setting_row_with_error_of_none_leaves_it_empty() {
        let row = row_for("wheel_strength");
        let sr = to_setting_row_with_error(&row, None);
        assert_eq!(sr.error, "");
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
    fn control_point_fracs_matches_curve_control_points() {
        let mut c = linear_curve();
        c.add_point(FULL / 2);

        let model: Vec<CurvePoint> = {
            use slint::Model as _;
            curve_control_points(&c).iter().collect()
        };
        let fracs = control_point_fracs(&c);

        assert_eq!(model.len(), fracs.len());
        for (point, &(x, y)) in model.iter().zip(fracs.iter()) {
            assert_eq!((point.x, point.y), (x, y));
        }
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

    // --- slot-text editor <-> Vec<String> conversions ---

    #[test]
    fn slot_text_row_maps_to_slot_text_tag() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile_names", "1: A\n2: B\n3: C\n4: D\n5: E");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Profiles)
            .into_iter()
            .find(|r| r.attr == "wheel_profile_names")
            .expect("no row for wheel_profile_names");

        let sr = to_setting_row(&row);
        assert_eq!(sr.kind, KIND_SLOTTEXT);
        assert_eq!(sr.display, "1: A  2: B  3: C  4: D  5: E");
    }

    #[test]
    fn slot_names_model_round_trips_names_in_order() {
        let names = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let model = slot_names_model(&names);
        let items: Vec<SlotNameRow> = model.iter().collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].slot, 1);
        assert_eq!(items[0].name, "A");
        assert_eq!(items[1].slot, 2);
        assert_eq!(items[1].name, "B");
        assert_eq!(items[2].slot, 3);
        assert_eq!(items[2].name, "C");
    }

    #[test]
    fn default_slot_names_is_all_empty_at_the_registry_slot_count() {
        let names = default_slot_names("wheel_profile_names");
        assert_eq!(names.len(), 5);
        assert!(names.iter().all(String::is_empty));
    }

    #[test]
    fn default_slot_names_for_an_unknown_attr_is_empty() {
        assert!(default_slot_names("not_a_real_attr").is_empty());
    }

    #[test]
    fn slot_text_max_len_reads_the_registrys_kind() {
        assert_eq!(slot_text_max_len("wheel_profile_names"), 9);
        assert_eq!(slot_text_max_len("not_a_real_attr"), 0);
    }

    #[test]
    fn apply_set_slot_name_updates_only_the_indexed_slot() {
        let mut names = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        apply_set_slot_name(&mut names, 2, "GT7");
        assert_eq!(names, vec!["A".to_string(), "GT7".to_string(), "C".to_string()]);
    }

    #[test]
    fn apply_set_slot_name_out_of_range_slot_is_a_no_op() {
        let mut names = vec!["A".to_string()];
        apply_set_slot_name(&mut names, 9, "X");
        assert_eq!(names, vec!["A".to_string()]);
    }

    // --- profile dropdown conversion ---

    #[test]
    fn apply_profile_choices_rewrites_kind_and_choices_but_keeps_int_value() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "2");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let row = vm
            .rows_for(Category::Profiles)
            .into_iter()
            .find(|r| r.attr == "wheel_profile")
            .expect("no row for wheel_profile");

        let mut sr = to_setting_row(&row);
        assert_eq!(sr.int_value, 2);

        let names = vec!["AC EVO".to_string(), "GT7".to_string(), String::new(), String::new(), String::new()];
        apply_profile_choices(&mut sr, &names);

        assert_eq!(sr.kind, KIND_PROFILE);
        assert_eq!(sr.int_value, 2);
        let choices: Vec<String> = sr.choices.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            choices,
            vec!["0: Desktop", "1: AC EVO", "2: GT7", "3: Profile 3", "4: Profile 4", "5: Profile 5"]
        );
    }

    // --- LIGHTSYNC page composition ---

    fn leds_setting_rows(effect: &str, slot: &str) -> Vec<SettingRow> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", effect);
        fs.set("wheel_led_slot", slot);
        fs.set("wheel_led_brightness", "80");
        fs.set("wheel_led_slot_name", "RACE");
        fs.set("wheel_led_direction", "2");
        fs.set("wheel_led_slot_brightness", "70");
        let ten = "ff0000 00ff00 0000ff 000000 000000 000000 000000 000000 000000 000000";
        fs.set("wheel_led_colors", ten);
        fs.set("wheel_rev_level", "3");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        setting_rows(&vm.rows_for(Category::Leds))
    }

    #[test]
    fn compose_lightsync_keeps_only_the_composed_rows_in_order() {
        let out = compose_lightsync(leds_setting_rows("3", "0"), &[]);
        let attrs: Vec<String> = out.iter().map(|r| r.attr.to_string()).collect();
        assert_eq!(attrs, vec!["wheel_led_effect", "wheel_led_brightness", LIGHT_EDIT_SLOT_ATTR]);
    }

    #[test]
    fn compose_lightsync_rewrites_the_effect_row_into_the_selector() {
        let names = vec!["GT7".to_string()];
        let out = compose_lightsync(leds_setting_rows("3", "0"), &names);
        let effect = out.iter().find(|r| r.attr == "wheel_led_effect").unwrap();
        assert_eq!(effect.kind, KIND_LIGHT_EFFECT);
        // effect 3 = "Right to left" = selector index 2
        assert_eq!(effect.int_value, 2);
        let choices: Vec<String> = effect.choices.iter().map(|s| s.to_string()).collect();
        assert_eq!(choices.len(), 9, "4 sweeps + 5 custom slots, no unlabeled effects");
        assert_eq!(choices[2], "Right to left");
        assert_eq!(choices[4], "CUSTOM 1: GT7");
        assert_eq!(choices[5], "CUSTOM 2");
        assert!(!choices.iter().any(|c| c.starts_with("Effect ")));
    }

    #[test]
    fn compose_lightsync_appends_the_raw_entry_for_an_out_of_range_effect() {
        let out = compose_lightsync(leds_setting_rows("7", "0"), &[]);
        let effect = out.iter().find(|r| r.attr == "wheel_led_effect").unwrap();
        assert_eq!(effect.int_value, 9, "the trailing raw entry is selected");
        let choices: Vec<String> = effect.choices.iter().map(|s| s.to_string()).collect();
        assert_eq!(choices.len(), 10);
        assert_eq!(choices[9], "Effect 7");
    }

    #[test]
    fn compose_lightsync_custom_effect_selects_the_slot_entry_and_enables_the_button() {
        let out = compose_lightsync(leds_setting_rows("5", "2"), &[]);
        let effect = out.iter().find(|r| r.attr == "wheel_led_effect").unwrap();
        // effect 5 + slot 2 = "CUSTOM 3" = selector index 6
        assert_eq!(effect.int_value, 6);
        let button = out.iter().find(|r| r.attr == LIGHT_EDIT_SLOT_ATTR).unwrap();
        assert_eq!(button.kind, KIND_LIGHT_SLOT);
        assert!(button.bool_value, "a CUSTOM effect enables the Edit slot button");
        assert!(button.available);
    }

    #[test]
    fn compose_lightsync_non_custom_effect_disables_the_button() {
        let out = compose_lightsync(leds_setting_rows("1", "0"), &[]);
        let button = out.iter().find(|r| r.attr == LIGHT_EDIT_SLOT_ATTR).unwrap();
        assert!(!button.bool_value);
    }

    #[test]
    fn compose_lightsync_leaves_other_categories_untouched() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        let items = setting_rows(&vm.rows_for(Category::Ffb));
        let attrs_before: Vec<String> = items.iter().map(|r| r.attr.to_string()).collect();
        let out = compose_lightsync(items, &[]);
        let attrs_after: Vec<String> = out.iter().map(|r| r.attr.to_string()).collect();
        assert_eq!(attrs_before, attrs_after);
    }

    #[test]
    fn apply_lightsync_effect_maps_raw_effect_to_selector_index() {
        let mut rows = leds_setting_rows("5", "0");
        let sr = rows.iter_mut().find(|r| r.attr == "wheel_led_effect").unwrap();
        assert_eq!(sr.int_value, 5, "precondition: raw effect value");
        apply_lightsync_effect(sr, 4, &[]);
        assert_eq!(sr.kind, KIND_LIGHT_EFFECT);
        assert_eq!(sr.int_value, 8, "effect 5 + slot 4 = CUSTOM 5 = index 8");
    }

    // --- the per-slot name cache ---

    #[test]
    fn record_led_slot_name_updates_only_the_named_slot() {
        // Regression for the poisoned cache the user saw ("CUSTOM 2:
        // CUSTOM 1", "CUSTOM 3: CUSTOM 1"): a name read while slot N was
        // active must land in entry N and nowhere else.
        let mut cache = vec![String::new(); 5];
        record_led_slot_name(&mut cache, 0, "RACE");
        assert_eq!(cache, vec!["RACE", "", "", "", ""]);
        record_led_slot_name(&mut cache, 2, "GT7");
        assert_eq!(cache, vec!["RACE", "", "GT7", "", ""]);
        // Re-reading slot 0's name never touches the other entries.
        record_led_slot_name(&mut cache, 0, "DRIFT");
        assert_eq!(cache, vec!["DRIFT", "", "GT7", "", ""]);
    }

    #[test]
    fn record_led_slot_name_follows_a_slot_switch_sequence() {
        // The sequence behind the bug report: activate slots 0, 1, 2 in
        // turn, reading each one's (own) name while it is active. Every
        // entry must hold the name read while THAT slot was active.
        let mut cache = vec![String::new(); 5];
        for (slot, name) in [(0, "CUSTOM 1"), (1, "CUSTOM 2"), (2, "MY SLOT")] {
            record_led_slot_name(&mut cache, slot, name);
        }
        assert_eq!(cache, vec!["CUSTOM 1", "CUSTOM 2", "MY SLOT", "", ""]);
    }

    #[test]
    fn record_led_slot_name_ignores_out_of_range_slots() {
        let mut cache = vec!["A".to_string()];
        record_led_slot_name(&mut cache, -1, "X");
        record_led_slot_name(&mut cache, 5, "X");
        assert_eq!(cache, vec!["A".to_string()]);
    }

    // --- advanced-shaping composition ---

    fn category_setting_rows(cat: Category) -> Vec<SettingRow> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        setting_rows(&vm.rows_for(cat))
    }

    fn attrs_of(rows: &[SettingRow]) -> Vec<String> {
        rows.iter().map(|r| r.attr.to_string()).collect()
    }

    use logi_dd_core::shaping::{Axis, AxisToggles};

    fn all_curves() -> AxisToggles {
        let mut t = AxisToggles::default();
        for ax in Axis::ALL {
            t.set(ax, true);
        }
        t
    }

    #[test]
    fn compose_shaping_simple_steering_shows_sensitivity_not_the_curve() {
        let out = compose_shaping(category_setting_rows(Category::Steering), AxisToggles::default());
        let attrs = attrs_of(&out);
        assert_eq!(
            attrs,
            vec![
                "wheel_range",
                "wheel_range_restore",
                shaping::toggle_attr(Axis::Steering),
                "wheel_sensitivity",
                "wheel_calibrate_here",
                "wheel_rev_level",
            ]
        );
    }

    #[test]
    fn compose_shaping_curve_steering_shows_the_curve_not_sensitivity() {
        let out = compose_shaping(category_setting_rows(Category::Steering), all_curves());
        let attrs = attrs_of(&out);
        assert_eq!(
            attrs,
            vec![
                "wheel_range",
                "wheel_range_restore",
                shaping::toggle_attr(Axis::Steering),
                "wheel_response_curve",
                "wheel_calibrate_here",
                "wheel_rev_level",
            ]
        );
    }

    #[test]
    fn compose_shaping_simple_pedals_heads_each_block_and_hides_curves() {
        let out = compose_shaping(category_setting_rows(Category::Pedals), AxisToggles::default());
        let attrs = attrs_of(&out);
        assert_eq!(
            attrs,
            vec![
                "wheel_combined_pedals",
                "wheel_brake_force",
                shaping::toggle_attr(Axis::Throttle),
                "wheel_throttle_sensitivity",
                "wheel_throttle_deadzone",
                shaping::toggle_attr(Axis::Brake),
                "wheel_brake_sensitivity",
                "wheel_brake_deadzone",
                shaping::toggle_attr(Axis::Clutch),
                "wheel_clutch_sensitivity",
                "wheel_clutch_deadzone",
                shaping::toggle_attr(Axis::Handbrake),
                "wheel_handbrake_sensitivity",
            ]
        );
    }

    #[test]
    fn compose_shaping_curve_pedals_keep_the_deadzone_right_after_the_curve() {
        let out = compose_shaping(category_setting_rows(Category::Pedals), all_curves());
        let attrs = attrs_of(&out);
        assert_eq!(
            attrs,
            vec![
                "wheel_combined_pedals",
                "wheel_brake_force",
                shaping::toggle_attr(Axis::Throttle),
                "wheel_throttle_curve",
                "wheel_throttle_deadzone",
                shaping::toggle_attr(Axis::Brake),
                "wheel_brake_curve",
                "wheel_brake_deadzone",
                shaping::toggle_attr(Axis::Clutch),
                "wheel_clutch_curve",
                "wheel_clutch_deadzone",
                shaping::toggle_attr(Axis::Handbrake),
                "wheel_handbrake_curve",
            ]
        );
    }

    #[test]
    fn compose_shaping_curve_pedals_keeps_deadzones_and_hides_sensitivities() {
        let out = compose_shaping(category_setting_rows(Category::Pedals), all_curves());
        let attrs = attrs_of(&out);
        for kept in [
            "wheel_throttle_deadzone",
            "wheel_throttle_curve",
            "wheel_brake_deadzone",
            "wheel_brake_curve",
            "wheel_clutch_deadzone",
            "wheel_clutch_curve",
            "wheel_handbrake_curve",
        ] {
            assert!(attrs.contains(&kept.to_string()), "missing {kept}");
        }
        assert!(
            !attrs.iter().any(|a| a.ends_with("_sensitivity")),
            "sensitivities hidden while every axis shows its curve: {attrs:?}"
        );
    }

    #[test]
    fn compose_shaping_mixes_axes_independently() {
        // The user's own example: throttle on simple sensitivity, brake on
        // the curve editor, at the same time.
        let mut toggles = AxisToggles::default();
        toggles.set(Axis::Brake, true);
        let out = compose_shaping(category_setting_rows(Category::Pedals), toggles);
        let attrs = attrs_of(&out);
        assert!(attrs.contains(&"wheel_throttle_sensitivity".to_string()));
        assert!(!attrs.contains(&"wheel_throttle_curve".to_string()));
        assert!(attrs.contains(&"wheel_brake_curve".to_string()));
        assert!(!attrs.contains(&"wheel_brake_sensitivity".to_string()));
        // Both blocks keep their deadzones and their own toggle row.
        assert!(attrs.contains(&"wheel_throttle_deadzone".to_string()));
        assert!(attrs.contains(&"wheel_brake_deadzone".to_string()));
        assert!(attrs.contains(&shaping::toggle_attr(Axis::Throttle).to_string()));
        assert!(attrs.contains(&shaping::toggle_attr(Axis::Brake).to_string()));
    }

    #[test]
    fn compose_shaping_toggle_rows_carry_the_axis_flag_and_help() {
        let mut toggles = AxisToggles::default();
        toggles.set(Axis::Brake, true);
        let out = compose_shaping(category_setting_rows(Category::Pedals), toggles);
        let brake = out.iter().find(|r| r.attr == shaping::toggle_attr(Axis::Brake)).unwrap();
        assert_eq!(brake.kind, KIND_SHAPING);
        assert!(brake.bool_value, "brake toggle is on");
        assert_eq!(brake.label, shaping::toggle_label(Axis::Brake));
        assert_eq!(brake.help, shaping::TOGGLE_HELP);
        assert!(brake.available && brake.mode_ok);
        let throttle = out.iter().find(|r| r.attr == shaping::toggle_attr(Axis::Throttle)).unwrap();
        assert!(!throttle.bool_value, "throttle toggle is off");
    }

    #[test]
    fn compose_shaping_leaves_other_categories_untouched() {
        for cat in [Category::Ffb, Category::Leds, Category::Profiles, Category::Info] {
            let items = category_setting_rows(cat);
            let before = attrs_of(&items);
            let after = attrs_of(&compose_shaping(items, all_curves()));
            assert_eq!(before, after, "{cat:?} must pass through");
        }
    }

    #[test]
    fn set_slot_name_round_trips_through_widget_input_to_a_device_write() {
        // The overlay's `set-slot-name(slot, name)` callback: apply locally
        // (what `push_slot_text_editor` does optimistically in main.rs)...
        let mut names = default_slot_names("wheel_profile_names");
        apply_set_slot_name(&mut names, 2, "GT7");
        assert_eq!(names[1], "GT7");

        // ...then the same (slot, name) becomes the `WidgetInput::SlotText`
        // the callback sends to the worker; `ViewModel::edit` converts it to
        // `Value::SlotName` and writes only that one slot.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile_names", "1: A\n2: B\n3: C\n4: D\n5: E");
        let vm = crate::viewmodel::ViewModel::with_io(fs);
        vm.edit(
            "wheel_profile_names",
            crate::viewmodel::WidgetInput::SlotText { slot: 2, text: "GT7".into() },
        )
        .unwrap();
        match vm.device_read("wheel_profile_names").unwrap() {
            Value::SlotNames(names) => assert_eq!(names[1], "GT7"),
            other => panic!("expected SlotNames, got {other:?}"),
        }
    }
}
