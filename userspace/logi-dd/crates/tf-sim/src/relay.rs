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
//! Packet layout (20 bytes, little-endian, fixed size, no padding):
//!
//! | offset | field     | type | notes                              |
//! |--------|-----------|------|------------------------------------|
//! | 0      | magic     | [u8;4] | `b"LTFR"`                        |
//! | 4      | version   | u8   | 1 (this version)                    |
//! | 5      | flags     | u8   | reserved, must be sent as 0         |
//! | 6      | rpm       | f32  | engine speed, rpm                   |
//! | 10     | max_rpm   | f32  | engine redline, rpm                 |
//! | 14     | throttle  | f32  | 0.0-1.0                              |
//! | 18     | gear      | i16  | -1 reverse, 0 neutral, 1..=N forward |

use crate::telemetry::Telemetry;

/// Game id the daemon reports while streaming from a relay packet. The
/// relay format does not carry which title produced it (that is the relay
/// executable's job to know, not ours), so every relay source shares one id
/// for config gating (`game.relay.enabled`, `game.relay.intensity`).
pub const ID: &str = "relay";

/// Default UDP port the daemon listens for relay packets on, distinct from
/// the native-UDP game ports.
pub const DEFAULT_PORT: u16 = 20780;

/// Fixed packet magic, identifying a relay datagram before anything else
/// is trusted.
pub const MAGIC: [u8; 4] = *b"LTFR";

/// The only wire version this daemon understands.
pub const VERSION: u8 = 1;

/// Encoded packet size in bytes.
pub const PACKET_LEN: usize = 20;

/// One decoded relay sample, before conversion to the pipeline's
/// [`Telemetry`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelayTelemetry {
    /// Engine speed in revolutions per minute.
    pub rpm: f32,
    /// Engine redline in revolutions per minute.
    pub max_rpm: f32,
    /// Throttle position, 0.0 to 1.0.
    pub throttle: f32,
    /// Selected gear: -1 reverse, 0 neutral, 1..=N forward.
    pub gear: i16,
}

impl RelayTelemetry {
    /// Convert to the normalized [`Telemetry`] the daemon's synth and LED
    /// pipeline consumes. `gear` has no analog there (the pipeline drives
    /// haptics and the rev display from rpm/max_rpm/throttle alone) and is
    /// dropped; `speed` is not carried by the relay format, so it reads as
    /// 0.0 (only used for a startup log line, never for synthesis).
    pub fn to_telemetry(&self) -> Telemetry {
        Telemetry {
            rpm: self.rpm,
            max_rpm: self.max_rpm,
            throttle: self.throttle.clamp(0.0, 1.0),
            speed: 0.0,
        }
    }
}

/// Encode `t` into the fixed relay wire format.
pub fn encode(t: &RelayTelemetry) -> [u8; PACKET_LEN] {
    let mut buf = [0u8; PACKET_LEN];
    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = VERSION;
    buf[5] = 0; // flags, reserved
    buf[6..10].copy_from_slice(&t.rpm.to_le_bytes());
    buf[10..14].copy_from_slice(&t.max_rpm.to_le_bytes());
    buf[14..18].copy_from_slice(&t.throttle.to_le_bytes());
    buf[18..20].copy_from_slice(&t.gear.to_le_bytes());
    buf
}

/// Decode one relay datagram. Returns `None` for a wrong length, a bad
/// magic or an unsupported version; a caller that wants to know which of
/// those failed should re-check the raw bytes itself, this is a strict
/// accept/reject gate.
pub fn decode(pkt: &[u8]) -> Option<RelayTelemetry> {
    if pkt.len() != PACKET_LEN {
        return None;
    }
    if pkt[0..4] != MAGIC || pkt[4] != VERSION {
        return None;
    }
    let rpm = f32::from_le_bytes(pkt[6..10].try_into().ok()?);
    let max_rpm = f32::from_le_bytes(pkt[10..14].try_into().ok()?);
    let throttle = f32::from_le_bytes(pkt[14..18].try_into().ok()?);
    let gear = i16::from_le_bytes(pkt[18..20].try_into().ok()?);
    if !rpm.is_finite() || !max_rpm.is_finite() || !throttle.is_finite() {
        return None;
    }
    Some(RelayTelemetry { rpm, max_rpm, throttle, gear })
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
    Some((ID, rt.to_telemetry()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let rt = RelayTelemetry { rpm: 4200.0, max_rpm: 8500.0, throttle: 0.42, gear: 4 };
        let encoded = encode(&rt);
        assert_eq!(encoded.len(), PACKET_LEN);
        assert_eq!(decode(&encoded), Some(rt));
    }

    #[test]
    fn round_trips_reverse_and_neutral_gear() {
        let reverse = RelayTelemetry { rpm: 1500.0, max_rpm: 7000.0, throttle: 0.1, gear: -1 };
        assert_eq!(decode(&encode(&reverse)), Some(reverse));
        let neutral = RelayTelemetry { rpm: 900.0, max_rpm: 7000.0, throttle: 0.0, gear: 0 };
        assert_eq!(decode(&encode(&neutral)), Some(neutral));
    }

    /// Golden bytes for a known sample, pinning the exact layout in this
    /// module's doc comment against the encoder.
    #[test]
    fn golden_bytes() {
        let rt = RelayTelemetry { rpm: 6500.0, max_rpm: 7200.0, throttle: 0.5, gear: 3 };
        let expected = [
            0x4c, 0x54, 0x46, 0x52, // magic "LTFR"
            0x01, // version
            0x00, // flags
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
        let mut pkt = encode(&RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 });
        pkt[0] = b'X';
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut pkt = encode(&RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 });
        pkt[4] = 2;
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn short_and_long_buffers_are_rejected() {
        let pkt = encode(&RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 });
        assert!(decode(&pkt[..PACKET_LEN - 1]).is_none(), "truncated");
        let mut long = pkt.to_vec();
        long.push(0);
        assert!(decode(&long).is_none(), "oversized");
        assert!(decode(&[]).is_none(), "empty");
    }

    #[test]
    fn nan_and_infinite_fields_are_rejected() {
        let mut pkt = encode(&RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: 0.0, gear: 0 });
        pkt[6..10].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&pkt).is_none());
    }

    #[test]
    fn to_telemetry_maps_fields_and_drops_gear() {
        let rt = RelayTelemetry { rpm: 3000.0, max_rpm: 7000.0, throttle: 0.8, gear: 2 };
        let tel = rt.to_telemetry();
        assert_eq!(tel.rpm, 3000.0);
        assert_eq!(tel.max_rpm, 7000.0);
        assert_eq!(tel.throttle, 0.8);
        assert_eq!(tel.speed, 0.0, "the relay format carries no speed field");
    }

    #[test]
    fn to_telemetry_clamps_throttle() {
        let over = RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: 1.5, gear: 0 };
        assert_eq!(over.to_telemetry().throttle, 1.0);
        let under = RelayTelemetry { rpm: 1.0, max_rpm: 1.0, throttle: -0.5, gear: 0 };
        assert_eq!(under.to_telemetry().throttle, 0.0);
    }

    #[test]
    fn parse_reports_the_relay_id() {
        let rt = RelayTelemetry { rpm: 5000.0, max_rpm: 8000.0, throttle: 0.6, gear: 5 };
        let pkt = encode(&rt);
        let (id, tel) = parse(&pkt).unwrap();
        assert_eq!(id, ID);
        assert_eq!(tel.rpm, 5000.0);
        assert_eq!(tel.max_rpm, 8000.0);
    }

    #[test]
    fn parse_rejects_menu_samples_with_zero_max_rpm() {
        let rt = RelayTelemetry { rpm: 0.0, max_rpm: 0.0, throttle: 0.0, gear: 0 };
        assert!(parse(&encode(&rt)).is_none());
    }

    #[test]
    fn parse_rejects_malformed_packets() {
        assert!(parse(&[]).is_none());
        assert!(parse(b"not a relay packet at all!!!").is_none());
    }
}
