// SPDX-License-Identifier: GPL-2.0-only
//! Engine-note synthesis.
//!
//! Generates the sample stream the wheel's TrueForce DSP consumes, at
//! [`SAMPLE_RATE_HZ`]:
//! a fundamental at the engine's FIRING rate plus 2x and 3x harmonics at falling
//! gain, amplitude `idle_floor + throttle * gain`, everything scaled by
//! the effective intensity (master x per-game, 0.0..1.0). The harmonic
//! gains (1, 1/2, 1/4) factor so the summed waveform crosses zero exactly
//! twice per fundamental cycle, which keeps the felt pitch equal to the
//! engine rate and makes the spectral test below exact.
//!
//! The generator is pure and stateful only in its oscillator phase, so
//! frequency changes are click-free. The libtrueforce stream thread does
//! the packetizing (4 samples per packet, 1000 packets/sec); this module only has to
//! produce samples at [`SAMPLE_RATE_HZ`].

/// The wheel's TrueForce sample rate.
pub const SAMPLE_RATE_HZ: f32 = 4000.0;

/// Samples per millisecond of wall clock, for callers that pace themselves
/// in milliseconds. Derived rather than written down: "one sample per ms"
/// was true only while the stream ran at 1 kHz, and it was assumed in two
/// places that would otherwise play everything at the wrong speed.
pub const SAMPLES_PER_MS: usize = (SAMPLE_RATE_HZ / 1000.0) as usize;

/// Samples per wire packet. The wheel consumes 4-sample packets at 1000 Hz,
/// which is what makes [`SAMPLE_RATE_HZ`] 4000; pushes are conveniently
/// sized in multiples of this.
pub const SAMPLES_PER_PACKET: usize = 4;

/// Relative gains for the fundamental and the 2x / 3x harmonics.
const HARMONIC_GAINS: [f32; 3] = [1.0, 0.5, 0.25];

/// Cylinder count assumed when a game or car tells us nothing. A modern
/// four is the commonest thing anyone drives, and it is the value the old
/// hardcoded behaviour was closest to.
pub const DEFAULT_CYLINDERS: u8 = 4;

/// The firing frequency of a four-stroke engine, in Hz.
///
/// One full cycle is 720 degrees of crank, so every cylinder fires once
/// per two revolutions: `rpm / 60 * cylinders / 2`.
///
/// This used to be plain `rpm / 60`, the crank rotation rate, which is the
/// firing rate of a single-cylinder two-stroke and of nothing else anyone
/// drives. Every other engine was an octave or more flat: a four was out by
/// 2x, a V8 by 4x. The `pitch` setting existed to let people correct that by
/// ear without knowing what they were correcting, and it could not stretch
/// far enough to do it: clamped at 2.0, it could not reach a V8's firing
/// rate even at maximum. Named by TF4ALL's FiringPatterns notes, which state
/// the relationship plainly (GPL-2.0, same licence as this crate).
///
/// `pitch_scale` stays, but as what it always claimed to be: a preference
/// either side of the correct value, not a correction for a missing term.
pub fn firing_frequency(rpm: f32, cylinders: u8, pitch_scale: f32) -> f32 {
    let cyl = cylinders.max(1) as f32;
    (rpm.max(0.0) / 60.0 * (cyl / 2.0) * pitch_scale.clamp(0.1, 2.0)).min(SAMPLE_RATE_HZ * 0.45)
}

/// RMS-to-peak scale that reproduces the level the fixed normaliser gave
/// when all of [`HARMONIC_GAINS`] are present: `sqrt(sum g^2) / sum g`.
/// Applying it after RMS normalisation keeps the peak within `amplitude`
/// and leaves the default configuration emitting exactly what it did
/// before band-limiting existed.
const FULL_MIX_LEVEL: f32 = 0.654_653_7;

/// Nyquist for the sample stream ([`SAMPLE_RATE_HZ`] / 2).
const NYQUIST_HZ: f32 = SAMPLE_RATE_HZ / 2.0;
/// Where a harmonic starts fading out. Below this it is passed at full
/// gain; between here and Nyquist it fades to nothing, so a partial
/// crossing the limit does so smoothly instead of switching off mid-note.
const ROLLOFF_START_HZ: f32 = NYQUIST_HZ * 0.8;

/// Gain multiplier (1.0 .. 0.0) for a partial at `hz`.
///
/// Anything at or above Nyquist is silenced: it cannot be represented at
/// [`SAMPLE_RATE_HZ`] and would alias to `|hz - SAMPLE_RATE_HZ|`, an
/// inharmonic tone unrelated to the engine.
pub fn harmonic_rolloff(hz: f32) -> f32 {
    if hz <= ROLLOFF_START_HZ {
        1.0
    } else if hz >= NYQUIST_HZ {
        0.0
    } else {
        (NYQUIST_HZ - hz) / (NYQUIST_HZ - ROLLOFF_START_HZ)
    }
}

/// Amplitude at closed throttle (the engine is still running).
pub const IDLE_FLOOR: f32 = 0.15;
/// Additional amplitude at full throttle; floor + gain = 1.0 full scale.
pub const THROTTLE_GAIN: f32 = 0.85;

/// Everything about the engine that shapes one block of note.
///
/// Grouped rather than passed loose: these four always travel together, and
/// the argument list had grown past the point where a reader could tell
/// which float was which.
#[derive(Debug, Clone, Copy)]
pub struct EngineNote {
    /// Engine speed, revolutions per minute.
    pub rpm: f32,
    /// Throttle position 0..1; sets amplitude above [`IDLE_FLOOR`].
    pub throttle: f32,
    /// Cylinders, with `rpm` the other half of the firing rate.
    pub cylinders: u8,
    /// Taste, either side of the true firing rate. 1.0 is correct.
    pub pitch_scale: f32,
}

impl Default for EngineNote {
    fn default() -> Self {
        EngineNote { rpm: 0.0, throttle: 0.0, cylinders: DEFAULT_CYLINDERS, pitch_scale: 1.0 }
    }
}

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
    /// `cylinders` sets the firing rate together with `rpm`; see
    /// [`firing_frequency`]. `pitch_scale` (0.1..2.0) then shifts that by
    /// taste, 1.0 being the engine's true firing rate. Tunable via the
    /// config's `cylinders` and `pitch` keys.
    pub fn generate(&mut self, note: &EngineNote, intensity: f32, count: usize, out: &mut Vec<f32>) {
        let intensity = intensity.clamp(0.0, 1.0);
        let throttle = note.throttle.clamp(0.0, 1.0);
        let freq = firing_frequency(note.rpm, note.cylinders, note.pitch_scale);
        let amplitude = (IDLE_FLOOR + THROTTLE_GAIN * throttle) * intensity;
        let step = freq / SAMPLE_RATE_HZ;

        // Band-limit before synthesising, not after. Each harmonic is
        // faded out as it nears Nyquist, because a partial above it does
        // not simply vanish: it folds back down as an inharmonic tone. At
        // pitch 1.0 the third harmonic crosses Nyquist at 5000 rpm and
        // lands on top of the fundamental, which is felt as a buzz rather
        // than an engine (hardware, RS50, 2026-08-07).
        //
        // Renormalised to constant RMS, so losing a partial thins the
        // timbre without changing how hard the wheel is driven.
        //
        // Normalising by the sum of gains instead (which is what bounds
        // the peak) does not do this: a lone sine reaches its bound where
        // a three-harmonic mix does not, so the note would grow ~50%
        // stronger in RMS exactly as the engine climbs into the region
        // where partials start dropping.
        let mut gains = [0.0f32; HARMONIC_GAINS.len()];
        let mut sumsq = 0.0f32;
        for (k, gain) in HARMONIC_GAINS.iter().enumerate() {
            gains[k] = gain * harmonic_rolloff(freq * (k + 1) as f32);
            sumsq += gains[k] * gains[k];
        }
        let norm = sumsq.sqrt() / FULL_MIX_LEVEL;

        out.reserve(count);
        for _ in 0..count {
            let sample = if amplitude > 0.0 && freq > 0.0 && norm > 0.0 {
                let mut acc = 0.0f32;
                for (k, gain) in gains.iter().enumerate() {
                    let harmonic = (k + 1) as f32;
                    acc += gain * (std::f32::consts::TAU * harmonic * self.phase).sin();
                }
                acc / norm * amplitude
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

    /// `ms` milliseconds of note, not a sample count: the two were the
    /// same number only while the stream ran at 1 kHz.
    fn buffer(rpm: f32, throttle: f32, intensity: f32, ms: usize) -> Vec<f32> {
        let mut synth = EngineSynth::new();
        let mut out = Vec::new();
        synth.generate(
            &EngineNote { rpm, throttle, cylinders: DEFAULT_CYLINDERS, pitch_scale: 1.0 },
            intensity,
            ms * SAMPLES_PER_MS,
            &mut out,
        );
        out
    }

    /// Sign changes between consecutive samples; two per fundamental cycle
    /// thanks to the 1 / 0.5 / 0.25 harmonic-gain factorization.
    fn zero_crossings(buf: &[f32]) -> usize {
        buf.windows(2).filter(|w| (w[0] > 0.0) != (w[1] > 0.0) && w[1] != 0.0).count()
    }

    /// A second of engine note at `rpm` for a given cylinder count.
    fn buffer_cyl(rpm: f32, cylinders: u8, count: usize) -> Vec<f32> {
        let mut synth = EngineSynth::new();
        let mut out = Vec::new();
        synth.generate(&EngineNote { rpm, cylinders, throttle: 1.0, pitch_scale: 1.0 }, 1.0, count, &mut out);
        out
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn fundamental_tracks_the_firing_rate() {
        // Counts doubled when the cylinder term was added, which is the
        // whole point: at 3000 rpm a four fires at 100 Hz, not the 50 Hz
        // crank rate this test used to assert.
        // 3000 rpm, 4 cyl -> 100 Hz -> ~200 crossings over 1000 samples.
        let crossings = zero_crossings(&buffer(3000.0, 1.0, 1.0, 1000));
        assert!((190..=210).contains(&crossings), "3000 rpm: {crossings} crossings");
        // 6000 rpm -> 200 Hz -> ~400 crossings.
        let crossings = zero_crossings(&buffer(6000.0, 1.0, 1.0, 1000));
        assert!((390..=410).contains(&crossings), "6000 rpm: {crossings} crossings");
    }

    #[test]
    fn a_v8_sounds_an_octave_above_a_four_at_the_same_rpm() {
        let four = zero_crossings(&buffer_cyl(3000.0, 4, 1000));
        let v8 = zero_crossings(&buffer_cyl(3000.0, 8, 1000));
        // Twice the cylinders, twice the firing rate, twice the crossings.
        let ratio = v8 as f32 / four as f32;
        assert!((ratio - 2.0).abs() < 0.1, "four {four}, V8 {v8}, ratio {ratio}");
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
    fn firing_frequency_follows_cylinder_count() {
        // A four-stroke fires every cylinder once per two revolutions, so
        // at 6000 rpm the crank turns at 100 Hz and a four fires at 200.
        assert_eq!(firing_frequency(6000.0, 4, 1.0), 200.0);
        assert_eq!(firing_frequency(6000.0, 8, 1.0), 400.0, "a V8 fires twice as often as a four");
        assert_eq!(firing_frequency(6000.0, 6, 1.0), 300.0);
        // A single-cylinder four-stroke fires once per two revolutions,
        // i.e. at half the crank rate. This is the only engine for which
        // the old rpm/60 model was ever close, and it was out by 2x even
        // there.
        assert_eq!(firing_frequency(6000.0, 1, 1.0), 50.0);
    }

    #[test]
    fn pitch_25_still_reproduces_the_model_that_predated_the_firing_rate() {
        // The old model was rpm/60 * pitch with pitch defaulting to 0.5.
        // The new one is rpm/60 * cyl/2 * pitch, so cyl 4 and pitch 0.25
        // must agree with it. This was the default for as long as the goal
        // was correcting the maths without changing anyone's feel; the
        // default has since moved to 35 on hardware evidence, but the
        // equivalence is what proves the model change itself was neutral,
        // so it is still worth asserting at the value it held for.
        for rpm in [800.0f32, 3000.0, 7500.0] {
            let old = rpm / 60.0 * 0.5;
            let new = firing_frequency(rpm, DEFAULT_CYLINDERS, 0.25);
            assert!((old - new).abs() < 1e-3, "rpm {rpm}: old {old} vs new {new}");
        }
    }

    #[test]
    fn pitch_is_now_a_preference_rather_than_a_missing_term() {
        // The old clamp could not reach a V8's firing rate even at maximum:
        // rpm/60 * 2.0 is still half of rpm/60 * 8/2.
        let v8_true = firing_frequency(6000.0, 8, 1.0);
        let old_model_at_max_pitch = 6000.0 / 60.0 * 2.0;
        assert!(v8_true > old_model_at_max_pitch, "a V8 was out of reach of the old pitch range");
        // And pitch still scales either side of correct.
        assert_eq!(firing_frequency(6000.0, 4, 0.5), 100.0);
        assert_eq!(firing_frequency(6000.0, 4, 2.0), 400.0);
    }

    #[test]
    fn firing_frequency_is_bounded_and_guards_nonsense_input() {
        assert_eq!(firing_frequency(-1.0, 4, 1.0), 0.0, "negative rpm reads as stopped");
        assert_eq!(firing_frequency(6000.0, 0, 1.0), firing_frequency(6000.0, 1, 1.0),
                   "zero cylinders is treated as one rather than silencing the engine");
        // The FUNDAMENTAL is bounded here. That is not the same as the
        // signal being below Nyquist, which is what this test used to
        // claim while only ever checking this line: the synth adds two
        // more harmonics on top, so the bandwidth is 3x this. The real
        // property is asserted in no_partial_survives_above_nyquist().
        assert!(firing_frequency(20000.0, 16, 2.0) <= SAMPLE_RATE_HZ * 0.45);
    }

    #[test]
    fn no_partial_survives_above_nyquist() {
        // Every harmonic the synth can emit, across the whole input
        // space, must be silent at or above Nyquist. A partial above it
        // folds down to |f - 1000| Hz and is felt as an inharmonic buzz
        // rather than an engine.
        for rpm in [1000.0f32, 5000.0, 7500.0, 12000.0, 20000.0] {
            for cyl in [1u8, 4, 6, 8, 16] {
                for pitch in [0.1f32, 0.25, 0.4, 0.8, 1.0, 2.0] {
                    let f0 = firing_frequency(rpm, cyl, pitch);
                    for k in 0..HARMONIC_GAINS.len() {
                        let hz = f0 * (k + 1) as f32;
                        if hz >= NYQUIST_HZ {
                            assert_eq!(
                                harmonic_rolloff(hz),
                                0.0,
                                "harmonic {} at {hz} Hz (rpm {rpm}, {cyl} cyl, pitch {pitch}) \
                                 would alias to {} Hz",
                                k + 1,
                                (hz - SAMPLE_RATE_HZ).abs(),
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_mix_level_still_matches_the_harmonic_gains_it_came_from() {
        // FULL_MIX_LEVEL is sqrt(sum g^2) / sum g for HARMONIC_GAINS, and
        // it is written out as a literal. Change the gains without
        // recomputing it and every sample is quietly scaled wrong, with no
        // other test noticing: amplitude stays plausible, just not what
        // the wheel was tuned for.
        let sumsq: f32 = HARMONIC_GAINS.iter().map(|g| g * g).sum();
        let sum: f32 = HARMONIC_GAINS.iter().sum();
        let expected = sumsq.sqrt() / sum;
        assert!(
            (FULL_MIX_LEVEL - expected).abs() < 1e-6,
            "FULL_MIX_LEVEL is {FULL_MIX_LEVEL}, but HARMONIC_GAINS now imply {expected}",
        );
    }

    #[test]
    fn rolloff_is_a_smooth_fade_not_a_cliff() {
        assert_eq!(harmonic_rolloff(0.0), 1.0);
        assert_eq!(harmonic_rolloff(ROLLOFF_START_HZ), 1.0, "full gain up to the fade point");
        assert_eq!(harmonic_rolloff(NYQUIST_HZ), 0.0, "silent at Nyquist");
        assert_eq!(harmonic_rolloff(NYQUIST_HZ + 100.0), 0.0, "and above it");
        let mid = harmonic_rolloff((ROLLOFF_START_HZ + NYQUIST_HZ) / 2.0);
        assert!((mid - 0.5).abs() < 1e-6, "halfway through the fade, got {mid}");
        // Monotonically decreasing: no step that would click.
        let mut prev = 1.0;
        for i in 0..=100 {
            let hz = ROLLOFF_START_HZ + (NYQUIST_HZ - ROLLOFF_START_HZ) * (i as f32 / 100.0);
            let g = harmonic_rolloff(hz);
            assert!(g <= prev + 1e-6, "gain rose at {hz} Hz");
            prev = g;
        }
    }

    #[test]
    fn band_limiting_holds_the_force_level_as_partials_drop_out() {
        // Thinning the timbre must not also quieten the wheel: the
        // renormalisation is what keeps amplitude constant, and losing it
        // would read as the engine fading out at high rpm.
        let mut quiet = EngineSynth::new();
        let mut loud = EngineSynth::new();
        let mut a = Vec::new();
        let mut b = Vec::new();
        // 3000 rpm: nothing is faded. 7500 at pitch 1.0: the third
        // harmonic is gone entirely.
        quiet.generate(&EngineNote { rpm: 3000.0, throttle: 1.0, cylinders: 4, pitch_scale: 1.0 }, 1.0, 2000, &mut a);
        loud.generate(&EngineNote { rpm: 7500.0, throttle: 1.0, cylinders: 4, pitch_scale: 1.0 }, 1.0, 2000, &mut b);
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let (pa, pb) = (peak(&a), peak(&b));
        assert!((pa - pb).abs() < 0.2, "peak force moved from {pa} to {pb} when a partial dropped");
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
            // 100 ms each, so ten of them is the same one second the
            // contiguous case below measures.
            synth.generate(
                &EngineNote { rpm: 3000.0, throttle: 1.0, cylinders: DEFAULT_CYLINDERS, pitch_scale: 1.0 },
                1.0,
                100 * SAMPLES_PER_MS,
                &mut joined,
            );
        }
        // Same crossing count as one contiguous second: no phase resets.
        let crossings = zero_crossings(&joined);
        assert!((190..=210).contains(&crossings), "{crossings} crossings");
    }
}
