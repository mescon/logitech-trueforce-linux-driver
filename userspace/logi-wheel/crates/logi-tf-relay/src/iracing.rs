// SPDX-License-Identifier: GPL-2.0-only
//! Decoder for iRacing's `Local\IRSDKMemMapFileName` shared-memory section.
//!
//! # Why this one can be written without a captured fixture
//!
//! The house rule for this crate is a real byte fixture before every
//! decoder, because guessing struct offsets produces plausible garbage that
//! nothing catches. iRacing is the one format where that risk mostly does
//! not apply: **the telemetry is self-describing**. The mapping starts with
//! a small fixed header, that header points at a table of variable
//! descriptors, and each descriptor carries the variable's *name* alongside
//! its offset and type. So the offsets of `RPM`, `Throttle` and `Gear` are
//! read out of the game's own table at runtime, never hardcoded here.
//!
//! What is transcribed is only the two fixed structs below, both documented
//! in the public `irsdk_defines.h` and unchanged for many years, and both
//! pinned by layout tests. That is the same discipline `logi-tf-scs` used
//! for the SCS SDK headers.
//!
//! What that still does not prove is that a real iRacing build lays these
//! out the way the public header says. Every read is therefore bounds
//! checked and range gated, and a mismatch produces `None` rather than a
//! wrong number: see [`decode`]. A `--dump` from a live session remains the
//! thing that would move this from "should work" to "known to work".
//!
//! ## Layout, from `irsdk_defines.h`
//!
//! `irsdk_header`, 112 bytes:
//!
//! | offset | field             | type | notes                          |
//! |--------|-------------------|------|--------------------------------|
//! | 0      | ver               | i32  | header version                  |
//! | 4      | status            | i32  | bit 0 = connected               |
//! | 8      | tickRate          | i32  | ticks per second                |
//! | 12     | sessionInfoUpdate | i32  | session string change counter   |
//! | 16     | sessionInfoLen    | i32  |                                 |
//! | 20     | sessionInfoOffset | i32  |                                 |
//! | 24     | numVars           | i32  | entries in the variable table   |
//! | 28     | varHeaderOffset   | i32  | byte offset of that table       |
//! | 32     | numBuf            | i32  | telemetry buffers, 1..=4        |
//! | 36     | bufLen            | i32  | bytes per buffer                |
//! | 40     | pad               | [i32;2] |                              |
//! | 48     | varBuf            | [irsdk_varBuf;4] | 16 bytes each     |
//!
//! `irsdk_varBuf`, 16 bytes: `tickCount` i32, `bufOffset` i32, `pad` [i32;2].
//!
//! `irsdk_varHeader`, 144 bytes: `type` i32, `offset` i32, `count` i32,
//! `countAsTime` bool + 3 pad, `name` [c_char;32], `desc` [c_char;64],
//! `unit` [c_char;32].

// The decoder only runs inside the prefix, so on a Linux host nothing calls
// it outside the tests and every constant reads as dead. Keeping it compiled
// (and tested) on both is deliberate: the layout assertions below are the
// cheapest place to catch a transcription slip, and they should run wherever
// `cargo test` does, not only on a Windows builder.
#![cfg_attr(not(windows), allow(dead_code))]

use logi_wheel_core::relay::RelayTelemetry;

/// Size of `irsdk_header`.
pub const HEADER_LEN: usize = 112;
/// Size of one `irsdk_varBuf` entry.
pub const VARBUF_LEN: usize = 16;
/// Number of `varBuf` slots in the header.
pub const MAX_BUFS: usize = 4;
/// Size of one `irsdk_varHeader` entry.
pub const VARHEADER_LEN: usize = 144;
/// Offset of `name` inside a `irsdk_varHeader`.
const VARHEADER_NAME_OFF: usize = 16;
/// Capacity of that name field.
const VARHEADER_NAME_LEN: usize = 32;

/// `irsdk_VarType` discriminants we can read. The others (char, bitField)
/// never carry the values this crate wants.
const VAR_TYPE_BOOL: i32 = 1;
const VAR_TYPE_INT: i32 = 2;
const VAR_TYPE_FLOAT: i32 = 4;
const VAR_TYPE_DOUBLE: i32 = 5;

/// `status` bit 0: the sim is connected and publishing.
const STATUS_CONNECTED: i32 = 1;

/// The largest engine speed treated as real, in rpm. Anything above this is
/// a decode that landed on the wrong bytes, not a racing engine.
const MAX_PLAUSIBLE_RPM: f32 = 30_000.0;

fn i32_at(buf: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn f32_at(buf: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn f64_at(buf: &[u8], off: usize) -> Option<f64> {
    Some(f64::from_le_bytes(buf.get(off..off + 8)?.try_into().ok()?))
}

/// One entry of the variable table: where a named value lives and how to
/// read it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Var {
    var_type: i32,
    offset: usize,
}

/// Read a NUL-terminated, fixed-capacity name field.
fn name_at(buf: &[u8], off: usize) -> Option<&str> {
    let raw = buf.get(off..off + VARHEADER_NAME_LEN)?;
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    std::str::from_utf8(&raw[..end]).ok()
}

/// Find the named variables in the game's own descriptor table.
///
/// Looked up by name, which is the whole reason this decoder does not need
/// a captured fixture to be trustworthy about *where* values live.
fn find_vars(map: &[u8], wanted: &[&str]) -> Vec<Option<Var>> {
    let mut found = vec![None; wanted.len()];
    let (Some(num_vars), Some(table_off)) = (i32_at(map, 24), i32_at(map, 28)) else {
        return found;
    };
    if num_vars <= 0 || table_off < 0 {
        return found;
    }
    let table_off = table_off as usize;
    // A malicious or corrupt header must not make this walk off the end or
    // spin: every entry is bounds checked against the real mapping length.
    for i in 0..num_vars as usize {
        let entry = table_off + i * VARHEADER_LEN;
        if entry + VARHEADER_LEN > map.len() {
            break;
        }
        let Some(name) = name_at(map, entry + VARHEADER_NAME_OFF) else {
            continue;
        };
        let Some(slot) = wanted.iter().position(|w| *w == name) else {
            continue;
        };
        let (Some(var_type), Some(offset)) = (i32_at(map, entry), i32_at(map, entry + 4)) else {
            continue;
        };
        if offset < 0 {
            continue;
        }
        found[slot] = Some(Var { var_type, offset: offset as usize });
    }
    found
}

/// The telemetry buffer with the newest tick, as a slice of `map`.
fn newest_buffer(map: &[u8]) -> Option<(usize, usize)> {
    let num_buf = i32_at(map, 32)?.clamp(0, MAX_BUFS as i32) as usize;
    let buf_len = i32_at(map, 36)?;
    if num_buf == 0 || buf_len <= 0 {
        return None;
    }
    let buf_len = buf_len as usize;
    let mut best: Option<(i32, usize)> = None;
    for i in 0..num_buf {
        let entry = 48 + i * VARBUF_LEN;
        let (Some(tick), Some(off)) = (i32_at(map, entry), i32_at(map, entry + 4)) else {
            continue;
        };
        if off < 0 {
            continue;
        }
        let off = off as usize;
        if off + buf_len > map.len() {
            continue;
        }
        // `Option::is_none_or` is stable only since 1.82; this crate
        // declares 1.74.
        if best.map_or(true, |(bt, _)| tick > bt) {
            best = Some((tick, off));
        }
    }
    best.map(|(_, off)| (off, buf_len))
}

/// Read one variable out of a telemetry buffer as f64, whatever its
/// declared type.
fn read_var(buf: &[u8], v: Var) -> Option<f64> {
    match v.var_type {
        VAR_TYPE_FLOAT => f32_at(buf, v.offset).map(f64::from),
        VAR_TYPE_DOUBLE => f64_at(buf, v.offset),
        VAR_TYPE_INT => i32_at(buf, v.offset).map(f64::from),
        VAR_TYPE_BOOL => buf.get(v.offset).map(|b| f64::from(*b != 0)),
        _ => None,
    }
}

/// Decode a snapshot of the mapping into a relay sample.
///
/// `None` when the sim is not connected, the header does not look like one,
/// a wanted variable is absent, or a value fails a plausibility gate. The
/// caller streams nothing rather than something wrong.
pub fn decode(map: &[u8]) -> Option<RelayTelemetry> {
    if map.len() < HEADER_LEN {
        return None;
    }
    if i32_at(map, 4)? & STATUS_CONNECTED == 0 {
        return None;
    }

    let wanted = ["RPM", "Throttle", "Gear", "PlayerCarSLBlinkRPM"];
    let vars = find_vars(map, &wanted);
    let (rpm_v, throttle_v, gear_v) = (vars[0]?, vars[1]?, vars[2]?);

    let (buf_off, buf_len) = newest_buffer(map)?;
    let buf = map.get(buf_off..buf_off + buf_len)?;

    let rpm = read_var(buf, rpm_v)? as f32;
    let throttle = read_var(buf, throttle_v)? as f32;
    let gear = read_var(buf, gear_v)? as i32;

    // The redline. iRacing exposes the shift-light blink point, which is at
    // or just below it, and that is what the synthesizer needs to scale the
    // engine note. Without it there is nothing honest to send: a guessed
    // redline makes every car sound wrong rather than slightly wrong.
    let max_rpm = vars[3].and_then(|v| read_var(buf, v)).map(|v| v as f32)?;

    if !rpm.is_finite() || !throttle.is_finite() || !max_rpm.is_finite() {
        return None;
    }
    // Written as a finite check plus ranges rather than negated
    // comparisons: a NaN must be refused, and `contains` refuses it for
    // free where `<=` would quietly let it through.
    if !max_rpm.is_finite()
        || max_rpm <= 0.0
        || max_rpm > MAX_PLAUSIBLE_RPM
        || !(0.0..=MAX_PLAUSIBLE_RPM).contains(&rpm)
    {
        return None;
    }

    Some(RelayTelemetry {
        game_id: "iracing",
        rpm,
        max_rpm,
        throttle: throttle.clamp(0.0, 1.0),
        gear: gear.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        // iRacing's telemetry has no wheels-off-ground field this reads.
        airborne: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a mapping the way `irsdk_defines.h` describes one, so the
    /// decoder is exercised against the documented layout end to end.
    ///
    /// This validates the decoding logic. It does NOT validate that a real
    /// iRacing build matches the public header; only a `--dump` from a live
    /// session can do that, which is why this decoder ships gated.
    struct Fixture {
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn new(vars: &[(&str, i32, f64)], connected: bool) -> Fixture {
            let table_off = HEADER_LEN;
            let buf_off = table_off + vars.len() * VARHEADER_LEN;
            let buf_len = vars.len() * 8;
            let mut b = vec![0u8; buf_off + buf_len];

            b[0..4].copy_from_slice(&1i32.to_le_bytes()); // ver
            b[4..8].copy_from_slice(&i32::from(connected).to_le_bytes());
            b[24..28].copy_from_slice(&(vars.len() as i32).to_le_bytes());
            b[28..32].copy_from_slice(&(table_off as i32).to_le_bytes());
            b[32..36].copy_from_slice(&1i32.to_le_bytes()); // numBuf
            b[36..40].copy_from_slice(&(buf_len as i32).to_le_bytes());
            // varBuf[0]: tick 7, offset buf_off
            b[48..52].copy_from_slice(&7i32.to_le_bytes());
            b[52..56].copy_from_slice(&(buf_off as i32).to_le_bytes());

            for (i, (name, ty, value)) in vars.iter().enumerate() {
                let e = table_off + i * VARHEADER_LEN;
                let value_off = i * 8;
                b[e..e + 4].copy_from_slice(&ty.to_le_bytes());
                b[e + 4..e + 8].copy_from_slice(&(value_off as i32).to_le_bytes());
                b[e + 8..e + 12].copy_from_slice(&1i32.to_le_bytes()); // count
                let n = name.as_bytes();
                b[e + VARHEADER_NAME_OFF..e + VARHEADER_NAME_OFF + n.len()].copy_from_slice(n);

                let at = buf_off + value_off;
                match *ty {
                    VAR_TYPE_FLOAT => b[at..at + 4].copy_from_slice(&(*value as f32).to_le_bytes()),
                    VAR_TYPE_DOUBLE => b[at..at + 8].copy_from_slice(&value.to_le_bytes()),
                    VAR_TYPE_INT => b[at..at + 4].copy_from_slice(&(*value as i32).to_le_bytes()),
                    _ => {}
                }
            }
            Fixture { bytes: b }
        }
    }

    fn typical() -> Fixture {
        Fixture::new(
            &[
                ("RPM", VAR_TYPE_FLOAT, 6200.0),
                ("Throttle", VAR_TYPE_FLOAT, 0.75),
                ("Gear", VAR_TYPE_INT, 4.0),
                ("PlayerCarSLBlinkRPM", VAR_TYPE_FLOAT, 7800.0),
            ],
            true,
        )
    }

    #[test]
    fn reads_values_by_name_not_by_position() {
        let f = typical();
        let t = decode(&f.bytes).expect("decodes");
        assert_eq!(t.game_id, "iracing");
        assert_eq!(t.rpm, 6200.0);
        assert_eq!(t.max_rpm, 7800.0);
        assert_eq!(t.throttle, 0.75);
        assert_eq!(t.gear, 4);
    }

    /// The point of a self-describing format: reorder the table and the
    /// decoder still finds the right values. A decoder that hardcoded
    /// offsets would silently return the wrong numbers here.
    #[test]
    fn a_reordered_variable_table_decodes_identically() {
        let shuffled = Fixture::new(
            &[
                ("Gear", VAR_TYPE_INT, 4.0),
                ("PlayerCarSLBlinkRPM", VAR_TYPE_FLOAT, 7800.0),
                ("Speed", VAR_TYPE_FLOAT, 55.0),
                ("Throttle", VAR_TYPE_FLOAT, 0.75),
                ("RPM", VAR_TYPE_FLOAT, 6200.0),
            ],
            true,
        );
        assert_eq!(decode(&shuffled.bytes), decode(&typical().bytes));
    }

    #[test]
    fn a_disconnected_sim_yields_nothing() {
        let f = Fixture::new(&[("RPM", VAR_TYPE_FLOAT, 6200.0)], false);
        assert!(decode(&f.bytes).is_none());
    }

    #[test]
    fn a_missing_variable_yields_nothing_rather_than_a_guess() {
        // No redline: the synthesizer cannot scale without it, and inventing
        // one makes every car wrong.
        let f = Fixture::new(
            &[
                ("RPM", VAR_TYPE_FLOAT, 6200.0),
                ("Throttle", VAR_TYPE_FLOAT, 0.5),
                ("Gear", VAR_TYPE_INT, 3.0),
            ],
            true,
        );
        assert!(decode(&f.bytes).is_none());
    }

    /// If the public header is wrong about this build, the values land on
    /// the wrong bytes. The gate is what stops that reaching the wheel.
    #[test]
    fn implausible_values_are_rejected() {
        for rpm in [1.0e9, -5.0, f32::NAN] {
            let f = Fixture::new(
                &[
                    ("RPM", VAR_TYPE_FLOAT, f64::from(rpm)),
                    ("Throttle", VAR_TYPE_FLOAT, 0.5),
                    ("Gear", VAR_TYPE_INT, 3.0),
                    ("PlayerCarSLBlinkRPM", VAR_TYPE_FLOAT, 7800.0),
                ],
                true,
            );
            assert!(decode(&f.bytes).is_none(), "rpm {rpm} should be rejected");
        }
    }

    /// A corrupt header must not walk off the end of the mapping.
    ///
    /// An overlarge `numVars` is deliberately NOT required to fail: the walk
    /// stops at the real end of the mapping, by which point it has already
    /// seen the genuine entries, so decoding is the right outcome. What is
    /// required is that it terminates without reading past the buffer, which
    /// is what this asserts.
    #[test]
    fn a_corrupt_header_cannot_read_out_of_bounds() {
        let mut f = typical();
        f.bytes[24..28].copy_from_slice(&1_000_000i32.to_le_bytes()); // numVars
        assert!(decode(&f.bytes).is_some(), "stops at the buffer end, still decodes");

        let mut g = typical();
        g.bytes[28..32].copy_from_slice(&i32::MAX.to_le_bytes()); // varHeaderOffset
        assert!(decode(&g.bytes).is_none());

        let mut h = typical();
        h.bytes[52..56].copy_from_slice(&i32::MAX.to_le_bytes()); // varBuf offset
        assert!(decode(&h.bytes).is_none());
    }

    #[test]
    fn a_truncated_mapping_yields_nothing() {
        let f = typical();
        for len in [0, 1, HEADER_LEN - 1] {
            assert!(decode(&f.bytes[..len]).is_none(), "len {len}");
        }
    }

    /// Newest tick wins when the sim is double buffering.
    #[test]
    fn the_freshest_buffer_is_used() {
        let mut f = typical();
        let first_off = i32_at(&f.bytes, 52).unwrap();
        let buf_len = i32_at(&f.bytes, 36).unwrap() as usize;
        // Add a second buffer holding a different rpm and a newer tick.
        let second_off = f.bytes.len();
        f.bytes.extend_from_slice(&vec![0u8; buf_len]);
        f.bytes[second_off..second_off + 4].copy_from_slice(&9000.0f32.to_le_bytes());
        for (i, v) in [0.75f32, 0.0, 7800.0].iter().enumerate() {
            let at = second_off + 8 * (i + 1);
            f.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        f.bytes[second_off + 16..second_off + 20].copy_from_slice(&4i32.to_le_bytes());
        f.bytes[32..36].copy_from_slice(&2i32.to_le_bytes()); // numBuf = 2
        f.bytes[64..68].copy_from_slice(&99i32.to_le_bytes()); // varBuf[1].tickCount
        f.bytes[68..72].copy_from_slice(&(second_off as i32).to_le_bytes());
        assert_ne!(first_off as usize, second_off);
        assert_eq!(decode(&f.bytes).unwrap().rpm, 9000.0);
    }

    #[test]
    fn layout_constants_match_the_public_header() {
        assert_eq!(HEADER_LEN, 112);
        assert_eq!(VARBUF_LEN, 16);
        assert_eq!(VARHEADER_LEN, 144);
        // varBuf array starts after ten i32 fields plus two pad i32.
        assert_eq!(48, 10 * 4 + 2 * 4);
        // and four of them fill the header exactly.
        assert_eq!(48 + MAX_BUFS * VARBUF_LEN, HEADER_LEN);
        // name sits after type/offset/count/countAsTime+pad.
        assert_eq!(VARHEADER_NAME_OFF, 4 + 4 + 4 + 4);
        // and the three text fields fill the entry exactly.
        assert_eq!(VARHEADER_NAME_OFF + 32 + 64 + 32, VARHEADER_LEN);
    }
}
