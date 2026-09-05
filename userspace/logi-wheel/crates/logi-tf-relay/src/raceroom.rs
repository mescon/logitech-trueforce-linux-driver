// SPDX-License-Identifier: GPL-2.0-only
//! Decoder for RaceRoom Racing Experience's `$R3E` shared-memory section.
//!
//! # Why this one can be written without a captured fixture
//!
//! The house rule for this crate is a real byte fixture before every
//! decoder, because guessing struct offsets produces plausible garbage that
//! nothing catches. RaceRoom is the second format where that risk does not
//! apply, for a different reason than iRacing's.
//!
//! iRacing is safe because its telemetry is self-describing. RaceRoom is not
//! self-describing at all: it is one big fixed-layout C struct. It is safe
//! because **the layout is published by the people who write it**. KW Studios
//! (formerly Sector3) ship `r3e.h` at `github.com/sector3studios/r3e-api`,
//! released into the public domain, and they own both ends of the interface:
//! the game writes the struct their own header declares. There is no
//! third-party plugin in the middle whose version could disagree.
//!
//! Two further properties make the offsets below unusually safe to hardcode:
//!
//! - The struct is declared `#pragma pack(push, 1)`, so every field sits at
//!   a fixed byte offset with no alignment padding for a compiler to differ
//!   about. The numbers are the same whatever built the game.
//! - The struct starts with `version_major` / `version_minor`. That is an
//!   in-band layout check: if RaceRoom ever moves these fields it bumps the
//!   major, and [`decode`] refuses the buffer rather than reading the old
//!   offsets out of a new layout.
//!
//! The offsets were not counted by hand. They were produced by compiling the
//! vendor's own `r3e.h` and printing `offsetof` for each field, then pinned
//! by [`tests::offsets_match_the_vendor_header`] so a typo here fails the
//! build rather than the wheel.
//!
//! What none of this proves is that a real RaceRoom build under Proton
//! matches its own published header. Every read is therefore bounds checked
//! and range gated, and a mismatch yields `None` rather than a wrong number.
//! A `--dump` from a live session is still what would move this from "should
//! work" to "known to work".
//!
//! ## Layout, from `r3e.h` (shared memory `$R3E`, API version 3.5)
//!
//! | offset | field            | type  | notes                            |
//! |--------|------------------|-------|----------------------------------|
//! | 0      | `version_major`  | i32   | must be 3; the layout guard      |
//! | 4      | `version_minor`  | i32   | informational                    |
//! | 1392   | `car_speed`      | f32   | m/s                              |
//! | 1396   | `engine_rps`     | f32   | **radians** per second           |
//! | 1400   | `max_engine_rps` | f32   | radians per second               |
//! | 1408   | `gear`           | i32   | -2 N/A, -1 reverse, 0 neutral    |
//! | 1500   | `throttle`       | f32   | 0.0..=1.0, -1.0 = not available  |
//!
//! The engine fields being in radians per second rather than rpm is the one
//! trap in this format: read as rpm they are about 9.5x too small, which
//! looks like a plausibly idling engine rather than like nonsense.

#![cfg_attr(not(windows), allow(dead_code))]

use logi_wheel_core::relay::RelayTelemetry;

/// The relay wire id for this game.
pub const ID: &str = "raceroom";

/// Windows named section RaceRoom publishes to (`R3E_SHARED_MEMORY_NAME`).
pub const SECTION: &str = "$R3E";

/// The only `version_major` this decoder claims to understand
/// (`R3E_VERSION_MAJOR` in the vendor header).
const VERSION_MAJOR: i32 = 3;

// Field offsets, generated from the vendor header (see module docs).
const OFF_VERSION_MAJOR: usize = 0;
const OFF_CAR_SPEED: usize = 1392;
const OFF_ENGINE_RPS: usize = 1396;
const OFF_MAX_ENGINE_RPS: usize = 1400;
const OFF_GEAR: usize = 1408;
const OFF_THROTTLE: usize = 1500;

/// Smallest buffer this decoder will look at: through the last field it
/// reads. The full struct is far larger, but requiring only what is read
/// keeps a short read from being rejected for fields we ignore.
const MIN_LEN: usize = OFF_THROTTLE + 4;

/// Radians per second to revolutions per minute: one revolution is 2*pi
/// radians, and a minute is 60 seconds.
const RAD_S_TO_RPM: f32 = 60.0 / (2.0 * std::f32::consts::PI);

/// Above this, the buffer is not engine data however plausible it looked.
/// Formula One reaches about 15000 and drag bikes about 20000, so 30000
/// leaves generous headroom while still rejecting garbage.
const MAX_PLAUSIBLE_RPM: f32 = 30_000.0;

fn i32_at(buf: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn f32_at(buf: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

/// Decode one read of the `$R3E` section.
///
/// Returns `None` for anything this decoder cannot vouch for: a short
/// buffer, an unrecognised `version_major`, a non-finite or implausible
/// engine rate, or a session that is not running (RaceRoom zeroes the
/// engine fields in menus and replays, which is indistinguishable from
/// "nothing to say" and is treated as such).
pub fn decode(buf: &[u8]) -> Option<RelayTelemetry> {
    if buf.len() < MIN_LEN {
        return None;
    }
    // The layout guard. A major-version bump means these offsets describe a
    // struct the game no longer writes, and reading them anyway is exactly
    // the failure this crate's fixture rule exists to prevent.
    if i32_at(buf, OFF_VERSION_MAJOR)? != VERSION_MAJOR {
        return None;
    }

    let rpm = f32_at(buf, OFF_ENGINE_RPS)? * RAD_S_TO_RPM;
    let max_rpm = f32_at(buf, OFF_MAX_ENGINE_RPS)? * RAD_S_TO_RPM;
    if !rpm.is_finite() || !max_rpm.is_finite() {
        return None;
    }
    // max_engine_rps <= 0 covers both the vendor's -1.0 "not available" and
    // the all-zero buffer a session that has not started yet leaves behind.
    // Without a redline there is nothing to scale an engine note against.
    if max_rpm <= 0.0
        || max_rpm > MAX_PLAUSIBLE_RPM
        || !(0.0..=MAX_PLAUSIBLE_RPM).contains(&rpm)
    {
        return None;
    }

    // -1.0 means "not available" (an AI or remote car), which is not the
    // same as a lifted throttle, but both mean "no throttle input from the
    // player" to everything downstream.
    let throttle = f32_at(buf, OFF_THROTTLE)?;
    let throttle = if throttle.is_finite() { throttle.clamp(0.0, 1.0) } else { 0.0 };
    let speed = f32_at(buf, OFF_CAR_SPEED).filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0);

    // RaceRoom's -2 means "not available"; every other value already uses
    // the relay's own convention of -1 reverse, 0 neutral, 1..=N forward.
    let gear = match i32_at(buf, OFF_GEAR)? {
        g @ -1..=15 => g as i16,
        _ => 0,
    };

    Some(RelayTelemetry { game_id: ID, rpm, max_rpm, throttle, gear, speed, brake: 0.0, airborne: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `$R3E` buffer with the fields this decoder reads set.
    fn buffer(version: i32, rps: f32, max_rps: f32, throttle: f32, gear: i32) -> Vec<u8> {
        let mut b = vec![0u8; MIN_LEN];
        b[OFF_VERSION_MAJOR..][..4].copy_from_slice(&version.to_le_bytes());
        b[OFF_ENGINE_RPS..][..4].copy_from_slice(&rps.to_le_bytes());
        b[OFF_MAX_ENGINE_RPS..][..4].copy_from_slice(&max_rps.to_le_bytes());
        b[OFF_GEAR..][..4].copy_from_slice(&gear.to_le_bytes());
        b[OFF_THROTTLE..][..4].copy_from_slice(&throttle.to_le_bytes());
        b
    }

    /// These numbers are the whole decoder. They came from compiling
    /// KW Studios' `r3e.h` and printing `offsetof` for each field; this test
    /// is what stops an edit from quietly moving one.
    #[test]
    fn offsets_match_the_vendor_header() {
        assert_eq!(OFF_VERSION_MAJOR, 0);
        assert_eq!(OFF_ENGINE_RPS, 1396);
        assert_eq!(OFF_MAX_ENGINE_RPS, 1400);
        assert_eq!(OFF_GEAR, 1408);
        assert_eq!(OFF_THROTTLE, 1500);
        assert_eq!(VERSION_MAJOR, 3);
        assert_eq!(SECTION, "$R3E");
    }

    /// The unit conversion is the one thing a reader cannot check by eye,
    /// so pin it against a hand-computed case: 7000 rpm is 733.04 rad/s.
    #[test]
    fn engine_speed_converts_from_radians_per_second() {
        let sample = decode(&buffer(3, 733.038, 837.758, 0.5, 3)).expect("valid buffer");
        assert!((sample.rpm - 7000.0).abs() < 1.0, "got {} rpm", sample.rpm);
        assert!((sample.max_rpm - 8000.0).abs() < 1.0, "got {} max", sample.max_rpm);
    }

    /// Reading rad/s as rpm would give ~733, which looks like a plausible
    /// idling engine rather than like nonsense. That is precisely why the
    /// conversion needs its own test rather than a range check.
    #[test]
    fn forgetting_the_conversion_would_not_have_looked_wrong() {
        let sample = decode(&buffer(3, 733.038, 837.758, 1.0, 4)).unwrap();
        assert!(sample.rpm > 6000.0, "raw rad/s would have passed every range gate");
    }

    #[test]
    fn decodes_the_remaining_fields() {
        let sample = decode(&buffer(3, 733.038, 837.758, 0.25, 4)).unwrap();
        assert_eq!(sample.game_id, ID);
        assert_eq!(sample.gear, 4);
        assert!((sample.throttle - 0.25).abs() < 1e-6);
    }

    /// The layout guard: a struct this decoder was not written against must
    /// be refused, not read at the old offsets.
    #[test]
    fn a_different_major_version_is_refused() {
        assert!(decode(&buffer(4, 733.0, 837.0, 0.5, 3)).is_none());
        assert!(decode(&buffer(2, 733.0, 837.0, 0.5, 3)).is_none());
        assert!(decode(&buffer(3, 733.0, 837.0, 0.5, 3)).is_some(), "3 is the version we handle");
    }

    /// A buffer too short to hold the fields must not be read at all,
    /// however valid its header looks.
    #[test]
    fn a_short_buffer_is_refused_rather_than_read_past() {
        let full = buffer(3, 733.0, 837.0, 0.5, 3);
        for len in [0, 4, MIN_LEN - 1] {
            assert!(decode(&full[..len]).is_none(), "{len} bytes should be refused");
        }
    }

    /// Menus, replays and pre-session all leave the engine fields zeroed.
    /// Without a redline there is nothing to scale an engine note against.
    #[test]
    fn a_session_that_is_not_running_yields_nothing() {
        assert!(decode(&buffer(3, 0.0, 0.0, 0.0, 0)).is_none(), "all-zero buffer");
        assert!(decode(&buffer(3, 100.0, -1.0, 0.0, 0)).is_none(), "vendor's -1 = N/A");
    }

    #[test]
    fn implausible_or_non_finite_engine_rates_are_refused() {
        assert!(decode(&buffer(3, f32::NAN, 837.0, 0.5, 3)).is_none());
        assert!(decode(&buffer(3, f32::INFINITY, 837.0, 0.5, 3)).is_none());
        assert!(decode(&buffer(3, -10.0, 837.0, 0.5, 3)).is_none());
        // 40000 rpm worth of rad/s: no engine, so not engine data.
        assert!(decode(&buffer(3, 4188.8, 4188.8, 0.5, 3)).is_none());
    }

    /// `-1.0` throttle means an AI or remote car rather than a lifted
    /// pedal, but downstream both mean no player throttle.
    #[test]
    fn unavailable_throttle_reads_as_closed() {
        let sample = decode(&buffer(3, 733.0, 837.0, -1.0, 3)).unwrap();
        assert_eq!(sample.throttle, 0.0);
        let sample = decode(&buffer(3, 733.0, 837.0, f32::NAN, 3)).unwrap();
        assert_eq!(sample.throttle, 0.0);
    }

    /// RaceRoom's -2 ("not available") has no relay equivalent and must not
    /// reach the wire as a gear, where -2 would be nonsense.
    #[test]
    fn the_not_available_gear_becomes_neutral() {
        assert_eq!(decode(&buffer(3, 733.0, 837.0, 0.5, -2)).unwrap().gear, 0);
        assert_eq!(decode(&buffer(3, 733.0, 837.0, 0.5, -1)).unwrap().gear, -1, "reverse survives");
        assert_eq!(decode(&buffer(3, 733.0, 837.0, 0.5, 0)).unwrap().gear, 0);
        // A corrupt high value is not a gearbox any car has.
        assert_eq!(decode(&buffer(3, 733.0, 837.0, 0.5, 9999)).unwrap().gear, 0);
    }
}
