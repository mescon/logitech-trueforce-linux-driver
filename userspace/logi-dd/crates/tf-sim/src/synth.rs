// SPDX-License-Identifier: GPL-2.0-only
//! Engine-note synthesis.
//!
//! Generates the 1 kHz sample stream the wheel's TrueForce DSP consumes:
//! a fundamental at `rpm / 60` Hz plus 2x and 3x harmonics at falling
//! gain, amplitude `idle_floor + throttle * gain`, everything scaled by
//! the effective intensity (master x per-game, 0.0..1.0). The harmonic
//! gains (1, 1/2, 1/4) factor so the summed waveform crosses zero exactly
//! twice per fundamental cycle, which keeps the felt pitch equal to the
//! engine rate and makes the spectral test below exact.
//!
//! The generator is pure and stateful only in its oscillator phase, so
//! frequency changes are click-free. The libtrueforce stream thread does
//! the packetizing (4 samples per 250 Hz packet); this module only has to
//! produce samples at [`SAMPLE_RATE_HZ`].

/// The wheel's TrueForce sample rate.
pub const SAMPLE_RATE_HZ: f32 = 1000.0;

/// Samples per wire packet (the wheel consumes 4-sample packets at
/// 250 Hz); pushes are conveniently sized in multiples of this.
pub const SAMPLES_PER_PACKET: usize = 4;

/// Relative gains for the fundamental and the 2x / 3x harmonics.
const HARMONIC_GAINS: [f32; 3] = [1.0, 0.5, 0.25];
/// Sum of [`HARMONIC_GAINS`]; normalizes the mix so |sample| <= amplitude.
const GAIN_NORM: f32 = 1.75;

/// Amplitude at closed throttle (the engine is still running).
pub const IDLE_FLOOR: f32 = 0.15;
/// Additional amplitude at full throttle; floor + gain = 1.0 full scale.
pub const THROTTLE_GAIN: f32 = 0.85;

/// Phase-continuous engine-note generator.
#[derive(Debug, Default)]
pub struct EngineSynth {
    /// Fundamental phase in cycles, kept in [0, 1).
    phase: f32,
}

impl EngineSynth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `count` samples for the given engine state to `out`.
    ///
    /// `rpm` sets the fundamental (`rpm / 60` Hz, capped below Nyquist),
    /// `throttle` (0..1) sets the amplitude above [`IDLE_FLOOR`], and
    /// `intensity` (0..1) scales the result. Intensity 0 emits exact
    /// silence. Out-of-range inputs are clamped.
    pub fn generate(&mut self, rpm: f32, throttle: f32, intensity: f32, count: usize, out: &mut Vec<f32>) {
        let intensity = intensity.clamp(0.0, 1.0);
        let throttle = throttle.clamp(0.0, 1.0);
        let freq = (rpm.max(0.0) / 60.0).min(SAMPLE_RATE_HZ * 0.45);
        let amplitude = (IDLE_FLOOR + THROTTLE_GAIN * throttle) * intensity;
        let step = freq / SAMPLE_RATE_HZ;

        out.reserve(count);
        for _ in 0..count {
            let sample = if amplitude > 0.0 && freq > 0.0 {
                let mut acc = 0.0f32;
                for (k, gain) in HARMONIC_GAINS.iter().enumerate() {
                    let harmonic = (k + 1) as f32;
                    acc += gain * (std::f32::consts::TAU * harmonic * self.phase).sin();
                }
                acc / GAIN_NORM * amplitude
            } else {
                0.0
            };
            out.push(sample);
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(rpm: f32, throttle: f32, intensity: f32, count: usize) -> Vec<f32> {
        let mut synth = EngineSynth::new();
        let mut out = Vec::new();
        synth.generate(rpm, throttle, intensity, count, &mut out);
        out
    }

    /// Sign changes between consecutive samples; two per fundamental cycle
    /// thanks to the 1 / 0.5 / 0.25 harmonic-gain factorization.
    fn zero_crossings(buf: &[f32]) -> usize {
        buf.windows(2).filter(|w| (w[0] > 0.0) != (w[1] > 0.0) && w[1] != 0.0).count()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn fundamental_tracks_rpm() {
        // 3000 rpm -> 50 Hz -> ~100 crossings over 1000 samples (1 s).
        let crossings = zero_crossings(&buffer(3000.0, 1.0, 1.0, 1000));
        assert!((95..=105).contains(&crossings), "3000 rpm: {crossings} crossings");
        // 6000 rpm -> 100 Hz -> ~200 crossings.
        let crossings = zero_crossings(&buffer(6000.0, 1.0, 1.0, 1000));
        assert!((190..=210).contains(&crossings), "6000 rpm: {crossings} crossings");
    }

    #[test]
    fn amplitude_scales_linearly_with_intensity() {
        let full = peak(&buffer(3000.0, 1.0, 1.0, 1000));
        let half = peak(&buffer(3000.0, 1.0, 0.5, 1000));
        assert!(full > 0.5, "full-intensity peak {full}");
        assert!((full / half - 2.0).abs() < 1e-3, "ratio {}", full / half);
    }

    #[test]
    fn amplitude_rises_with_throttle_above_the_idle_floor() {
        let idle = peak(&buffer(3000.0, 0.0, 1.0, 1000));
        let wot = peak(&buffer(3000.0, 1.0, 1.0, 1000));
        assert!(idle > 0.0, "idle floor keeps the engine audible");
        assert!(wot / idle > 4.0, "throttle swing: idle {idle}, wot {wot}");
    }

    #[test]
    fn silence_at_intensity_zero() {
        assert!(buffer(6000.0, 1.0, 0.0, 500).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silence_at_zero_rpm() {
        assert!(buffer(0.0, 1.0, 1.0, 500).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn samples_stay_in_range_and_inputs_are_clamped() {
        let buf = buffer(50_000.0, 7.0, 3.0, 2000);
        assert!(buf.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn phase_is_continuous_across_calls() {
        let mut synth = EngineSynth::new();
        let mut joined = Vec::new();
        for _ in 0..10 {
            synth.generate(3000.0, 1.0, 1.0, 100, &mut joined);
        }
        // Same crossing count as one contiguous second: no phase resets.
        let crossings = zero_crossings(&joined);
        assert!((95..=105).contains(&crossings), "{crossings} crossings");
    }
}
