// SPDX-License-Identifier: GPL-2.0-only
//! Codemasters F1 (F1 22 / 23 / 24 / 25) modern UDP telemetry.
//!
//! Unlike the classic Codemasters float array, the modern F1 titles send a
//! headered, packed, little-endian protocol on UDP port 20777 (the same
//! port the classic format uses, so the daemon disambiguates by header and
//! length). Every datagram starts with a `PacketHeader`; we decode only the
//! Car Telemetry packet (`m_packetId == 6`), reading the PLAYER car's slot
//! out of the 22-car array.
//!
//! The header grew a field between game years, so its size and the offsets
//! after `m_packetFormat` shift:
//!
//! | field                       | type | 2022 off | 2023+ off |
//! |-----------------------------|------|----------|-----------|
//! | m_packetFormat              | u16  | 0        | 0         |
//! | m_gameYear (2023+ only)     | u8   | -        | 2         |
//! | m_gameMajorVersion          | u8   | 2        | 3         |
//! | m_gameMinorVersion          | u8   | 3        | 4         |
//! | m_packetVersion             | u8   | 4        | 5         |
//! | m_packetId                  | u8   | 5        | 6         |
//! | m_sessionUID                | u64  | 6        | 7         |
//! | m_sessionTime               | f32  | 14       | 15        |
//! | m_frameIdentifier           | u32  | 18       | 19        |
//! | m_overallFrameId (2023+)    | u32  | -        | 23        |
//! | m_playerCarIndex            | u8   | 22       | 27        |
//! | m_secondaryPlayerCarIndex   | u8   | 23       | 28        |
//!
//! header length: 24 bytes (2022), 29 bytes (2023-2025).
//!
//! One `CarTelemetryData` entry is 60 bytes; the fields we read:
//!
//! | offset | field        | type | unit |
//! |--------|--------------|------|------|
//! | 0      | m_speed      | u16  | km/h |
//! | 2      | m_throttle   | f32  | 0..1 |
//! | 16     | m_engineRPM  | u16  | rpm  |
//!
//! The Car Telemetry packet carries no redline, so `max_rpm` is the running
//! maximum engine RPM seen this session (the [`Decoder`] holds that state;
//! it resets when the daemon tears the stream down). Sources: the F1 24 UDP
//! specification (EA Forums) and community parsers (MacManley/f1-24-udp,
//! raweceek-temeletry/f1-23-udp).

use crate::telemetry::Telemetry;

/// Game id emitted for every modern F1 title (F1 22-25 share the format).
pub const ID: &str = "f1";

/// The `m_packetId` value of the Car Telemetry packet.
const CAR_TELEMETRY_PACKET_ID: u8 = 6;
/// Bytes per `CarTelemetryData` entry.
const ENTRY_LEN: usize = 60;
/// Cars in the telemetry array.
const NUM_CARS: usize = 22;

// CarTelemetryData field offsets, from the entry start.
const E_SPEED: usize = 0;
const E_THROTTLE: usize = 2;
const E_RPM: usize = 16;

/// km/h to m/s.
const KMH_TO_MS: f32 = 1000.0 / 3600.0;
/// Reject engine rates above this as non-F1 (real cars redline ~15k).
const RPM_CEILING: f32 = 30_000.0;

fn u16_at(pkt: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(pkt.get(off..off + 2)?.try_into().ok()?))
}

fn f32_at(pkt: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(pkt.get(off..off + 4)?.try_into().ok()?))
}

/// The parts of the header the Car Telemetry decode needs.
struct Header {
    len: usize,
    packet_id: u8,
    player_index: usize,
}

/// Parse the header, returning `None` for any `m_packetFormat` we do not
/// recognize (which is also how a classic-format or foreign packet is
/// rejected before the length gate).
fn parse_header(pkt: &[u8]) -> Option<Header> {
    let (len, id_off, player_off) = match u16_at(pkt, 0)? {
        2022 => (24usize, 5usize, 22usize),
        2023..=2025 => (29, 6, 27),
        _ => return None,
    };
    let player_index = usize::from(*pkt.get(player_off)?);
    if player_index >= NUM_CARS {
        return None;
    }
    Some(Header { len, packet_id: *pkt.get(id_off)?, player_index })
}

/// A stateful F1 telemetry decoder. Stateful only to track the running
/// redline (`max_rpm`) the Car Telemetry packet omits; the per-packet decode
/// is otherwise pure.
#[derive(Debug, Default)]
pub struct Decoder {
    running_max_rpm: f32,
}

impl Decoder {
    pub fn new() -> Self {
        Decoder::default()
    }

    /// Forget the learned redline (called when the stream is torn down, so a
    /// new session with a different car re-learns from scratch).
    pub fn reset(&mut self) {
        self.running_max_rpm = 0.0;
    }

    /// Parse one datagram. Returns the [`ID`] and a sample for a Car
    /// Telemetry packet with a running engine, or `None` for any other
    /// packet type, format, length, or an engine-off/menu sample.
    pub fn parse(&mut self, pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
        let h = parse_header(pkt)?;
        if h.packet_id != CAR_TELEMETRY_PACKET_ID {
            return None;
        }
        // The player's slot must lie wholly inside the packet; require the
        // full car array so a truncated datagram never reads past the end.
        if pkt.len() < h.len + NUM_CARS * ENTRY_LEN {
            return None;
        }
        let base = h.len + h.player_index * ENTRY_LEN;
        let speed = f32::from(u16_at(pkt, base + E_SPEED)?) * KMH_TO_MS;
        let throttle = f32_at(pkt, base + E_THROTTLE)?;
        let rpm = f32::from(u16_at(pkt, base + E_RPM)?);

        // rpm == 0 is the engine off (menu, replay, paused); nothing to feel.
        if !throttle.is_finite() || rpm <= 0.0 || rpm > RPM_CEILING {
            return None;
        }
        self.running_max_rpm = self.running_max_rpm.max(rpm);
        let max_rpm = self.running_max_rpm.max(1.0);
        Some((ID, Telemetry { rpm, max_rpm, throttle: throttle.clamp(0.0, 1.0), speed }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Car Telemetry datagram for `format` with the player at index
    /// `player` carrying the given engine values, sized like the real packet
    /// (header + 22 entries + a small trailer).
    fn telemetry_packet(format: u16, player: usize, rpm: u16, throttle: f32, speed_kmh: u16) -> Vec<u8> {
        let (len, id_off, player_off) = match format {
            2022 => (24usize, 5usize, 22usize),
            _ => (29, 6, 27),
        };
        let mut pkt = vec![0u8; len + NUM_CARS * ENTRY_LEN + 3];
        pkt[0..2].copy_from_slice(&format.to_le_bytes());
        pkt[id_off] = CAR_TELEMETRY_PACKET_ID;
        pkt[player_off] = player as u8;
        let base = len + player * ENTRY_LEN;
        pkt[base + E_SPEED..base + E_SPEED + 2].copy_from_slice(&speed_kmh.to_le_bytes());
        pkt[base + E_THROTTLE..base + E_THROTTLE + 4].copy_from_slice(&throttle.to_le_bytes());
        pkt[base + E_RPM..base + E_RPM + 2].copy_from_slice(&rpm.to_le_bytes());
        pkt
    }

    #[test]
    fn f1_24_telemetry_parses_the_player_slot() {
        let mut d = Decoder::new();
        let pkt = telemetry_packet(2024, 5, 11000, 0.5, 216);
        let (id, t) = d.parse(&pkt).unwrap();
        assert_eq!(id, ID);
        assert_eq!(t.rpm, 11000.0);
        assert_eq!(t.max_rpm, 11000.0, "first sample: running max == rpm");
        assert!((t.throttle - 0.5).abs() < 1e-6);
        // 216 km/h == 60 m/s.
        assert!((t.speed - 60.0).abs() < 0.05, "speed {}", t.speed);
    }

    #[test]
    fn f1_22_uses_the_shorter_header() {
        let mut d = Decoder::new();
        let (_, t) = d.parse(&telemetry_packet(2022, 0, 9000, 1.0, 108)).unwrap();
        assert_eq!(t.rpm, 9000.0);
        assert!((t.speed - 30.0).abs() < 0.05);
    }

    #[test]
    fn running_max_tracks_the_session_high() {
        let mut d = Decoder::new();
        d.parse(&telemetry_packet(2024, 3, 6000, 0.2, 100)).unwrap();
        let (_, t) = d.parse(&telemetry_packet(2024, 3, 12500, 1.0, 300)).unwrap();
        assert_eq!(t.max_rpm, 12500.0);
        // A later lower sample keeps the learned high.
        let (_, t2) = d.parse(&telemetry_packet(2024, 3, 8000, 0.5, 200)).unwrap();
        assert_eq!(t2.max_rpm, 12500.0);
        d.reset();
        let (_, t3) = d.parse(&telemetry_packet(2024, 3, 8000, 0.5, 200)).unwrap();
        assert_eq!(t3.max_rpm, 8000.0, "reset re-learns from scratch");
    }

    #[test]
    fn other_packet_types_are_ignored() {
        let mut d = Decoder::new();
        let mut pkt = telemetry_packet(2024, 0, 10000, 1.0, 200);
        pkt[6] = 7; // Car Status, not Car Telemetry
        assert!(d.parse(&pkt).is_none());
    }

    #[test]
    fn engine_off_and_out_of_range_are_ignored() {
        let mut d = Decoder::new();
        assert!(d.parse(&telemetry_packet(2024, 0, 0, 0.0, 0)).is_none(), "menu / engine off");
        assert!(d.parse(&telemetry_packet(2024, 0, 40000, 1.0, 0)).is_none(), "absurd rpm");
    }

    #[test]
    fn foreign_and_short_packets_are_rejected() {
        let mut d = Decoder::new();
        // Classic-format length with a leading value that is not a known year.
        let mut classic = vec![0u8; 264];
        classic[0..2].copy_from_slice(&1234u16.to_le_bytes());
        assert!(d.parse(&classic).is_none(), "unknown packetFormat");
        // A known year but truncated before the player's slot.
        let mut short = telemetry_packet(2024, 21, 10000, 1.0, 200);
        short.truncate(100);
        assert!(d.parse(&short).is_none(), "truncated car array");
        assert!(d.parse(&[]).is_none(), "empty");
    }
}
