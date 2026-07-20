//! Real-wheel evdev force-feedback sink.
//!
//! The kernel driver for the direct-drive wheel already exposes a complete
//! evdev force-feedback interface (`FF_CONSTANT`, `FF_RAMP`, `FF_SPRING`,
//! `FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA`, `FF_PERIODIC` with all
//! waveforms, `FF_RUMBLE`, `FF_GAIN`). This module translates decoded PID
//! effect operations (`pidff::EffectOp`) onto that interface via
//! `EVIOCSFF`/`EVIOCRMFF` and `EV_FF` play/stop/gain writes, reusing the
//! kernel's FF core rather than reimplementing effect math in userspace.
//!
//! `struct ff_effect`'s trailing union is modeled as a plain `[u8; 32]` byte
//! array (wrapped in [`FfUnion`]); typed sub-fields are written into and read
//! out of it with `to_le_bytes`/`from_le_bytes` rather than by taking a
//! reference to a field of a packed struct (undefined behavior for
//! misaligned fields), the same convention `source` and `uhid` use.
//!
//! Layout was checked against the running system's `<linux/input.h>` and a
//! small C probe (`offsetof`/`sizeof`) rather than assumed: on a 64-bit
//! kernel `struct ff_effect` is 48 bytes, 8-byte aligned, with the union
//! starting at offset 16 (`ff_periodic_effect`, the largest union member,
//! embeds an 8-byte `custom_data` pointer that forces this). [`FfUnion`]
//! carries `#[repr(C, align(8))]` so `size_of::<ff_effect>()` (which nix's
//! ioctl macros bake into the `EVIOCSFF` request number) matches the
//! kernel's; get the alignment wrong and the ioctl fails outright.

use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, OwnedFd};

use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::write;
use nix::{ioctl_write_int, ioctl_write_ptr};

use crate::pidff::{DeviceControlOp, EffectKind, EffectOp};
use crate::source::{input_event, Timeval};
use crate::{Error, Result};

/// `EV_FF`, the evdev event type for force-feedback play/stop/gain events
/// (`linux/input-event-codes.h`).
pub const EV_FF: u16 = 0x15;

// FF effect type constants (`linux/input-event-codes.h`). Verified against
// the running system's `/usr/include/linux/input.h` and cross-checked
// against this repo's own `mainline/hid-logitech-hidpp.c`, which the kernel
// driver builds against the same real header: FF_SPRING=0x53 and
// FF_INERTIA=0x56 (not the other way around).
pub const FF_RUMBLE: u16 = 0x50;
pub const FF_PERIODIC: u16 = 0x51;
pub const FF_CONSTANT: u16 = 0x52;
pub const FF_SPRING: u16 = 0x53;
pub const FF_FRICTION: u16 = 0x54;
pub const FF_DAMPER: u16 = 0x55;
pub const FF_INERTIA: u16 = 0x56;
pub const FF_RAMP: u16 = 0x57;
pub const FF_SQUARE: u16 = 0x58;
pub const FF_TRIANGLE: u16 = 0x59;
pub const FF_SINE: u16 = 0x5a;
pub const FF_SAW_UP: u16 = 0x5b;
pub const FF_SAW_DOWN: u16 = 0x5c;
pub const FF_GAIN: u16 = 0x60;

/// Size of the union embedded at the end of `struct ff_effect`, sized to fit
/// the largest member (`ff_periodic_effect`, 32 bytes on a 64-bit kernel).
const FF_UNION_SIZE: usize = 32;

/// Mirrors the kernel's `struct ff_trigger`.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ff_trigger {
    pub button: u16,
    pub interval: u16,
}

/// Mirrors the kernel's `struct ff_replay`.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ff_replay {
    pub length: u16,
    pub delay: u16,
}

/// The trailing `union` of the kernel's `struct ff_effect`, modeled as a
/// byte array rather than a Rust union: we only ever write and read scalar
/// sub-fields through explicit offsets, never hold a live typed reference
/// into it. See the module doc for why the alignment matters. `Deref`/
/// `DerefMut` to `[u8; FF_UNION_SIZE]` let callers index it (`u[0]`,
/// `u[4..6]`) exactly like a plain byte array.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct FfUnion(pub [u8; FF_UNION_SIZE]);

impl std::ops::Deref for FfUnion {
    type Target = [u8; FF_UNION_SIZE];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for FfUnion {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Mirrors the kernel's `struct ff_effect` (`linux/input.h`), field for
/// field: `type`, `id`, `direction`, `trigger`, `replay`, then the union.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ff_effect {
    pub type_: u16,
    pub id: i16,
    pub direction: u16,
    pub trigger: ff_trigger,
    pub replay: ff_replay,
    pub u: FfUnion,
}

/// Accumulated, decoded PID effect fields for one effect block. `apply`
/// folds successive `SetEffect`/`SetConstant`/`SetRamp`/`SetPeriodic`/
/// `SetCondition`/`SetEnvelope` reports into one of these per block, then
/// re-derives the whole `ff_effect` from it on every update (the PID
/// protocol updates one field group at a time; the kernel wants the whole
/// effect on every `EVIOCSFF`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectParams {
    pub duration_ms: u16,
    pub direction: u16,
    pub constant_level: i16,
    pub ramp_start: i16,
    pub ramp_end: i16,
    pub periodic_magnitude: i16,
    pub periodic_offset: i16,
    pub periodic_phase: u16,
    pub periodic_period_ms: u16,
    pub cond_center: i16,
    pub cond_coeff_pos: i16,
    pub cond_coeff_neg: i16,
    pub cond_sat_pos: u16,
    pub cond_sat_neg: u16,
    pub attack_ms: u16,
    pub attack_level: u16,
    pub fade_ms: u16,
    pub fade_level: u16,
}

/// Maps a decoded PID [`EffectKind`] to the kernel `ff_effect.type` value
/// and, for the periodic waveforms, the `ff_periodic_effect.waveform` value
/// that goes inside the union alongside it.
///
/// The kernel only accepts `type` in `FF_EFFECT_MIN..=FF_EFFECT_MAX`
/// (`FF_RAMP` is the max); `FF_SQUARE`/`FF_SINE`/etc. are waveform
/// selectors, valid only inside `ff_periodic_effect.waveform`, never as
/// `ff_effect.type` itself. So every periodic waveform kind maps to
/// `type_ = FF_PERIODIC` with the specific waveform written into the union.
fn effect_type_and_waveform(kind: EffectKind) -> (u16, Option<u16>) {
    match kind {
        EffectKind::Constant => (FF_CONSTANT, None),
        EffectKind::Ramp => (FF_RAMP, None),
        EffectKind::Sine => (FF_PERIODIC, Some(FF_SINE)),
        EffectKind::Square => (FF_PERIODIC, Some(FF_SQUARE)),
        EffectKind::Triangle => (FF_PERIODIC, Some(FF_TRIANGLE)),
        EffectKind::SawUp => (FF_PERIODIC, Some(FF_SAW_UP)),
        EffectKind::SawDown => (FF_PERIODIC, Some(FF_SAW_DOWN)),
        EffectKind::Spring => (FF_SPRING, None),
        EffectKind::Damper => (FF_DAMPER, None),
        EffectKind::Inertia => (FF_INERTIA, None),
        EffectKind::Friction => (FF_FRICTION, None),
    }
}

/// Writes a `ff_envelope { attack_length, attack_level, fade_length,
/// fade_level }` (all `u16`, 8 bytes) at union offset `at`.
fn write_envelope(u: &mut [u8; FF_UNION_SIZE], at: usize, p: &EffectParams) {
    u[at..at + 2].copy_from_slice(&p.attack_ms.to_le_bytes());
    u[at + 2..at + 4].copy_from_slice(&p.attack_level.to_le_bytes());
    u[at + 4..at + 6].copy_from_slice(&p.fade_ms.to_le_bytes());
    u[at + 6..at + 8].copy_from_slice(&p.fade_level.to_le_bytes());
}

/// Pure conversion from a decoded effect kind + accumulated params to a
/// kernel `ff_effect`, ready for `EVIOCSFF`. `id` should be `-1` for a new
/// upload (the kernel assigns one) or the existing kernel-assigned id to
/// update an effect already uploaded for this block.
pub fn to_ff_effect(kind: EffectKind, params: &EffectParams, id: i16) -> ff_effect {
    let mut u = [0u8; FF_UNION_SIZE];
    let (type_, waveform) = effect_type_and_waveform(kind);

    match kind {
        EffectKind::Constant => {
            // ff_constant_effect: level:i16 @0, then envelope @2.
            u[0..2].copy_from_slice(&params.constant_level.to_le_bytes());
            write_envelope(&mut u, 2, params);
        }
        EffectKind::Ramp => {
            // ff_ramp_effect: start_level:i16 @0, end_level:i16 @2, envelope @4.
            u[0..2].copy_from_slice(&params.ramp_start.to_le_bytes());
            u[2..4].copy_from_slice(&params.ramp_end.to_le_bytes());
            write_envelope(&mut u, 4, params);
        }
        EffectKind::Sine
        | EffectKind::Square
        | EffectKind::Triangle
        | EffectKind::SawUp
        | EffectKind::SawDown => {
            // ff_periodic_effect: waveform:u16 @0, period:u16 @2,
            // magnitude:i16 @4, offset:i16 @6, phase:u16 @8, envelope @10.
            // custom_len/custom_data (offset 24..32 with padding) are left
            // zeroed: we never upload custom-waveform sample data.
            let waveform = waveform.expect("periodic kinds always carry a waveform");
            u[0..2].copy_from_slice(&waveform.to_le_bytes());
            u[2..4].copy_from_slice(&params.periodic_period_ms.to_le_bytes());
            u[4..6].copy_from_slice(&params.periodic_magnitude.to_le_bytes());
            u[6..8].copy_from_slice(&params.periodic_offset.to_le_bytes());
            u[8..10].copy_from_slice(&params.periodic_phase.to_le_bytes());
            write_envelope(&mut u, 10, params);
        }
        EffectKind::Spring | EffectKind::Damper | EffectKind::Inertia | EffectKind::Friction => {
            // ff_condition_effect[2]; we only drive axis 0 (single-axis
            // wheel), leaving axis 1 (offset 12..24) zeroed:
            // right_saturation:u16 @0, left_saturation:u16 @2,
            // right_coeff:i16 @4, left_coeff:i16 @6, deadband:u16 @8
            // (no PID field feeds this, left 0), center:i16 @10.
            // Positive PID values map to "right", negative to "left".
            u[0..2].copy_from_slice(&params.cond_sat_pos.to_le_bytes());
            u[2..4].copy_from_slice(&params.cond_sat_neg.to_le_bytes());
            u[4..6].copy_from_slice(&params.cond_coeff_pos.to_le_bytes());
            u[6..8].copy_from_slice(&params.cond_coeff_neg.to_le_bytes());
            u[10..12].copy_from_slice(&params.cond_center.to_le_bytes());
        }
    }

    // The PID wire carries direction in hundredths of degrees (the
    // descriptor's Direction field: logical 0..35900, unit degrees,
    // exponent -2); evdev's ff_effect.direction spans the same circle over
    // the full u16 (0x4000 = 90 degrees). Without this rescale a 270-degree
    // PID direction (27000) lands in evdev's upper-left quadrant instead of
    // west, flipping the force sign for every leftward effect.
    let direction = ((params.direction as u32 * 0x10000) / 36000).min(0xFFFF) as u16;

    ff_effect {
        type_,
        id,
        direction,
        trigger: ff_trigger::default(),
        replay: ff_replay { length: params.duration_ms, delay: 0 },
        u: FfUnion(u),
    }
}

/// Reads back `ff_constant_effect.level` (also `ff_ramp_effect.start_level`,
/// same offset) from the union. Test/debug helper.
pub fn constant_level(e: &ff_effect) -> i16 {
    i16::from_le_bytes([e.u[0], e.u[1]])
}

/// Reads back `ff_condition_effect[0].right_coeff` from the union. Test/
/// debug helper.
pub fn condition_right_coeff(e: &ff_effect) -> i16 {
    i16::from_le_bytes([e.u[4], e.u[5]])
}

// `EVIOCSFF` / `EVIOCRMFF` (`linux/input.h`): `_IOW('E', 0x80, struct
// ff_effect)` and `_IOW('E', 0x81, int)`. EVIOCSFF is declared as a
// write-only ioctl but the kernel actually writes the assigned effect id
// back into the same buffer (`put_user(effect.id, ...)` in evdev.c), which
// is why `upload` below reads `effect.id` back out after the call.
ioctl_write_ptr!(eviocsff, b'E', 0x80, ff_effect);
ioctl_write_int!(eviocrmff, b'E', 0x81);

// Byte layout of an encoded `input_event` on the wire (64-bit kernel ABI):
// tv_sec(8) + tv_usec(8) + type(2) + code(2) + value(4) = 24 bytes. Mirrors
// `source::decode_event`'s offsets in the other direction.
const EVENT_SIZE: usize = 24;

fn encode_event(ev: &input_event) -> [u8; EVENT_SIZE] {
    let mut b = [0u8; EVENT_SIZE];
    b[0..8].copy_from_slice(&ev.time.tv_sec.to_le_bytes());
    b[8..16].copy_from_slice(&ev.time.tv_usec.to_le_bytes());
    b[16..18].copy_from_slice(&ev.type_.to_le_bytes());
    b[18..20].copy_from_slice(&ev.code.to_le_bytes());
    b[20..24].copy_from_slice(&ev.value.to_le_bytes());
    b
}

/// The real wheel's evdev force-feedback node. Drives the kernel's FF core
/// via `EVIOCSFF`/`EVIOCRMFF` and `EV_FF` writes; holds no effect math of
/// its own.
pub struct Sink {
    fd: OwnedFd,
    /// PID effect block index -> kernel-assigned `ff_effect.id`, once
    /// uploaded.
    effects: HashMap<u8, i16>,
    /// PID effect block index -> its waveform/condition kind, set by
    /// `Create` and needed on every later `Set*` re-upload.
    kinds: HashMap<u8, EffectKind>,
    /// PID effect block index -> its accumulated decoded fields.
    params: HashMap<u8, EffectParams>,
}

impl Sink {
    /// Open the real wheel's evdev FF node read-write.
    ///
    /// Device gain scales every uploaded effect, and it powers up unset (and is
    /// left at 0 by a prior [`Sink::shutdown`]). A DirectInput host that never
    /// sends a Device Gain report assumes the device defaults to full gain, so
    /// without this an effect uploads successfully but renders as zero force.
    /// Set it to full on open; a later `Gain` op from the host overrides it.
    pub fn open(evdev_path: &str) -> Result<Sink> {
        let fd = open(evdev_path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
            .map_err(|e| Error::Io(format!("open {evdev_path}"), std::io::Error::from(e)))?;
        let mut sink = Sink { fd, effects: HashMap::new(), kinds: HashMap::new(), params: HashMap::new() };
        sink.write_ff_event(FF_GAIN, 0xFFFF)?;
        Ok(sink)
    }

    /// Writes one `EV_FF` event (`code`/`value`) followed implicitly by the
    /// kernel's own handling; force-feedback writes need no `SYN_REPORT`.
    fn write_ff_event(&mut self, code: u16, value: i32) -> Result<()> {
        let ev = input_event { time: Timeval::default(), type_: EV_FF, code, value };
        write(&self.fd, &encode_event(&ev))
            .map_err(|e| Error::Io("write EV_FF".into(), std::io::Error::from(e)))?;
        Ok(())
    }

    /// Rebuilds the `ff_effect` for `block` from its accumulated params and
    /// re-uploads it via `EVIOCSFF`, storing back whatever id the kernel
    /// assigns (or keeps, on an update).
    fn upload(&mut self, block: u8) -> Result<()> {
        let kind = *self
            .kinds
            .get(&block)
            .ok_or_else(|| Error::Protocol(format!("effect block {block} has no Create")))?;
        let id = self.effects.get(&block).copied().unwrap_or(-1);
        let params = self.params.entry(block).or_default();
        let mut effect = to_ff_effect(kind, params, id);

        // See the note above `eviocsff`: this ioctl is declared write-only
        // but the kernel writes the assigned id back through the same
        // pointer, so we deliberately go through a `*mut` derived from a
        // mutable binding (not a `*const` cast from a shared reference) and
        // read `effect.id` back out afterwards.
        unsafe { eviocsff(self.fd.as_raw_fd(), &mut effect as *mut ff_effect as *const ff_effect) }
            .map_err(|e| Error::Io("EVIOCSFF".into(), std::io::Error::from(e)))?;

        self.effects.insert(block, effect.id);
        Ok(())
    }

    /// Apply one decoded PID effect operation to the real wheel.
    pub fn apply(&mut self, op: EffectOp) -> Result<()> {
        match op {
            EffectOp::Create { block, kind } => {
                self.kinds.insert(block, kind);
                self.params.entry(block).or_default();
                Ok(())
            }
            EffectOp::SetEffect { block, duration_ms, direction } => {
                let p = self.params.entry(block).or_default();
                p.duration_ms = duration_ms;
                p.direction = direction;
                self.upload(block)
            }
            EffectOp::SetConstant { block, magnitude } => {
                self.params.entry(block).or_default().constant_level = magnitude;
                self.upload(block)
            }
            EffectOp::SetRamp { block, start, end } => {
                let p = self.params.entry(block).or_default();
                p.ramp_start = start;
                p.ramp_end = end;
                self.upload(block)
            }
            EffectOp::SetPeriodic { block, magnitude, offset, phase, period_ms } => {
                let p = self.params.entry(block).or_default();
                p.periodic_magnitude = magnitude;
                p.periodic_offset = offset;
                p.periodic_phase = phase;
                p.periodic_period_ms = period_ms;
                self.upload(block)
            }
            EffectOp::SetCondition { block, center, coeff_pos, coeff_neg, sat_pos, sat_neg } => {
                let p = self.params.entry(block).or_default();
                p.cond_center = center;
                p.cond_coeff_pos = coeff_pos;
                p.cond_coeff_neg = coeff_neg;
                p.cond_sat_pos = sat_pos;
                p.cond_sat_neg = sat_neg;
                self.upload(block)
            }
            EffectOp::SetEnvelope { block, attack_ms, attack_level, fade_ms, fade_level } => {
                let p = self.params.entry(block).or_default();
                p.attack_ms = attack_ms;
                p.attack_level = attack_level;
                p.fade_ms = fade_ms;
                p.fade_level = fade_level;
                self.upload(block)
            }
            EffectOp::Operation { block, start, loop_count } => {
                let id = *self
                    .effects
                    .get(&block)
                    .ok_or_else(|| Error::Protocol(format!("effect block {block} was never uploaded")))?;
                let value = if start { loop_count.max(1) as i32 } else { 0 };
                self.write_ff_event(id as u16, value)
            }
            EffectOp::Destroy { block } => {
                // Block Free: forget the block. A block that was Created but
                // never uploaded (no Set* report arrived, e.g. hid-pidff's
                // autocenter-detection probe effect) has no kernel id yet;
                // removing the bookkeeping is all there is to do.
                self.kinds.remove(&block);
                self.params.remove(&block);
                if let Some(id) = self.effects.remove(&block) {
                    let _ = unsafe { eviocrmff(self.fd.as_raw_fd(), id as u64) };
                }
                Ok(())
            }
            EffectOp::Gain { value } => {
                let scaled = (value as i32) * 0xFFFF / 255;
                self.write_ff_event(FF_GAIN, scaled)
            }
            EffectOp::DeviceControl { op } => match op {
                // Actuator enable/disable gates whether uploaded effects
                // may produce force; model it with the device gain, keeping
                // the uploaded effects intact so Enable resumes them. A
                // host-set gain is clobbered by Enable, which matches the
                // PID default of full gain after actuators come back.
                DeviceControlOp::EnableActuators => self.write_ff_event(FF_GAIN, 0xFFFF),
                DeviceControlOp::DisableActuators => self.write_ff_event(FF_GAIN, 0),
                DeviceControlOp::StopAllEffects => {
                    let ids: Vec<i16> = self.effects.values().copied().collect();
                    for id in ids {
                        let _ = self.write_ff_event(id as u16, 0);
                    }
                    Ok(())
                }
                // Reset returns the device to its default state: no effects
                // loaded, full gain. hid-pidff sends this before the first
                // effect of a session and re-sends gain only at init, so
                // zeroing the gain here (as the old shutdown-based handling
                // did) would leave every subsequent effect silent.
                DeviceControlOp::Reset => {
                    for (_, id) in self.effects.drain() {
                        let _ = unsafe { eviocrmff(self.fd.as_raw_fd(), id as u64) };
                    }
                    self.kinds.clear();
                    self.params.clear();
                    self.write_ff_event(FF_GAIN, 0xFFFF)
                }
                // Pause/Continue have no evdev counterpart; ack and ignore.
                DeviceControlOp::Pause | DeviceControlOp::Continue => Ok(()),
            },
        }
    }

    /// Erase every effect uploaded so far and zero the device gain. Errors
    /// removing an individual effect are not fatal (the effect may already
    /// be gone, e.g. the device was reset out from under us) so this does
    /// not return a `Result`; it is meant as an unconditional cleanup.
    pub fn shutdown(&mut self) {
        for (_, id) in self.effects.drain() {
            let _ = unsafe { eviocrmff(self.fd.as_raw_fd(), id as u64) };
        }
        self.kinds.clear();
        self.params.clear();
        let _ = self.write_ff_event(FF_GAIN, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pidff::EffectKind;

    #[test]
    fn constant_maps_type_and_level() {
        let p = EffectParams { constant_level: -8000, duration_ms: 500, ..Default::default() };
        let e = to_ff_effect(EffectKind::Constant, &p, 3);
        assert_eq!(e.type_, FF_CONSTANT);
        assert_eq!(e.id, 3);
        assert_eq!(e.replay.length, 500);
        assert_eq!(constant_level(&e), -8000);
    }

    #[test]
    fn spring_maps_condition_coeffs() {
        let p = EffectParams { cond_coeff_pos: 1000, cond_coeff_neg: -1000, cond_center: 0, ..Default::default() };
        let e = to_ff_effect(EffectKind::Spring, &p, 1);
        assert_eq!(e.type_, FF_SPRING);
        assert_eq!(condition_right_coeff(&e), 1000);
    }

    #[test]
    fn ramp_maps_start_and_end_levels() {
        let p = EffectParams { ramp_start: -3000, ramp_end: 3000, ..Default::default() };
        let e = to_ff_effect(EffectKind::Ramp, &p, 2);
        assert_eq!(e.type_, FF_RAMP);
        assert_eq!(constant_level(&e), -3000);
        assert_eq!(i16::from_le_bytes([e.u[2], e.u[3]]), 3000);
    }

    #[test]
    fn periodic_kind_uses_ff_periodic_type_with_waveform_in_union() {
        // The kernel only accepts FF_EFFECT_MIN..=FF_EFFECT_MAX (FF_RAMP is
        // the max) as ff_effect.type; FF_SINE etc. are waveform selectors
        // that belong inside the union, not the type field.
        let p = EffectParams { periodic_magnitude: 9000, periodic_period_ms: 20, ..Default::default() };
        let e = to_ff_effect(EffectKind::Sine, &p, 5);
        assert_eq!(e.type_, FF_PERIODIC);
        assert_eq!(u16::from_le_bytes([e.u[0], e.u[1]]), FF_SINE);
        assert_eq!(i16::from_le_bytes([e.u[4], e.u[5]]), 9000);
    }

    #[test]
    fn envelope_written_at_kind_specific_offset() {
        let p = EffectParams {
            attack_ms: 10,
            attack_level: 100,
            fade_ms: 20,
            fade_level: 50,
            ..Default::default()
        };
        let e = to_ff_effect(EffectKind::Constant, &p, 0);
        // ff_constant_effect envelope starts at union offset 2.
        assert_eq!(u16::from_le_bytes([e.u[2], e.u[3]]), 10);
        assert_eq!(u16::from_le_bytes([e.u[4], e.u[5]]), 100);
        assert_eq!(u16::from_le_bytes([e.u[6], e.u[7]]), 20);
        assert_eq!(u16::from_le_bytes([e.u[8], e.u[9]]), 50);
    }

    #[test]
    fn ff_effect_layout_matches_kernel_abi() {
        // Verified against the running system's <linux/input.h> via a small
        // C probe: sizeof(struct ff_effect) == 48, alignof == 8, union at
        // offset 16. Getting this wrong desyncs the EVIOCSFF request number
        // nix's ioctl macro bakes in from size_of::<ff_effect>().
        assert_eq!(std::mem::size_of::<ff_effect>(), 48);
        assert_eq!(std::mem::align_of::<ff_effect>(), 8);

        // Compute union offset via pointer arithmetic (offset_of! requires Rust 1.77+).
        let e = ff_effect {
            type_: 0,
            id: 0,
            direction: 0,
            trigger: ff_trigger::default(),
            replay: ff_replay::default(),
            u: FfUnion([0u8; FF_UNION_SIZE]),
        };
        let union_offset = (&e.u as *const _ as usize) - (&e as *const _ as usize);
        assert_eq!(union_offset, 16);
    }

    #[test]
    fn direction_rescales_pid_centidegrees_to_evdev_circle() {
        // PID 90.00 degrees (9000) is evdev 0x4000; PID 270.00 degrees
        // (27000) is evdev 0xC000. Getting this wrong flips force signs.
        let p = EffectParams { direction: 9000, ..Default::default() };
        assert_eq!(to_ff_effect(EffectKind::Constant, &p, 0).direction, 0x4000);
        let p = EffectParams { direction: 27000, ..Default::default() };
        assert_eq!(to_ff_effect(EffectKind::Constant, &p, 0).direction, 0xC000);
        let p = EffectParams { direction: 0, ..Default::default() };
        assert_eq!(to_ff_effect(EffectKind::Constant, &p, 0).direction, 0);
    }

    /// A Sink over /dev/null: exercises apply()'s bookkeeping without a real
    /// evdev node (writes succeed, ioctls are never reached on these paths).
    fn null_sink() -> Sink {
        let f = std::fs::File::options().write(true).open("/dev/null").expect("open /dev/null");
        Sink { fd: f.into(), effects: HashMap::new(), kinds: HashMap::new(), params: HashMap::new() }
    }

    #[test]
    fn destroy_of_created_but_never_uploaded_block_is_ok() {
        // hid-pidff's autocenter probe: Create then immediately Block Free,
        // with no Set* (and thus no kernel upload) in between.
        let mut sink = null_sink();
        sink.apply(EffectOp::Create { block: 1, kind: EffectKind::Constant }).unwrap();
        sink.apply(EffectOp::Destroy { block: 1 }).unwrap();
        assert!(sink.kinds.is_empty());
        assert!(sink.params.is_empty());
    }

    #[test]
    fn destroy_of_unknown_block_is_ok() {
        let mut sink = null_sink();
        sink.apply(EffectOp::Destroy { block: 9 }).unwrap();
    }

    #[test]
    fn reset_clears_bookkeeping_and_keeps_gain_path_ok() {
        let mut sink = null_sink();
        sink.apply(EffectOp::Create { block: 1, kind: EffectKind::Constant }).unwrap();
        sink.apply(EffectOp::DeviceControl { op: DeviceControlOp::Reset }).unwrap();
        assert!(sink.kinds.is_empty());
        assert!(sink.params.is_empty());
        assert!(sink.effects.is_empty());
    }

    #[test]
    fn pause_and_continue_are_accepted_noops() {
        let mut sink = null_sink();
        sink.apply(EffectOp::DeviceControl { op: DeviceControlOp::Pause }).unwrap();
        sink.apply(EffectOp::DeviceControl { op: DeviceControlOp::Continue }).unwrap();
    }

    #[test]
    fn apply_without_create_reports_protocol_error() {
        // Sink::open needs a real evdev node, so this exercises apply()'s
        // block-tracking logic (kinds/effects maps) directly via a Sink over
        // a harmless real fd (/dev/null): SetConstant for a block with no
        // prior Create must fail before it ever reaches the ioctl, so `fd`
        // is never touched on this path.
        let f = std::fs::File::open("/dev/null").expect("open /dev/null");
        let mut sink =
            Sink { fd: f.into(), effects: HashMap::new(), kinds: HashMap::new(), params: HashMap::new() };
        let result = sink.apply(EffectOp::SetConstant { block: 9, magnitude: 100 });
        assert!(matches!(result, Err(Error::Protocol(_))));
    }
}
