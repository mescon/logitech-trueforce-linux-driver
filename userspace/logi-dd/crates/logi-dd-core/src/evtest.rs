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
    (0x2c5, "R Encoder CW"),
    (0x2c6, "R Encoder CCW"),
    (0x2c7, "R Encoder Push"),
    (0x2c8, "L Encoder CW"),
    (0x2c9, "L Encoder CCW"),
    (0x2ca, "L Encoder Push"),
    (0x2cb, "G1 (Logo)"),
];

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
    let looks_like_wheel = upper.contains("RS50") || upper.contains("G PRO");
    let excluded =
        upper.contains("CONSUMER CONTROL") || upper.contains("KEYBOARD") || upper.contains("MOUSE");
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
/// match in ascending `eventN` order.
fn scan_wheel_input(sysfs_input: &Path) -> Option<WheelInput> {
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
        assert!(!is_wheel_name("Logi Litra Glow Consumer Control"));
        assert!(!is_wheel_name("RS50 Wireless Keyboard"));
        assert!(!is_wheel_name("G PRO Wireless Mouse"));
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
