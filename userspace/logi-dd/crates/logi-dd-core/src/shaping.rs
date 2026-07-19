//! Simple-vs-advanced classification for the response-shaping settings,
//! per axis.
//!
//! The driver programs ONE response curve (0x80A4) per axis. A sensitivity
//! percentage (50 = linear) and the full curve editor are two generators
//! for that same device state: whichever writes last wins. Showing both at
//! once therefore misleads (editing a curve makes the sensitivity number
//! meaningless, and vice versa), so front-ends show only one of the two at
//! a time. Which one is a PER-AXIS choice: e.g. throttle on the simple
//! sensitivity control while the brake uses the full curve editor. Each
//! axis gets its own view toggle, rendered as a synthetic row heading that
//! axis's block. The toggles are pure view state: never persisted, never
//! written to sysfs. Deadzones feed the same composed curve but stay
//! meaningful in both modes, so they are `Neutral` and always shown.

/// How a settings row relates to an axis's shaping view toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapingRole {
    /// A sensitivity percentage: shown while the axis's toggle is off.
    Simple,
    /// A full response-curve editor: shown while the axis's toggle is on.
    Advanced,
    /// Unrelated to the simple/advanced choice (deadzones included):
    /// always shown.
    Neutral,
}

/// The axes whose response shaping the driver exposes. Steering lives on
/// its own page; the other four are the pedal blocks (the handbrake is the
/// RS accessory's analog axis, shaped exactly like a pedal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Steering,
    Throttle,
    Brake,
    Clutch,
    Handbrake,
}

impl Axis {
    /// Every axis, in the order the settings pages list their blocks.
    pub const ALL: [Axis; 5] =
        [Axis::Steering, Axis::Throttle, Axis::Brake, Axis::Clutch, Axis::Handbrake];

    fn index(self) -> usize {
        match self {
            Axis::Steering => 0,
            Axis::Throttle => 1,
            Axis::Brake => 2,
            Axis::Clutch => 3,
            Axis::Handbrake => 4,
        }
    }
}

/// Which axis `attr` shapes, for the shaping-related attributes (a
/// sensitivity, a curve, or a deadzone). Everything else (range, strength,
/// LEDs, ...) is `None`.
pub fn axis(attr: &str) -> Option<Axis> {
    match attr {
        "wheel_sensitivity" | "wheel_response_curve" => Some(Axis::Steering),
        "wheel_throttle_sensitivity" | "wheel_throttle_curve" | "wheel_throttle_deadzone" => {
            Some(Axis::Throttle)
        }
        "wheel_brake_sensitivity" | "wheel_brake_curve" | "wheel_brake_deadzone" => Some(Axis::Brake),
        "wheel_clutch_sensitivity" | "wheel_clutch_curve" | "wheel_clutch_deadzone" => Some(Axis::Clutch),
        "wheel_handbrake_sensitivity" | "wheel_handbrake_curve" => Some(Axis::Handbrake),
        _ => None,
    }
}

/// Classify `attr` for the per-axis shaping toggles.
pub fn role(attr: &str) -> ShapingRole {
    match attr {
        "wheel_sensitivity"
        | "wheel_throttle_sensitivity"
        | "wheel_brake_sensitivity"
        | "wheel_clutch_sensitivity"
        | "wheel_handbrake_sensitivity" => ShapingRole::Simple,
        "wheel_response_curve"
        | "wheel_throttle_curve"
        | "wheel_brake_curve"
        | "wheel_clutch_curve"
        | "wheel_handbrake_curve" => ShapingRole::Advanced,
        _ => ShapingRole::Neutral,
    }
}

/// The synthetic per-axis toggle rows' attrs. Not sysfs attributes: they
/// must never reach the device layer; front-ends intercept them in their
/// own edit path and flip local view state instead.
pub fn toggle_attr(axis: Axis) -> &'static str {
    match axis {
        Axis::Steering => "ui:shaping:steering",
        Axis::Throttle => "ui:shaping:throttle",
        Axis::Brake => "ui:shaping:brake",
        Axis::Clutch => "ui:shaping:clutch",
        Axis::Handbrake => "ui:shaping:handbrake",
    }
}

/// The inverse of [`toggle_attr`]: which axis a synthetic toggle row's attr
/// stands for. `None` for every real attribute.
pub fn toggle_axis(attr: &str) -> Option<Axis> {
    Axis::ALL.into_iter().find(|ax| toggle_attr(*ax) == attr)
}

/// The per-axis toggle rows' labels, shared by both front-ends.
pub fn toggle_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Steering => "Steering shaping",
        Axis::Throttle => "Throttle shaping",
        Axis::Brake => "Brake shaping",
        Axis::Clutch => "Clutch shaping",
        Axis::Handbrake => "Handbrake shaping",
    }
}

/// The toggle rows' help text, shared by both front-ends.
pub const TOGGLE_HELP: &str =
    "Sensitivity and the curve write the same device curve; pick which control to use for this axis.";

/// The per-axis view toggles: `true` means the axis shows its curve editor
/// instead of the simple sensitivity control. Pure UI-session state, held
/// by each front-end; the default (`false` everywhere) is the simple view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AxisToggles {
    curve: [bool; Axis::ALL.len()],
}

impl AxisToggles {
    /// Whether `axis` currently shows its curve editor.
    pub fn get(self, axis: Axis) -> bool {
        self.curve[axis.index()]
    }

    /// Set `axis`'s view to the curve editor (`true`) or sensitivity
    /// (`false`).
    pub fn set(&mut self, axis: Axis, curve: bool) {
        self.curve[axis.index()] = curve;
    }

    /// Flip `axis`'s view.
    pub fn toggle(&mut self, axis: Axis) {
        self.curve[axis.index()] = !self.curve[axis.index()];
    }
}

/// Whether `attr`'s row is visible with the given per-axis toggles:
/// `Simple` rows only while their axis's toggle is off, `Advanced` rows
/// only while it is on, `Neutral` rows always.
pub fn visible(attr: &str, toggles: AxisToggles) -> bool {
    let curve = axis(attr).is_some_and(|ax| toggles.get(ax));
    match role(attr) {
        ShapingRole::Simple => !curve,
        ShapingRole::Advanced => curve,
        ShapingRole::Neutral => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, REGISTRY};

    const SENSITIVITIES: [&str; 5] = [
        "wheel_sensitivity",
        "wheel_throttle_sensitivity",
        "wheel_brake_sensitivity",
        "wheel_clutch_sensitivity",
        "wheel_handbrake_sensitivity",
    ];

    const CURVES: [&str; 5] = [
        "wheel_response_curve",
        "wheel_throttle_curve",
        "wheel_brake_curve",
        "wheel_clutch_curve",
        "wheel_handbrake_curve",
    ];

    #[test]
    fn every_sensitivity_attr_is_simple() {
        for attr in SENSITIVITIES {
            assert_eq!(role(attr), ShapingRole::Simple, "{attr}");
        }
    }

    #[test]
    fn every_curve_attr_is_advanced() {
        for attr in CURVES {
            assert_eq!(role(attr), ShapingRole::Advanced, "{attr}");
        }
    }

    #[test]
    fn everything_else_is_neutral() {
        for attr in [
            "wheel_strength",
            "wheel_range",
            "wheel_throttle_deadzone",
            "wheel_brake_deadzone",
            "wheel_clutch_deadzone",
            "wheel_mode",
            toggle_attr(Axis::Throttle),
            "not_a_real_attr",
        ] {
            assert_eq!(role(attr), ShapingRole::Neutral, "{attr}");
        }
    }

    #[test]
    fn axis_pairs_each_generator_and_deadzone_with_its_axis() {
        assert_eq!(axis("wheel_sensitivity"), Some(Axis::Steering));
        assert_eq!(axis("wheel_response_curve"), Some(Axis::Steering));
        assert_eq!(axis("wheel_throttle_sensitivity"), Some(Axis::Throttle));
        assert_eq!(axis("wheel_throttle_curve"), Some(Axis::Throttle));
        assert_eq!(axis("wheel_throttle_deadzone"), Some(Axis::Throttle));
        assert_eq!(axis("wheel_brake_sensitivity"), Some(Axis::Brake));
        assert_eq!(axis("wheel_brake_curve"), Some(Axis::Brake));
        assert_eq!(axis("wheel_brake_deadzone"), Some(Axis::Brake));
        assert_eq!(axis("wheel_clutch_sensitivity"), Some(Axis::Clutch));
        assert_eq!(axis("wheel_clutch_curve"), Some(Axis::Clutch));
        assert_eq!(axis("wheel_clutch_deadzone"), Some(Axis::Clutch));
        assert_eq!(axis("wheel_handbrake_sensitivity"), Some(Axis::Handbrake));
        assert_eq!(axis("wheel_handbrake_curve"), Some(Axis::Handbrake));
    }

    #[test]
    fn axis_is_none_for_non_shaping_attrs() {
        for attr in ["wheel_range", "wheel_strength", "wheel_mode", toggle_attr(Axis::Brake), "nope"] {
            assert_eq!(axis(attr), None, "{attr}");
        }
    }

    #[test]
    fn every_generator_has_an_axis() {
        // A Simple/Advanced row without an axis could never be toggled
        // back into view; the classifier must cover every generator.
        for attr in SENSITIVITIES.iter().chain(CURVES.iter()) {
            assert!(axis(attr).is_some(), "{attr} has no axis");
        }
    }

    #[test]
    fn roles_agree_with_the_registry() {
        // Every Simple/Advanced attr must exist in the registry with the
        // matching kind, and every registry Curve row must be Advanced, so
        // the classification cannot silently drift from the registry.
        for attr in SENSITIVITIES {
            let spec = REGISTRY.iter().find(|s| s.attr == attr).unwrap_or_else(|| panic!("{attr} missing"));
            assert!(matches!(spec.kind, Kind::Percent), "{attr} is not a percent");
        }
        for attr in CURVES {
            let spec = REGISTRY.iter().find(|s| s.attr == attr).unwrap_or_else(|| panic!("{attr} missing"));
            assert!(matches!(spec.kind, Kind::Curve), "{attr} is not a curve");
        }
        for spec in REGISTRY.iter().filter(|s| matches!(s.kind, Kind::Curve)) {
            assert_eq!(role(spec.attr), ShapingRole::Advanced, "{} is a curve but not Advanced", spec.attr);
        }
    }

    #[test]
    fn toggle_attr_and_axis_round_trip() {
        for ax in Axis::ALL {
            assert_eq!(toggle_axis(toggle_attr(ax)), Some(ax));
        }
        assert_eq!(toggle_axis("wheel_sensitivity"), None);
        assert_eq!(toggle_axis("ui:shaping:nope"), None);
    }

    #[test]
    fn toggles_default_to_simple_and_flip_independently() {
        let mut t = AxisToggles::default();
        for ax in Axis::ALL {
            assert!(!t.get(ax), "{ax:?} defaults to simple");
        }
        t.set(Axis::Brake, true);
        assert!(t.get(Axis::Brake));
        assert!(!t.get(Axis::Throttle), "other axes untouched");
        t.toggle(Axis::Brake);
        assert!(!t.get(Axis::Brake));
        t.toggle(Axis::Steering);
        assert!(t.get(Axis::Steering));
    }

    #[test]
    fn visible_filters_each_axis_by_its_own_toggle() {
        let mut t = AxisToggles::default();
        t.set(Axis::Brake, true);
        // The brake shows its curve, not its sensitivity.
        assert!(!visible("wheel_brake_sensitivity", t));
        assert!(visible("wheel_brake_curve", t));
        // The throttle (still simple) shows the opposite pair.
        assert!(visible("wheel_throttle_sensitivity", t));
        assert!(!visible("wheel_throttle_curve", t));
        // Steering follows its own toggle too.
        assert!(visible("wheel_sensitivity", t));
        assert!(!visible("wheel_response_curve", t));
        t.set(Axis::Steering, true);
        assert!(!visible("wheel_sensitivity", t));
        assert!(visible("wheel_response_curve", t));
    }

    #[test]
    fn visible_keeps_neutral_rows_in_both_modes() {
        let simple = AxisToggles::default();
        let mut curves = AxisToggles::default();
        for ax in Axis::ALL {
            curves.set(ax, true);
        }
        for attr in ["wheel_throttle_deadzone", "wheel_brake_deadzone", "wheel_range", "wheel_strength"] {
            assert!(visible(attr, simple), "{attr} in simple");
            assert!(visible(attr, curves), "{attr} in advanced");
        }
    }
}
