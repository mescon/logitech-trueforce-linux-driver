//! Simple-vs-advanced classification for the response-shaping settings.
//!
//! The driver programs ONE response curve (0x80A4) per axis. A sensitivity
//! percentage (50 = linear) and the full curve editor are two generators
//! for that same device state: whichever writes last wins. Showing both at
//! once therefore misleads (editing a curve makes the sensitivity number
//! meaningless, and vice versa), so front-ends show only one of the two at
//! a time, chosen by a plain per-session "Advanced shaping" view toggle.
//! The toggle is pure view state: never persisted, never written to sysfs.
//! Deadzones feed the same composed curve but stay meaningful in both
//! modes, so they are `Neutral` and always shown.

/// How a settings row relates to the advanced-shaping view toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapingRole {
    /// A sensitivity percentage: shown in simple mode only.
    Simple,
    /// A full response-curve editor: shown in advanced mode only.
    Advanced,
    /// Unrelated to the simple/advanced choice (deadzones included):
    /// always shown.
    Neutral,
}

/// The synthetic toggle row's attr. Not a sysfs attribute: it must never
/// reach the device layer; front-ends intercept it in their own edit path
/// and flip local view state instead.
pub const TOGGLE_ATTR: &str = "ui:advanced_shaping";

/// The toggle row's label, shared by both front-ends.
pub const TOGGLE_LABEL: &str = "Advanced shaping";

/// The toggle row's help text, shared by both front-ends.
pub const TOGGLE_HELP: &str = "Sensitivity and the curve editor program the same device response curve; \
the last one written wins. Advanced shows the full curve editor instead of the simple sensitivity control.";

/// Classify `attr` for the advanced-shaping toggle.
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

/// Whether `attr`'s row is visible with the toggle in the given state:
/// `Simple` rows only while simple, `Advanced` rows only while advanced,
/// `Neutral` rows always.
pub fn visible(attr: &str, advanced: bool) -> bool {
    match role(attr) {
        ShapingRole::Simple => !advanced,
        ShapingRole::Advanced => advanced,
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
            TOGGLE_ATTR,
            "not_a_real_attr",
        ] {
            assert_eq!(role(attr), ShapingRole::Neutral, "{attr}");
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
    fn visible_hides_exactly_the_off_mode_generator() {
        // Simple mode: sensitivity shown, curve hidden.
        assert!(visible("wheel_sensitivity", false));
        assert!(!visible("wheel_response_curve", false));
        // Advanced mode: curve shown, sensitivity hidden.
        assert!(!visible("wheel_sensitivity", true));
        assert!(visible("wheel_response_curve", true));
        // Neutral rows (deadzones and everything else) always shown.
        assert!(visible("wheel_throttle_deadzone", false));
        assert!(visible("wheel_throttle_deadzone", true));
        assert!(visible("wheel_range", false));
        assert!(visible("wheel_range", true));
    }
}
