// SPDX-License-Identifier: GPL-2.0-only
//! Project CARS 2 / Automobilista 2 shared UDP telemetry.
//!
//! Both games emit the SMS "Project CARS 2" UDP protocol (protocol
//! version 2) on port 5606; AMS2 sends identical packets when its UDP
//! protocol option is set to "Project CARS 2". The wire structs are
//! packed (no padding) and little-endian, per the official
//! SMS_UDP_Definitions header shipped with the PCARS2 patch-5 API docs.
//!
//! Every packet starts with the 12-byte PacketBase header:
//!
//! | offset | field                  | type |
//! |--------|------------------------|------|
//! | 0      | mPacketNumber          | u32  |
//! | 4      | mCategoryPacketNumber  | u32  |
//! | 8      | mPartialPacketIndex    | u8   |
//! | 9      | mPartialPacketNumber   | u8   |
//! | 10     | mPacketType            | u8   |
//! | 11     | mPacketVersion         | u8   |
//!
//! We only decode packet type 0 (eCarPhysics, `sTelemetryData`, 559
//! bytes). Fields used, offsets from packet start:
//!
//! | offset | field     | type | unit  |
//! |--------|-----------|------|-------|
//! | 30     | sThrottle | u8   | 0-255 |
//! | 36     | sSpeed    | f32  | m/s   |
//! | 40     | sRpm      | u16  | rpm   |
//! | 42     | sMaxRpm   | u16  | rpm   |

use crate::telemetry::Telemetry;

/// Default listen port for the PCARS2/AMS2 shared protocol.
pub const DEFAULT_PORT: u16 = 5606;

/// Game id for this family (the two titles share the format 1:1).
pub const ID_FAMILY: &str = "ams2-pcars2";

/// `sTelemetryData` packet size in protocol version 2.
pub const TELEMETRY_LEN: usize = 559;

const OFF_PACKET_TYPE: usize = 10;
const OFF_PACKET_VERSION: usize = 11;
/// eCarPhysics in the SMS packet-type enum.
const PACKET_TYPE_TELEMETRY: u8 = 0;

const OFF_THROTTLE: usize = 30;
const OFF_SPEED: usize = 36;
const OFF_RPM: usize = 40;
const OFF_MAX_RPM: usize = 42;

fn u16_at(pkt: &[u8], off: usize) -> Option<u16> {
    let bytes = pkt.get(off..off + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn f32_at(pkt: &[u8], off: usize) -> Option<f32> {
    let bytes = pkt.get(off..off + 4)?;
    Some(f32::from_le_bytes(bytes.try_into().ok()?))
}

/// The PacketBase `mPacketVersion` field, for any packet in this format.
pub fn packet_version(pkt: &[u8]) -> Option<u8> {
    pkt.get(OFF_PACKET_VERSION).copied()
}

/// Parse one `sTelemetryData` packet.
///
/// Returns `None` for other packet types (race data, timings, ...), other
/// lengths, or samples without usable engine data (`sMaxRpm == 0`, as sent
/// in menus).
pub fn parse(pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
    if pkt.len() != TELEMETRY_LEN || *pkt.get(OFF_PACKET_TYPE)? != PACKET_TYPE_TELEMETRY {
        return None;
    }
    let throttle = f32::from(*pkt.get(OFF_THROTTLE)?) / 255.0;
    let speed = f32_at(pkt, OFF_SPEED)?;
    let rpm = f32::from(u16_at(pkt, OFF_RPM)?);
    let max_rpm = f32::from(u16_at(pkt, OFF_MAX_RPM)?);

    if max_rpm <= 0.0 || !speed.is_finite() {
        return None;
    }
    Some((ID_FAMILY, Telemetry { rpm, max_rpm, throttle, speed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `sTelemetryData` fixture with known engine values.
    fn packet(rpm: u16, max_rpm: u16, throttle: u8, speed: f32) -> Vec<u8> {
        let mut pkt = vec![0u8; TELEMETRY_LEN];
        pkt[OFF_PACKET_TYPE] = PACKET_TYPE_TELEMETRY;
        pkt[OFF_PACKET_VERSION] = 2;
        pkt[OFF_THROTTLE] = throttle;
        pkt[OFF_SPEED..OFF_SPEED + 4].copy_from_slice(&speed.to_le_bytes());
        pkt[OFF_RPM..OFF_RPM + 2].copy_from_slice(&rpm.to_le_bytes());
        pkt[OFF_MAX_RPM..OFF_MAX_RPM + 2].copy_from_slice(&max_rpm.to_le_bytes());
        pkt
    }

    #[test]
    fn telemetry_packet_parses() {
        let pkt = packet(4321, 8500, 128, 55.5);
        let (id, t) = parse(&pkt).unwrap();
        assert_eq!(id, ID_FAMILY);
        assert_eq!(t.rpm, 4321.0);
        assert_eq!(t.max_rpm, 8500.0);
        assert!((t.throttle - 128.0 / 255.0).abs() < 1e-6);
        assert!((t.speed - 55.5).abs() < 1e-6);
        assert_eq!(packet_version(&pkt), Some(2));
    }

    #[test]
    fn full_throttle_is_one() {
        let (_, t) = parse(&packet(1000, 7000, 255, 0.0)).unwrap();
        assert_eq!(t.throttle, 1.0);
    }

    #[test]
    fn other_packet_types_are_rejected() {
        let mut pkt = packet(4321, 8500, 128, 55.5);
        pkt[OFF_PACKET_TYPE] = 3; // eTimings
        assert!(parse(&pkt).is_none());
    }

    #[test]
    fn other_lengths_are_rejected() {
        let mut pkt = packet(4321, 8500, 128, 55.5);
        pkt.pop();
        assert!(parse(&pkt).is_none(), "truncated");
        pkt.extend_from_slice(&[0, 0]);
        assert!(parse(&pkt).is_none(), "oversized");
        assert!(parse(&[]).is_none(), "empty");
    }

    #[test]
    fn menu_packets_with_zero_max_rpm_are_rejected() {
        assert!(parse(&packet(0, 0, 0, 0.0)).is_none());
    }
}
