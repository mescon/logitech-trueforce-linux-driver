// SPDX-License-Identifier: GPL-2.0-only
//! Rev-display feeder: mirrors telemetry RPM onto the wheel's rev LEDs
//! through the driver's `wheel_rev_level` sysfs attribute (0-10 LEDs lit;
//! on the RS50 the fill uses the active LIGHTSYNC slot's colours and
//! direction, on a real G PRO rim the onboard profile owns the colours).
//!
//! Pacing: the protocol docs require rev-level writes no faster than
//! ~160 ms apart (faster bursts starve the wheel's shared HID++ command
//! processor and can cut FFB), so [`RevLeds::update`] rate-limits itself
//! and only writes when the level actually changed. Everything here is
//! best-effort: a wheel without the attribute, a failed write or a missing
//! driver never disturbs the TrueForce stream that rides alongside.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The driver attribute this module writes.
pub const ATTR: &str = "wheel_rev_level";

/// The sibling attribute whose re-write restores the idle pattern.
const IDLE_ATTR: &str = "wheel_led_effect";

/// Where the driver's per-device attribute directories live.
const SYSFS_ROOT: &str = "/sys/bus/hid/devices";

/// Minimum spacing between two rev-level writes (the ~160 ms floor from
/// the protocol docs).
pub const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(160);

/// The rev level for `rpm` out of `max_rpm`: `round(10 * rpm / max_rpm)`
/// clamped to 0-10. A zero (or negative, or NaN) `max_rpm` reads as 0 so
/// a car that never reported its limiter shows a dark strip instead of a
/// division artifact.
pub fn rev_level(rpm: f32, max_rpm: f32) -> u8 {
    if max_rpm.is_nan() || max_rpm <= 0.0 {
        return 0;
    }
    // NaN rpm falls out as 0 through the `as` cast's saturation.
    (10.0 * rpm / max_rpm).round().clamp(0.0, 10.0) as u8
}

/// One wheel's rev display, found at stream start and driven while
/// telemetry flows.
pub struct RevLeds {
    /// Full path of the `wheel_rev_level` attribute file.
    attr: PathBuf,
    /// The last level actually written; writes are skipped while the
    /// level is unchanged.
    last_level: Option<u8>,
    /// When the last write landed, for the pacing floor.
    last_write: Option<Instant>,
}

impl RevLeds {
    /// Scan for a wheel exposing [`ATTR`]. `LOGI_DD_SYSFS_DIR`, when set,
    /// overrides the scan with that directory (the same development/test
    /// override the logi-dd front-ends honor); otherwise the first match
    /// under `/sys/bus/hid/devices/*/wheel_rev_level` wins.
    pub fn discover() -> Option<RevLeds> {
        if let Ok(dir) = std::env::var("LOGI_DD_SYSFS_DIR") {
            let attr = PathBuf::from(dir).join(ATTR);
            return attr.exists().then(|| RevLeds::at(attr));
        }
        for entry in std::fs::read_dir(SYSFS_ROOT).ok()?.flatten() {
            let attr = entry.path().join(ATTR);
            if attr.exists() {
                return Some(RevLeds::at(attr));
            }
        }
        None
    }

    /// A rev display at an explicit attribute path (tests point this at a
    /// plain file in a temp directory).
    pub fn at(attr: PathBuf) -> RevLeds {
        RevLeds { attr, last_level: None, last_write: None }
    }

    /// Feed one telemetry sample at time `now` (injected so tests control
    /// the clock). Writes the new level only when it CHANGED and the last
    /// write is at least [`MIN_WRITE_INTERVAL`] old; a skipped change is
    /// picked up by a later call since `last_level` still differs. Write
    /// failures are ignored (and not recorded, so the level is retried).
    pub fn update(&mut self, rpm: f32, max_rpm: f32, now: Instant) {
        let level = rev_level(rpm, max_rpm);
        if self.last_level == Some(level) {
            return;
        }
        if self.last_write.is_some_and(|t| now.duration_since(t) < MIN_WRITE_INTERVAL) {
            return;
        }
        if std::fs::write(&self.attr, level.to_string()).is_ok() {
            self.last_level = Some(level);
            self.last_write = Some(now);
        }
    }

    /// Blank the display and hand the strip back: write level 0, then
    /// restore the idle pattern by reading the sibling `wheel_led_effect`
    /// and writing the same value back (the driver re-applies the effect,
    /// which exits the rev fill). Both writes are best-effort; called on
    /// telemetry silence and on shutdown.
    pub fn stop(&mut self) {
        let _ = std::fs::write(&self.attr, "0");
        if let Some(dir) = self.attr.parent() {
            let idle = dir.join(IDLE_ATTR);
            if let Ok(current) = std::fs::read_to_string(&idle) {
                let _ = std::fs::write(&idle, current.trim());
            }
        }
        self.last_level = None;
        self.last_write = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, unique temp directory per test (std only, same pattern as
    /// the config tests).
    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-sim-leds-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn rev_level_maps_the_rpm_range_onto_0_to_10() {
        assert_eq!(rev_level(0.0, 8000.0), 0);
        assert_eq!(rev_level(4000.0, 8000.0), 5);
        assert_eq!(rev_level(8000.0, 8000.0), 10);
        assert_eq!(rev_level(360.0, 8000.0), 0, "rounds down below half a step");
        assert_eq!(rev_level(440.0, 8000.0), 1, "rounds up above half a step");
        assert_eq!(rev_level(9000.0, 8000.0), 10, "over-rev clamps");
        assert_eq!(rev_level(-100.0, 8000.0), 0, "negative rpm clamps");
    }

    #[test]
    fn rev_level_guards_a_missing_limiter() {
        assert_eq!(rev_level(5000.0, 0.0), 0);
        assert_eq!(rev_level(5000.0, -1.0), 0);
        assert_eq!(rev_level(5000.0, f32::NAN), 0);
        assert_eq!(rev_level(f32::NAN, 8000.0), 0);
    }

    #[test]
    fn update_writes_only_changed_levels() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();

        leds.update(0.0, 100.0, t0);
        assert_eq!(read(&attr), "0", "first sample always lands");

        // Same level again, well past the pacing floor: no write (pinned
        // via a sentinel the skipped write would have replaced).
        fs::write(&attr, "sentinel").unwrap();
        leds.update(1.0, 100.0, t0 + Duration::from_secs(1));
        assert_eq!(read(&attr), "sentinel", "unchanged level writes nothing");

        leds.update(50.0, 100.0, t0 + Duration::from_secs(2));
        assert_eq!(read(&attr), "5", "changed level lands");
    }

    #[test]
    fn update_respects_the_160ms_floor() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();

        leds.update(20.0, 100.0, t0);
        assert_eq!(read(&attr), "2");

        // A changed level inside the floor is skipped...
        leds.update(50.0, 100.0, t0 + Duration::from_millis(50));
        assert_eq!(read(&attr), "2", "no write 50 ms after the last one");

        // ...and picked up by the next call past it (the level still
        // differs from the last WRITTEN one).
        leds.update(50.0, 100.0, t0 + Duration::from_millis(160));
        assert_eq!(read(&attr), "5");
    }

    #[test]
    fn stop_blanks_and_restores_the_idle_pattern() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        let idle = dir.join("wheel_led_effect");
        fs::write(&attr, "").unwrap();
        fs::write(&idle, "5\n").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        leds.update(100.0, 100.0, Instant::now());
        assert_eq!(read(&attr), "10");

        leds.stop();
        assert_eq!(read(&attr), "0", "display blanked");
        assert_eq!(read(&idle), "5", "current effect written back to exit the fill");

        // After a stop the feeder starts fresh: the next sample writes
        // regardless of what was last written before the stop.
        leds.update(0.0, 100.0, Instant::now());
        assert_eq!(read(&attr), "0");
    }

    #[test]
    fn stop_without_an_idle_attr_still_blanks() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "7").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        leds.stop();
        assert_eq!(read(&attr), "0");
    }

    #[test]
    fn discover_honors_the_sysfs_dir_override() {
        // The only test here that touches the environment; nothing else
        // in this crate reads LOGI_DD_SYSFS_DIR, so it cannot race the
        // other tests.
        let dir = tempdir();
        std::env::set_var("LOGI_DD_SYSFS_DIR", &dir);
        assert!(RevLeds::discover().is_none(), "no attribute file yet");
        fs::write(dir.join(ATTR), "0").unwrap();
        let leds = RevLeds::discover().expect("attribute file present");
        assert_eq!(leds.attr, dir.join(ATTR));
        std::env::remove_var("LOGI_DD_SYSFS_DIR");
    }
}
