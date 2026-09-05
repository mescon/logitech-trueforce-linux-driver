// SPDX-License-Identifier: GPL-2.0-only
//! Decoder for rFactor 2 and Le Mans Ultimate, via the community
//! `rF2SharedMemoryMapPlugin`.
//!
//! # Why this one was the last to be written
//!
//! The other shared-memory decoders here read a layout somebody guarantees:
//! iRacing describes itself, RaceRoom's is published by KW Studios, Assetto
//! Corsa's needed fields sit in a part of its struct that has not moved.
//! This one has none of that. The game exposes its internals to a plugin,
//! and a *community* plugin copies them into shared memory, so the bytes on
//! the other end depend on which fork and which build the user installed.
//! That is why this crate's rule is a captured fixture first, and why this
//! decoder came after the others rather than instead of them.
//!
//! What makes it writable anyway is that the format carries enough in-band
//! state to tell a good read from a bad one, which is the property the rule
//! was really protecting:
//!
//! - **Torn reads are detectable.** Each mapped buffer is preceded by an
//!   8-byte version block: the plugin increments `mVersionUpdateBegin`
//!   before writing and `mVersionUpdateEnd` after. Unequal means the read
//!   caught a write in progress, and [`decode`] drops the sample.
//! - **The layout is mirrored, not invented.** The plugin's structs carry
//!   `static_assert(sizeof(rF2VehicleTelemetry) == sizeof(TelemInfoV01))`
//!   against Studio 397's own `InternalsPlugin.hpp`, so the offsets below
//!   are the game's own, not a third party's reinterpretation of them.
//! - **A wrong layout does not read as a plausible engine.** Every value is
//!   range gated, and the player's car is found by matching an id between
//!   two independently written buffers, which a misaligned read fails.
//!
//! The offsets were produced by compiling the plugin's headers with a
//! Windows toolchain, not counted by hand. Compiling them on Linux would
//! have given wrong answers: `long` is 4 bytes on Windows and 8 here, and
//! this struct is full of them.
//!
//! # Finding the player's car
//!
//! Both buffers are arrays of every car in the session, and the player is
//! not reliably index 0. Only the *scoring* buffer says which car is the
//! player (`mIsPlayer`); only the *telemetry* buffer has engine speed. So
//! the two are read together: scoring gives the player's slot id, and that
//! id is looked up in telemetry. Driving the engine note off the wrong row
//! would mean feeling somebody else's gearbox, which is the kind of wrong
//! that feels like a bug in the effect rather than in the decoder.
//!
//! ## Layout (`$rFactor2SMMP_Telemetry$`)
//!
//! | offset | field                | type | notes                        |
//! |--------|----------------------|------|------------------------------|
//! | 0      | `mVersionUpdateBegin`| u32  | torn-read guard              |
//! | 4      | `mVersionUpdateEnd`  | u32  | must equal the above         |
//! | 12     | `mNumVehicles`       | i32  | cars with telemetry          |
//! | 16     | `mVehicles[0]`       |      | stride 1888                  |
//!
//! Within one vehicle: `mID` +0 (i32), `mGear` +352 (i32), `mEngineRPM`
//! +356 (f64), `mUnfilteredThrottle` +388 (f64), `mEngineMaxRPM` +532 (f64).
//!
//! ## Layout (`$rFactor2SMMP_Scoring$`)
//!
//! | offset | field          | type | notes                              |
//! |--------|----------------|------|------------------------------------|
//! | 0/4    | version block  | u32  | as above                           |
//! | 116    | `mNumVehicles` | i32  | cars in the session                |
//! | 560    | `mVehicles[0]` |      | stride 584                         |
//!
//! Within one scoring entry: `mID` +0 (i32), `mIsPlayer` +196 (u8).

#![cfg_attr(not(windows), allow(dead_code))]

use logi_wheel_core::relay::RelayTelemetry;

/// Relay wire id for rFactor 2.
pub const ID_RF2: &str = "rf2";

/// Relay wire id for Le Mans Ultimate. Same engine and same plugin, but a
/// separate id so the two get separate enable switches and intensities.
pub const ID_LMU: &str = "lmu";

/// Section carrying per-car telemetry.
pub const SECTION_TELEMETRY: &str = "$rFactor2SMMP_Telemetry$";

/// Section carrying per-car scoring, which is where `mIsPlayer` lives.
pub const SECTION_SCORING: &str = "$rFactor2SMMP_Scoring$";

/// Full size of the telemetry mapping, version block included.
pub const TELEMETRY_LEN: usize = 241_680;

/// Full size of the scoring mapping, version block included.
pub const SCORING_LEN: usize = 75_312;

/// Cars either buffer can describe (`MAX_MAPPED_VEHICLES`).
const MAX_VEHICLES: usize = 128;

// Telemetry buffer (file offsets, version block included).
const TEL_NUM_VEHICLES: usize = 12;
const TEL_VEHICLES: usize = 16;
const TEL_STRIDE: usize = 1888;
const VEH_ID: usize = 0;
const VEH_LOCAL_VEL: usize = 184;
const VEH_GEAR: usize = 352;
const VEH_ENGINE_RPM: usize = 356;
const VEH_THROTTLE: usize = 388;
const VEH_BRAKE: usize = 396;
const VEH_ENGINE_MAX_RPM: usize = 532;

// Scoring buffer (file offsets, version block included).
const SCO_NUM_VEHICLES: usize = 116;
const SCO_VEHICLES: usize = 560;
const SCO_STRIDE: usize = 584;
const SCO_ID: usize = 0;
const SCO_IS_PLAYER: usize = 196;

/// Above this, the buffer is not engine data however plausible it looked.
const MAX_PLAUSIBLE_RPM: f64 = 30_000.0;

fn u32_at(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn i32_at(buf: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn f64_at(buf: &[u8], off: usize) -> Option<f64> {
    Some(f64::from_le_bytes(buf.get(off..off + 8)?.try_into().ok()?))
}

/// Whether this buffer was read cleanly, rather than mid-write.
///
/// The plugin bumps `mVersionUpdateBegin` before writing and
/// `mVersionUpdateEnd` after, so equal counters mean no write was in flight
/// across the read. This is the one guarantee the format gives for free,
/// and it is why a fixed-layout community format is decodable at all.
fn read_was_clean(buf: &[u8]) -> bool {
    match (u32_at(buf, 0), u32_at(buf, 4)) {
        (Some(begin), Some(end)) => begin == end,
        _ => false,
    }
}

/// How many entries a buffer claims, clamped to what it can actually hold.
fn vehicle_count(buf: &[u8], count_off: usize) -> usize {
    let claimed = i32_at(buf, count_off).unwrap_or(0);
    (claimed.max(0) as usize).min(MAX_VEHICLES)
}

/// The slot id of the player's car, from the scoring buffer.
fn player_slot_id(scoring: &[u8]) -> Option<i32> {
    let n = vehicle_count(scoring, SCO_NUM_VEHICLES);
    (0..n).find_map(|i| {
        let base = SCO_VEHICLES + i * SCO_STRIDE;
        // A C++ `bool` is one byte; anything other than 0 or 1 in it means
        // this is not the field the offsets claim, so it is not trusted.
        match scoring.get(base + SCO_IS_PLAYER) {
            Some(1) => i32_at(scoring, base + SCO_ID),
            _ => None,
        }
    })
}

/// Decode one paired read of the telemetry and scoring sections.
///
/// `game_id` selects which title's settings the sample is gated by; the two
/// share an engine, a plugin and a decoder, but not a switch.
///
/// Returns `None` for a torn read of either buffer, a session with no
/// player car, a player whose id has no telemetry row yet (normal for the
/// first ticks after joining), or engine values outside what an engine does.
pub fn decode(telemetry: &[u8], scoring: &[u8], game_id: &'static str) -> Option<RelayTelemetry> {
    if !read_was_clean(telemetry) || !read_was_clean(scoring) {
        return None;
    }
    let player_id = player_slot_id(scoring)?;

    let n = vehicle_count(telemetry, TEL_NUM_VEHICLES);
    let base = (0..n)
        .map(|i| TEL_VEHICLES + i * TEL_STRIDE)
        .find(|base| i32_at(telemetry, base + VEH_ID) == Some(player_id))?;

    let rpm = f64_at(telemetry, base + VEH_ENGINE_RPM)?;
    let max_rpm = f64_at(telemetry, base + VEH_ENGINE_MAX_RPM)?;
    if !rpm.is_finite() || !max_rpm.is_finite() {
        return None;
    }
    if max_rpm <= 0.0
        || max_rpm > MAX_PLAUSIBLE_RPM
        || !(0.0..=MAX_PLAUSIBLE_RPM).contains(&rpm)
    {
        return None;
    }

    let throttle = f64_at(telemetry, base + VEH_THROTTLE)?;
    let throttle = if throttle.is_finite() { throttle.clamp(0.0, 1.0) as f32 } else { 0.0 };
    // For the base's screen: the car's speed is the length of its local
    // velocity vector, and the brake is the unfiltered pedal. Zeros if
    // either reads as nonsense, never a dropped sample.
    let speed = (0..3)
        .map(|i| f64_at(telemetry, base + VEH_LOCAL_VEL + i * 8).unwrap_or(0.0))
        .map(|v| if v.is_finite() { v * v } else { 0.0 })
        .sum::<f64>()
        .sqrt() as f32;
    let brake = f64_at(telemetry, base + VEH_BRAKE).unwrap_or(0.0);
    let brake = if brake.is_finite() { brake.clamp(0.0, 1.0) as f32 } else { 0.0 };

    // rFactor 2 already uses the relay's convention: -1 reverse, 0 neutral,
    // 1..=N forward. Only an implausible value is squashed.
    let gear = match i32_at(telemetry, base + VEH_GEAR)? {
        g @ -1..=15 => g as i16,
        _ => 0,
    };

    Some(RelayTelemetry { game_id, rpm: rpm as f32, max_rpm: max_rpm as f32, throttle, gear, speed, brake, airborne: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One car's telemetry row.
    struct Car {
        id: i32,
        rpm: f64,
        max_rpm: f64,
        throttle: f64,
        gear: i32,
    }

    fn telemetry(cars: &[Car], clean: bool) -> Vec<u8> {
        let mut b = vec![0u8; TELEMETRY_LEN];
        b[0..4].copy_from_slice(&1u32.to_le_bytes());
        b[4..8].copy_from_slice(&if clean { 1u32 } else { 2u32 }.to_le_bytes());
        b[TEL_NUM_VEHICLES..][..4].copy_from_slice(&(cars.len() as i32).to_le_bytes());
        for (i, c) in cars.iter().enumerate() {
            let base = TEL_VEHICLES + i * TEL_STRIDE;
            b[base + VEH_ID..][..4].copy_from_slice(&c.id.to_le_bytes());
            b[base + VEH_GEAR..][..4].copy_from_slice(&c.gear.to_le_bytes());
            b[base + VEH_ENGINE_RPM..][..8].copy_from_slice(&c.rpm.to_le_bytes());
            b[base + VEH_THROTTLE..][..8].copy_from_slice(&c.throttle.to_le_bytes());
            b[base + VEH_ENGINE_MAX_RPM..][..8].copy_from_slice(&c.max_rpm.to_le_bytes());
        }
        b
    }

    /// `players` gives each car's id and whether it is the player.
    fn scoring(players: &[(i32, bool)], clean: bool) -> Vec<u8> {
        let mut b = vec![0u8; SCORING_LEN];
        b[0..4].copy_from_slice(&7u32.to_le_bytes());
        b[4..8].copy_from_slice(&if clean { 7u32 } else { 8u32 }.to_le_bytes());
        b[SCO_NUM_VEHICLES..][..4].copy_from_slice(&(players.len() as i32).to_le_bytes());
        for (i, (id, is_player)) in players.iter().enumerate() {
            let base = SCO_VEHICLES + i * SCO_STRIDE;
            b[base + SCO_ID..][..4].copy_from_slice(&id.to_le_bytes());
            b[base + SCO_IS_PLAYER] = u8::from(*is_player);
        }
        b
    }

    fn one_car() -> Vec<Car> {
        vec![Car { id: 42, rpm: 6400.0, max_rpm: 8200.0, throttle: 0.6, gear: 4 }]
    }

    /// These numbers are the whole decoder, and they came from compiling the
    /// plugin's headers with a Windows toolchain. Compiled on Linux they
    /// would differ: this struct is full of `long`, which is 4 bytes on
    /// Windows and 8 here.
    #[test]
    fn offsets_match_the_windows_layout() {
        assert_eq!(TEL_VEHICLES, 16);
        assert_eq!(TEL_STRIDE, 1888);
        assert_eq!(VEH_GEAR, 352);
        assert_eq!(VEH_ENGINE_RPM, 356);
        assert_eq!(VEH_THROTTLE, 388);
        assert_eq!(VEH_ENGINE_MAX_RPM, 532);
        assert_eq!(SCO_VEHICLES, 560);
        assert_eq!(SCO_STRIDE, 584);
        assert_eq!(SCO_IS_PLAYER, 196);
        // The mapping sizes follow from the strides, so a bad stride shows
        // up here too.
        assert_eq!(TELEMETRY_LEN, 8 + 8 + MAX_VEHICLES * TEL_STRIDE);
        assert_eq!(SCORING_LEN, SCO_VEHICLES + MAX_VEHICLES * SCO_STRIDE);
    }

    #[test]
    fn decodes_the_player_in_a_single_car_session() {
        let s = decode(&telemetry(&one_car(), true), &scoring(&[(42, true)], true), ID_RF2)
            .expect("a clean read of a live session");
        assert_eq!(s.game_id, ID_RF2);
        assert_eq!(s.rpm, 6400.0);
        assert_eq!(s.max_rpm, 8200.0);
        assert_eq!(s.gear, 4);
        assert!((s.throttle - 0.6).abs() < 1e-6);
    }

    /// The reason scoring is read at all. The player is not index 0 here,
    /// and taking index 0 would drive the engine note from another car.
    #[test]
    fn the_player_is_found_by_id_not_by_position() {
        let cars = vec![
            Car { id: 7, rpm: 3000.0, max_rpm: 9000.0, throttle: 0.1, gear: 2 },
            Car { id: 99, rpm: 1200.0, max_rpm: 7000.0, throttle: 0.0, gear: 1 },
            Car { id: 42, rpm: 6400.0, max_rpm: 8200.0, throttle: 0.6, gear: 4 },
        ];
        let sco = scoring(&[(7, false), (99, false), (42, true)], true);
        let s = decode(&telemetry(&cars, true), &sco, ID_RF2).unwrap();
        assert_eq!(s.rpm, 6400.0, "took another car's engine speed");
        assert_eq!(s.gear, 4);
    }

    /// Telemetry rows are not in scoring's order, and the id is what ties
    /// them together.
    #[test]
    fn the_two_buffers_need_not_agree_on_ordering() {
        let cars = vec![
            Car { id: 42, rpm: 6400.0, max_rpm: 8200.0, throttle: 0.6, gear: 4 },
            Car { id: 7, rpm: 3000.0, max_rpm: 9000.0, throttle: 0.1, gear: 2 },
        ];
        let sco = scoring(&[(7, false), (42, true)], true);
        assert_eq!(decode(&telemetry(&cars, true), &sco, ID_RF2).unwrap().rpm, 6400.0);
    }

    /// A read that caught the plugin mid-write is thrown away rather than
    /// partly believed. This is the guarantee that makes the format usable.
    #[test]
    fn a_torn_read_of_either_buffer_is_dropped() {
        let good_t = telemetry(&one_car(), true);
        let good_s = scoring(&[(42, true)], true);
        assert!(decode(&telemetry(&one_car(), false), &good_s, ID_RF2).is_none(), "torn telemetry");
        assert!(decode(&good_t, &scoring(&[(42, true)], false), ID_RF2).is_none(), "torn scoring");
        assert!(decode(&good_t, &good_s, ID_RF2).is_some(), "both clean");
    }

    /// Sitting in the monitor with no car, or the first ticks after joining
    /// before telemetry catches up, are both normal and both silent.
    #[test]
    fn no_player_or_no_matching_row_yields_nothing() {
        let t = telemetry(&one_car(), true);
        assert!(decode(&t, &scoring(&[(42, false)], true), ID_RF2).is_none(), "nobody is player");
        assert!(decode(&t, &scoring(&[], true), ID_RF2).is_none(), "empty session");
        assert!(decode(&t, &scoring(&[(1234, true)], true), ID_RF2).is_none(), "no telemetry row");
    }

    /// A count the buffer cannot back must not walk off the end, and a
    /// negative one must not become a huge unsigned length.
    #[test]
    fn a_lying_vehicle_count_cannot_read_past_the_buffer() {
        let mut t = telemetry(&one_car(), true);
        t[TEL_NUM_VEHICLES..][..4].copy_from_slice(&9999i32.to_le_bytes());
        assert!(decode(&t, &scoring(&[(42, true)], true), ID_RF2).is_some(), "clamped, not crashed");

        let mut s = scoring(&[(42, true)], true);
        s[SCO_NUM_VEHICLES..][..4].copy_from_slice(&(-5i32).to_le_bytes());
        assert!(decode(&telemetry(&one_car(), true), &s, ID_RF2).is_none());
    }

    /// A truncated mapping must be refused rather than read past.
    #[test]
    fn short_buffers_are_refused() {
        let t = telemetry(&one_car(), true);
        let s = scoring(&[(42, true)], true);
        assert!(decode(&t[..100], &s, ID_RF2).is_none());
        assert!(decode(&t, &s[..100], ID_RF2).is_none());
        assert!(decode(&[], &[], ID_RF2).is_none());
    }

    /// `mIsPlayer` is a C++ bool: one byte holding 0 or 1. Anything else
    /// means these offsets do not describe this buffer.
    #[test]
    fn a_non_boolean_is_player_byte_is_not_trusted() {
        let mut s = scoring(&[(42, true)], true);
        s[SCO_VEHICLES + SCO_IS_PLAYER] = 0xff;
        assert!(decode(&telemetry(&one_car(), true), &s, ID_RF2).is_none());
    }

    #[test]
    fn implausible_engine_values_are_refused() {
        let sco = scoring(&[(42, true)], true);
        let bad = |rpm, max_rpm| {
            let cars = vec![Car { id: 42, rpm, max_rpm, throttle: 0.5, gear: 3 }];
            decode(&telemetry(&cars, true), &sco, ID_RF2)
        };
        assert!(bad(6400.0, 0.0).is_none(), "no redline");
        assert!(bad(-1.0, 8200.0).is_none());
        assert!(bad(90_000.0, 8200.0).is_none());
        assert!(bad(f64::NAN, 8200.0).is_none());
        assert!(bad(6400.0, f64::INFINITY).is_none());
    }

    /// The two titles share every byte of this decoder but must not share a
    /// settings switch.
    #[test]
    fn the_game_id_is_the_callers_choice() {
        let t = telemetry(&one_car(), true);
        let s = scoring(&[(42, true)], true);
        assert_eq!(decode(&t, &s, ID_LMU).unwrap().game_id, "lmu");
        assert_eq!(decode(&t, &s, ID_RF2).unwrap().game_id, "rf2");
    }
}
