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
const REPORT_SET_PERIODIC: u8 = 0x5D;

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

impl EffectKind {
    fn from_usage(usage: u8) -> Option<Self> {
        match usage {
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
    Gain { value: u8 },
    DeviceControl { enable: bool },
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
        // Pause=5, Device Continue=6). We collapse it to enable/disable.
        REPORT_DEVICE_CONTROL => {
            let control = *report.get(1)?;
            Some(EffectOp::DeviceControl { enable: control == 0x01 })
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
            let duration_ms = u16_at(report, 3)?;
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

        // CREATE_NEW_EFFECT: byte 1 = Effect Block Index, byte 2 = Effect
        // Type usage byte (mapped to EffectKind via EFFECT_TYPE_*).
        REPORT_CREATE_NEW_EFFECT => {
            let block = *report.get(1)?;
            let usage = *report.get(2)?;
            let kind = EffectKind::from_usage(usage)?;
            Some(EffectOp::Create { block, kind })
        }

        // SET_CONSTANT: byte 1 = Effect Block Index, bytes 2..4 = Magnitude
        // (LE i16).
        REPORT_SET_CONSTANT => {
            let block = *report.get(1)?;
            let magnitude = i16_at(report, 2)?;
            Some(EffectOp::SetConstant { block, magnitude })
        }

        // SET_RAMP: byte 1 = Effect Block Index, bytes 2..4 = Ramp Start (LE
        // i16), bytes 4..6 = Ramp End (LE i16).
        REPORT_SET_RAMP => {
            let block = *report.get(1)?;
            let start = i16_at(report, 2)?;
            let end = i16_at(report, 4)?;
            Some(EffectOp::SetRamp { block, start, end })
        }

        // DEVICE_GAIN: byte 1 = Device Gain value (no block index).
        REPORT_DEVICE_GAIN => {
            let value = *report.get(1)?;
            Some(EffectOp::Gain { value })
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
    fn decodes_set_constant() {
        // id 0x55, block 1, magnitude -8000 (LE i16)
        let mag = (-8000i16).to_le_bytes();
        let r = [0x55, 0x01, mag[0], mag[1]];
        assert!(matches!(decode(&r), Some(EffectOp::SetConstant { block: 1, magnitude: -8000 })));
    }

    #[test]
    fn decodes_create_new_effect_kind() {
        // id 0x54, block 1, effect-type usage byte for Constant
        let r = [0x54, 0x01, EFFECT_TYPE_CONSTANT];
        assert!(matches!(decode(&r), Some(EffectOp::Create { block: 1, kind: EffectKind::Constant })));
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
    fn unrecognized_effect_type_usage_is_none() {
        // 0x28 (Custom Force Data) is a real usage in the descriptor but has
        // no EffectKind variant.
        let r = [0x54, 0x01, 0x28];
        assert!(decode(&r).is_none());
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
    fn decodes_set_ramp() {
        // id 0x58, block 5, start -5000, end 6000
        let mut r = vec![0x58, 0x05];
        r.extend_from_slice(&(-5000i16).to_le_bytes());
        r.extend_from_slice(&6000i16.to_le_bytes());
        assert!(matches!(
            decode(&r),
            Some(EffectOp::SetRamp { block: 5, start: -5000, end: 6000 })
        ));
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
    fn decodes_device_control_enable() {
        let r = [0x50, 0x01];
        assert!(matches!(decode(&r), Some(EffectOp::DeviceControl { enable: true })));
    }

    #[test]
    fn decodes_device_control_disable() {
        let r = [0x50, 0x02];
        assert!(matches!(decode(&r), Some(EffectOp::DeviceControl { enable: false })));
    }
}
