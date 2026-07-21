// SPDX-License-Identifier: GPL-2.0-only
//! EA Sports WRC UDP telemetry.
//!
//! WRC has no fixed wire format: the game emits whatever channels a packet
//! structure file (`telemetry/udp/<id>.json`) lists, in that order, over
//! UDP. To get a stable format, logi-tf-sim defines its own compact packet
//! and parses exactly it; the user drops the matching structure file into
//! the game and points its channel at 127.0.0.1:20777 (the same port the
//! classic Codemasters and F1 formats use, disambiguated here by length).
//!
//! The `logi-tf-sim` WRC packet (`session_update`, empty header, 16 bytes),
//! all little-endian f32, in channel order:
//!
//! | offset | channel                      | unit |
//! |--------|------------------------------|------|
//! | 0      | vehicle_speed                | m/s  |
//! | 4      | vehicle_engine_rpm_current   | rpm  |
//! | 8      | vehicle_engine_rpm_max       | rpm  |
//! | 12     | vehicle_throttle             | 0..1 |
//!
//! Unlike Codemasters classic and F1, this packet DOES carry the redline
//! (`vehicle_engine_rpm_max`), so the parser is pure and needs no running
//! state. Sources: EA Sports WRC UDP telemetry docs (readme/channels.json)
//! and community structure files.

use crate::telemetry::Telemetry;

/// Game id for EA Sports WRC.
pub const ID: &str = "ea-wrc";

/// The `logi-tf-sim` WRC packet length (four f32 channels).
pub const PACKET_LEN: usize = 16;

const OFF_SPEED: usize = 0;
const OFF_RPM: usize = 4;
const OFF_MAX_RPM: usize = 8;
const OFF_THROTTLE: usize = 12;

/// Reject engine rates above this as not a real WRC sample.
const RPM_CEILING: f32 = 30_000.0;

fn f32_at(pkt: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(pkt.get(off..off + 4)?.try_into().ok()?))
}

/// Parse one `logi-tf-sim` WRC packet.
///
/// Returns the [`ID`] and a sample, or `None` for a wrong length or a
/// sample with no usable engine data (`max_rpm == 0`, sent in menus).
pub fn parse(pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
    if pkt.len() != PACKET_LEN {
        return None;
    }
    let speed = f32_at(pkt, OFF_SPEED)?;
    let rpm = f32_at(pkt, OFF_RPM)?;
    let max_rpm = f32_at(pkt, OFF_MAX_RPM)?;
    let throttle = f32_at(pkt, OFF_THROTTLE)?;

    if !speed.is_finite() || !throttle.is_finite() {
        return None;
    }
    if rpm < 0.0 || max_rpm <= 0.0 || rpm > RPM_CEILING || max_rpm > RPM_CEILING {
        return None;
    }
    Some((ID, Telemetry { rpm, max_rpm, throttle: throttle.clamp(0.0, 1.0), speed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(speed: f32, rpm: f32, max_rpm: f32, throttle: f32) -> Vec<u8> {
        let mut pkt = vec![0u8; PACKET_LEN];
        pkt[OFF_SPEED..OFF_SPEED + 4].copy_from_slice(&speed.to_le_bytes());
        pkt[OFF_RPM..OFF_RPM + 4].copy_from_slice(&rpm.to_le_bytes());
        pkt[OFF_MAX_RPM..OFF_MAX_RPM + 4].copy_from_slice(&max_rpm.to_le_bytes());
        pkt[OFF_THROTTLE..OFF_THROTTLE + 4].copy_from_slice(&throttle.to_le_bytes());
        pkt
    }

    #[test]
    fn wrc_packet_parses() {
        let (id, t) = parse(&packet(33.0, 5500.0, 7500.0, 0.6)).unwrap();
        assert_eq!(id, ID);
        assert_eq!(t.rpm, 5500.0);
        assert_eq!(t.max_rpm, 7500.0);
        assert!((t.throttle - 0.6).abs() < 1e-6);
        assert!((t.speed - 33.0).abs() < 1e-6);
    }

    #[test]
    fn throttle_is_clamped() {
        let (_, t) = parse(&packet(0.0, 1000.0, 7000.0, 1.4)).unwrap();
        assert_eq!(t.throttle, 1.0);
    }

    #[test]
    fn menu_and_garbage_samples_are_rejected() {
        assert!(parse(&packet(0.0, 0.0, 0.0, 0.0)).is_none(), "zero max rpm");
        assert!(parse(&packet(0.0, -10.0, 7000.0, 0.5)).is_none(), "negative rpm");
        assert!(parse(&packet(0.0, 5000.0, 99000.0, 0.5)).is_none(), "absurd max rpm");
        assert!(parse(&packet(f32::NAN, 5000.0, 7000.0, 0.5)).is_none(), "nan speed");
    }

    #[test]
    fn wrong_lengths_are_rejected() {
        let mut pkt = packet(0.0, 5000.0, 7000.0, 0.5);
        pkt.pop();
        assert!(parse(&pkt).is_none(), "15 bytes");
        assert!(parse(&[]).is_none(), "empty");
        // Must not swallow a classic-Codemasters-sized datagram.
        assert!(parse(&vec![0u8; 264]).is_none());
    }
}
