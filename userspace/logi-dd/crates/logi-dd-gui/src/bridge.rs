//! Converts a `viewmodel::Row` into the Slint-generated `SettingRow` struct
//! and builds the `ModelRc<SettingRow>` a `SettingsList` renders.
//!
//! Kept separate from `viewmodel` so that module stays Slint-free; this one
//! is the only place that knows about both worlds.

use logi_dd_core::{Access, Kind, Value, REGISTRY};

use crate::viewmodel::Row;
use crate::SettingRow;

// Stable `SettingRow.kind` tag numbering; keep in sync with the doc comment
// on `SettingRow` and the per-kind branches in `ui/widgets.slint`.
pub const KIND_PERCENT: i32 = 0;
pub const KIND_INT_RANGE: i32 = 1;
pub const KIND_ENUM: i32 = 2;
pub const KIND_TOGGLE: i32 = 3;
pub const KIND_TEXT: i32 = 4;
pub const KIND_ACTION: i32 = 5;
/// Everything without a live editor yet (curve, deadzone pair, RGB strip,
/// slot text) and any read-only attribute: rendered as a plain value.
pub const KIND_READONLY: i32 = 6;

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
        Kind::Curve | Kind::Pair { .. } | Kind::RgbStrip { .. } | Kind::SlotText { .. } => {
            KIND_READONLY
        }
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
}
