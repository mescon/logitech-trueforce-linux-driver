// SPDX-License-Identifier: GPL-2.0-only
//! The normalized telemetry sample all parsers decode into.

/// One decoded telemetry sample, normalized across wire formats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Telemetry {
    /// Engine speed in revolutions per minute.
    pub rpm: f32,
    /// Engine redline in revolutions per minute (> 0 for a valid sample).
    pub max_rpm: f32,
    /// Throttle position, 0.0 to 1.0.
    pub throttle: f32,
    /// Vehicle speed in meters per second.
    pub speed: f32,
}
