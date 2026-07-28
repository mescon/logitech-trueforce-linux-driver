//! Pure logic for the Test view: wheel evdev discovery, raw
//! `input_event` decoding, steering-to-degrees conversion and button
//! naming.
//!
//! Everything here is either plain `std::fs` (discovery) or pure
//! functions over bytes/numbers, so both front-ends share one tested
//! implementation. The parts that need `ioctl` (the force-feedback
//! simulations) stay in the front-end crates; this module never opens a
//! device node.

use std::fs;
use std::path::Path;

use crate::device::WheelModel;

/// evdev event types (`linux/input-event-codes.h`).
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

/// evdev absolute-axis codes the wheel reports
/// (`linux/input-event-codes.h`); pedal assignments verified on an RS50
/// (see docs/SYSFS_API.md, "RS Shifter & Handbrake input mapping" and
/// `wheel_combined_pedals`).
pub const ABS_X: u16 = 0x00;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// The driver's report descriptor declares every analog axis as a full
/// 16-bit range: 0..65535, centered (for the steering axis) at 32767.5.
pub const AXIS_MAX: i32 = 65535;

/// Size of one `struct input_event` on a 64-bit kernel:
/// tv_sec(8) + tv_usec(8) + type(2) + code(2) + value(4).
pub const EVENT_SIZE: usize = 24;

/// One decoded wheel input event, reduced to what the Test view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestEvent {
    /// The steering axis (`ABS_X`), raw 0..65535.
    Steering(i32),
    /// A button transition; `pressed` is true for press and auto-repeat.
    Button { code: u16, pressed: bool },
    /// Any other absolute axis (pedals, handbrake, D-pad hat).
    Axis { code: u16, value: i32 },
}

/// Decode the first `EVENT_SIZE` bytes of `buf` as a `struct input_event`
/// (64-bit ABI, little-endian fields) and reduce it to a [`TestEvent`].
/// Returns `None` for a short buffer and for event types the Test view
/// does not show (`EV_SYN`, `EV_MSC`, `EV_FF` echoes, ...).
pub fn parse_event(buf: &[u8]) -> Option<TestEvent> {
    if buf.len() < EVENT_SIZE {
        return None;
    }
    let type_ = u16::from_le_bytes([buf[16], buf[17]]);
    let code = u16::from_le_bytes([buf[18], buf[19]]);
    let value = i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
    match type_ {
        EV_KEY => Some(TestEvent::Button { code, pressed: value != 0 }),
        EV_ABS if code == ABS_X => Some(TestEvent::Steering(value)),
        EV_ABS => Some(TestEvent::Axis { code, value }),
        _ => None,
    }
}

/// Map a raw absolute-axis reading to signed steering degrees, 0 at
/// center: `raw == min` is full left (`-range/2`), `raw == max` full
/// right (`+range/2`). `range_deg` is the wheel's configured rotation
/// range (`wheel_range`), the full lock-to-lock sweep.
pub fn steering_degrees(raw: i32, min: i32, max: i32, range_deg: u32) -> f32 {
    let span = (max as f32) - (min as f32);
    if span <= 0.0 {
        return 0.0;
    }
    let center = (min as f32 + max as f32) / 2.0;
    (raw as f32 - center) / span * range_deg as f32
}

/// The wheel's physical buttons in display order: evdev code and label.
///
/// docs/BUTTON_MAPPING.md lists the joystick button *indices*; the kernel
/// maps index 0-15 to `BTN_JOYSTICK + n` (0x120..) and index 16 onward to
/// `BTN_TRIGGER_HAPPY + (n - 16)` (0x2c0..), the default sequential
/// mapping the driver deliberately keeps (see `hidpp_dd_input_mapping`
/// in mainline/hid-logitech-hidpp.c). Indices 12-20 are descriptor gaps.
pub const WHEEL_BUTTONS: &[(u16, &str)] = &[
    (0x120, "A"),
    (0x121, "X"),
    (0x122, "B"),
    (0x123, "Y"),
    (0x124, "Right Paddle"),
    (0x125, "Left Paddle"),
    (0x126, "RT"),
    (0x127, "LT"),
    (0x128, "Camera / View"),
    (0x129, "Menu"),
    (0x12a, "RSB"),
    (0x12b, "LSB"),
    // Encoder twist directions hardware-verified 2026-07-19 (guided live
    // capture, fixed twist order): L CW=0x2c8, L CCW=0x2c9, R CW=0x2c5,
    // R CCW=0x2c6. CW = the dial's top edge moving right, facing the wheel.
    (0x2c5, "R Encoder CW"),
    (0x2c6, "R Encoder CCW"),
    (0x2c7, "R Encoder Push"),
    (0x2c8, "L Encoder CW"),
    (0x2c9, "L Encoder CCW"),
    (0x2ca, "L Encoder Push"),
    (0x2cb, "G1 (Logo)"),
    // GL/GR are their own buttons, NOT aliases of the shifter paddles:
    // guided capture 2026-07-20, GL=0x2cc, GR=0x2cd (bits 0/1 of the
    // report's byte 20 on the joystick interface).
    (0x2cc, "GL"),
    (0x2cd, "GR"),
];

/// The G923's physical buttons in display order: evdev code and label.
/// Hardware-captured 2026-07-27 by a guided live capture on the owner's own
/// G923 (PS edition, PID 0xc266): every physical button was pressed in
/// turn and its evdev code recorded, the same method `docs/BUTTON_MAPPING.md`
/// used for the RS50. Unlike the RS50, the G923 has exactly one dial (right
/// hand only, no left encoder), so its dial labels do not need an "R"/"L"
/// prefix; its 0x2c5-0x2c7 happen to be the same codes `WHEEL_BUTTONS` uses
/// for the RS50's *right* encoder, but 0x2c8 is this wheel's PS button,
/// while the RS50 reports its *left* encoder's CW twist on that same code -
/// which is exactly why sharing one table between the two wheels was wrong.
/// Indices 13-19 of the joystick's sequential numbering (evdev
/// 0x12c-0x12f, 0x2c0-0x2c2) are descriptor gaps with no physical button on
/// this wheel and are simply absent here; see [`button_codes_for_model`].
pub const G923_BUTTONS: &[(u16, &str)] = &[
    (0x120, "X"),
    (0x121, "Square"),
    (0x122, "Circle"),
    (0x123, "Triangle"),
    (0x124, "Right Paddle"),
    (0x125, "Left Paddle"),
    (0x126, "R2"),
    (0x127, "L2"),
    (0x128, "Share"),
    (0x129, "Options"),
    (0x12a, "R3"),
    (0x12b, "L3"),
    (0x2c3, "Plus (Up)"),
    (0x2c4, "Minus (Down)"),
    (0x2c5, "Dial CW"),
    (0x2c6, "Dial CCW"),
    (0x2c7, "Dial Push"),
    (0x2c8, "PS"),
];

/// [`G923_BUTTONS`] lookup, mirroring [`button_label`]'s shape for the RS50
/// table.
pub fn g923_button_label(code: u16) -> Option<&'static str> {
    G923_BUTTONS.iter().find(|(c, _)| *c == code).map(|(_, l)| *l)
}

/// One numbered callout box on the button-layout diagram
/// (`docs/images/rs-wheel-hub-button-layout.png`, 2500x2160): the box's
/// center and size as fractions of the image dimensions, and the evdev
/// button codes that light it up. Extracted from the PNG itself
/// (connected-components analysis of the white boxes), so a front-end can
/// tint the pressed button's box by scaling these fractions to whatever
/// size it draws the image at.
///
/// The numbering on the diagram follows the wheel manual, not the
/// joystick indices in `docs/BUTTON_MAPPING.md`. Two quirks:
/// - The hub's round GL/GR buttons (boxes 13 and 7) report the same gear
///   inputs as the shift paddles (boxes 16 and 17), so both boxes of a
///   pair light together.
/// - Each encoder's twist/push callout (boxes 12 and 8) is one block for
///   all three of its codes (CW, CCW, push).
///
/// An empty `codes` slice marks the D-pad box: the hat is not a button,
/// so the front-end lights it whenever the hat leaves center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalloutBox {
    /// Box center, as fractions of the image width/height.
    pub cx: f32,
    pub cy: f32,
    /// Box size, as fractions of the image width/height.
    pub w: f32,
    pub h: f32,
    /// The evdev codes that light this box (empty = the D-pad box).
    pub codes: &'static [u16],
}

/// Standard callout box size on the diagram (77x52 px of 2500x2160).
const BOX_W: f32 = 0.0308;
const BOX_H: f32 = 0.0241;
/// The two encoder twist/push label blocks (177x127 px).
const KNOB_W: f32 = 0.0708;
const KNOB_H: f32 = 0.0588;

/// Every callout box on the layout diagram; see [`CalloutBox`].
pub const CALLOUT_BOXES: &[CalloutBox] = &[
    // Box 1: X.
    CalloutBox { cx: 0.1082, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x121] },
    // Box 2: Y.
    CalloutBox { cx: 0.1882, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x123] },
    // Box 3: A.
    CalloutBox { cx: 0.7682, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x120] },
    // Box 4: B.
    CalloutBox { cx: 0.8482, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x122] },
    // Box 5: RT.
    CalloutBox { cx: 0.9722, cy: 0.1750, w: BOX_W, h: BOX_H, codes: &[0x126] },
    // Box 6: RSB.
    CalloutBox { cx: 0.9722, cy: 0.2444, w: BOX_W, h: BOX_H, codes: &[0x12a] },
    // Box 7: GR (its own button, hardware-verified 2026-07-20).
    CalloutBox { cx: 0.9722, cy: 0.3370, w: BOX_W, h: BOX_H, codes: &[0x2cd] },
    // Box 8: right encoder (twist CW/CCW + push).
    CalloutBox { cx: 0.9522, cy: 0.6553, w: KNOB_W, h: KNOB_H, codes: &[0x2c5, 0x2c6, 0x2c7] },
    // Box 9: Menu.
    CalloutBox { cx: 0.9722, cy: 0.4528, w: BOX_W, h: BOX_H, codes: &[0x129] },
    // Box 10: G1 (Logitech logo).
    CalloutBox { cx: 0.4842, cy: 0.7769, w: BOX_W, h: BOX_H, codes: &[0x2cb] },
    // Box 11: Camera / View.
    CalloutBox { cx: 0.0282, cy: 0.5222, w: BOX_W, h: BOX_H, codes: &[0x128] },
    // Box 12: left encoder (twist CW/CCW + push).
    CalloutBox { cx: 0.0482, cy: 0.6553, w: KNOB_W, h: KNOB_H, codes: &[0x2c8, 0x2c9, 0x2ca] },
    // Box 13: GL (its own button, hardware-verified 2026-07-20).
    CalloutBox { cx: 0.0282, cy: 0.4065, w: BOX_W, h: BOX_H, codes: &[0x2cc] },
    // Box 14: LSB.
    CalloutBox { cx: 0.0282, cy: 0.2444, w: BOX_W, h: BOX_H, codes: &[0x12b] },
    // Box 15: LT.
    CalloutBox { cx: 0.0282, cy: 0.1750, w: BOX_W, h: BOX_H, codes: &[0x127] },
    // Box 16: left paddle.
    CalloutBox { cx: 0.0282, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x125] },
    // Box 17: right paddle.
    CalloutBox { cx: 0.9682, cy: 0.0269, w: BOX_W, h: BOX_H, codes: &[0x124] },
    // Box D: the D-pad hat (62x52 px box; lit while the hat is off center).
    CalloutBox { cx: 0.0252, cy: 0.3139, w: 0.0248, h: BOX_H, codes: &[] },
];

/// Whether `b` should be tinted: any of its button codes held, or (for
/// the D-pad box) the hat off center. `held` answers "is this evdev code
/// currently pressed" from whatever state the front-end keeps.
pub fn callout_lit(b: &CalloutBox, hat: (i32, i32), held: impl Fn(u16) -> bool) -> bool {
    if b.codes.is_empty() {
        return hat != (0, 0);
    }
    b.codes.iter().any(|c| held(*c))
}

/// The label for a wheel button's evdev code, or `None` for a code not in
/// [`WHEEL_BUTTONS`] (a descriptor gap, or another device's button).
pub fn button_label(code: u16) -> Option<&'static str> {
    WHEEL_BUTTONS.iter().find(|(c, _)| *c == code).map(|(_, l)| *l)
}

/// [`button_label`] with the "BTN <code>" fallback both front-ends show
/// for an unmapped code.
pub fn button_name(code: u16) -> String {
    match button_label(code) {
        Some(l) => l.to_string(),
        None => format!("BTN {code}"),
    }
}

/// The honest, model-agnostic fallback label for a code covered by neither
/// [`WHEEL_BUTTONS`] nor [`G923_BUTTONS`]: "Button N", numbered by the
/// joystick index the kernel's default sequential input mapping implies
/// (`hidpp_dd_input_mapping` in mainline/hid-logitech-hidpp.c). Index 0-15
/// is `BTN_JOYSTICK + n` (evdev 0x120-0x12f, 16 codes) -> "Button 1"
/// through "Button 16"; index 16-24 is `BTN_TRIGGER_HAPPY + (n - 16)`
/// (evdev 0x2c0-0x2c8, 9 codes) -> "Button 17" through "Button 25". `None`
/// outside both ranges (falls back further to [`button_name_for_model`]'s
/// "BTN <code>").
///
/// This was the G923's only lookup before its 2026-07-27 guided capture
/// ([`G923_BUTTONS`]); it now only fires for the 7 descriptor-gap codes a
/// live G923 never actually reports (0x12c-0x12f, 0x2c0-0x2c2 -
/// [`button_codes_for_model`] excludes them from the G923's own list), kept
/// as a defensive fallback rather than deleted outright.
pub fn generic_button_label(code: u16) -> Option<String> {
    if (0x120..=0x12f).contains(&code) {
        Some(format!("Button {}", code - 0x120 + 1))
    } else if (0x2c0..=0x2c8).contains(&code) {
        Some(format!("Button {}", 16 + (code - 0x2c0) + 1))
    } else {
        None
    }
}

/// The evdev codes to track and show on the Test page's button grid, in
/// display order, for a wheel of `model`. RS50/G PRO (and `Unknown`, which
/// is treated as a DD wheel everywhere else in this crate too) get the
/// captured `WHEEL_BUTTONS` diagram list; a G923 gets its own captured
/// `G923_BUTTONS` list - exactly its 18 real buttons, none of the 7
/// descriptor-gap codes (0x12c-0x12f, 0x2c0-0x2c2) it never actually
/// reports, so the Test page's button grid shows only buttons that exist.
pub fn button_codes_for_model(model: WheelModel) -> Vec<u16> {
    match model {
        WheelModel::G923 => G923_BUTTONS.iter().map(|(c, _)| *c).collect(),
        WheelModel::Rs50 | WheelModel::GPro | WheelModel::Unknown => {
            WHEEL_BUTTONS.iter().map(|(c, _)| *c).collect()
        }
    }
}

/// [`button_label_for_model`] with the "BTN <code>" fallback, mirroring
/// [`button_name`] but per-model.
pub fn button_name_for_model(model: WheelModel, code: u16) -> String {
    button_label_for_model(model, code).unwrap_or_else(|| format!("BTN {code}"))
}

/// The label for `code` on a wheel of `model`: the RS50/G PRO diagram
/// table for those models (and `Unknown`, same reasoning as
/// [`button_codes_for_model`]), or the G923's own captured
/// [`G923_BUTTONS`] table, falling back further to
/// [`generic_button_label`] for one of the 7 gap codes a live G923 never
/// actually sends.
pub fn button_label_for_model(model: WheelModel, code: u16) -> Option<String> {
    match model {
        WheelModel::G923 => {
            g923_button_label(code).map(str::to_string).or_else(|| generic_button_label(code))
        }
        WheelModel::Rs50 | WheelModel::GPro | WheelModel::Unknown => {
            button_label(code).map(str::to_string)
        }
    }
}

/// A short label for the non-steering axes the Test view bars show.
pub fn axis_label(code: u16) -> Option<&'static str> {
    match code {
        ABS_X => Some("Steering"),
        ABS_RX => Some("Throttle"),
        ABS_RY => Some("Brake"),
        ABS_RZ => Some("Clutch"),
        ABS_Z => Some("Handbrake"),
        _ => None,
    }
}

/// The D-pad hat state as a compass label; `x`/`y` are the current
/// `ABS_HAT0X`/`ABS_HAT0Y` values (-1, 0 or 1; y is negative up).
pub fn hat_label(x: i32, y: i32) -> &'static str {
    match (x.signum(), y.signum()) {
        (0, 0) => "centered",
        (0, -1) => "up",
        (1, -1) => "up-right",
        (1, 0) => "right",
        (1, 1) => "down-right",
        (0, 1) => "down",
        (-1, 1) => "down-left",
        (-1, 0) => "left",
        _ => "up-left",
    }
}

/// True if `name` looks like a Logitech direct-drive wheel and not one of
/// its sibling input nodes (the same physical device exposes separate
/// evdev nodes for consumer-control keys, and some setups have unrelated
/// keyboard/mouse nodes with overlapping substrings). Same heuristic the
/// ffb-proxy crate uses for its own discovery.
pub fn is_wheel_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    let looks_like_wheel = upper.contains("RS50")
        || upper.contains("PRO RACING WHEEL")
        || upper.contains("G PRO")
        || upper.contains("G923");
    let excluded = upper.contains("CONSUMER CONTROL")
        || upper.contains("KEYBOARD")
        || upper.contains("MOUSE")
        || upper.contains("HEADSET");
    looks_like_wheel && !excluded
}

/// The discovered wheel's evdev node and human-readable name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelInput {
    /// `/dev/input/eventN`.
    pub event_path: String,
    /// The device name sysfs reports (e.g. "Logitech RS50 ...").
    pub name: String,
}

/// Numeric suffix of an `eventN` entry name, for a stable scan order.
fn event_index(file_name: &str) -> u32 {
    file_name.trim_start_matches("event").parse().unwrap_or(u32::MAX)
}

/// Scan `sysfs_input` (normally `/sys/class/input`) for `event*` entries
/// whose `device/name` passes [`is_wheel_name`], returning the first
/// match in ascending `eventN` order. `pub` (not just `discover_wheel_input`'s
/// hardcoded real path) so `device::wheel_display_name_at` can exercise the
/// same scan against a test fixture directory.
pub fn scan_wheel_input(sysfs_input: &Path) -> Option<WheelInput> {
    let mut entries: Vec<_> = fs::read_dir(sysfs_input)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
        .collect();
    entries.sort_by_key(|e| event_index(&e.file_name().to_string_lossy()));

    for entry in entries {
        let event_name = entry.file_name().to_string_lossy().into_owned();
        let name = match fs::read_to_string(entry.path().join("device/name")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        if is_wheel_name(&name) {
            return Some(WheelInput { event_path: format!("/dev/input/{event_name}"), name });
        }
    }
    None
}

/// Find the wheel's evdev node, or `None` when no wheel is connected.
pub fn discover_wheel_input() -> Option<WheelInput> {
    scan_wheel_input(Path::new("/sys/class/input"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built 24-byte `input_event`: time fields zeroed, then
    /// type/code/value little-endian at offsets 16/18/20.
    fn event_bytes(type_: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
        let mut b = [0u8; EVENT_SIZE];
        b[16..18].copy_from_slice(&type_.to_le_bytes());
        b[18..20].copy_from_slice(&code.to_le_bytes());
        b[20..24].copy_from_slice(&value.to_le_bytes());
        b
    }

    #[test]
    fn parse_decodes_steering_from_abs_x() {
        let b = event_bytes(EV_ABS, ABS_X, 40000);
        assert_eq!(parse_event(&b), Some(TestEvent::Steering(40000)));
    }

    #[test]
    fn parse_decodes_button_press_and_release() {
        let b = event_bytes(EV_KEY, 0x120, 1);
        assert_eq!(parse_event(&b), Some(TestEvent::Button { code: 0x120, pressed: true }));
        let b = event_bytes(EV_KEY, 0x2cb, 0);
        assert_eq!(parse_event(&b), Some(TestEvent::Button { code: 0x2cb, pressed: false }));
    }

    #[test]
    fn parse_decodes_other_axes_as_axis() {
        let b = event_bytes(EV_ABS, ABS_RX, 65535);
        assert_eq!(parse_event(&b), Some(TestEvent::Axis { code: ABS_RX, value: 65535 }));
        let b = event_bytes(EV_ABS, ABS_HAT0Y, -1);
        assert_eq!(parse_event(&b), Some(TestEvent::Axis { code: ABS_HAT0Y, value: -1 }));
    }

    #[test]
    fn parse_ignores_syn_and_short_buffers() {
        // EV_SYN / SYN_REPORT.
        let b = event_bytes(0x00, 0x00, 0);
        assert_eq!(parse_event(&b), None);
        // EV_FF play echo.
        let b = event_bytes(0x15, 0x52, 1);
        assert_eq!(parse_event(&b), None);
        assert_eq!(parse_event(&[0u8; 10]), None);
    }

    #[test]
    fn parse_reads_negative_values() {
        let b = event_bytes(EV_ABS, ABS_HAT0X, -1);
        assert_eq!(parse_event(&b), Some(TestEvent::Axis { code: ABS_HAT0X, value: -1 }));
    }

    #[test]
    fn degrees_center_is_zero() {
        let d = steering_degrees(32767, 0, AXIS_MAX, 900);
        assert!(d.abs() < 0.02, "near-center raw maps to ~0 deg, got {d}");
    }

    #[test]
    fn degrees_full_lock_is_half_range_each_way() {
        let right = steering_degrees(AXIS_MAX, 0, AXIS_MAX, 900);
        let left = steering_degrees(0, 0, AXIS_MAX, 900);
        assert!((right - 450.0).abs() < 0.01, "full right at 900 deg = +450, got {right}");
        assert!((left + 450.0).abs() < 0.01, "full left at 900 deg = -450, got {left}");
    }

    #[test]
    fn degrees_scale_with_the_configured_range() {
        let right_1080 = steering_degrees(AXIS_MAX, 0, AXIS_MAX, 1080);
        assert!((right_1080 - 540.0).abs() < 0.01, "full right at 1080 deg = +540, got {right_1080}");
        let quarter = steering_degrees(49151, 0, AXIS_MAX, 1080);
        assert!((quarter - 270.0).abs() < 0.5, "3/4 raw at 1080 deg = ~+270, got {quarter}");
    }

    #[test]
    fn degrees_survive_a_degenerate_range() {
        assert_eq!(steering_degrees(100, 0, 0, 900), 0.0);
    }

    #[test]
    fn button_labels_cover_the_mapped_codes_and_fall_back() {
        assert_eq!(button_label(0x120), Some("A"));
        assert_eq!(button_label(0x125), Some("Left Paddle"));
        assert_eq!(button_label(0x2cb), Some("G1 (Logo)"));
        assert_eq!(button_label(0x12c), None, "descriptor gap");
        assert_eq!(button_name(0x129), "Menu");
        assert_eq!(button_name(0x2c0), "BTN 704");
    }

    #[test]
    fn generic_button_label_covers_both_joystick_ranges() {
        assert_eq!(generic_button_label(0x120), Some("Button 1".to_string()));
        assert_eq!(generic_button_label(0x12f), Some("Button 16".to_string()));
        assert_eq!(generic_button_label(0x2c0), Some("Button 17".to_string()));
        assert_eq!(generic_button_label(0x2c8), Some("Button 25".to_string()));
        assert_eq!(generic_button_label(0x2c9), None, "past the G923's known range");
        assert_eq!(generic_button_label(0x100), None, "outside both ranges");
    }

    #[test]
    fn button_label_for_model_never_gives_a_g923_an_rs50_label() {
        // The live G923 (PID 0xc266) actually reports 0x2c8, which
        // WHEEL_BUTTONS calls the RS50's "L Encoder CW" - the G923 has no
        // left encoder at all, so that label would be an outright lie for
        // it. Its own captured table names it correctly: the PS button.
        assert_eq!(button_label(0x2c8), Some("L Encoder CW"));
        assert_eq!(button_label_for_model(WheelModel::G923, 0x2c8), Some("PS".to_string()));
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c8), "PS");
        // A gap code the live device never sends still falls back to the
        // generic joystick-index label, not the RS50's table.
        assert_eq!(button_name_for_model(WheelModel::G923, 0x12c), "Button 13");
        // A code outside every known range falls back further still,
        // honestly, to the raw code rather than any label.
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2ff), "BTN 767");
    }

    #[test]
    fn button_label_for_model_keeps_the_rs50_and_gpro_table() {
        for model in [WheelModel::Rs50, WheelModel::GPro, WheelModel::Unknown] {
            assert_eq!(button_label_for_model(model, 0x2c8), Some("L Encoder CW".to_string()));
            assert_eq!(button_name_for_model(model, 0x129), "Menu");
        }
    }

    #[test]
    fn every_g923_button_resolves_to_its_captured_label() {
        for (code, label) in G923_BUTTONS {
            assert_eq!(
                button_label_for_model(WheelModel::G923, *code),
                Some(label.to_string()),
                "code {code:#x}"
            );
        }
        // Spot-check a few against the live 2026-07-27 capture directly.
        assert_eq!(button_name_for_model(WheelModel::G923, 0x120), "X");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x124), "Right Paddle");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c3), "Plus (Up)");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c4), "Minus (Down)");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c5), "Dial CW");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c6), "Dial CCW");
        assert_eq!(button_name_for_model(WheelModel::G923, 0x2c7), "Dial Push");
    }

    #[test]
    fn button_codes_for_model_matches_the_live_g923_and_keeps_dd_wheels_unchanged() {
        let g923 = button_codes_for_model(WheelModel::G923);
        assert_eq!(g923.len(), 18, "the 18 real buttons the 2026-07-27 capture found");
        assert_eq!(
            g923,
            vec![
                0x120, 0x121, 0x122, 0x123, 0x124, 0x125, 0x126, 0x127, 0x128, 0x129, 0x12a,
                0x12b, 0x2c3, 0x2c4, 0x2c5, 0x2c6, 0x2c7, 0x2c8,
            ],
            "display order: 0x120-0x12b then 0x2c3-0x2c8"
        );
        // The 7 descriptor-gap codes are simply absent: no phantom buttons
        // on the Test page.
        for gap in [0x12c, 0x12d, 0x12e, 0x12f, 0x2c0, 0x2c1, 0x2c2] {
            assert!(!g923.contains(&gap), "gap code {gap:#x} must not appear");
        }

        for model in [WheelModel::Rs50, WheelModel::GPro, WheelModel::Unknown] {
            let codes = button_codes_for_model(model);
            let expected: Vec<u16> = WHEEL_BUTTONS.iter().map(|(c, _)| *c).collect();
            assert_eq!(codes, expected, "{model:?}");
        }
    }

    #[test]
    fn callout_boxes_cover_every_wheel_button_and_nothing_else() {
        // Every code a box lights must be a real wheel button, and every
        // wheel button must light at least one box (the paddles and the
        // GL/GR hub buttons share codes, so some codes light two).
        for b in CALLOUT_BOXES {
            for code in b.codes {
                assert!(
                    button_label(*code).is_some(),
                    "callout code {code:#x} is not in WHEEL_BUTTONS"
                );
            }
        }
        for (code, label) in WHEEL_BUTTONS {
            assert!(
                CALLOUT_BOXES.iter().any(|b| b.codes.contains(code)),
                "button {label} ({code:#x}) has no callout box"
            );
        }
    }

    #[test]
    fn callout_boxes_stay_inside_the_image() {
        for b in CALLOUT_BOXES {
            assert!(b.w > 0.0 && b.h > 0.0, "degenerate box");
            assert!(b.cx - b.w / 2.0 >= 0.0 && b.cx + b.w / 2.0 <= 1.0, "x out of range");
            assert!(b.cy - b.h / 2.0 >= 0.0 && b.cy + b.h / 2.0 <= 1.0, "y out of range");
        }
        let hat_boxes = CALLOUT_BOXES.iter().filter(|b| b.codes.is_empty()).count();
        assert_eq!(hat_boxes, 1, "exactly one D-pad box");
    }

    #[test]
    fn callout_lit_checks_codes_and_the_hat() {
        let paddle = CALLOUT_BOXES.iter().find(|b| b.codes == [0x124]).unwrap();
        assert!(callout_lit(paddle, (0, 0), |c| c == 0x124));
        assert!(!callout_lit(paddle, (1, 0), |c| c == 0x120), "other buttons stay dark");
        let knob = CALLOUT_BOXES.iter().find(|b| b.codes.contains(&0x2c6)).unwrap();
        assert!(callout_lit(knob, (0, 0), |c| c == 0x2c6), "any encoder code lights the block");
        let dpad = CALLOUT_BOXES.iter().find(|b| b.codes.is_empty()).unwrap();
        assert!(!callout_lit(dpad, (0, 0), |_| true), "centered hat stays dark");
        assert!(callout_lit(dpad, (0, -1), |_| false), "hat off center lights up");
    }

    #[test]
    fn axis_labels_name_the_pedals_and_handbrake() {
        assert_eq!(axis_label(ABS_RX), Some("Throttle"));
        assert_eq!(axis_label(ABS_RY), Some("Brake"));
        assert_eq!(axis_label(ABS_RZ), Some("Clutch"));
        assert_eq!(axis_label(ABS_Z), Some("Handbrake"));
        assert_eq!(axis_label(0x28), None);
    }

    #[test]
    fn hat_labels_cover_all_nine_states() {
        assert_eq!(hat_label(0, 0), "centered");
        assert_eq!(hat_label(0, -1), "up");
        assert_eq!(hat_label(1, 1), "down-right");
        assert_eq!(hat_label(-1, -1), "up-left");
        assert_eq!(hat_label(-1, 0), "left");
    }

    #[test]
    fn wheel_name_heuristic_matches_ffb_proxys() {
        assert!(is_wheel_name("Logitech RS50 Base for PlayStation/PC"));
        assert!(is_wheel_name("Logitech G PRO Racing Wheel"));
        assert!(is_wheel_name("Logitech  PRO Racing Wheel"));
        assert!(is_wheel_name("PRO Racing Wheel"));
        assert!(is_wheel_name("Logitech G923 Racing Wheel"));
        assert!(!is_wheel_name("Logi Litra Glow Consumer Control"));
        assert!(!is_wheel_name("RS50 Wireless Keyboard"));
        assert!(!is_wheel_name("G PRO Wireless Mouse"));
        assert!(!is_wheel_name("G923 Wireless Headset"));
        assert!(!is_wheel_name("Some Other Gamepad"));
    }

    #[test]
    fn scan_finds_the_wheel_by_sysfs_name() {
        let dir = std::env::temp_dir().join(format!("evtest-scan-{}", std::process::id()));
        let mk = |event: &str, name: &str| {
            let d = dir.join(event).join("device");
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("name"), format!("{name}\n")).unwrap();
        };
        mk("event3", "Logi Litra Glow Consumer Control");
        mk("event11", "Logitech RS50 Base for PlayStation/PC");
        mk("event2", "AT Translated Set 2 keyboard");
        let found = scan_wheel_input(&dir).expect("wheel found");
        assert_eq!(found.event_path, "/dev/input/event11");
        assert_eq!(found.name, "Logitech RS50 Base for PlayStation/PC");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_of_a_missing_dir_finds_nothing() {
        assert_eq!(scan_wheel_input(Path::new("/nonexistent-evtest-dir")), None);
    }
}
