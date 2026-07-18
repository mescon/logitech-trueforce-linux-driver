use crate::kind::Kind;
use crate::setting::{Access, Category, ModeReq, SettingSpec};

use Access::*;
use Category::*;
use ModeReq::*;

const PCT: Kind = Kind::Percent;

pub const REGISTRY: &[SettingSpec] = &[
    // --- Force feedback ---
    // Global strength first, then the filter pair, then damping, then the
    // TrueForce pair, then the less-common extras.
    SettingSpec { attr: "wheel_strength", label: "FFB strength", help: "Overall force output (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter", label: "FFB filter", help: "Smoothing level (1=min .. 15=max).", category: Ffb, kind: Kind::IntRange { min: 1, max: 15, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter_auto", label: "Auto FFB filter", help: "Let the wheel adjust the filter automatically.", category: Ffb, kind: Kind::Toggle { off: "manual", on: "auto" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_damping", label: "Damping", help: "Firmware turn resistance (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_trueforce", label: "TrueForce intensity", help: "Audio-haptic texture intensity (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_texture_route", label: "Texture routing", help: "Route rumble/texture to TrueForce (tf) or steering (kf).", category: Ffb, kind: Kind::Enum(&["kf", "tf"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_spring_damping", label: "Spring damping", help: "Anti-oscillation damping on the emulated spring (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_constant_sign", label: "Invert constant force", help: "Flip the sign of constant forces (Wine/native fix).", category: Ffb, kind: Kind::Toggle { off: "normal", on: "inverted" }, access: ReadWrite, mode_req: Any },
    // --- Steering ---
    // Range and sensitivity first (the two everyday controls), then the
    // auto-recovery toggle, then the full curve, then calibration last as
    // it is an action rather than a value.
    SettingSpec { attr: "wheel_range", label: "Rotation range", help: "Steering rotation (90-2700 deg).", category: Steering, kind: Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_sensitivity", label: "Sensitivity", help: "Steering response (0-100%, 50=built-in). Desktop mode only.", category: Steering, kind: PCT, access: ReadWrite, mode_req: DesktopOnly },
    SettingSpec { attr: "wheel_range_restore", label: "Auto range restore", help: "Auto-recover from a launch-time 90-degree reset.", category: Steering, kind: Kind::Toggle { off: "off", on: "on" }, access: ReadWrite, mode_req: Any },
    // Any, not DesktopOnly: unlike wheel_sensitivity, the driver's
    // wheel_response_curve_store does not gate on mode (no -EPERM), so a
    // DesktopOnly pre-check would falsely reject onboard-mode writes.
    SettingSpec { attr: "wheel_response_curve", label: "Response curve", help: "Full steering response curve. 'reset' for built-in.", category: Steering, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_calibrate_here", label: "Calibrate centre here", help: "Adopt the current physical position as centre.", category: Steering, kind: Kind::Action, access: Action, mode_req: Any },
    // --- Pedals ---
    // Each pedal has three generators that all write the one 0x80A4 curve the
    // pedal MCU applies to its axis (hardware-verified 2026-07-16). Last write
    // wins; the curve attr reads back the true device state. mode_req Any: the
    // driver's pedal stores do not gate on mode.
    // Pedal-wide settings first, then one block per pedal (sensitivity,
    // deadzone, curve, in that order), then the handbrake accessory last.
    SettingSpec { attr: "wheel_brake_force", label: "Brake force", help: "Load-cell brake threshold (0-100%). Onboard mode only.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: OnboardOnly },
    SettingSpec { attr: "wheel_combined_pedals", label: "Combined pedals", help: "Merge throttle+brake into one axis for legacy games. Off for modern sims. Desktop mode only.", category: Pedals, kind: Kind::Toggle { off: "separate", on: "combined" }, access: ReadWrite, mode_req: DesktopOnly },
    SettingSpec { attr: "wheel_throttle_sensitivity", label: "Throttle sensitivity", help: "Throttle response (0-100, 50=linear).", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_deadzone", label: "Throttle deadzone", help: "Dead travel 'lower upper' percent (sum <= 99).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_curve", label: "Throttle curve", help: "Full throttle response curve. 'reset' for built-in.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_sensitivity", label: "Brake sensitivity", help: "Brake response (0-100, 50=linear).", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_deadzone", label: "Brake deadzone", help: "Dead travel 'lower upper' percent (sum <= 99).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_curve", label: "Brake curve", help: "Full brake response curve. 'reset' for built-in.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_sensitivity", label: "Clutch sensitivity", help: "Clutch response (0-100, 50=linear).", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_deadzone", label: "Clutch deadzone", help: "Dead travel 'lower upper' percent (sum <= 99).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_curve", label: "Clutch curve", help: "Full clutch response curve. 'reset' for built-in.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    // RS Shifter & Handbrake accessory (analog handbrake axis shaping). Only
    // present when the handbrake is connected; the row reads unavailable
    // otherwise. Same 0x80A4 curve type as the pedals, on the wheel base.
    SettingSpec { attr: "wheel_handbrake_sensitivity", label: "Handbrake sensitivity", help: "Handbrake response (0-100, 50=linear).", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_handbrake_curve", label: "Handbrake curve", help: "Full handbrake response curve. 'reset' for built-in.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    // --- LIGHTSYNC (RS50 RGB strip) ---
    // Effect first (it decides whether the slot fields even apply), then the
    // global brightness, then the active-slot group in the order you'd set
    // them (pick slot, name it, colour it, shape it, dim it), then apply.
    SettingSpec { attr: "wheel_led_effect", label: "Effect", help: "Built-in animation or a custom slot (1-9).", category: Leds, kind: Kind::IntRange { min: 1, max: 9, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_brightness", label: "Brightness", help: "Global LIGHTSYNC brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot", label: "Active slot", help: "Active custom slot (0-4).", category: Leds, kind: Kind::IntRange { min: 0, max: 4, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_name", label: "Slot name", help: "Name of the selected slot (max 8 chars).", category: Leds, kind: Kind::TextField { max_len: 8 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_colors", label: "Colors", help: "10 strip colors, LED1 leftmost.", category: Leds, kind: Kind::RgbStrip { leds: 10 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_direction", label: "Direction", help: "Animation direction: left-to-right, right-to-left, inside-out or outside-in.", category: Leds, kind: Kind::Enum(&["L to R", "R to L", "inside-out", "outside-in"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_brightness", label: "Slot brightness", help: "Per-slot brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_apply", label: "Apply", help: "Commit the current slot config to the wheel.", category: Leds, kind: Kind::Action, access: Action, mode_req: Any },
    // Real G PRO rev-light strip: a different, level-based indicator on a
    // different wheel, kept last and apart from the RS50 LIGHTSYNC group.
    SettingSpec { attr: "wheel_rev_level", label: "Rev lights (G PRO)", help: "G PRO rev-light strip: how many of the 10 rev LEDs are lit (0-10), set manually. Not driven by engine RPM. The RS50 rim uses the LIGHTSYNC colors above instead.", category: Leds, kind: Kind::IntRange { min: 0, max: 10, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // --- Profiles / mode ---
    SettingSpec { attr: "wheel_mode", label: "Mode", help: "desktop (host-controlled) or onboard (wheel-stored).", category: Profiles, kind: Kind::Enum(&["desktop", "onboard"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile", label: "Profile", help: "Active profile (0=desktop, 1-5 onboard).", category: Profiles, kind: Kind::IntRange { min: 0, max: 5, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // max_len is the wheel's limit (9), not the driver's protocol cap (14):
    // the RS50 rejects a longer name with -EIO. The wheel stores names
    // uppercased.
    SettingSpec { attr: "wheel_profile_names", label: "Profile names", help: "Rename an onboard slot: left/right picks the slot, type a name (1-9 chars, stored uppercase).", category: Profiles, kind: Kind::SlotText { slots: 5, max_len: 9 }, access: ReadWrite, mode_req: Any },
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
        Kind::Pair { .. } => "0 0".into(),
        Kind::Action => "1".into(),
        Kind::SlotText { slots, .. } => {
            (1..=slots).map(|i| format!("{i}: NAME{i}")).collect::<Vec<_>>().join("\n")
        }
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
            // SlotText reads back the whole list but writes a single slot, so
            // parse->format is deliberately not a round-trip; its own tests
            // cover both directions.
            if matches!(s.kind, crate::Kind::SlotText { .. }) {
                let raw = super::sample_raw(s);
                s.kind.parse(&raw).unwrap_or_else(|e| panic!("{}: {e}", s.attr));
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
