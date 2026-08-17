use crate::kind::Kind;
use crate::setting::{Access, Category, ModeReq, SettingSpec};

use Access::*;
use Category::*;
use ModeReq::*;

const PCT: Kind = Kind::Percent;

pub const REGISTRY: &[SettingSpec] = &[
    // --- Force feedback ---
    // Global strength first, then the filter pair, then the two damping
    // controls together, then the TrueForce pair, then the native texture-
    // merge tuning group (intensity, cylinders, its rpm-feed diagnostic,
    // and the merge state itself, last since it is not user-editable), then
    // the sign fix last.
    SettingSpec { attr: "wheel_strength", label: "FFB strength", help: "Overall strength of every force. Turn it down if the wheel feels too heavy or the strongest effects clip and flatten out (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter", label: "FFB filter", help: "Smooths the force signal so notchy or noisy feedback feels cleaner. Higher is smoother but blurs fine detail; lower keeps the road sharp (1-15).", category: Ffb, kind: Kind::IntRange { min: 1, max: 15, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter_auto", label: "Auto FFB filter", help: "Lets the wheel pick the smoothing amount for you instead of holding the fixed level you set. Turn off if you want full manual control.", category: Ffb, kind: Kind::Toggle { off: "manual", on: "auto" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_damping", label: "Damping", help: "Adds steady resistance so the wheel turns less freely, like a heavier steering rack. Raise it to calm a twitchy or darty wheel (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_spring_damping", label: "Spring damping", help: "Tames the self-centring spring so it settles instead of oscillating or shaking, which matters most on a strong direct-drive wheel (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_trueforce", label: "TrueForce intensity", help: "How strong the fine engine and road-surface vibration feels on top of the main forces. Raise for more texture, lower to quiet it (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_texture_route", label: "Texture routing", help: "Whether rumble and surface texture play through the TrueForce vibration path (tf) or are pushed into the steering force itself (kf).", category: Ffb, kind: Kind::Enum(&["kf", "tf"]), access: ReadWrite, mode_req: Any },
    // Native texture-merge tuning: the interceptor on interface 2 that
    // splices synthesized engine texture into an SDK game's TrueForce
    // stream (see `docs/SYSFS_API.md`'s `wheel_tf_merge` section). Intensity
    // and cylinders are the two knobs a driver worth tuning; the rpm feed is
    // a diagnostic, and the merge state is informational only, so it closes
    // the group rather than opening it.
    SettingSpec { attr: "wheel_texture_intensity", label: "Texture intensity", help: "How strong the synthesized engine texture feels, as a percent of the amplitude fitted to Logitech's own capture. 100 = matched to G HUB; lower to quiet it, raise past 100 to exaggerate it (0-200%).", category: Ffb, kind: Kind::IntRange { min: 0, max: 200, step: 1, unit: "%" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_texture_cylinders", label: "Engine cylinders (texture pitch)", help: "Cylinder count for the firing-frequency model the texture is built from (f0 = rpm/60 * cylinders/2). A feel knob more than an engine spec: 4 is the default (seat-tested; higher counts push most of the rev range above what a direct-drive rim can express), raise it for a finer, busier buzz (1-16).", category: Ffb, kind: Kind::IntRange { min: 1, max: 16, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // Read-only in this app even though the sysfs attr itself is RW:
    // logi-rpm-bridge is the normal writer, and this row exists so the
    // status line can double as the feed's own diagnostic, not so the app
    // can drive it by hand.
    SettingSpec { attr: "wheel_texture_rpm", label: "Texture RPM feed", help: "Live engine RPM the texture is synthesized from, normally fed by logi-rpm-bridge at around 60 Hz (logi-launch starts it per game). Shows the last rpm while it is fresh, or that no telemetry is arriving.", category: Ffb, kind: Kind::RpmFeed, access: ReadOnly, mode_req: Any },
    // Not a toggle: logi-launch switches this per game, so exposing it as
    // an editable control here would race whatever it just set. Shown as
    // plain state text instead, the same read-only idiom `wheel_accessory_mode`
    // uses elsewhere on this page.
    SettingSpec { attr: "wheel_tf_merge", label: "Texture merge", help: "Whether the native engine texture is currently being spliced into this game's TrueForce stream. Turned on and off automatically per game by logi-launch; not editable here.", category: Ffb, kind: Kind::Toggle { off: "off", on: "on" }, access: ReadOnly, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_constant_sign", label: "Invert constant force", help: "Flips the direction of steady forces if the wheel pulls the wrong way in a game. Try this when the force feedback feels backwards.", category: Ffb, kind: Kind::Toggle { off: "normal", on: "inverted" }, access: ReadWrite, mode_req: Any },
    // --- Steering ---
    // Range and its auto-recovery toggle first, then the shaping pair
    // (sensitivity before the curve: the per-axis shaping toggle row is
    // injected right before the axis's first row, so this pair forms the
    // block the toggle heads), then calibration (an action, not a value),
    // then the G PRO rev-light strip: it sits on the steering rim, so it
    // lives here rather than with the RS50 LIGHTSYNC strip.
    SettingSpec { attr: "wheel_range", label: "Rotation range", help: "How far the wheel turns lock to lock. Match it to the car: rally around 540, F1 around 360, drift wider (90-2700 degrees).", category: Steering, kind: Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_range_restore", label: "Auto range restore", help: "Automatically puts your rotation range back if a game or launch resets it to 90 degrees. Leave on unless it fights a game you trust.", category: Steering, kind: Kind::Toggle { off: "off", on: "on" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_sensitivity", label: "Sensitivity", help: "Reshapes how far you turn the wheel for a given in-game steering angle. Below 50 is calmer near centre, above 50 quicker; 50 is the built-in feel. Works in desktop mode only.", category: Steering, kind: PCT, access: ReadWrite, mode_req: DesktopOnly },
    // Any, not DesktopOnly: unlike wheel_sensitivity, the driver's
    // wheel_response_curve_store does not gate on mode (no -EPERM), so a
    // DesktopOnly pre-check would falsely reject onboard-mode writes.
    SettingSpec { attr: "wheel_response_curve", label: "Response curve", help: "Shapes the whole steering response by hand, so you can soften or sharpen the feel at any point of the turn. Use 'reset' to go back to the built-in feel.", category: Steering, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_calibrate_here", label: "Calibrate centre here", help: "Sets the wheel's current physical position as dead centre. Hold the wheel straight, then run this if the centre point has drifted.", category: Steering, kind: Kind::Action, access: Action, mode_req: Any },
    // Read-only in this app for the reason `wheel_texture_rpm` above is: the
    // strip is a live feed, driven by logi-rpm-bridge during a texture-merge
    // session and by logi-tf-sim's rev feeder otherwise, both at up to 60 Hz.
    // An editable control here would be a third writer racing them, and the
    // one that lost would look like a broken slider. The wheel itself takes
    // the write (`Device::write_test_pattern` is how the LED test borrows the
    // strip); this flag is about who owns it, not what the hardware accepts.
    SettingSpec { attr: "wheel_rev_level", label: "Rev lights", help: "How many of the 10 rev LEDs are lit right now (0-10), in the active colour set. Driven live by logi-rpm-bridge during a game, or by logi-tf-sim from telemetry. Two mappings: by default the bar fills across the whole rev range, and LOGI_REV_MODE=shift keeps it dark until the car's first shift light, like the dashboard.", category: Steering, kind: Kind::IntRange { min: 0, max: 10, step: 1, unit: "" }, access: ReadOnly, mode_req: Any },
    // --- Pedals ---
    // Each pedal has three generators that all write the one 0x80A4 curve the
    // pedal MCU applies to its axis (hardware-verified 2026-07-16). Last write
    // wins; the curve attr reads back the true device state. mode_req Any: the
    // driver's pedal stores do not gate on mode.
    // Pedal-wide settings first (combined toggle, then the load-cell
    // threshold), then one block per pedal in sensitivity, curve, deadzone
    // order: the shaping toggle row is injected before the sensitivity, and
    // showing sensitivity OR curve keeps the deadzone right after whichever
    // generator is visible. The handbrake accessory comes last.
    SettingSpec { attr: "wheel_combined_pedals", label: "Combined pedals", help: "Merges throttle and brake onto one axis for older games that expect a single pedal input. Leave off for modern sims. Works in desktop mode only.", category: Pedals, kind: Kind::Toggle { off: "separate", on: "combined" }, access: ReadWrite, mode_req: DesktopOnly },
    SettingSpec { attr: "wheel_brake_force", label: "Brake force", help: "How hard you must press the load-cell brake for full braking (0-100%). Stored on the wheel in onboard mode only; in desktop mode use Brake sensitivity or the brake curve.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: OnboardOnly },
    SettingSpec { attr: "wheel_throttle_sensitivity", label: "Throttle sensitivity", help: "Reshapes how pedal travel maps to throttle. Below 50 eases in for finer control off idle, above 50 responds faster; 50 is straight linear.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_curve", label: "Throttle curve", help: "Shapes the whole throttle response by hand for precise control over how power comes in through the pedal travel. Use 'reset' for the straight built-in response.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_deadzone", label: "Throttle deadzone", help: "Ignores a slice of travel at the top and bottom of the throttle so light presses and the pinned position register cleanly. Enter lower and upper percent (they must sum to 99 or less).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_sensitivity", label: "Brake sensitivity", help: "Reshapes how pedal travel maps to braking. Below 50 eases in for finer trail-braking, above 50 bites harder early; 50 is straight linear.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_curve", label: "Brake curve", help: "Shapes the whole brake response by hand so you can fine-tune bite point and modulation across the pedal travel. Use 'reset' for the straight built-in response.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_deadzone", label: "Brake deadzone", help: "Ignores a slice of travel at the top and bottom of the brake so a resting foot and full braking register cleanly. Enter lower and upper percent (they must sum to 99 or less).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_sensitivity", label: "Clutch sensitivity", help: "Reshapes how pedal travel maps to the clutch. Below 50 eases in for smoother bite-point control, above 50 engages faster; 50 is straight linear.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_curve", label: "Clutch curve", help: "Shapes the whole clutch response by hand so you can dial in the bite point for clean launches. Use 'reset' for the straight built-in response.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_deadzone", label: "Clutch deadzone", help: "Ignores a slice of travel at the top and bottom of the clutch so a resting foot and full engagement register cleanly. Enter lower and upper percent (they must sum to 99 or less).", category: Pedals, kind: Kind::Pair { max: 99 }, access: ReadWrite, mode_req: Any },
    // RS Shifter & Handbrake accessory (analog handbrake axis shaping). Same
    // 0x80A4 curve type as the pedals, but applied on the wheel base, not on
    // the accessory: with nothing attached these still read and still write
    // successfully (verified on an RS50, 2026-07-28), so nothing the wheel
    // says marks them inapplicable. `Device::requires_accessory` gates them on
    // `wheel_accessory` instead, which is what makes the rows read unavailable
    // when no handbrake is connected.
    SettingSpec { attr: "wheel_accessory_mode", label: "Accessory mode", help: "Which of its three jobs the RS Shifter & Handbrake is doing right now, set by the physical switch on its base: sequential shifter, digital handbrake, or analog handbrake. Settings that belong to a different mode are shown greyed out.", category: Pedals, kind: Kind::TextField { max_len: 24 }, access: ReadOnly, mode_req: Any },
    SettingSpec { attr: "wheel_handbrake_sensitivity", label: "Handbrake sensitivity", help: "Reshapes how the analog handbrake's pull maps in game. Below 50 eases in for gentler slides, above 50 grabs sooner; 50 is straight linear. Needs the handbrake accessory connected.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_handbrake_curve", label: "Handbrake curve", help: "Shapes the whole handbrake response by hand for precise slide control across its travel. Use 'reset' for the straight built-in response. Needs the handbrake accessory connected.", category: Pedals, kind: Kind::Curve, access: ReadWrite, mode_req: Any },
    // The accessory's own actuation points (0x80B1), after the handbrake
    // shaping pair so that pair stays adjacent for the shaping toggle.
    // These are trigger POINTS, not response shaping, so they are not
    // part of any axis's sensitivity/curve block.
    SettingSpec { attr: "wheel_shift_actuation", label: "Shift actuation", help: "How far the sequential shifter must be pushed before a shift registers. Lower triggers further from centre, higher triggers sooner. Needs the shifter accessory connected, in shifter mode.", category: Pedals, kind: Kind::IntRange { min: 1, max: 100, step: 1, unit: "%" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_handbrake_actuation", label: "Handbrake actuation", help: "How far the handbrake must be pulled before the digital handbrake button fires. Only applies in digital-handbrake mode; the analog mode uses the handbrake curve instead. Needs the handbrake accessory connected.", category: Pedals, kind: Kind::IntRange { min: 1, max: 100, step: 1, unit: "%" }, access: ReadWrite, mode_req: Any },
    // --- LIGHTSYNC (RS50 RGB strip) ---
    // Effect first (it decides whether the slot fields even apply), then the
    // global brightness, then the active-slot group in the order you'd set
    // them (pick slot, name it, colour it, shape it, dim it), then apply.
    SettingSpec { attr: "wheel_led_effect", label: "Effect", help: "Picks what the light strip shows: 1-4 are built-in sweeps, 5-9 are your five saved custom looks. The strip acts as a rev display, filling with engine RPM when a game or telemetry bridge feeds it.", category: Leds, kind: Kind::IntRange { min: 1, max: 9, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_brightness", label: "Brightness", help: "Overall brightness of the whole light strip. Turn it down if the lights are distracting or too bright at night (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot", label: "Active slot", help: "Chooses which of the five custom light presets you are editing and showing (0-4). Switch slots to build up different looks you can recall later.", category: Leds, kind: Kind::IntRange { min: 0, max: 4, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_name", label: "Slot name", help: "A short label for the current custom preset so you can tell your saved looks apart (up to 8 characters).", category: Leds, kind: Kind::TextField { max_len: 8 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_colors", label: "Colors", help: "Sets the colour of each of the 10 lights, left to right. This doubles as the rev gradient once RPM is fed, for example green at the edges rising to red in the centre.", category: Leds, kind: Kind::RgbStrip { leds: 10 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_direction", label: "Direction", help: "Which way the lights animate: left to right, right to left, from the centre outward, or from the edges inward. Pick whichever fill you prefer to watch.", category: Leds, kind: Kind::Enum(&["L to R", "R to L", "inside-out", "outside-in"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_brightness", label: "Slot brightness", help: "Brightness for just this custom preset, letting one saved look sit dimmer or brighter than the others (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_apply", label: "Apply", help: "Saves the colours, name and settings of the current preset onto the wheel so it keeps them. Run this after editing a slot.", category: Leds, kind: Kind::Action, access: Action, mode_req: Any },
    // --- Profiles / mode ---
    SettingSpec { attr: "wheel_mode", label: "Mode", help: "Desktop mode lets this app drive the wheel live; onboard mode makes the wheel run its own saved settings so it works the same on any computer.", category: Profiles, kind: Kind::Enum(&["desktop", "onboard"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile", label: "Profile", help: "Which of the five settings presets stored on the wheel is active (1-5). The wheel follows the chosen one while in onboard mode.", category: Profiles, kind: Kind::IntRange { min: 0, max: 5, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // max_len is the wheel's limit (9), not the driver's protocol cap (14):
    // the RS50 rejects a longer name with -EIO. The wheel stores names
    // uppercased.
    SettingSpec { attr: "wheel_profile_names", label: "Profile names", help: "Gives each of the wheel's saved presets a name so you can tell them apart. Pick a slot with left/right and type a label (1-9 characters, stored in capitals).", category: Profiles, kind: Kind::SlotText { slots: 5, max_len: 9 }, access: ReadWrite, mode_req: Any },
    // --- Info ---
    SettingSpec { attr: "wheel_serial", label: "Serial", help: "The wheel's unique serial number, handy for warranty or support. Read-only.", category: Info, kind: Kind::TextField { max_len: 32 }, access: ReadOnly, mode_req: Any },
    SettingSpec { attr: "wheel_firmware", label: "Firmware", help: "The firmware versions running on the wheel base and motor, useful when checking for updates. Read-only.", category: Info, kind: Kind::TextField { max_len: 128 }, access: ReadOnly, mode_req: Any },
];

/// The G923's classic lg4ff-style settings (`range`, `gain`, `autocenter`,
/// `combine_pedals`, no `wheel_` prefix): a different FFB engine from the
/// direct-drive wheels above, with its own attribute names and scales, so
/// it gets its own registry rather than folding into [`REGISTRY`]. Selected
/// by `Device::settings()` once discovery identifies the connected wheel as
/// [`crate::device::WheelModel::G923`]; `Device::spec` still resolves any
/// attr from either registry, since the two attribute namespaces never
/// collide.
///
/// No LEDs/Profiles/Info entries: the classic engine has no onboard
/// profile slots or LIGHTSYNC strip, and exposes no serial/firmware sysfs
/// attrs (see `mainline/dd-lg4ff.c`'s sysfs doc comment). Those category
/// pages simply render empty for this wheel.
pub const CLASSIC_REGISTRY: &[SettingSpec] = &[
    // 40-900 degrees, ported from new-lg4ff's G923 row (dd-lg4ff.c: device
    // table, min/max 40/900); the store clamps rather than rejecting, but
    // this app validates up front like every other IntRange.
    SettingSpec { attr: "range", label: "Rotation range", help: "How far the wheel turns lock to lock (40-900 degrees).", category: Steering, kind: Kind::IntRange { min: 40, max: 900, step: 10, unit: "deg" }, access: ReadWrite, mode_req: Any },
    // Shown and edited as a percent (0-100%); the sysfs attr itself stays
    // the driver's raw 0-65535 range for Oversteer compatibility. See
    // `Kind::ScaledPercent`.
    SettingSpec { attr: "gain", label: "Overall gain", help: "Global force feedback strength, independent of any game's own gain setting (0-100%).", category: Ffb, kind: Kind::ScaledPercent { raw_max: 65535 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "autocenter", label: "Autocenter", help: "Strength of the wheel's self-centring spring (0-100%, 0 turns it off).", category: Ffb, kind: Kind::ScaledPercent { raw_max: 65535 }, access: ReadWrite, mode_req: Any },
    // Semantics per new-lg4ff's README (combine_pedals section): 1 merges
    // the accelerator with the brake, 2 with the clutch, onto the
    // accelerator's own axis; 0 leaves every pedal on its own axis.
    SettingSpec { attr: "combine_pedals", label: "Combine pedals", help: "Merges the accelerator with another pedal onto one axis, for older games that expect a single combined-pedal input. Leave separate for modern sims.", category: Pedals, kind: Kind::Enum(&["separate", "gas + brake", "gas + clutch"]), access: ReadWrite, mode_req: Any },
];

/// A trivially-valid raw string for each kind, used by the registry coherence
/// test to prove every spec can round-trip.
#[cfg(test)]
pub(crate) fn sample_raw(s: &SettingSpec) -> String {
    match s.kind {
        Kind::Percent => "50".into(),
        Kind::ScaledPercent { .. } => "0".into(),
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
        Kind::RpmFeed => "6500 14000 12".into(),
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

    /// The attrs of one category, in registry order.
    fn category_attrs(cat: Category) -> Vec<&'static str> {
        REGISTRY.iter().filter(|s| s.category == cat).map(|s| s.attr).collect()
    }

    #[test]
    fn category_orders_read_top_down() {
        // The front-ends render each category in registry order (plus the
        // injected per-axis shaping toggle rows), so the order here IS the
        // on-screen order; keep it deliberate.
        assert_eq!(
            category_attrs(Category::Ffb),
            vec![
                "wheel_strength",
                "wheel_ffb_filter",
                "wheel_ffb_filter_auto",
                "wheel_damping",
                "wheel_spring_damping",
                "wheel_trueforce",
                "wheel_texture_route",
                "wheel_texture_intensity",
                "wheel_texture_cylinders",
                "wheel_texture_rpm",
                "wheel_tf_merge",
                "wheel_ffb_constant_sign",
            ]
        );
        assert_eq!(
            category_attrs(Category::Steering),
            vec![
                "wheel_range",
                "wheel_range_restore",
                "wheel_sensitivity",
                "wheel_response_curve",
                "wheel_calibrate_here",
                "wheel_rev_level",
            ]
        );
        assert_eq!(
            category_attrs(Category::Pedals),
            vec![
                "wheel_combined_pedals",
                "wheel_brake_force",
                "wheel_throttle_sensitivity",
                "wheel_throttle_curve",
                "wheel_throttle_deadzone",
                "wheel_brake_sensitivity",
                "wheel_brake_curve",
                "wheel_brake_deadzone",
                "wheel_clutch_sensitivity",
                "wheel_clutch_curve",
                "wheel_clutch_deadzone",
                "wheel_accessory_mode",
                "wheel_handbrake_sensitivity",
                "wheel_handbrake_curve",
                "wheel_shift_actuation",
                "wheel_handbrake_actuation",
            ]
        );
        assert_eq!(
            category_attrs(Category::Profiles),
            vec!["wheel_mode", "wheel_profile", "wheel_profile_names"]
        );
        assert_eq!(category_attrs(Category::Info), vec!["wheel_serial", "wheel_firmware"]);
    }

    #[test]
    fn brake_force_is_onboard_only() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_brake_force").unwrap();
        assert!(matches!(s.mode_req, super::super::setting::ModeReq::OnboardOnly));
        let _ = Category::Pedals;
    }

    #[test]
    fn texture_intensity_clamps_to_0_200() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_texture_intensity").unwrap();
        assert!(matches!(s.kind, Kind::IntRange { min: 0, max: 200, .. }));
        assert!(s.kind.parse("-1").is_err());
        assert!(s.kind.parse("201").is_err());
        assert_eq!(s.kind.parse("0").unwrap(), crate::Value::Int(0));
        assert_eq!(s.kind.parse("100").unwrap(), crate::Value::Int(100));
        assert_eq!(s.kind.parse("200").unwrap(), crate::Value::Int(200));
        assert_eq!(s.access, Access::ReadWrite);
    }

    #[test]
    fn texture_cylinders_clamps_to_1_16() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_texture_cylinders").unwrap();
        assert!(matches!(s.kind, Kind::IntRange { min: 1, max: 16, .. }));
        assert!(s.kind.parse("0").is_err());
        assert!(s.kind.parse("17").is_err());
        assert_eq!(s.kind.parse("1").unwrap(), crate::Value::Int(1));
        assert_eq!(s.kind.parse("8").unwrap(), crate::Value::Int(8));
        assert_eq!(s.kind.parse("16").unwrap(), crate::Value::Int(16));
        assert_eq!(s.access, Access::ReadWrite);
    }

    /// `wheel_texture_rpm` and `wheel_tf_merge` are genuinely RW on the wire
    /// (see `docs/SYSFS_API.md`), but this app never writes either: the rpm
    /// feed is logi-rpm-bridge's job and the merge switch is logi-launch's,
    /// so both are modeled `ReadOnly` here to keep this app from racing
    /// them. `Device::write` enforces the same rule at the sysfs boundary.
    #[test]
    fn texture_rpm_and_merge_state_are_read_only_in_this_app() {
        for attr in ["wheel_texture_rpm", "wheel_tf_merge"] {
            let s = REGISTRY.iter().find(|s| s.attr == attr).unwrap();
            assert_eq!(s.access, Access::ReadOnly, "{attr}");
        }
    }

    /// The rev strip is a live feed's, not a slider's: logi-rpm-bridge
    /// drives it during a texture-merge session and logi-tf-sim's rev feeder
    /// otherwise, both at up to 60 Hz. An editable row here was a third
    /// writer, and whichever lost looked like a broken control. The LED test
    /// still borrows the strip through `Device::write_test_pattern`.
    #[test]
    fn the_rev_strip_is_read_only_in_this_app() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_rev_level").unwrap();
        assert_eq!(s.access, Access::ReadOnly);
    }

    /// The rpm feed's live-vs-stale display, exercised through the actual
    /// registry entry rather than a bare `Kind::RpmFeed` (belt-and-braces
    /// against the registry drifting to a different `Kind` later).
    #[test]
    fn texture_rpm_feed_parses_and_shows_freshness() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_texture_rpm").unwrap();
        let fresh = s.kind.parse("6500 14000 12").unwrap();
        assert_eq!(fresh, crate::Value::RpmFeed { rpm: 6500, max_rpm: 14000, age_ms: 12 });
        assert_eq!(s.kind.display(&fresh), "6500 rpm");
        let stale = s.kind.parse("6500 14000 5000").unwrap();
        assert_eq!(s.kind.display(&stale), "no telemetry");
    }

    /// The four texture-tuning attrs only ever appear on a direct-drive
    /// wheel's registry: a G923 (`CLASSIC_REGISTRY`) has no `wheel_`-prefixed
    /// attrs at all (`device::a_g923_device_has_no_dd_settings_available`
    /// covers that generally), so naming them here documents the intent
    /// directly rather than relying on the prefix check alone.
    #[test]
    fn texture_group_is_absent_from_the_classic_registry() {
        for attr in [
            "wheel_texture_intensity",
            "wheel_texture_cylinders",
            "wheel_texture_rpm",
            "wheel_tf_merge",
        ] {
            assert!(REGISTRY.iter().any(|s| s.attr == attr), "{attr} missing from REGISTRY");
            assert!(
                !CLASSIC_REGISTRY.iter().any(|s| s.attr == attr),
                "{attr} leaked into CLASSIC_REGISTRY"
            );
        }
    }
}

#[cfg(test)]
mod classic_registry_tests {
    use super::*;
    use crate::setting::Access;

    #[test]
    fn classic_registry_has_no_duplicate_attrs() {
        let mut seen = std::collections::HashSet::new();
        for s in CLASSIC_REGISTRY {
            assert!(seen.insert(s.attr), "duplicate attr {}", s.attr);
        }
    }

    #[test]
    fn classic_attrs_never_collide_with_the_dd_registry() {
        for s in CLASSIC_REGISTRY {
            assert!(
                !REGISTRY.iter().any(|r| r.attr == s.attr),
                "{} exists in both registries",
                s.attr
            );
        }
    }

    #[test]
    fn every_classic_kind_roundtrips_a_sample() {
        for s in CLASSIC_REGISTRY {
            let raw = super::sample_raw(s);
            let v = s.kind.parse(&raw).unwrap_or_else(|e| panic!("{}: {e}", s.attr));
            let back = s.kind.format(&v).unwrap();
            assert!(!back.is_empty(), "{}: empty format", s.attr);
        }
    }

    #[test]
    fn classic_attrs_are_all_read_write_and_mode_agnostic() {
        // The classic engine has no desktop/onboard split; every setting
        // must stay writable in any mode.
        for s in CLASSIC_REGISTRY {
            assert_eq!(s.access, Access::ReadWrite, "{}", s.attr);
            assert!(matches!(s.mode_req, super::super::setting::ModeReq::Any), "{}", s.attr);
        }
    }

    #[test]
    fn range_clamps_to_the_g923s_40_900_span() {
        let s = CLASSIC_REGISTRY.iter().find(|s| s.attr == "range").unwrap();
        assert!(matches!(s.kind, Kind::IntRange { min: 40, max: 900, .. }));
        assert!(s.kind.parse("39").is_err());
        assert!(s.kind.parse("901").is_err());
        assert!(s.kind.parse("40").is_ok());
        assert!(s.kind.parse("900").is_ok());
    }

    #[test]
    fn gain_and_autocenter_are_scaled_percent_over_the_full_u16_range() {
        for attr in ["gain", "autocenter"] {
            let s = CLASSIC_REGISTRY.iter().find(|s| s.attr == attr).unwrap();
            assert!(matches!(s.kind, Kind::ScaledPercent { raw_max: 65535 }), "{attr}");
            assert_eq!(s.kind.parse("65535").unwrap(), crate::Value::Percent(100), "{attr}");
            assert_eq!(s.kind.parse("0").unwrap(), crate::Value::Percent(0), "{attr}");
            assert!(s.kind.parse("65536").is_err(), "{attr}");
            assert!(s.kind.parse("-1").is_err(), "{attr}");
        }
    }

    #[test]
    fn gain_and_autocenter_roundtrip_every_key_percent() {
        // Writing 0/1/50/99/100% and reading the resulting raw value back
        // must show the same percent again (the app's round-trip contract).
        for attr in ["gain", "autocenter"] {
            let s = CLASSIC_REGISTRY.iter().find(|s| s.attr == attr).unwrap();
            for pct in [0u8, 1, 50, 99, 100] {
                let raw = s.kind.format(&crate::Value::Percent(pct)).unwrap();
                let back = s.kind.parse(&raw).unwrap();
                assert_eq!(back, crate::Value::Percent(pct), "{attr} at {pct}% via raw {raw}");
            }
        }
    }

    #[test]
    fn combine_pedals_is_a_three_way_enum() {
        let s = CLASSIC_REGISTRY.iter().find(|s| s.attr == "combine_pedals").unwrap();
        let Kind::Enum(variants) = s.kind else { panic!("expected Enum") };
        assert_eq!(variants, ["separate", "gas + brake", "gas + clutch"]);
        assert!(s.kind.parse("0").is_ok());
        assert!(s.kind.parse("2").is_ok());
        assert!(s.kind.parse("3").is_err());
    }

    #[test]
    fn classic_categories_place_each_attr_sensibly() {
        let cat = |attr: &str| CLASSIC_REGISTRY.iter().find(|s| s.attr == attr).unwrap().category;
        assert_eq!(cat("range"), Category::Steering);
        assert_eq!(cat("gain"), Category::Ffb);
        assert_eq!(cat("autocenter"), Category::Ffb);
        assert_eq!(cat("combine_pedals"), Category::Pedals);
    }
}
