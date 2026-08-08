// SPDX-License-Identifier: GPL-2.0-only
//! Decoders for the Assetto Corsa family.
//!
//! Assetto Corsa and Competizione share the `acpmf_physics` /
//! `acpmf_static` sections and their layout, so one decoder reads both.
//! EVO renamed every section and moved the redline into the physics block,
//! so it gets [`decode_evo`]; it still shares the physics head and the gear
//! convention, which is why it lives here rather than in its own module.
//!
//! # Why this one can be written without a captured fixture
//!
//! Two reasons, and they cover the two blocks separately, because the risk
//! is not the same in both.
//!
//! **The physics block is safe because everything read here is in its first
//! 32 bytes.** Kunos have appended fields to that struct steadily since
//! 2014, but appending does not move what came before, and `packetId`,
//! `gas`, `gear`, `rpms` and `speedKmh` have been the first six members
//! since AC 1.0. The part of this format that drifts is the tail, which is
//! the part this decoder never touches.
//!
//! **The static block is the risky one**, because `maxRpm` sits at offset
//! 412, behind five `wchar_t[33]` name fields. That offset is only correct
//! if Windows' 16-bit `wchar_t` is what the game wrote. So it is checked
//! rather than assumed: see [`static_layout_looks_right`], which reads the
//! `smVersion` string that opens the block and confirms it really is UTF-16.
//! If the assumption were wrong that check fails and the sample is dropped,
//! rather than a number from the wrong place becoming a redline.
//!
//! The offsets were not counted by hand. They were produced by declaring the
//! documented structs and printing `offsetof`, cross-checked against two
//! independent MIT-licensed implementations of the same interface, and are
//! pinned by [`tests::offsets_match_the_documented_layout`].
//!
//! # Why this is not the UDP route
//!
//! Assetto Corsa also has a documented UDP telemetry protocol on port 9996.
//! It is not used here because it is a *conversational* protocol: the client
//! sends a handshake, the game replies, the client subscribes, and only then
//! does data flow. The daemon's other sources are all passive listeners, and
//! shared memory keeps this game the same shape as iRacing and RaceRoom
//! rather than adding a second pattern for one title.
//!
//! ## Layout (`Local\acpmf_physics`, first 32 bytes)
//!
//! | offset | field      | type | notes                                  |
//! |--------|------------|------|----------------------------------------|
//! | 0      | `packetId` | i32  | increments per physics tick            |
//! | 4      | `gas`      | f32  | throttle, 0.0..=1.0                    |
//! | 8      | `brake`    | f32  | unused here                            |
//! | 12     | `fuel`     | f32  | unused here                            |
//! | 16     | `gear`     | i32  | **0 reverse, 1 neutral, 2 first**      |
//! | 20     | `rpms`     | i32  | engine speed, already rpm              |
//! | 28     | `speedKmh` | f32  | unused here                            |
//!
//! ## Layout (`Local\acpmf_static`)
//!
//! | offset | field       | type       | notes                           |
//! |--------|-------------|------------|---------------------------------|
//! | 0      | `smVersion` | wchar_t[15]| UTF-16; the layout guard        |
//! | 412    | `maxRpm`    | i32        | redline                         |
//!
//! The gear convention is this format's trap: Assetto Corsa numbers reverse
//! as 0 and neutral as 1, one higher than every other source the daemon
//! reads, so an untranslated gear silently reports first gear as second.

#![cfg_attr(not(windows), allow(dead_code))]

use logi_wheel_core::relay::RelayTelemetry;

/// The relay wire id for Assetto Corsa.
pub const ID: &str = "assetto";

/// The relay wire id for Assetto Corsa EVO.
///
/// EVO renamed its sections and dropped the static block's `maxRpm`, so it
/// does not share Competizione's "identical, only the id differs" story. It
/// shares the physics head and the gear convention, and nothing else this
/// module needs. See [`decode_evo`].
pub const ID_EVO: &str = "ac-evo";

/// The relay wire id for Assetto Corsa Competizione.
///
/// Competizione publishes the *same section names* as Assetto Corsa, and
/// both blocks are byte-identical through every field read here, so this
/// decoder reads it unchanged. Only the id differs, because the two are
/// separate games to somebody setting an intensity.
///
/// The shared section name has a consequence worth knowing: only one of the
/// two can be running at a time, so whichever is up is what gets read,
/// whatever `--game` said. That misroutes a setting, never a reading.
pub const ID_ACC: &str = "acc";

/// Windows named section carrying the per-tick physics block.
pub const SECTION_PHYSICS: &str = "Local\\acpmf_physics";

/// Windows named section carrying the per-session static block.
pub const SECTION_STATIC: &str = "Local\\acpmf_static";

/// Assetto Corsa EVO's physics section. EVO renamed every block, so it does
/// not collide with the two older games and can be told apart from them.
pub const SECTION_PHYSICS_EVO: &str = "Local\\acevo_pmf_physics";

// Physics offsets (see module docs).
const OFF_GAS: usize = 4;
const OFF_GEAR: usize = 16;
const OFF_RPMS: usize = 20;

/// Through the last physics field read, `rpms`.
const MIN_PHYSICS_LEN: usize = OFF_RPMS + 4;

/// `wheelLoad[4]`, the vertical load on each tyre.
///
/// Offset computed from the documented field order (packetId, gas, brake,
/// fuel, gear, rpms, steerAngle, speedKmh, velocity[3], accG[3],
/// wheelSlip[4], then this), the same method the offsets above came from.
const OFF_WHEEL_LOAD: usize = 72;
/// Physics block long enough to contain it.
const MIN_PHYSICS_AIRBORNE_LEN: usize = OFF_WHEEL_LOAD + 16;
/// Below this, in newtons, a tyre is carrying nothing.
///
/// Deliberately not zero: a wheel barely kissing the road reads as a few
/// newtons and is not airborne, and a float from a physics engine rarely
/// lands on exact zero anyway.
const WHEEL_LOAD_EPSILON: f32 = 1.0;

/// EVO's redline, `currentMaxRpm`, which lives in the physics block rather
/// than a static one and is republished every tick.
const OFF_CURRENT_MAX_RPM: usize = 588;

/// Through EVO's `currentMaxRpm`.
const MIN_PHYSICS_EVO_LEN: usize = OFF_CURRENT_MAX_RPM + 4;

/// No engine with a redline worth synthesizing against reports one below
/// this. It is the EVO decoder's main defence: with no static block to
/// sanity-check and no version field, a wrong `currentMaxRpm` offset would
/// most likely land on a temperature, a pressure or a pedal, all of which
/// are far below any real redline.
const MIN_PLAUSIBLE_REDLINE: f32 = 1_000.0;

// Static offsets (see module docs).
const OFF_MAX_RPM: usize = 412;

/// Through `maxRpm`.
const MIN_STATIC_LEN: usize = OFF_MAX_RPM + 4;

/// Above this, the buffer is not engine data however plausible it looked.
const MAX_PLAUSIBLE_RPM: f32 = 30_000.0;

fn i32_at(buf: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn f32_at(buf: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

/// Confirm the static block really begins with a UTF-16 `smVersion` string,
/// which is what makes [`OFF_MAX_RPM`] correct.
///
/// `smVersion` holds something like `"1.7"`. Encoded as Windows UTF-16 that
/// is `31 00 2E 00 ...`: two consecutive 16-bit units, both printable ASCII.
/// Read with any other character width the second unit would be zero, so
/// requiring two printable units in a row is what distinguishes the layout
/// this decoder assumes from the one it would misread.
fn static_layout_looks_right(buf: &[u8]) -> bool {
    let Some(head) = buf.get(0..4) else { return false };
    let first = u16::from_le_bytes([head[0], head[1]]);
    let second = u16::from_le_bytes([head[2], head[3]]);
    let printable = |u: u16| (0x20..0x7f).contains(&u);
    printable(first) && printable(second)
}

/// Decode one read of the physics and static sections.
///
/// Both are required: the physics block carries engine speed but no
/// redline, and without a redline there is nothing to scale an engine note
/// against. Returns `None` for a short buffer, a static block whose layout
/// fails its guard, or a session that is not running (Assetto Corsa leaves
/// the engine fields zeroed in menus).
pub fn decode(physics: &[u8], statics: &[u8], game_id: &'static str) -> Option<RelayTelemetry> {
    if physics.len() < MIN_PHYSICS_LEN || statics.len() < MIN_STATIC_LEN {
        return None;
    }
    if !static_layout_looks_right(statics) {
        return None;
    }

    let max_rpm = i32_at(statics, OFF_MAX_RPM)? as f32;
    let rpm = i32_at(physics, OFF_RPMS)? as f32;
    if max_rpm <= 0.0
        || max_rpm > MAX_PLAUSIBLE_RPM
        || !(0.0..=MAX_PLAUSIBLE_RPM).contains(&rpm)
    {
        return None;
    }

    let (throttle, gear) = head_inputs(physics)?;
    Some(RelayTelemetry {
        game_id,
        rpm,
        max_rpm,
        throttle,
        gear,
        airborne: airborne(physics),
    })
}

/// All four wheels carrying no load.
///
/// Assetto Corsa's own documentation says `wheelLoad` is unused in
/// Competizione, and if that is true here the field reads zero always and a
/// naive test would report a car permanently airborne, which is worse than
/// never reporting it at all: the airborne layer ducks the road surface, so
/// a false positive silences haptics for the whole session.
///
/// So the reading is only trusted once the field has been seen carrying
/// load. Any car that is driving has weight on its wheels within a moment,
/// and a field that is never populated never passes that gate, so an
/// unpopulated field yields a permanent `false` rather than a permanent
/// `true`. The state is per-process, which suits a relay that runs for one
/// session alongside one game.
fn airborne(physics: &[u8]) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEN_LOAD: AtomicBool = AtomicBool::new(false);

    if physics.len() < MIN_PHYSICS_AIRBORNE_LEN {
        return false;
    }
    let mut loaded = 0;
    for i in 0..4 {
        match f32_at(physics, OFF_WHEEL_LOAD + i * 4) {
            Some(v) if v.is_finite() && v > WHEEL_LOAD_EPSILON => loaded += 1,
            Some(v) if v.is_finite() => {}
            _ => return false,
        }
    }
    if loaded > 0 {
        SEEN_LOAD.store(true, Ordering::Relaxed);
        return false;
    }
    // Nothing loaded. Only airborne if this field has ever meant anything.
    SEEN_LOAD.load(Ordering::Relaxed)
}

/// Read throttle and gear from the physics head, which every Assetto Corsa
/// generation shares. Factored out so the two decoders cannot drift on the
/// gear translation, which is the field most easily got wrong.
fn head_inputs(physics: &[u8]) -> Option<(f32, i16)> {
    let throttle = f32_at(physics, OFF_GAS)?;
    let throttle = if throttle.is_finite() { throttle.clamp(0.0, 1.0) } else { 0.0 };
    // Assetto Corsa numbers reverse 0, neutral 1, first 2. The relay wants
    // reverse -1, neutral 0, first 1, so every gear shifts down by one.
    let gear = match i32_at(physics, OFF_GEAR)? {
        g @ 0..=16 => (g - 1) as i16,
        _ => 0,
    };
    Some((throttle, gear))
}

/// Decode one read of Assetto Corsa EVO's physics section.
///
/// # Why EVO gets its own function
///
/// EVO shares this module because it shares the physics head and the gear
/// convention. It does not share the rest. Kunos renamed every section, and
/// they removed the car spec sheet from the static block, so the redline
/// that Assetto Corsa and Competizione read from `acpmf_static` is simply
/// not there. Its replacement is `currentMaxRpm`, in the physics block, and
/// republished every tick because in EVO it varies with engine state.
///
/// That leaves EVO with one section and no layout guard: no static block to
/// cross-check, no version field, no packed struct. `currentMaxRpm` sits at
/// offset 588, well past the stable head, so it is the one number here that
/// a layout change could move.
///
/// The defence is that a redline is a distinctive value. A wrong offset in a
/// physics block lands on a temperature, a pressure, a pedal position or a
/// slip ratio, and none of those reaches [`MIN_PLAUSIBLE_REDLINE`]. The
/// sample is dropped rather than sent, so a layout change makes EVO go quiet
/// instead of making the wheel behave strangely.
pub fn decode_evo(physics: &[u8]) -> Option<RelayTelemetry> {
    if physics.len() < MIN_PHYSICS_EVO_LEN {
        return None;
    }
    let rpm = i32_at(physics, OFF_RPMS)? as f32;
    let max_rpm = i32_at(physics, OFF_CURRENT_MAX_RPM)? as f32;
    if !(MIN_PLAUSIBLE_REDLINE..=MAX_PLAUSIBLE_RPM).contains(&max_rpm)
        || !(0.0..=MAX_PLAUSIBLE_RPM).contains(&rpm)
    {
        return None;
    }
    // A car does not turn half again its own redline. Reading well past it
    // means these two numbers did not come from one engine, which is what a
    // moved offset looks like from here.
    if rpm > max_rpm * 1.5 {
        return None;
    }
    let (throttle, gear) = head_inputs(physics)?;
    Some(RelayTelemetry { game_id: ID_EVO, rpm, max_rpm, throttle, gear, airborne: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn physics(gas: f32, gear: i32, rpms: i32) -> Vec<u8> {
        let mut b = vec![0u8; MIN_PHYSICS_LEN];
        b[OFF_GAS..][..4].copy_from_slice(&gas.to_le_bytes());
        b[OFF_GEAR..][..4].copy_from_slice(&gear.to_le_bytes());
        b[OFF_RPMS..][..4].copy_from_slice(&rpms.to_le_bytes());
        b
    }

    /// A static block with a believable UTF-16 `smVersion` of "1.7".
    fn statics(max_rpm: i32) -> Vec<u8> {
        let mut b = vec![0u8; MIN_STATIC_LEN];
        for (i, c) in "1.7".encode_utf16().enumerate() {
            b[i * 2..][..2].copy_from_slice(&c.to_le_bytes());
        }
        b[OFF_MAX_RPM..][..4].copy_from_slice(&max_rpm.to_le_bytes());
        b
    }

    /// These numbers are the whole decoder. `maxRpm` in particular sits
    /// behind five wchar_t[33] fields, so it is the one most easily moved by
    /// a careless edit.
    #[test]
    fn offsets_match_the_documented_layout() {
        assert_eq!(OFF_GAS, 4);
        assert_eq!(OFF_GEAR, 16);
        assert_eq!(OFF_RPMS, 20);
        assert_eq!(OFF_MAX_RPM, 412);
        assert_eq!(SECTION_PHYSICS, "Local\\acpmf_physics");
        assert_eq!(SECTION_STATIC, "Local\\acpmf_static");
    }

    /// An EVO physics block: same head, plus `currentMaxRpm` where EVO put
    /// it instead of in a static block.
    fn physics_evo(gas: f32, gear: i32, rpms: i32, current_max_rpm: i32) -> Vec<u8> {
        let mut b = vec![0u8; MIN_PHYSICS_EVO_LEN];
        b[OFF_GAS..][..4].copy_from_slice(&gas.to_le_bytes());
        b[OFF_GEAR..][..4].copy_from_slice(&gear.to_le_bytes());
        b[OFF_RPMS..][..4].copy_from_slice(&rpms.to_le_bytes());
        b[OFF_CURRENT_MAX_RPM..][..4].copy_from_slice(&current_max_rpm.to_le_bytes());
        b
    }

    #[test]
    fn evo_offsets_match_the_documented_layout() {
        assert_eq!(OFF_CURRENT_MAX_RPM, 588);
        assert_eq!(SECTION_PHYSICS_EVO, "Local\\acevo_pmf_physics");
        assert_ne!(SECTION_PHYSICS_EVO, SECTION_PHYSICS, "EVO renamed its sections");
    }

    #[test]
    fn evo_decodes_a_running_session_from_one_section() {
        let s = decode_evo(&physics_evo(0.8, 4, 6800, 8400)).expect("valid EVO block");
        assert_eq!(s.game_id, "ac-evo");
        assert_eq!(s.rpm, 6800.0);
        assert_eq!(s.max_rpm, 8400.0, "redline comes from the physics block, not a static one");
        assert_eq!(s.gear, 3, "EVO shares the 0=R, 1=N gear convention");
        assert!((s.throttle - 0.8).abs() < 1e-6);
    }

    /// EVO has no static block to cross-check and no version field, so the
    /// redline's own implausibility is the only thing standing between a
    /// moved offset and a wrong number reaching the wheel. A wrong offset in
    /// a physics block lands on a temperature, a pressure or a pedal.
    #[test]
    fn evo_refuses_a_redline_no_engine_would_report() {
        for wrong in [0, 1, 85, 200, 999] {
            assert!(
                decode_evo(&physics_evo(0.5, 3, 6000, wrong)).is_none(),
                "{wrong} is not a redline and must not be used as one"
            );
        }
        assert!(decode_evo(&physics_evo(0.5, 3, 6000, 7000)).is_some(), "7000 is a redline");
    }

    /// Reading far past the redline means the two numbers did not come from
    /// one engine, which is what a moved offset looks like from here.
    #[test]
    fn evo_refuses_revs_that_cannot_belong_to_that_redline() {
        assert!(decode_evo(&physics_evo(0.5, 3, 20_000, 7000)).is_none());
        // A touch over the limiter is ordinary and must still pass.
        assert!(decode_evo(&physics_evo(0.5, 3, 7200, 7000)).is_some());
    }

    #[test]
    fn evo_refuses_short_buffers_and_dead_sessions() {
        let full = physics_evo(0.5, 3, 6000, 7000);
        assert!(decode_evo(&full[..MIN_PHYSICS_EVO_LEN - 1]).is_none());
        assert!(decode_evo(&[]).is_none());
        // A block long enough for the older games' fields but not EVO's
        // redline must not be decoded on the strength of the head alone.
        assert!(decode_evo(&full[..MIN_PHYSICS_LEN]).is_none());
        assert!(decode_evo(&physics_evo(0.0, 0, 0, 0)).is_none(), "menu");
    }

    /// The gear translation is shared with the older games precisely so it
    /// cannot drift; this is what says so.
    #[test]
    fn evo_and_assetto_corsa_translate_gears_identically() {
        for g in 0..=8 {
            let ac = decode(&physics(0.5, g, 6000), &statics(7500), ID).unwrap();
            let evo = decode_evo(&physics_evo(0.5, g, 6000, 7500)).unwrap();
            assert_eq!(ac.gear, evo.gear, "gear {g} must mean the same in both");
        }
    }

    /// Competizione is read by this decoder on the claim that its physics
    /// and static blocks are byte-identical to Assetto Corsa's through every
    /// field used here. The claim is what makes the reuse legitimate, so the
    /// id is the only thing allowed to differ between the two.
    #[test]
    fn competizione_decodes_identically_and_differs_only_in_id() {
        let p = physics(0.75, 3, 6200);
        let s = statics(7500);
        let ac = decode(&p, &s, ID).unwrap();
        let acc = decode(&p, &s, ID_ACC).unwrap();
        assert_eq!(acc.game_id, "acc");
        assert_ne!(ac.game_id, acc.game_id, "separate games, separate switches");
        assert_eq!((ac.rpm, ac.max_rpm, ac.gear), (acc.rpm, acc.max_rpm, acc.gear));
        assert_eq!(ac.throttle, acc.throttle);
    }

    #[test]
    fn decodes_a_running_session() {
        let s = decode(&physics(0.75, 3, 6200), &statics(7500), ID).expect("valid buffers");
        assert_eq!(s.game_id, ID);
        assert_eq!(s.rpm, 6200.0);
        assert_eq!(s.max_rpm, 7500.0);
        assert!((s.throttle - 0.75).abs() < 1e-6);
    }

    /// The trap in this format. Assetto Corsa's 2 is first gear, not second,
    /// and an untranslated read is wrong by exactly one the whole way up.
    #[test]
    fn gears_shift_down_by_one_from_assetto_corsas_numbering() {
        let cases = [(0, -1), (1, 0), (2, 1), (3, 2), (8, 7)];
        for (ac, expected) in cases {
            let s = decode(&physics(0.5, ac, 6000), &statics(7500), ID).unwrap();
            assert_eq!(s.gear, expected, "AC gear {ac} should relay as {expected}");
        }
    }

    /// The static block's layout guard. Anything that is not a UTF-16
    /// version string at offset 0 means `maxRpm` is not at 412 either.
    #[test]
    fn a_static_block_that_is_not_utf16_is_refused() {
        let mut wrong = statics(7500);
        // UTF-32-shaped: '1', 0, 0, 0. The second 16-bit unit reads zero.
        wrong[2] = 0;
        wrong[3] = 0;
        assert!(decode(&physics(0.5, 3, 6000), &wrong, ID).is_none());

        let mut zeroed = statics(7500);
        zeroed[0..4].fill(0);
        assert!(decode(&physics(0.5, 3, 6000), &zeroed, ID).is_none(), "unwritten block");
    }

    #[test]
    fn short_buffers_are_refused_rather_than_read_past() {
        let p = physics(0.5, 3, 6000);
        let s = statics(7500);
        assert!(decode(&p[..MIN_PHYSICS_LEN - 1], &s, ID).is_none());
        assert!(decode(&p, &s[..MIN_STATIC_LEN - 1], ID).is_none());
        assert!(decode(&[], &[], ID).is_none());
    }

    /// Menus leave the engine fields zeroed, and a car with no redline
    /// gives an engine note nothing to scale against.
    #[test]
    fn a_session_that_is_not_running_yields_nothing() {
        assert!(decode(&physics(0.0, 0, 0), &statics(0), ID).is_none(), "no redline");
        assert!(decode(&physics(0.0, 0, 0), &statics(7500), ID).is_some(), "idle is still a session");
    }

    #[test]
    fn implausible_engine_values_are_refused() {
        assert!(decode(&physics(0.5, 3, -100), &statics(7500), ID).is_none());
        assert!(decode(&physics(0.5, 3, 90_000), &statics(7500), ID).is_none());
        assert!(decode(&physics(0.5, 3, 6000), &statics(90_000), ID).is_none());
    }

    #[test]
    fn a_non_finite_or_out_of_range_throttle_is_tamed() {
        assert_eq!(decode(&physics(f32::NAN, 3, 6000), &statics(7500), ID).unwrap().throttle, 0.0);
        assert_eq!(decode(&physics(5.0, 3, 6000), &statics(7500), ID).unwrap().throttle, 1.0);
        assert_eq!(decode(&physics(-5.0, 3, 6000), &statics(7500), ID).unwrap().throttle, 0.0);
    }
}

#[cfg(test)]
mod airborne_tests {
    use super::*;

    /// A physics block long enough to carry wheelLoad, with the four loads set.
    fn physics_with_loads(loads: [f32; 4]) -> Vec<u8> {
        let mut b = vec![0u8; MIN_PHYSICS_AIRBORNE_LEN];
        for (i, v) in loads.iter().enumerate() {
            b[OFF_WHEEL_LOAD + i * 4..OFF_WHEEL_LOAD + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// The failure this is built to make impossible.
    ///
    /// Assetto Corsa's docs say wheelLoad is unused in Competizione. If that
    /// is true the field reads zero forever, and a naive all-zero test would
    /// call the car airborne for the whole session, ducking the road surface
    /// and silencing haptics. Zeros alone must never be enough.
    #[test]
    fn a_field_that_never_carries_load_never_reports_airborne() {
        for _ in 0..50 {
            assert!(
                !airborne(&physics_with_loads([0.0; 4])),
                "zeros alone must not be read as flight",
            );
        }
    }

    #[test]
    fn a_short_block_is_not_airborne() {
        assert!(!airborne(&[0u8; MIN_PHYSICS_AIRBORNE_LEN - 1]));
    }

    #[test]
    fn nonsense_loads_are_not_airborne() {
        assert!(!airborne(&physics_with_loads([f32::NAN, 0.0, 0.0, 0.0])));
    }

    /// Ordering matters here and the test states it: load first, then flight.
    /// Run as one test because the gate is process-wide state and separate
    /// tests would race each other for it.
    #[test]
    fn flight_is_reported_only_after_the_field_has_proved_itself() {
        // On the ground: loaded, so not airborne, and the gate opens.
        assert!(!airborne(&physics_with_loads([3000.0, 3100.0, 2900.0, 3050.0])));
        // Now all four unloaded, with the field proven: airborne.
        assert!(airborne(&physics_with_loads([0.0; 4])));
        // A tyre barely touching is not flight.
        assert!(!airborne(&physics_with_loads([0.0, 0.0, 0.0, 40.0])));
    }
}
