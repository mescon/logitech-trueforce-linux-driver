// SPDX-License-Identifier: GPL-2.0-only
//! The force-feedback strength the running game asked for.
//!
//! A game's own force-feedback slider reaches the wheel as evdev
//! `FF_GAIN`, and the driver publishes what it was last told as
//! `wheel_ffb_game_gain`, a percentage. Reading it lets the synthesized
//! engine note obey that slider too.
//!
//! Why it matters: for a title with no TrueForce of its own, everything
//! the wheel does on the game's behalf comes from here, so a driver who
//! turns force feedback down and still feels a full-strength engine note
//! has been ignored. That was reported as the wheel buzzing with the car
//! parked and the strength at zero (issue #59).
//!
//! Absent attribute means no scaling rather than silence: a wheel whose
//! driver predates this, or one whose force path this does not describe,
//! must not lose its haptics to a file that is not there.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the value is re-read while streaming.
///
/// A game sets its gain when its settings change, not per frame, so this
/// is about noticing within a moment rather than tracking. Four times a
/// second costs one small read against a hundred blocks rendered.
const POLL: Duration = Duration::from_millis(250);

/// The attribute the driver publishes, in the wheel's own sysfs directory.
const ATTR: &str = "wheel_ffb_game_gain";

/// A cached reading of one wheel's game gain.
pub struct GameGain {
    path: Option<PathBuf>,
    value: f32,
    next: Instant,
}

impl GameGain {
    /// Watch the wheel whose sysfs directory is named `hid_id` (the same
    /// identity the rev-LED writer and the lease use). `None` disables the
    /// scaling, which is what a wheel with no such attribute gets.
    pub fn new(hid_id: Option<&str>, enabled: bool) -> Self {
        let path = if enabled {
            hid_id.map(|id| Path::new("/sys/bus/hid/devices").join(id).join(ATTR))
        } else {
            None
        };
        Self { path, value: 1.0, next: Instant::now() }
    }

    /// The current scale, 0.0..1.0, re-read at most every [`POLL`].
    ///
    /// A read that fails leaves the last value alone rather than falling
    /// back to full strength: a wheel unplugged mid-session should not
    /// produce a burst of full-strength haptics on its way out.
    pub fn scale(&mut self, now: Instant) -> f32 {
        let Some(path) = self.path.as_ref() else { return 1.0 };
        if now >= self.next {
            self.next = now + POLL;
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pct) = text.trim().parse::<u32>() {
                    self.value = (pct.min(100) as f32) / 100.0;
                }
            }
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_without_the_attribute_is_left_at_full_strength() {
        let mut g = GameGain::new(Some("no-such-device"), true);
        assert_eq!(g.scale(Instant::now()), 1.0);
    }

    #[test]
    fn disabled_never_scales() {
        let mut g = GameGain::new(Some("anything"), false);
        assert!(g.path.is_none());
        assert_eq!(g.scale(Instant::now()), 1.0);
    }

    /// The reading is a percentage and becomes a 0..1 scale, with a
    /// game's zero meaning silence rather than "unset".
    #[test]
    fn a_percentage_becomes_a_scale() {
        let dir = std::env::temp_dir().join(format!("gg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let attr = dir.join(ATTR);

        let mut g = GameGain { path: Some(attr.clone()), value: 1.0, next: Instant::now() };
        for (written, expected) in [("100\n", 1.0), ("50\n", 0.5), ("0\n", 0.0)] {
            std::fs::write(&attr, written).unwrap();
            // Force the poll rather than waiting for it.
            g.next = Instant::now();
            assert_eq!(g.scale(Instant::now()), expected, "gain {written:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
