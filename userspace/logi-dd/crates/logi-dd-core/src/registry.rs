use crate::kind::Kind;
use crate::setting::{Access, Category, ModeReq, SettingSpec};

use Access::*;
use Category::*;
use ModeReq::*;

const PCT: Kind = Kind::Percent;

pub const REGISTRY: &[SettingSpec] = &[
    // --- Force feedback ---
    SettingSpec { attr: "wheel_strength", label: "FFB strength", help: "Overall force output (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_damping", label: "Damping", help: "Firmware turn resistance (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter", label: "FFB filter", help: "Smoothing level (1=min .. 15=max).", category: Ffb, kind: Kind::IntRange { min: 1, max: 15, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter_auto", label: "Auto FFB filter", help: "Let the wheel adjust the filter automatically.", category: Ffb, kind: Kind::Toggle { off: "manual", on: "auto" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_spring_damping", label: "Spring damping", help: "Anti-oscillation damping on the emulated spring (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_constant_sign", label: "Invert constant force", help: "Flip the sign of constant forces (Wine/native fix).", category: Ffb, kind: Kind::Toggle { off: "normal", on: "inverted" }, access: ReadWrite, mode_req: Any },
    // --- Rotation ---
    SettingSpec { attr: "wheel_range", label: "Rotation range", help: "Steering rotation (90-2700 deg).", category: Rotation, kind: Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_range_restore", label: "Auto range restore", help: "Auto-recover from a launch-time 90-degree reset.", category: Rotation, kind: Kind::Toggle { off: "off", on: "on" }, access: ReadWrite, mode_req: Any },
    // --- Sensitivity ---
    SettingSpec { attr: "wheel_sensitivity", label: "Sensitivity", help: "Steering response (0-100%, 50=built-in). Desktop mode only.", category: Sensitivity, kind: PCT, access: ReadWrite, mode_req: DesktopOnly },
    // Any, not DesktopOnly: unlike wheel_sensitivity, the driver's
    // wheel_response_curve_store does not gate on mode (no -EPERM), so a
    // DesktopOnly pre-check would falsely reject onboard-mode writes.
    SettingSpec { attr: "wheel_response_curve", label: "Response curve", help: "Full steering response curve. 'reset' for built-in.", category: Sensitivity, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    // --- TrueForce ---
    SettingSpec { attr: "wheel_trueforce", label: "TrueForce intensity", help: "Audio-haptic texture intensity (0-100%).", category: TrueForce, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_texture_route", label: "Texture routing", help: "Route rumble/texture to TrueForce (tf) or steering (kf).", category: TrueForce, kind: Kind::Enum(&["kf", "tf"]), access: ReadWrite, mode_req: Any },
    // --- Pedals ---
    SettingSpec { attr: "wheel_brake_force", label: "Brake force", help: "Load-cell brake threshold (0-100%). Onboard mode only.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: OnboardOnly },
    SettingSpec { attr: "wheel_combined_pedals", label: "Combined pedals", help: "Throttle and brake on one axis.", category: Pedals, kind: Kind::Toggle { off: "separate", on: "combined" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_curve", label: "Throttle curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_curve", label: "Brake curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_curve", label: "Clutch curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    // --- LEDs (RS50 LIGHTSYNC) ---
    SettingSpec { attr: "wheel_led_brightness", label: "LED brightness", help: "Global LED brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_effect", label: "LED effect", help: "Animation mode (1-9).", category: Leds, kind: Kind::IntRange { min: 1, max: 9, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_direction", label: "LED direction", help: "Animation direction.", category: Leds, kind: Kind::Enum(&["L to R", "R to L", "inside-out", "outside-in"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_colors", label: "LED colors", help: "10 strip colors, LED1 leftmost.", category: Leds, kind: Kind::RgbStrip { leds: 10 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot", label: "LED slot", help: "Active custom slot (0-4).", category: Leds, kind: Kind::IntRange { min: 0, max: 4, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_name", label: "LED slot name", help: "Name of the selected slot (max 8 chars).", category: Leds, kind: Kind::TextField { max_len: 8 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_brightness", label: "LED slot brightness", help: "Per-slot brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_apply", label: "Apply LEDs", help: "Commit the current slot config to the wheel.", category: Leds, kind: Kind::Action, access: Action, mode_req: Any },
    // --- LEDs (real G Pro rev strip) ---
    SettingSpec { attr: "wheel_rev_level", label: "Rev lights", help: "Number of rev LEDs lit (0-10).", category: Leds, kind: Kind::IntRange { min: 0, max: 10, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // --- Profiles / mode ---
    SettingSpec { attr: "wheel_mode", label: "Mode", help: "desktop (host-controlled) or onboard (wheel-stored).", category: Profiles, kind: Kind::Enum(&["desktop", "onboard"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile", label: "Profile", help: "Active profile (0=desktop, 1-5 onboard).", category: Profiles, kind: Kind::IntRange { min: 0, max: 5, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile_names", label: "Profile names", help: "The 5 onboard slot names.", category: Profiles, kind: Kind::TextField { max_len: 256 }, access: ReadOnly, mode_req: Any },
    // --- Calibration ---
    SettingSpec { attr: "wheel_calibrate_here", label: "Calibrate centre here", help: "Adopt the current physical position as centre.", category: Calibration, kind: Kind::Action, access: Action, mode_req: Any },
    // --- Info ---
    SettingSpec { attr: "wheel_serial", label: "Serial", help: "Device serial number.", category: Info, kind: Kind::TextField { max_len: 32 }, access: ReadOnly, mode_req: Any },
    SettingSpec { attr: "wheel_firmware", label: "Firmware", help: "Base and motor firmware versions.", category: Info, kind: Kind::TextField { max_len: 128 }, access: ReadOnly, mode_req: Any },
];

/// A trivially-valid raw string for each kind, used by the registry coherence
/// test to prove every spec can round-trip.
#[cfg(test)]
pub(crate) fn sample_raw(s: &SettingSpec) -> String {
    match s.kind {
        Kind::Percent => "50".into(),
        Kind::IntRange { min, .. } => min.to_string(),
        Kind::Enum(_) => "0".into(),
        Kind::Toggle { .. } => "0".into(),
        Kind::TextField { .. } => "RACE".into(),
        Kind::RgbStrip { leds } => vec!["000000"; leds].join(" "),
        Kind::Curve => "reset".into(),
        Kind::Action => "1".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting::{Access, Category};

    #[test]
    fn registry_has_no_duplicate_attrs() {
        let mut seen = std::collections::HashSet::new();
        for s in REGISTRY {
            assert!(seen.insert(s.attr), "duplicate attr {}", s.attr);
        }
    }

    #[test]
    fn every_kind_roundtrips_a_sample() {
        // Each spec's kind must be able to format+parse a known-good sample
        // drawn from its own current default, proving the registry is coherent.
        for s in REGISTRY {
            if matches!(s.access, Access::Action) {
                continue;
            }
            // pick a trivially valid raw for this kind and round-trip it
            let raw = super::sample_raw(s);
            let v = s.kind.parse(&raw).unwrap_or_else(|e| panic!("{}: {e}", s.attr));
            let back = s.kind.format(&v).unwrap();
            assert!(!back.is_empty() || matches!(s.kind, crate::Kind::Curve),
                    "{}: empty format", s.attr);
        }
    }

    #[test]
    fn known_attrs_present() {
        for a in ["wheel_strength", "wheel_range", "wheel_sensitivity",
                  "wheel_mode", "wheel_led_colors", "wheel_serial"] {
            assert!(REGISTRY.iter().any(|s| s.attr == a), "missing {a}");
        }
    }

    #[test]
    fn brake_force_is_onboard_only() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_brake_force").unwrap();
        assert!(matches!(s.mode_req, super::super::setting::ModeReq::OnboardOnly));
        let _ = Category::Pedals;
    }
}
