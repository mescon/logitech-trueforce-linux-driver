// SPDX-License-Identifier: GPL-2.0-only
//! Classic Codemasters/EA float-array UDP telemetry.
//!
//! DiRT Rally 2.0 (and the older DiRT / GRID / legacy-mode F1 titles, plus
//! EA WRC's compatibility output) broadcast a packed array of little-endian
//! f32 values on UDP port 20777. There is no header: the packet IS the
//! float array, so format detection is by packet length.
//!
//! Field offsets follow the community-documented DiRT Rally 2.0 layout
//! (66 floats, 264 bytes, with the game's `extradata="3"` telemetry
//! setting; the first 64 floats are the base motion block shared by the
//! older titles). The same table is used by open telemetry tools such as
//! the dr2logger project and SimHub's Codemasters providers:
//!
//! | float index | field                | unit          |
//! |-------------|----------------------|---------------|
//! | 7           | speed                | m/s           |
//! | 29          | throttle             | 0..1          |
//! | 33          | gear                 | -1..n         |
//! | 37          | engine rate          | rpm / 10      |
//! | 63          | max engine rate      | rpm / 10      |
//!
//! Modern F1 (2018+) UDP is a different, headered format (leading u16
//! packetFormat, packets over 1 kB); those packets fail the length gate
//! here and are ignored in v1.

use crate::telemetry::Telemetry;

/// Default listen port for the Codemasters/EA family.
pub const DEFAULT_PORT: u16 = 20777;

/// Game id for the DR2-signature packet length.
pub const ID_DIRT_RALLY_2: &str = "dirt-rally-2";
/// Family id when the exact title is not distinguishable from the packet.
pub const ID_FAMILY: &str = "codemasters";

// Float-array indices (see the module table above for sources).
const F_SPEED: usize = 7;
const F_THROTTLE: usize = 29;
const F_RPM: usize = 37;
const F_MAX_RPM: usize = 63;
/// Engine-rate fields carry rpm divided by 10.
const RPM_SCALE: f32 = 10.0;

/// Base motion block: 64 floats. The largest index we read (63) needs all
/// of it, so shorter extradata levels cannot be used for synthesis.
const BASE_LEN: usize = 64 * 4;
/// DiRT Rally 2.0 with `extradata="3"`: 66 floats.
const DR2_LEN: usize = 66 * 4;
/// Tolerate a few trailing extension floats from sibling titles.
const MAX_LEN: usize = 70 * 4;

fn f32_at(pkt: &[u8], idx: usize) -> Option<f32> {
    let bytes = pkt.get(idx * 4..idx * 4 + 4)?;
    Some(f32::from_le_bytes(bytes.try_into().ok()?))
}

/// Parse one UDP packet in the classic float-array format.
///
/// Returns the game id (`dirt-rally-2` for the DR2-signature length, else
/// the `codemasters` family id) and the decoded sample, or `None` when the
/// packet is not this format or carries no usable engine data (menus and
/// replays send zeroed engine fields).
pub fn parse(pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
    if pkt.len() < BASE_LEN || pkt.len() > MAX_LEN || pkt.len() % 4 != 0 {
        return None;
    }
    let speed = f32_at(pkt, F_SPEED)?;
    let throttle = f32_at(pkt, F_THROTTLE)?;
    let rpm = f32_at(pkt, F_RPM)? * RPM_SCALE;
    let max_rpm = f32_at(pkt, F_MAX_RPM)? * RPM_SCALE;

    if !rpm.is_finite() || !max_rpm.is_finite() || !throttle.is_finite() || !speed.is_finite() {
        return None;
    }
    // Sanity gates: negative or absurd engine rates mean this is not a
    // classic-format packet (or the game is idling in a menu).
    if rpm < 0.0 || max_rpm <= 0.0 || max_rpm > 100_000.0 || rpm > 100_000.0 {
        return None;
    }

    let id = if pkt.len() == DR2_LEN { ID_DIRT_RALLY_2 } else { ID_FAMILY };
    Some((id, Telemetry { rpm, max_rpm, throttle: throttle.clamp(0.0, 1.0), speed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a classic packet of `floats` little-endian f32 slots with the
    /// engine fields set to known values.
    fn packet(floats: usize, rpm: f32, max_rpm: f32, throttle: f32, speed: f32) -> Vec<u8> {
        let mut pkt = vec![0u8; floats * 4];
        let mut put = |idx: usize, v: f32| {
            // Short fixtures (undersized packets) simply omit tail fields.
            if let Some(slot) = pkt.get_mut(idx * 4..idx * 4 + 4) {
                slot.copy_from_slice(&v.to_le_bytes());
            }
        };
        put(F_SPEED, speed);
        put(F_THROTTLE, throttle);
        put(F_RPM, rpm / RPM_SCALE);
        put(F_MAX_RPM, max_rpm / RPM_SCALE);
        pkt
    }

    #[test]
    fn dr2_length_maps_to_dirt_rally_2() {
        let pkt = packet(66, 6500.0, 7300.0, 0.5, 42.0);
        let (id, t) = parse(&pkt).unwrap();
        assert_eq!(id, ID_DIRT_RALLY_2);
        assert!((t.rpm - 6500.0).abs() < 0.5, "rpm {}", t.rpm);
        assert!((t.max_rpm - 7300.0).abs() < 0.5, "max_rpm {}", t.max_rpm);
        assert!((t.throttle - 0.5).abs() < 1e-6);
        assert!((t.speed - 42.0).abs() < 1e-6);
    }

    #[test]
    fn base_length_maps_to_family_id() {
        let pkt = packet(64, 3000.0, 8000.0, 1.0, 10.0);
        let (id, t) = parse(&pkt).unwrap();
        assert_eq!(id, ID_FAMILY);
        assert!((t.rpm - 3000.0).abs() < 0.5);
    }

    #[test]
    fn throttle_is_clamped() {
        let pkt = packet(66, 1000.0, 7000.0, 1.5, 0.0);
        let (_, t) = parse(&pkt).unwrap();
        assert_eq!(t.throttle, 1.0);
    }

    #[test]
    fn wrong_lengths_are_rejected() {
        assert!(parse(&packet(63, 3000.0, 8000.0, 0.5, 1.0)).is_none(), "too short");
        assert!(parse(&packet(71, 3000.0, 8000.0, 0.5, 1.0)).is_none(), "too long");
        let mut odd = packet(66, 3000.0, 8000.0, 0.5, 1.0);
        odd.push(0);
        assert!(parse(&odd).is_none(), "not a multiple of 4");
        assert!(parse(&[]).is_none(), "empty");
    }

    #[test]
    fn menu_packets_with_zero_max_rpm_are_rejected() {
        assert!(parse(&packet(66, 0.0, 0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn garbage_engine_values_are_rejected() {
        assert!(parse(&packet(66, -100.0, 8000.0, 0.5, 1.0)).is_none(), "negative rpm");
        assert!(parse(&packet(66, 3000.0, 2e6, 0.5, 1.0)).is_none(), "absurd max rpm");
        assert!(parse(&packet(66, f32::NAN, 8000.0, 0.5, 1.0)).is_none(), "nan rpm");
    }

    #[test]
    fn modern_f1_header_packet_is_ignored() {
        // Modern F1 motion packets are > 1 kB with a u16 packetFormat header;
        // they must not parse as the classic array.
        let mut pkt = vec![0u8; 1464];
        pkt[0..2].copy_from_slice(&2023u16.to_le_bytes());
        assert!(parse(&pkt).is_none());
    }
}
