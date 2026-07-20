//! PID (Physical Interface Device) output report decoder.
//!
//! The virtual wheel exposes the PID output collection from the driver's HID
//! report descriptor (`descriptor::report_descriptor`, report ids
//! `0x50..=0x5D`, see `mainline/hid-logitech-hidpp.c`). When a game's
//! DirectInput/HID FFB stack drives our uhid device, the kernel forwards each
//! PID output report to us as an `UHID_OUTPUT` event. This module turns the
//! raw bytes of one such report into a typed `EffectOp` the rest of the proxy
//! can act on, with no device or I/O dependency of its own.
//!
//! Field offsets: the byte layout inside each report follows the field order
//! declared in the driver's PID collection, the same order the kernel's
//! in-tree `hid-pidff` driver fills. Each report's decoding is kept in one
//! `match` arm below with the field offsets spelled out in comments, so that
//! if a real game's wire capture (the hardware validation task) finds a
//! mismatch, fixing it is a one-line change to that arm's offsets.

/// PID Effect Type usages (HID Usage Page 0x0F), as declared in the driver's
/// `CREATE_NEW_EFFECT` (0x54) report collection.
pub const EFFECT_TYPE_CONSTANT: u8 = 0x26;
pub const EFFECT_TYPE_RAMP: u8 = 0x27;
pub const EFFECT_TYPE_SQUARE: u8 = 0x30;
pub const EFFECT_TYPE_SINE: u8 = 0x31;
pub const EFFECT_TYPE_TRIANGLE: u8 = 0x32;
pub const EFFECT_TYPE_SAWUP: u8 = 0x33;
pub const EFFECT_TYPE_SAWDOWN: u8 = 0x34;
pub const EFFECT_TYPE_SPRING: u8 = 0x40;
pub const EFFECT_TYPE_DAMPER: u8 = 0x41;
pub const EFFECT_TYPE_INERTIA: u8 = 0x42;
pub const EFFECT_TYPE_FRICTION: u8 = 0x43;

/// PID output report ids, from the driver's PID collection
/// (`descriptor::PID_COLLECTION`).
const REPORT_DEVICE_CONTROL: u8 = 0x50;
const REPORT_SET_EFFECT: u8 = 0x51;
const REPORT_SET_ENVELOPE: u8 = 0x52;
const REPORT_SET_CONDITION: u8 = 0x53;
const REPORT_CREATE_NEW_EFFECT: u8 = 0x54;
const REPORT_SET_CONSTANT: u8 = 0x55;
const REPORT_SET_RAMP: u8 = 0x58;
const REPORT_DEVICE_GAIN: u8 = 0x59;
const REPORT_EFFECT_OPERATION: u8 = 0x5A;
const REPORT_BLOCK_FREE: u8 = 0x5B;
const REPORT_SET_PERIODIC: u8 = 0x5D;

/// The Set Effect report's Duration field declares logical max 0x7FFF, but
/// the PID spec (and the kernel's hid-pidff, which fills the field's full 16
/// bits) uses the all-ones value to mean "play until stopped". evdev encodes
/// the same thing as `replay.length == 0`.
pub const PID_DURATION_INFINITE: u16 = 0xFFFF;

/// The waveform/condition family of a PID effect, decoded from the
/// `CREATE_NEW_EFFECT` (0x54) report's Effect Type usage byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    Constant,
    Ramp,
    Square,
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Spring,
    Damper,
    Inertia,
    Friction,
}

/// Maps the Effect Type byte carried by the Create New Effect feature
/// report (`0x54`) to the waveform/condition family it selects.
///
/// Two encodings arrive on the wire for this HID *array* field:
///
/// - The HID-conformant one: array fields carry a *logical value*, the
///   1-based index into the collection's usage list (the descriptor
///   declares Logical Min 1). This is what the kernel's hid-pidff sends
///   (`create_new_effect_type->value[0] = type_id`, a `find_usage()+1`
///   index) and what a descriptor-driven host stack produces.
/// - The raw usage byte itself (`0x26` Constant .. `0x43` Friction), which
///   some PID host stacks write directly.
///
/// The two ranges (1..=12 and 0x26..=0x43) do not overlap, so both are
/// accepted here. Returns `None` for anything else, including index 3 /
/// usage `0x28` (Custom Force Data, declared but unsupported downstream).
pub fn effect_kind_from_type_byte(b: u8) -> Option<EffectKind> {
    match b {
        // 1-based usage-list indices, in the descriptor's declared order:
        // 0x26, 0x27, 0x28, 0x30..0x34, 0x40..0x43.
        1 => Some(EffectKind::Constant),
        2 => Some(EffectKind::Ramp),
        // 3 is Custom Force Data: no EffectKind variant.
        4 => Some(EffectKind::Square),
        5 => Some(EffectKind::Sine),
        6 => Some(EffectKind::Triangle),
        7 => Some(EffectKind::SawUp),
        8 => Some(EffectKind::SawDown),
        9 => Some(EffectKind::Spring),
        10 => Some(EffectKind::Damper),
        11 => Some(EffectKind::Inertia),
        12 => Some(EffectKind::Friction),

        EFFECT_TYPE_CONSTANT => Some(EffectKind::Constant),
        EFFECT_TYPE_RAMP => Some(EffectKind::Ramp),
        EFFECT_TYPE_SQUARE => Some(EffectKind::Square),
        EFFECT_TYPE_SINE => Some(EffectKind::Sine),
        EFFECT_TYPE_TRIANGLE => Some(EffectKind::Triangle),
        EFFECT_TYPE_SAWUP => Some(EffectKind::SawUp),
        EFFECT_TYPE_SAWDOWN => Some(EffectKind::SawDown),
        EFFECT_TYPE_SPRING => Some(EffectKind::Spring),
        EFFECT_TYPE_DAMPER => Some(EffectKind::Damper),
        EFFECT_TYPE_INERTIA => Some(EffectKind::Inertia),
        EFFECT_TYPE_FRICTION => Some(EffectKind::Friction),
        _ => None,
    }
}

/// Reply report for a `Get_Report(Feature, 0x56)` (PID Block Load) request.
///
/// Byte 0 is the report id (0x56): hidraw copies the bytes we supply straight
/// into the caller's buffer without adding a report id itself, and the host
/// expects a numbered report's id in byte 0, so the id must be included here.
/// The remaining bytes follow the descriptor's report id 0x56 collection
/// (`descriptor::PID_COLLECTION`: Effect Block Index u8, Block Load Status u8
/// where 1 = Block Load Success, RAM Pool Available u16 LE).
pub fn pid_block_load_reply(block: u8) -> [u8; 5] {
    let ram = PID_POOL_RAM_SIZE.to_le_bytes();
    [0x56, block, 1, ram[0], ram[1]]
}

/// RAM Pool Size / RAM Pool Available reported to the host: the maximum the
/// descriptor's logical range allows (`0x27, 0xFF, 0xFF, 0x00, 0x00` in the
/// 0x57 collection, i.e. logical max 0xFFFF). This proxy forwards effects to
/// the real wheel's own FF core rather than managing a fixed-size pool
/// itself, so it never has a real capacity figure to report; reporting the
/// maximum tells the host it will never be refused for lack of pool space.
pub const PID_POOL_RAM_SIZE: u16 = 0xFFFF;

/// Simultaneous Effects Max reported to the host (report id 0x57, Usage
/// 0x83, logical range 0..0xFF). 40 is an arbitrary generous figure (the
/// real wheel's kernel FF core enforces its own actual limit on upload);
/// this only needs to be large enough that a game does not truncate its
/// effect count against it.
pub const PID_POOL_SIMULTANEOUS_MAX: u8 = 40;

/// Reply report for a `Get_Report(Feature, 0x57)` (PID Pool) request.
///
/// Byte 0 is the report id (0x57), for the same reason as
/// [`pid_block_load_reply`]. The remaining bytes follow the field layout the
/// descriptor declares for report id 0x57 (`descriptor::PID_COLLECTION`): RAM
/// Pool Size (u16 LE), Simultaneous Effects Max (u8), then one flags byte
/// packing Device Managed Pool (bit 0, Usage 0xA9) and Shared Parameter Blocks
/// (bit 1, Usage 0xAA), with the remaining 6 bits padding (the descriptor
/// declares them `Const,Var,Abs`, i.e. always read as 0).
///
/// Device Managed Pool is set: this proxy assigns block indices itself (see
/// the `0x54` handling in `Proxy::run`) rather than requiring the host to
/// track free blocks. Shared Parameter Blocks is left clear: each effect
/// block gets its own independent parameter state in `sink::Sink`, never
/// shared across blocks. These flag values are a best-effort reading of the
/// descriptor's intent, not something confirmed against a real game's
/// request yet; flagged for confirmation during hardware validation (Task
/// 8) if a game's behavior after this reply looks off.
pub fn pid_pool_reply() -> [u8; 5] {
    const FLAG_DEVICE_MANAGED_POOL: u8 = 0x01; // Shared Parameter Blocks (bit 1) left clear.
    let ram = PID_POOL_RAM_SIZE.to_le_bytes();
    [0x57, ram[0], ram[1], PID_POOL_SIMULTANEOUS_MAX, FLAG_DEVICE_MANAGED_POOL]
}

/// The PID Device Control selector (report `0x50`, usages `0x97..=0x9C`).
/// The wire value is the array field's logical value, 1..=6 in the
/// descriptor's declared usage order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceControlOp {
    EnableActuators,
    DisableActuators,
    StopAllEffects,
    Reset,
    Pause,
    Continue,
}

impl DeviceControlOp {
    /// Maps the wire selector byte to the control operation, `None` for a
    /// value outside the declared logical range.
    pub fn from_wire(b: u8) -> Option<DeviceControlOp> {
        match b {
            0x01 => Some(DeviceControlOp::EnableActuators),
            0x02 => Some(DeviceControlOp::DisableActuators),
            0x03 => Some(DeviceControlOp::StopAllEffects),
            0x04 => Some(DeviceControlOp::Reset),
            0x05 => Some(DeviceControlOp::Pause),
            0x06 => Some(DeviceControlOp::Continue),
            _ => None,
        }
    }
}

/// A single decoded PID output report, one variant per report id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectOp {
    Create { block: u8, kind: EffectKind },
    SetEffect { block: u8, duration_ms: u16, direction: u16 },
    SetConstant { block: u8, magnitude: i16 },
    SetRamp { block: u8, start: i16, end: i16 },
    SetPeriodic { block: u8, magnitude: i16, offset: i16, phase: u16, period_ms: u16 },
    SetCondition { block: u8, center: i16, coeff_pos: i16, coeff_neg: i16, sat_pos: u16, sat_neg: u16 },
    SetEnvelope { block: u8, attack_ms: u16, attack_level: u16, fade_ms: u16, fade_level: u16 },
    Operation { block: u8, start: bool, loop_count: u8 },
    /// PID Block Free (`0x5B`): the host is done with this effect block.
    Destroy { block: u8 },
    Gain { value: u8 },
    DeviceControl { op: DeviceControlOp },
}

/// Reads a little-endian `u16` at byte offset `at`, or `None` if the report
/// is too short.
fn u16_at(report: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = report.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Reads a little-endian `i16` at byte offset `at`, or `None` if the report
/// is too short.
fn i16_at(report: &[u8], at: usize) -> Option<i16> {
    u16_at(report, at).map(|v| v as i16)
}

/// Decodes one PID output report (`report[0]` is the report id) into a typed
/// `EffectOp`. Returns `None` for an unrecognized report id or a buffer too
/// short to hold the fields that report id requires.
pub fn decode(report: &[u8]) -> Option<EffectOp> {
    let id = *report.first()?;
    match id {
        // DEVICE_CONTROL: byte 1 is the control selector (Enable Actuators=1,
        // Disable Actuators=2, Stop All Effects=3, Device Reset=4, Device
        // Pause=5, Device Continue=6).
        REPORT_DEVICE_CONTROL => {
            let op = DeviceControlOp::from_wire(*report.get(1)?)?;
            Some(EffectOp::DeviceControl { op })
        }

        // SET_EFFECT: byte 1 = Effect Block Index, byte 2 = Effect Type
        // (unused here, Create carries the kind), bytes 3..5 = Duration (ms,
        // LE u16), bytes 5..7 = Trigger Repeat Interval (unused), bytes 7..9
        // = Sample Period (unused), bytes 9..11 = Start Delay (unused), byte
        // 11 = Gain (unused), byte 12 = Trigger Button (unused), byte 13 =
        // Axes Enable/Direction Enable bits (unused), bytes 14..16 =
        // Direction (LE u16).
        REPORT_SET_EFFECT => {
            let block = *report.get(1)?;
            // The PID "infinite" duration sentinel maps to evdev's 0.
            let duration_ms = match u16_at(report, 3)? {
                PID_DURATION_INFINITE => 0,
                d => d,
            };
            let direction = u16_at(report, 14)?;
            Some(EffectOp::SetEffect { block, duration_ms, direction })
        }

        // SET_ENVELOPE: byte 1 = Effect Block Index, bytes 2..4 = Attack
        // Level (LE u16), bytes 4..6 = Fade Level (LE u16), bytes 6..8 =
        // Attack Time (ms, LE u16), bytes 8..10 = Fade Time (ms, LE u16).
        REPORT_SET_ENVELOPE => {
            let block = *report.get(1)?;
            let attack_level = u16_at(report, 2)?;
            let fade_level = u16_at(report, 4)?;
            let attack_ms = u16_at(report, 6)?;
            let fade_ms = u16_at(report, 8)?;
            Some(EffectOp::SetEnvelope { block, attack_ms, attack_level, fade_ms, fade_level })
        }

        // SET_CONDITION: byte 1 = Effect Block Index, byte 2 = Parameter
        // Block Offset (unused, selects which axis this condition applies
        // to), bytes 3..5 = CP Offset/Center (LE i16), bytes 5..7 = Positive
        // Coefficient (LE i16), bytes 7..9 = Negative Coefficient (LE i16),
        // bytes 9..11 = Positive Saturation (LE u16), bytes 11..13 = Negative
        // Saturation (LE u16).
        REPORT_SET_CONDITION => {
            let block = *report.get(1)?;
            let center = i16_at(report, 3)?;
            let coeff_pos = i16_at(report, 5)?;
            let coeff_neg = i16_at(report, 7)?;
            let sat_pos = u16_at(report, 9)?;
            let sat_neg = u16_at(report, 11)?;
            Some(EffectOp::SetCondition { block, center, coeff_pos, coeff_neg, sat_pos, sat_neg })
        }

        // CREATE_NEW_EFFECT is a Feature report (0xB1 in the descriptor), not
        // an Output report: the host creates an effect via
        // Set_Report(Feature, 0x54) and the device assigns the block index,
        // read back via Get_Report(Feature, 0x56). It never arrives on the
        // interrupt Output path this function decodes, so this id is
        // intentionally not decoded here; the proxy handles it directly from
        // `uhid::Event::SetReport` via `effect_kind_from_type_byte`.
        REPORT_CREATE_NEW_EFFECT => None,

        // SET_CONSTANT: byte 1 = Effect Block Index, bytes 2..4 = Magnitude
        // (LE i16).
        REPORT_SET_CONSTANT => {
            let block = *report.get(1)?;
            let magnitude = i16_at(report, 2)?;
            Some(EffectOp::SetConstant { block, magnitude })
        }

        // SET_RAMP: byte 1 = Effect Block Index, byte 2 = Ramp Start, byte 3
        // = Ramp End. The descriptor declares Start/End as single unsigned
        // bytes (logical 0..255), so a descriptor-driven host (the kernel's
        // hid-pidff, Wine's HidP) sends the levels magnitude-only, one byte
        // each; the sign is not expressible on this wire. Upscale 0..255
        // back to the evdev level range 0..32767.
        REPORT_SET_RAMP => {
            let block = *report.get(1)?;
            let start = (*report.get(2)? as i32 * 32767 / 255) as i16;
            let end = (*report.get(3)? as i32 * 32767 / 255) as i16;
            Some(EffectOp::SetRamp { block, start, end })
        }

        // DEVICE_GAIN: byte 1 = Device Gain value (no block index).
        REPORT_DEVICE_GAIN => {
            let value = *report.get(1)?;
            Some(EffectOp::Gain { value })
        }

        // BLOCK_FREE: byte 1 = Effect Block Index. Sent by the kernel's
        // hid-pidff when an effect is erased (and once at init, to discard
        // its autocenter-detection probe effect).
        REPORT_BLOCK_FREE => {
            let block = *report.get(1)?;
            Some(EffectOp::Destroy { block })
        }

        // EFFECT_OPERATION: byte 1 = Effect Block Index, byte 2 = Operation
        // (Op Effect Start=1, Op Effect Start Solo=2, Op Effect Stop=3), byte
        // 3 = Loop Count.
        REPORT_EFFECT_OPERATION => {
            let block = *report.get(1)?;
            let op = *report.get(2)?;
            let loop_count = *report.get(3)?;
            let start = op == 0x01 || op == 0x02;
            Some(EffectOp::Operation { block, start, loop_count })
        }

        // SET_PERIODIC: byte 1 = Effect Block Index, bytes 2..4 = Magnitude
        // (LE i16), bytes 4..6 = Offset (LE i16), bytes 6..8 = Phase (LE
        // u16), bytes 8..10 = Period (ms, LE u16).
        REPORT_SET_PERIODIC => {
            let block = *report.get(1)?;
            let magnitude = i16_at(report, 2)?;
            let offset = i16_at(report, 4)?;
            let phase = u16_at(report, 6)?;
            let period_ms = u16_at(report, 8)?;
            Some(EffectOp::SetPeriodic { block, magnitude, offset, phase, period_ms })
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_kind_from_type_byte_maps_constant() {
        assert_eq!(effect_kind_from_type_byte(EFFECT_TYPE_CONSTANT), Some(EffectKind::Constant));
    }

    #[test]
    fn effect_kind_from_type_byte_maps_periodic() {
        assert_eq!(effect_kind_from_type_byte(EFFECT_TYPE_SINE), Some(EffectKind::Sine));
    }

    #[test]
    fn effect_kind_from_type_byte_maps_condition() {
        assert_eq!(effect_kind_from_type_byte(EFFECT_TYPE_SPRING), Some(EffectKind::Spring));
    }

    #[test]
    fn effect_kind_from_type_byte_unknown_is_none() {
        // 0x28 (Custom Force Data) is a real usage in the descriptor but has
        // no EffectKind variant.
        assert_eq!(effect_kind_from_type_byte(0x28), None);
    }

    #[test]
    fn decodes_set_constant() {
        // id 0x55, block 1, magnitude -8000 (LE i16)
        let mag = (-8000i16).to_le_bytes();
        let r = [0x55, 0x01, mag[0], mag[1]];
        assert!(matches!(decode(&r), Some(EffectOp::SetConstant { block: 1, magnitude: -8000 })));
    }

    #[test]
    fn block_load_reply_carries_report_id_block_success_and_pool_size() {
        let rep = pid_block_load_reply(3);
        assert_eq!(rep[0], 0x56, "report id must lead a numbered feature report");
        assert_eq!(rep[1], 3);
        assert_eq!(rep[2], 1);
        assert_eq!(u16::from_le_bytes([rep[3], rep[4]]), PID_POOL_RAM_SIZE);
    }

    #[test]
    fn pool_reply_carries_report_id_ram_size_and_max_effects() {
        let rep = pid_pool_reply();
        assert_eq!(rep[0], 0x57, "report id must lead a numbered feature report");
        assert_eq!(u16::from_le_bytes([rep[1], rep[2]]), PID_POOL_RAM_SIZE);
        assert_eq!(rep[3], PID_POOL_SIMULTANEOUS_MAX);
        assert_eq!(rep[4] & 0x01, 0x01, "device managed pool bit should be set");
    }

    #[test]
    fn create_new_effect_is_not_decoded_as_an_output_report() {
        // Create New Effect (0x54) is a Feature report; the proxy handles it
        // via uhid::Event::SetReport + effect_kind_from_type_byte, never via
        // this interrupt Output decoder, even with a recognized usage byte.
        let r = [0x54, 0x01, EFFECT_TYPE_CONSTANT];
        assert!(decode(&r).is_none());
    }

    #[test]
    fn decodes_effect_operation_start() {
        // id 0x5A, block 2, op=1 (start), loop 3
        let r = [0x5A, 0x02, 0x01, 0x03];
        assert!(matches!(decode(&r), Some(EffectOp::Operation { block: 2, start: true, loop_count: 3 })));
    }

    #[test]
    fn decodes_device_gain() {
        let r = [0x59, 0x7f];
        assert!(matches!(decode(&r), Some(EffectOp::Gain { value: 0x7f })));
    }

    #[test]
    fn unknown_report_id_is_none() {
        assert!(decode(&[0x77, 0x00]).is_none());
    }

    #[test]
    fn short_buffer_is_none_not_a_panic() {
        assert!(decode(&[0x55, 0x01]).is_none());
        assert!(decode(&[0x54]).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn decodes_set_periodic() {
        // id 0x5D, block 4, magnitude 12000, offset -500, phase 9000, period 20ms
        let mut r = vec![0x5D, 0x04];
        r.extend_from_slice(&12000i16.to_le_bytes());
        r.extend_from_slice(&(-500i16).to_le_bytes());
        r.extend_from_slice(&9000u16.to_le_bytes());
        r.extend_from_slice(&20u16.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetPeriodic {
                block: 4,
                magnitude: 12000,
                offset: -500,
                phase: 9000,
                period_ms: 20,
            })
        ));
    }

    #[test]
    fn decodes_set_condition() {
        // id 0x53, block 1, parameter block offset byte (unused), center
        // -100, coeff_pos 8000, coeff_neg -8000, sat_pos 10000, sat_neg 10000
        let mut r = vec![0x53, 0x01, 0x00];
        r.extend_from_slice(&(-100i16).to_le_bytes());
        r.extend_from_slice(&8000i16.to_le_bytes());
        r.extend_from_slice(&(-8000i16).to_le_bytes());
        r.extend_from_slice(&10000u16.to_le_bytes());
        r.extend_from_slice(&10000u16.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetCondition {
                block: 1,
                center: -100,
                coeff_pos: 8000,
                coeff_neg: -8000,
                sat_pos: 10000,
                sat_neg: 10000,
            })
        ));
    }

    #[test]
    fn decodes_set_envelope() {
        // id 0x52, block 3, attack_level 200, fade_level 50, attack_ms 300, fade_ms 400
        let mut r = vec![0x52, 0x03];
        r.extend_from_slice(&200u16.to_le_bytes());
        r.extend_from_slice(&50u16.to_le_bytes());
        r.extend_from_slice(&300u16.to_le_bytes());
        r.extend_from_slice(&400u16.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetEnvelope {
                block: 3,
                attack_ms: 300,
                attack_level: 200,
                fade_ms: 400,
                fade_level: 50,
            })
        ));
    }

    #[test]
    fn decodes_set_ramp_single_byte_levels() {
        // id 0x58, block 5, start 0, end 255: the descriptor declares the
        // ramp levels as single unsigned bytes, upscaled to 0..32767.
        let r = [0x58, 0x05, 0x00, 0xFF];
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetRamp { block: 5, start: 0, end: 32767 })
        ));
    }

    #[test]
    fn decodes_block_free_as_destroy() {
        let r = [0x5B, 0x07];
        assert!(matches!(decode(&r), Some(EffectOp::Destroy { block: 7 })));
    }

    #[test]
    fn set_effect_infinite_duration_maps_to_zero() {
        // hid-pidff writes the full-16-bit sentinel 0xFFFF for "play until
        // stopped"; evdev spells that replay.length == 0.
        let mut r = vec![0u8; 18];
        r[0] = 0x51;
        r[1] = 0x01;
        r[3..5].copy_from_slice(&PID_DURATION_INFINITE.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetEffect { block: 1, duration_ms: 0, .. })
        ));
    }

    #[test]
    fn effect_kind_accepts_array_index_encoding() {
        // The kernel's hid-pidff writes the Create New Effect array field as
        // a 1-based usage-list index, not the usage byte.
        assert_eq!(effect_kind_from_type_byte(1), Some(EffectKind::Constant));
        assert_eq!(effect_kind_from_type_byte(5), Some(EffectKind::Sine));
        assert_eq!(effect_kind_from_type_byte(9), Some(EffectKind::Spring));
        assert_eq!(effect_kind_from_type_byte(12), Some(EffectKind::Friction));
        // Index 3 is Custom Force Data: declared, unsupported.
        assert_eq!(effect_kind_from_type_byte(3), None);
        // Past the usage list.
        assert_eq!(effect_kind_from_type_byte(13), None);
    }

    #[test]
    fn decodes_set_effect() {
        // id 0x51, block 6, duration 1500ms at offset 3, direction 9000 at offset 14
        let mut r = vec![0u8; 18];
        r[0] = 0x51;
        r[1] = 0x06;
        r[3..5].copy_from_slice(&1500u16.to_le_bytes());
        r[14..16].copy_from_slice(&9000u16.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetEffect { block: 6, duration_ms: 1500, direction: 9000 })
        ));
    }

    #[test]
    fn decodes_device_control_selectors() {
        let cases = [
            (0x01, DeviceControlOp::EnableActuators),
            (0x02, DeviceControlOp::DisableActuators),
            (0x03, DeviceControlOp::StopAllEffects),
            (0x04, DeviceControlOp::Reset),
            (0x05, DeviceControlOp::Pause),
            (0x06, DeviceControlOp::Continue),
        ];
        for (wire, want) in cases {
            let r = [0x50, wire];
            assert_eq!(decode(&r), Some(EffectOp::DeviceControl { op: want }));
        }
        // Out-of-range selector is not an op.
        assert_eq!(decode(&[0x50, 0x07]), None);
    }
}
