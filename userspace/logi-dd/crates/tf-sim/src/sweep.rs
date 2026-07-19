// SPDX-License-Identifier: GPL-2.0-only
//! `--sweep`: a synthetic RPM sweep through the full synthesis + stream
//! path, no game required. This is the hardware validation hook and the
//! target of the frontend's consent-gated "Test simulated TrueForce"
//! button. It drives the wheel with real haptic force.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::config::{Config, DEFAULT_INTENSITY};
use crate::error::Result;
use crate::synth::EngineSynth;
use crate::tf::TfStream;

/// Total sweep duration: idle -> redline -> idle.
pub const SWEEP_SECS: f32 = 6.0;
const IDLE_RPM: f32 = 1000.0;
const REDLINE_RPM: f32 = 7500.0;
/// Samples (= milliseconds) generated per push.
const CHUNK_MS: usize = 20;

/// The sweep profile at time `t` seconds: a symmetric triangle from
/// [`IDLE_RPM`] to [`REDLINE_RPM`] and back, throttle following the same
/// shape (rising on the way up, falling on the way down).
pub fn sweep_at(t: f32) -> (f32, f32) {
    let half = SWEEP_SECS / 2.0;
    let t = t.clamp(0.0, SWEEP_SECS);
    let frac = if t <= half { t / half } else { (SWEEP_SECS - t) / half };
    (IDLE_RPM + (REDLINE_RPM - IDLE_RPM) * frac, frac)
}

/// Play the sweep on controller 0 at the config's master intensity
/// (falling back to the default if it is set to 0, so the test is always
/// feelable), then exit. `stop` aborts early; the stream teardown clears
/// any queued force either way.
pub fn run(cfg: &Config, stop: &AtomicBool) -> Result<()> {
    let intensity_pct = if cfg.intensity == 0 { DEFAULT_INTENSITY } else { cfg.intensity };
    let intensity = f32::from(intensity_pct) / 100.0;

    eprintln!("logi-tf-sim: sweep: {SWEEP_SECS} s synthetic RPM sweep at intensity {intensity_pct}%");
    eprintln!("logi-tf-sim: sweep: the wheel WILL produce haptic force; hold the rim");
    for n in (1..=3u32).rev() {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        eprintln!("logi-tf-sim: sweep: starting in {n}...");
        thread::sleep(Duration::from_secs(1));
    }

    let mut stream = TfStream::open(0)?;
    let mut synth = EngineSynth::new();
    let mut samples = Vec::with_capacity(CHUNK_MS);

    let steps = (SWEEP_SECS * 1000.0) as usize / CHUNK_MS;
    for i in 0..steps {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let t = (i * CHUNK_MS) as f32 / 1000.0;
        let (rpm, throttle) = sweep_at(t);
        samples.clear();
        synth.generate(rpm, throttle, intensity, CHUNK_MS, &mut samples);
        stream.push(&samples)?;
        thread::sleep(Duration::from_millis(CHUNK_MS as u64));
    }

    // Let the library's 250 Hz thread drain the tail before teardown
    // clears the stream.
    thread::sleep(Duration::from_millis(200));
    eprintln!("logi-tf-sim: sweep: done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_profile_shape() {
        let (rpm, throttle) = sweep_at(0.0);
        assert_eq!((rpm, throttle), (IDLE_RPM, 0.0), "starts at idle");
        let (rpm, throttle) = sweep_at(SWEEP_SECS / 2.0);
        assert_eq!((rpm, throttle), (REDLINE_RPM, 1.0), "peaks at redline");
        let (rpm, throttle) = sweep_at(SWEEP_SECS);
        assert_eq!((rpm, throttle), (IDLE_RPM, 0.0), "returns to idle");
        // Clamped outside the window.
        assert_eq!(sweep_at(-1.0), sweep_at(0.0));
        assert_eq!(sweep_at(100.0), sweep_at(SWEEP_SECS));
    }

    #[test]
    fn sweep_profile_is_monotonic_per_half() {
        let mut prev = sweep_at(0.0).0;
        for i in 1..=30 {
            let rpm = sweep_at(i as f32 * 0.1).0;
            assert!(rpm >= prev, "rising half");
            prev = rpm;
        }
        for i in 31..=60 {
            let rpm = sweep_at(i as f32 * 0.1).0;
            assert!(rpm <= prev, "falling half");
            prev = rpm;
        }
    }
}
