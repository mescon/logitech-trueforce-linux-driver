// SPDX-License-Identifier: GPL-2.0-only
//! The shared-memory telemetry relay wire format.
//!
//! Some sims (iRacing, rFactor 2 / Le Mans Ultimate, RaceRoom, Assetto
//! Corsa / Competizione) never emit UDP telemetry: they publish it into a
//! named Windows shared-memory section that only the game's own SDK reads.
//! `logi-tf-sim`'s parsers cannot reach that from the Linux side, so those
//! titles instead go through a small relay executable that runs inside the
//! game's Wine/Proton prefix, reads the shared memory with the normal
//! Windows API, and forwards the handful of fields we need over localhost
//! UDP in this format. See `dev/docs/shared-memory-telemetry-plan.md` for
//! the full relay spec; this module is only the wire format and the
//! listener side, which is real today independent of any relay executable.
//!
//! Packet layout (28 bytes, little-endian, fixed size, no padding):
//!
//! | offset | field     | type | notes                              |
//! |--------|-----------|------|------------------------------------|
//! | 0      | magic     | [u8;4] | `b"LTFR"`                        |
//! | 4      | version   | u8   | 2 (this version)                    |
//! | 5      | flags     | u8   | bit 0 = airborne; other bits reserved |
//! | 6      | game_id   | [u8;8] | NUL-padded ASCII, e.g. `ets2`     |
//! | 14     | rpm       | f32  | engine speed, rpm                   |
//! | 18     | max_rpm   | f32  | engine redline, rpm                 |
//! | 22     | throttle  | f32  | 0.0-1.0                              |
//! | 26     | gear      | i16  | -1 reverse, 0 neutral, 1..=N forward |
//!
//! # Why the game id is on the wire
//!
//! Version 1 had no way to say which title a packet came from, so every
//! sender collapsed onto one `relay` id and therefore one enable switch and
//! one intensity in the Setup page. A truck sim and a GT car wanting the
//! same engine-haptic strength is not a real assumption, and the registry
//! could not map a title to its own settings either. Version 1 shipped in
//! 0.28.0 with the listener only and no sender anywhere, so nothing was
//! deployed to be compatible with; adding the field cost nothing then and
//! would have cost a version negotiation later.

use crate::telemetry::Telemetry;

/// Fallback game id, used when a packet carries an empty or unrecognised
/// game id. Keeps an unknown sender working (gated as `game.relay.*`)
/// rather than dropping its telemetry on the floor.
pub const ID: &str = "relay";

/// Game ids this format defines, each gated independently in tf-sim.conf as
/// `game.<id>.enabled` / `game.<id>.intensity`.
///
/// A sender writes its id into the packet; the daemon looks it up here and
/// falls back to [`ID`] for anything it does not recognise, so a newer
/// sender against an older daemon degrades to the shared switch instead of
/// going silent.
pub const GAME_IDS: &[&str] =
    &["ets2", "ats", "iracing", "raceroom", "assetto", "acc", "ac-evo", "lmu", "rf2", ID];

/// Width of the on-wire game id field. Eight bytes holds every id above
/// with room to spare, and keeps the packet a round 28 bytes.
pub const GAME_ID_LEN: usize = 8;

/// Default UDP port the daemon listens for relay packets on, distinct from
/// the native-UDP game ports.
pub const DEFAULT_PORT: u16 = 20780;

/// Fixed packet magic, identifying a relay datagram before anything else
/// is trusted.
pub const MAGIC: [u8; 4] = *b"LTFR";

/// Bit 0 of the flags byte: the car has all four wheels off the ground.
///
/// Added without a version bump because `decode` never read the byte, so an
/// older listener ignores it and a newer one reading an older sender sees
/// zero, which is the correct answer for a sender that cannot tell. The byte
/// was reserved for exactly this.
pub const FLAG_AIRBORNE: u8 = 1 << 0;

/// The only wire version this daemon understands.
pub const VERSION: u8 = 2;

/// Encoded packet size in bytes.
pub const PACKET_LEN: usize = 28;

/// One decoded relay sample, before conversion to the pipeline's
/// [`Telemetry`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelayTelemetry {
    /// Which title produced this sample, one of [`GAME_IDS`]. Decides which
    /// `game.<id>.*` settings gate it.
    pub game_id: &'static str,
    /// Engine speed in revolutions per minute.
    pub rpm: f32,
    /// Engine redline in revolutions per minute.
    pub max_rpm: f32,
    /// Throttle position, 0.0 to 1.0.
    pub throttle: f32,
    /// Selected gear: -1 reverse, 0 neutral, 1..=N forward.
    pub gear: i16,
    /// All four wheels off the ground, when the source can tell. Senders
    /// that cannot leave this false, which is what a listener assumes of
    /// any sender predating the flag.
    pub airborne: bool,
}

impl RelayTelemetry {
    /// Convert to the normalized [`Telemetry`] the daemon's synth and LED
    /// pipeline consumes. `speed` is not carried by the relay format, so it
    /// reads as 0.0 (only used for a startup log line, never for synthesis).
    ///
    /// Everything the relay does not carry keeps its `Default`, which is the
    /// value each effect reads as "not happening".
    pub fn to_telemetry(&self) -> Telemetry {
        Telemetry {
            rpm: self.rpm,
            max_rpm: self.max_rpm,
            throttle: self.throttle.clamp(0.0, 1.0),
            // The wire field is i16 for headroom; no real gearbox reaches
            // beyond i8, and saturating keeps a corrupt packet from wrapping
            // a high gear round to reverse.
            gear: self.gear.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
            airborne: self.airborne,
            ..Default::default()
        }
    }
}

/// Encode `t` into the fixed relay wire format.
pub fn encode(t: &RelayTelemetry) -> [u8; PACKET_LEN] {
    let mut buf = [0u8; PACKET_LEN];
    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = VERSION;
    buf[5] = if t.airborne { FLAG_AIRBORNE } else { 0 };
    // NUL-padded, never NUL-terminated-and-truncated: an id longer than the
    // field would silently become a different id, so ids are kept short and
    // the excess is dropped rather than encoded wrong.
    let id = t.game_id.as_bytes();
    let n = id.len().min(GAME_ID_LEN);
    buf[6..6 + n].copy_from_slice(&id[..n]);
    buf[14..18].copy_from_slice(&t.rpm.to_le_bytes());
    buf[18..22].copy_from_slice(&t.max_rpm.to_le_bytes());
    buf[22..26].copy_from_slice(&t.throttle.to_le_bytes());
    buf[26..28].copy_from_slice(&t.gear.to_le_bytes());
    buf
}

/// Resolve an on-wire game id to one of [`GAME_IDS`], or [`ID`] when it is
/// empty or unknown.
fn game_id_from_wire(raw: &[u8]) -> &'static str {
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    let Ok(name) = std::str::from_utf8(&raw[..end]) else {
        return ID;
    };
    GAME_IDS.iter().copied().find(|g| *g == name).unwrap_or(ID)
}

/// Decode one relay datagram. Returns `None` for a short length, a bad
/// magic or an unsupported version; a caller that wants to know which of
/// those failed should re-check the raw bytes itself, this is a strict
/// accept/reject gate.
///
/// The wire format is append-only within a version: a sender may extend
/// the packet past [`PACKET_LEN`] (the dinput8 escape proxy appends the
/// game's first-shift-light rpm at bytes 28-31 for the rev-LED bridge),
/// and this decoder reads the 28 bytes it knows and ignores the rest.
pub fn decode(pkt: &[u8]) -> Option<RelayTelemetry> {
    if pkt.len() < PACKET_LEN {
        return None;
    }
    if pkt[0..4] != MAGIC || pkt[4] != VERSION {
        return None;
    }
    let game_id = game_id_from_wire(&pkt[6..14]);
    let rpm = f32::from_le_bytes(pkt[14..18].try_into().ok()?);
    let max_rpm = f32::from_le_bytes(pkt[18..22].try_into().ok()?);
    let throttle = f32::from_le_bytes(pkt[22..26].try_into().ok()?);
    let gear = i16::from_le_bytes(pkt[26..28].try_into().ok()?);
    if !rpm.is_finite() || !max_rpm.is_finite() || !throttle.is_finite() {
        return None;
    }
    let airborne = pkt[5] & FLAG_AIRBORNE != 0;
    Some(RelayTelemetry { game_id, rpm, max_rpm, throttle, gear, airborne })
}

/// Decode one relay datagram straight to a pipeline sample, matching the
/// `&[u8] -> Option<(game id, Telemetry)>` signature the daemon's other
/// telemetry decoders use. Rejects a sample with no usable engine data
/// (`max_rpm <= 0`), the same "menu or paused" gate the other decoders use.
pub fn parse(pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
    let rt = decode(pkt)?;
    if rt.max_rpm <= 0.0 {
        return None;
    }
    Some((rt.game_id, rt.to_telemetry()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let rt = RelayTelemetry { game_id: "ets2", rpm: 4200.0, max_rpm: 8500.0, throttle: 0.42, gear: 4 , airborne: false };
        let encoded = encode(&rt);
        assert_eq!(encoded.len(), PACKET_LEN);
        assert_eq!(decode(&encoded), Some(rt));
    }

    #[test]
    fn accepts_append_only_extensions() {
        // The escape proxy appends the first-shift-light rpm at 28-31;
        // the known fields must decode identically from the longer form.
        let rt = RelayTelemetry { game_id: "relay", rpm: 2950.0, max_rpm: 14250.0, throttle: 0.0, gear: 0, airborne: false };
        let mut extended = encode(&rt).to_vec();
        extended.extend_from_slice(&11250.0_f32.to_le_bytes());
        assert_eq!(decode(&extended), Some(rt));
        // Short datagrams stay rejected.
        assert_eq!(decode(&extended[..27]), None);
    }

    #[test]
    fn round_trips_reverse_and_neutral_gear() {
        let reverse = RelayTelemetry { game_id: "ets2", rpm: 1500.0, max_rpm: 7000.0, throttle: 0.1, gear: -1 , airborne: false };
        assert_eq!(decode(&encode(&reverse)), Some(reverse));
        let neutral = RelayTelemetry { game_id: "ets2", rpm: 900.0, max_rpm: 7000.0, throttle: 0.0, gear: 0 , airborne: false };
        assert_eq!(decode(&encode(&neutral)), Some(neutral));
    }

    /// Golden bytes for a known sample, pinning the exact layout in this
    /// module's doc comment against the encoder.
    #[test]
    fn golden_bytes() {
        let rt = RelayTelemetry { game_id: "ets2", rpm: 6500.0, max_rpm: 7200.0, throttle: 0.5, gear: 3 , airborne: false };
        let expected = [
            0x4c, 0x54, 0x46, 0x52, // magic "LTFR"
            0x02, // version
            0x00, // flags
            b'e', b't', b's', b'2', 0x00, 0x00, 0x00, 0x00, // game id, NUL-padded
            0x00, 0x20, 0xcb, 0x45, // rpm 6500.0
            0x00, 0x00, 0xe1, 0x45, // max_rpm 7200.0
            0x00, 0x00, 0x00, 0x3f, // throttle 0.5
            0x03, 0x00, // gear 3
        ];
        assert_eq!(encode(&rt), expected);
        assert_eq!(decode(&expected), Some(rt));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut pkt = encode(&RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 , airborne: false });
        pkt[0] = b'X';
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut pkt = encode(&RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 , airborne: false });
        pkt[4] = VERSION + 1;
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn short_buffers_are_rejected_long_ones_accepted() {
        // Append-only wire contract: senders may extend past PACKET_LEN
        // (the escape proxy does, for the rev-LED first-light field) and
        // the extension must not cost them the packet.
        let pkt = encode(&RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 , airborne: false });
        assert!(decode(&pkt[..PACKET_LEN - 1]).is_none(), "truncated");
        let mut long = pkt.to_vec();
        long.push(0);
        assert!(decode(&long).is_some(), "append-only extension");
        assert!(decode(&[]).is_none(), "empty");
    }

    #[test]
    fn nan_and_infinite_fields_are_rejected() {
        let mut pkt = encode(&RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 , airborne: false });
        pkt[14..18].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn to_telemetry_maps_fields_and_drops_gear() {
        let rt = RelayTelemetry { game_id: "ets2", rpm: 3000.0, max_rpm: 7000.0, throttle: 0.8, gear: 2 , airborne: false };
        let tel = rt.to_telemetry();
        assert_eq!(tel.rpm, 3000.0);
        assert_eq!(tel.max_rpm, 7000.0);
        assert_eq!(tel.throttle, 0.8);
        assert_eq!(tel.speed, 0.0, "the relay format carries no speed field");
    }

    #[test]
    fn to_telemetry_clamps_throttle() {
        let over = RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: 1.5, gear: 0 , airborne: false };
        assert_eq!(over.to_telemetry().throttle, 1.0);
        let under = RelayTelemetry { game_id: "ets2", rpm: 1.0, max_rpm: 1.0, throttle: -0.5, gear: 0 , airborne: false };
        assert_eq!(under.to_telemetry().throttle, 0.0);
    }

    #[test]
    fn parse_reports_the_senders_game_id() {
        let rt = RelayTelemetry { game_id: "ets2", rpm: 5000.0, max_rpm: 8000.0, throttle: 0.6, gear: 5 , airborne: false };
        let pkt = encode(&rt);
        let (id, tel) = parse(&pkt).unwrap();
        assert_eq!(id, "ets2", "the sender's own id decides which settings gate it");
        assert_eq!(tel.rpm, 5000.0);
        assert_eq!(tel.max_rpm, 8000.0);
    }

    /// Every id in the table survives a round trip. A silent truncation or a
    /// lookup miss would send a title's telemetry to another title's
    /// settings, which is worse than dropping it.
    #[test]
    fn every_known_game_id_round_trips() {
        for id in GAME_IDS {
            let rt = RelayTelemetry {
                game_id: id,
                rpm: 3000.0,
                max_rpm: 7000.0,
                throttle: 0.5,
                gear: 2, airborne: false };
            assert_eq!(decode(&encode(&rt)).unwrap().game_id, *id, "{id}");
            assert!(id.len() <= GAME_ID_LEN, "{id} does not fit the wire field");
        }
    }

    /// A sender newer than this daemon degrades to the shared switch rather
    /// than going silent, which is the whole reason the fallback exists.
    #[test]
    fn an_unknown_game_id_falls_back_to_the_shared_switch() {
        let rt = RelayTelemetry {
            game_id: "ets2",
            rpm: 3000.0,
            max_rpm: 7000.0,
            throttle: 0.5,
            gear: 2, airborne: false };
        let mut pkt = encode(&rt);
        pkt[6..14].copy_from_slice(b"wipeout\0");
        assert_eq!(parse(&pkt).unwrap().0, ID);

        // An empty id, i.e. a sender that did not set one at all.
        let mut blank = encode(&rt);
        blank[6..14].fill(0);
        assert_eq!(parse(&blank).unwrap().0, ID);
    }

    /// Non-UTF-8 in the id field must not panic or be interpreted.
    #[test]
    fn a_corrupt_game_id_is_not_trusted() {
        let rt = RelayTelemetry {
            game_id: "ets2",
            rpm: 3000.0,
            max_rpm: 7000.0,
            throttle: 0.5,
            gear: 2, airborne: false };
        let mut pkt = encode(&rt);
        pkt[6..14].copy_from_slice(&[0xff, 0xfe, 0xfd, 0, 0, 0, 0, 0]);
        assert_eq!(parse(&pkt).unwrap().0, ID);
    }

    #[test]
    fn parse_rejects_menu_samples_with_zero_max_rpm() {
        let rt = RelayTelemetry { game_id: "ets2", rpm: 0.0, max_rpm: 0.0, throttle: 0.0, gear: 0 , airborne: false };
        assert!(parse(&encode(&rt)).is_none());
    }

    #[test]
    fn parse_rejects_malformed_packets() {
        assert!(parse(&[]).is_none());
        assert!(parse(b"not a relay packet at all!!!").is_none());
    }
}

#[cfg(test)]
mod daemon_agreement {
    use super::*;

    /// Every id this format can put on the wire must be one the daemon
    /// gates on, or a sender would stream to a switch that does not exist
    /// and the user could never turn it off.
    #[test]
    fn every_relay_game_id_is_known_to_the_daemon() {
        for id in GAME_IDS {
            assert!(
                crate::tfsim::DAEMON_GAME_IDS.contains(id),
                "{id} is on the wire but absent from DAEMON_GAME_IDS"
            );
        }
    }
}

#[cfg(test)]
mod airborne_flag_tests {
    use super::*;

    fn sample(airborne: bool) -> RelayTelemetry {
        RelayTelemetry {
            game_id: "acc",
            rpm: 4000.0,
            max_rpm: 8000.0,
            throttle: 0.5,
            gear: 3,
            airborne,
        }
    }

    #[test]
    fn the_airborne_flag_round_trips() {
        for airborne in [false, true] {
            let decoded = decode(&encode(&sample(airborne))).expect("valid packet");
            assert_eq!(decoded.airborne, airborne);
        }
    }

    #[test]
    fn a_sender_that_predates_the_flag_reads_as_grounded() {
        // The byte was documented "reserved, must be sent as 0" and decode
        // never read it, so every existing sender emits zero there. That has
        // to mean "not airborne" rather than anything else, or adding the
        // bit would have changed the meaning of packets already in flight.
        let mut pkt = encode(&sample(true));
        pkt[5] = 0;
        assert!(!decode(&pkt).expect("valid packet").airborne);
    }

    #[test]
    fn unknown_flag_bits_are_ignored_rather_than_rejected() {
        // Room to add more flags later without an older listener refusing
        // the packet outright.
        let mut pkt = encode(&sample(true));
        pkt[5] |= 0b1111_1110;
        let decoded = decode(&pkt).expect("unknown bits must not invalidate a packet");
        assert!(decoded.airborne, "the bit we do know still reads");
    }
}

#[cfg(test)]
mod airborne_end_to_end {
    use super::*;

    /// The whole chain: a relay says airborne, the daemon's Telemetry says
    /// airborne. Worth asserting because the flag crosses three
    /// representations (wire byte, RelayTelemetry, Telemetry) and the effect
    /// that consumes it has never run, so nothing else would notice a break.
    #[test]
    fn the_wire_flag_becomes_telemetry_airborne() {
        for airborne in [false, true] {
            let pkt = encode(&RelayTelemetry {
                game_id: "acc",
                rpm: 5000.0,
                max_rpm: 8000.0,
                throttle: 1.0,
                gear: 4,
                airborne,
            });
            let (_, tel) = parse(&pkt).expect("a running sample parses");
            assert_eq!(tel.airborne, airborne, "airborne must survive the wire");
        }
    }
}
