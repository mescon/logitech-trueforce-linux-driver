// SPDX-License-Identifier: GPL-2.0-only
//! Telemetry-driven haptic effects, mixed into the TrueForce sample stream.
//!
//! The engine note ([`crate::synth`]) is one layer of what a wheel can be
//! told to feel. This module adds the rest: the limiters, the drivetrain
//! events, the surface, and the impacts. Each is an [`Effect`] that reads
//! the normalized [`Telemetry`] sample and writes 1 kHz audio into the
//! stream, and [`Mixer`] sums them.
//!
//! ## What is real and what is inert
//!
//! An effect can only be as good as its input. [`Telemetry`] defaults every
//! channel to the value meaning "not happening", so an effect whose input no
//! format supplies produces silence rather than a guess. That is deliberate:
//! it lets the whole set ship now and light up per format as the decoders
//! learn to fill the channels, instead of holding all ten back for the
//! slowest one.
//!
//! Today OutGauge (BeamNG and anything else speaking it) feeds the engine,
//! both limiters, gear shifts, ABS and traction. Nothing yet feeds surface
//! roughness, impacts, airborne or DRS, so [`RoadBumps`], [`Collision`],
//! [`Airborne`] and [`Drs`] are silent everywhere until a decoder supplies
//! them. They are implemented rather than deferred because the missing part
//! is a decoder field, not the effect.
//!
//! ## Two layers, not one sum
//!
//! Most effects add. Airborne does not: with the wheels off the ground there
//! is no road to feel, so it attenuates rather than contributes. The mixer
//! therefore keeps a continuous layer (engine, limiters, surface, slip) apart
//! from transients (shifts, clicks, impacts), applies ducking to the
//! continuous layer alone, and sums after. An impact must still be felt
//! mid-flight, which is exactly what that split buys.
//!
//! ## The 1 kHz ceiling
//!
//! # Low-frequency layers on a direct-drive wheel
//!
//! Three layers sit well below the engine note: the pit limiter at 10 Hz,
//! ABS at 15 Hz and the rev limiter at 25 Hz, against an engine note that
//! runs from about 12 Hz at idle to 250 Hz at the redline.
//!
//! That matters more than it looks. Wheel excursion for a given torque goes
//! roughly as 1/f^2, so at equal amplitude a 10 Hz layer displaces the rim
//! on the order of a hundred times further than a 100 Hz one: it steers the
//! wheel where the engine note vibrates it. Measured on an RS50 (2026-08-08)
//! the same engine note moved the rim 899 degrees at pitch 25 and 216 at
//! pitch 45, purely from the frequency change, and an isolated 25 Hz rev
//! limiter at full gain was violent.
//!
//! So these gains were chosen as torque levels while what is actually felt
//! is excursion. **Decided (2026-08-08): do not compensate for that with a
//! frequency curve.** Three reasons, and the first is the one that settles
//! it:
//!
//! - The 1/f^2 relation is for a FREE wheel. In use the rim is held, and a
//!   hand adds damping and stiffness that dominate at exactly the low
//!   frequencies in question, so compensating would attenuate hardest where
//!   the hand already attenuates hardest: correcting the same thing twice.
//!   Every measurement behind this note was taken hands-off.
//! - Constant excursion is the wrong target anyway. Tactile sensitivity is
//!   not flat and below roughly 40 Hz displacement is what is perceived at
//!   all, so equalising excursion would cut the 10 Hz pit limiter to about
//!   a hundredth of its amplitude, which is deletion rather than balance.
//! - The gains above are an importance ranking with no relation to
//!   frequency. A curve on top would look principled while resting on an
//!   uncalibrated exponent, which is worse than an honest ranking because
//!   it discourages the measurement that would settle it.
//!
//! The hazard is real but it lives in hands-off bench testing, not driving:
//! isolated, at high gain, with nothing else mixed in. That is bounded where
//! it belongs, in the self-test path, which already refuses to run without
//! the force-feedback session that keeps the wheel stable.
//!
//! If these are ever retuned, measure excursion per layer with
//! `tools/wheel-rotation-watch.py` and cap it, rather than modelling it.
//! Meanwhile treat any layer below roughly 40 Hz as a thing that moves the
//! wheel, and test it at low gain first.
//!
//! The stream runs at [`SAMPLE_RATE_HZ`], so nothing here may approach
//! 500 Hz. That is not much of a constraint in practice: the wheel is a
//! motor moving a rim with real inertia, and the frequencies a driver feels
//! through it are tens of hertz. Every constant below is chosen in that band.

use crate::synth::{EngineNote, EngineSynth, SAMPLE_RATE_HZ};
use crate::telemetry::Telemetry;

/// Per-effect identity: the config key it answers to and its default gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffectId {
    Engine,
    RevLimiter,
    PitLimiter,
    GearShift,
    Abs,
    TractionLoss,
    RoadBumps,
    Airborne,
    Collision,
    Drs,
}

impl EffectId {
    /// Every effect, in mix order.
    pub const ALL: [EffectId; 10] = [
        EffectId::Engine,
        EffectId::RevLimiter,
        EffectId::PitLimiter,
        EffectId::GearShift,
        EffectId::Abs,
        EffectId::TractionLoss,
        EffectId::RoadBumps,
        EffectId::Airborne,
        EffectId::Collision,
        EffectId::Drs,
    ];

    /// The config key suffix, as in `effect_rev_limiter`.
    pub fn key(self) -> &'static str {
        match self {
            EffectId::Engine => "engine",
            EffectId::RevLimiter => "rev_limiter",
            EffectId::PitLimiter => "pit_limiter",
            EffectId::GearShift => "gear_shift",
            EffectId::Abs => "abs",
            EffectId::TractionLoss => "traction_loss",
            EffectId::RoadBumps => "road_bumps",
            EffectId::Airborne => "airborne",
            EffectId::Collision => "collision",
            EffectId::Drs => "drs",
        }
    }

    /// Resolve a config key suffix to its effect.
    pub fn from_key(key: &str) -> Option<EffectId> {
        EffectId::ALL.into_iter().find(|e| e.key() == key)
    }

    /// Default gain in percent.
    ///
    /// The engine sits at 100 because it is the layer that already shipped
    /// and its level is the one users have tuned `intensity` against. The
    /// rest are deliberately below it: they are additions to a mix that was
    /// already balanced, and an effect nobody has felt yet should not be the
    /// loudest thing in it.
    pub fn default_gain(self) -> u8 {
        match self {
            EffectId::Engine => 100,
            EffectId::Collision => 80,
            EffectId::Airborne => 85,
            EffectId::RevLimiter => 70,
            EffectId::GearShift => 60,
            EffectId::Abs => 60,
            EffectId::PitLimiter => 50,
            EffectId::TractionLoss => 50,
            EffectId::RoadBumps => 40,
            EffectId::Drs => 40,
        }
    }
}

/// Gain per effect, in percent. 0 silences one without disabling the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectGains {
    gains: [u8; EffectId::ALL.len()],
}

impl Default for EffectGains {
    fn default() -> Self {
        let mut gains = [0u8; EffectId::ALL.len()];
        for id in EffectId::ALL {
            gains[id as usize] = id.default_gain();
        }
        EffectGains { gains }
    }
}

impl EffectGains {
    pub fn get(&self, id: EffectId) -> u8 {
        self.gains[id as usize]
    }

    pub fn set(&mut self, id: EffectId, pct: u8) {
        self.gains[id as usize] = pct.min(100);
    }

    fn scale(&self, id: EffectId) -> f32 {
        f32::from(self.get(id)) / 100.0
    }
}

/// One haptic layer.
///
/// [`update`](Effect::update) folds in the newest telemetry and is called
/// once per block; [`render`](Effect::render) then adds that block's samples.
/// Splitting them keeps effects phase-continuous across blocks whose length
/// varies with scheduling, which is what stops the mix clicking when the
/// daemon's loop jitters.
pub trait Effect {
    fn id(&self) -> EffectId;

    /// Fold in the newest telemetry sample.
    /// `block_ms` is how much time this block covers.
    ///
    /// Needed because anything time-dependent here runs once per block, and
    /// a block is not a millisecond: the daemon renders roughly 50 ms at a
    /// time. Two effects assumed otherwise and were wrong by that factor in
    /// every real session, while looking correct under tests that rendered
    /// one sample at a time.
    fn update(&mut self, tel: &Telemetry, block_ms: f32);

    /// Add this effect's contribution to `out`, one sample per element,
    /// scaled by `gain`.
    ///
    /// Additive, never assigning: the buffer already holds the layers mixed
    /// before this one. Gain is applied here, on this effect's own samples,
    /// rather than to the sum: that keeps each effect's internal state
    /// (oscillator phase, burst envelope) independent of its level, so
    /// turning one down and back up does not make it jump.
    ///
    /// An effect must still advance its state when `gain` is 0, or it
    /// resumes mid-event when the level returns.
    fn render(&mut self, out: &mut [f32], gain: f32);

    /// Whether this belongs to the continuous layer, and so is subject to
    /// ducking. Transients (a shift, an impact) are not: they must still be
    /// felt while the car is airborne.
    fn continuous(&self) -> bool {
        true
    }

    /// Attenuation to apply to the continuous layer, 1.0 being none.
    fn duck(&self) -> f32 {
        1.0
    }
}

// ---------------------------------------------------------------------
// Shared building blocks
// ---------------------------------------------------------------------

/// Deterministic noise source.
///
/// Effects that need noise (surface, slip) need it reproducible: a test that
/// asserts on an amplitude envelope cannot do so against a system RNG, and a
/// haptic bug that only shows up with one seed is not one anybody could
/// reproduce. Xorshift32 is far more randomness than a rumble needs.
#[derive(Debug, Clone, Copy)]
struct Noise {
    state: u32,
}

impl Noise {
    fn new(seed: u32) -> Self {
        // Any nonzero state; xorshift is stuck at zero.
        Noise { state: seed | 1 }
    }

    /// Next sample in -1.0..1.0.
    fn next(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        // Map the top bits to a signed unit range.
        (self.state >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// One-pole low-pass, used to keep noise-based effects in the band a wheel
/// can actually reproduce and to smooth level changes into ramps.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    y: f32,
}

/// Below this, a smoother is close enough to its target to snap.
///
/// An exponential approach never actually arrives, which would leave a
/// silenced effect emitting an ever-smaller nonzero forever. The stream
/// quantizes to 16 bits, so anything under ~1.5e-5 is already unrepresentable
/// on the wire; snapping well below that costs nothing audible and makes
/// "off" mean exactly zero, which is the thing a test can assert and a
/// listener can hear.
const SMOOTH_SNAP: f32 = 1e-6;

impl OnePole {
    /// `a` is the per-sample coefficient: smaller is smoother.
    fn step(&mut self, x: f32, a: f32) -> f32 {
        self.y += (x - self.y) * a;
        if (x - self.y).abs() < SMOOTH_SNAP {
            self.y = x;
        }
        self.y
    }
}

/// Per-sample coefficient for a smoother reaching ~63% of a step in `ms`.
fn smoothing_for(ms: f32) -> f32 {
    (1.0 / (ms.max(1.0) * SAMPLE_RATE_HZ / 1000.0)).clamp(0.0, 1.0)
}

/// The same smoother applied once for a whole block instead of per sample.
///
/// A one-pole stepped `n` times moves `1 - (1-a)^n`, so a caller that steps
/// once per block has to use that, not the per-sample coefficient. Using
/// the per-sample value once per block is what made the airborne duck take
/// about three seconds to ramp in a real session instead of the 60 ms it
/// asks for: correct only under tests that rendered a single sample at a
/// time, where a block and a sample were the same thing.
fn smoothing_for_block(ms: f32, block_ms: f32) -> f32 {
    let a = smoothing_for(ms);
    let n = (block_ms * SAMPLE_RATE_HZ / 1000.0).max(1.0);
    (1.0 - (1.0 - a).powf(n)).clamp(0.0, 1.0)
}

/// A decaying oscillator burst: the shape of every transient here.
///
/// Amplitude falls exponentially from `peak` with time-constant `decay_ms`
/// while a sine at `freq_hz` runs underneath, which is what a struck object
/// does and so what a shift or an impact should feel like.
#[derive(Debug, Clone, Copy, Default)]
struct Burst {
    remaining: usize,
    phase: f32,
    amp: f32,
    freq_hz: f32,
    decay: f32,
}

impl Burst {
    /// Arm (or re-arm) the burst. Re-arming mid-burst restarts it rather
    /// than layering, so a fast double-shift feels like two hits and not one
    /// loud one.
    fn fire(&mut self, peak: f32, freq_hz: f32, decay_ms: f32, len_ms: f32) {
        self.remaining = (len_ms * SAMPLE_RATE_HZ / 1000.0) as usize;
        self.phase = 0.0;
        self.amp = peak;
        self.freq_hz = freq_hz;
        self.decay = (-1.0 / (decay_ms.max(1.0) * SAMPLE_RATE_HZ / 1000.0)).exp();
    }

    /// Whether the burst still has samples to emit. Only the tests read
    /// this: the render path just runs the counter down.
    #[cfg(test)]
    fn active(&self) -> bool {
        self.remaining > 0
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        if gain <= 0.0 {
            // Still consume the burst: a silenced effect must not resume
            // audibly when its gain comes back up mid-event.
            self.remaining = self.remaining.saturating_sub(out.len());
            return;
        }
        let step = self.freq_hz / SAMPLE_RATE_HZ;
        for slot in out.iter_mut() {
            if self.remaining == 0 {
                break;
            }
            *slot += (std::f32::consts::TAU * self.phase).sin() * self.amp * gain;
            self.phase = (self.phase + step).fract();
            self.amp *= self.decay;
            self.remaining -= 1;
        }
    }
}

// ---------------------------------------------------------------------
// The effects
// ---------------------------------------------------------------------

/// The engine note: harmonics at the firing rate. See [`crate::synth`].
pub struct EnginePulse {
    synth: EngineSynth,
    scratch: Vec<f32>,
    note: EngineNote,
}

impl EnginePulse {
    pub fn new(cylinders: u8, pitch_scale: f32) -> Self {
        EnginePulse {
            synth: EngineSynth::new(),
            scratch: Vec::new(),
            note: EngineNote { rpm: 0.0, throttle: 0.0, cylinders, pitch_scale },
        }
    }
}

impl Effect for EnginePulse {
    fn id(&self) -> EffectId {
        EffectId::Engine
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        // A sample above the redline is either a decoder artefact or a
        // learned redline that has not caught up yet; either way, let the
        // note run slightly over rather than track it anywhere.
        self.note.rpm = if tel.max_rpm > 0.0 { tel.rpm.min(tel.max_rpm * 1.05) } else { tel.rpm };
        self.note.throttle = tel.throttle;
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        self.scratch.clear();
        self.synth.generate(&self.note, 1.0, out.len(), &mut self.scratch);
        for (slot, s) in out.iter_mut().zip(&self.scratch) {
            *slot += *s * gain;
        }
    }
}

/// The hard cut of a rev limiter sitting on the stop.
pub struct RevLimiter {
    engaged: bool,
    dwell: u32,
    last_max_rpm: f32,
    phase: f32,
    level: OnePole,
}

/// Fraction of the redline above which the limiter is considered to be
/// cutting.
const REV_LIMIT_FRACTION: f32 = 0.98;
/// How long the engine must sit up there, with a settled redline, first.
///
/// This is not a debounce for its own sake. OutGauge carries no redline, so
/// `max_rpm` is the highest RPM seen this session, which means `rpm` equals
/// `max_rpm` for the whole of an acceleration, not just at the top. A bare
/// threshold buzzes all the way up the range, and so does a bare dwell,
/// because the condition holds continuously while climbing.
///
/// What separates the two cases is the redline itself: while the engine is
/// still climbing, `max_rpm` is being revised upward every sample; once it
/// is genuinely on the limiter, the peak has stopped moving. So the dwell
/// only accumulates while the redline is settled (see
/// [`REV_LIMIT_SETTLED_FRACTION`]). For a format that reports a true fixed
/// redline the condition is trivially satisfied and this reduces to the
/// plain debounce it looks like.
const REV_LIMIT_DWELL_MS: u32 = 150;
/// Relative change in the reported redline that counts as "still moving".
const REV_LIMIT_SETTLED_FRACTION: f32 = 0.001;
/// Cut rate. Real limiters interrupt somewhere in the tens of hertz.
///
/// "Comfortably inside what the rim can reproduce", as this used to say, is
/// the hazard rather than the reassurance on a direct-drive wheel: a
/// frequency the rim can follow is one where it MOVES instead of buzzing.
/// Wheel excursion for a given torque goes roughly as 1/f^2, so this 25 Hz
/// pulse displaces the rim on the order of a hundred times further than the
/// engine note at 250 Hz does for the same amplitude. Isolated at gain 100
/// with the engine silenced, it threw an RS50 back and forth hard enough to
/// sound like damage (2026-08-08).
///
/// Left as it is because it is only felt alongside everything else and the
/// project has no measurement of what these wheels tolerate. See the module
/// note on low-frequency layers before changing it or its gain.
const REV_LIMIT_HZ: f32 = 25.0;

impl RevLimiter {
    pub fn new() -> Self {
        RevLimiter {
            engaged: false,
            dwell: 0,
            last_max_rpm: 0.0,
            phase: 0.0,
            level: OnePole::default(),
        }
    }
}

impl Default for RevLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RevLimiter {
    fn id(&self) -> EffectId {
        EffectId::RevLimiter
    }

    fn update(&mut self, tel: &Telemetry, block_ms: f32) {
        let settled = (tel.max_rpm - self.last_max_rpm).abs()
            <= tel.max_rpm * REV_LIMIT_SETTLED_FRACTION;
        self.last_max_rpm = tel.max_rpm;

        let at_limit = tel.max_rpm > 0.0 && tel.rpm >= tel.max_rpm * REV_LIMIT_FRACTION;
        if at_limit && settled {
            self.dwell = self.dwell.saturating_add(block_ms.max(0.0) as u32);
        } else {
            self.dwell = 0;
        }
        // Accumulated in milliseconds, from the block's own duration. It
        // counted blocks before, which is the same thing only if a block is
        // a millisecond; the daemon renders ~50 ms at a time, so this
        // wanted 7.5 seconds at the limiter rather than 150 ms.
        self.engaged = self.dwell >= REV_LIMIT_DWELL_MS;
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        let target = if self.engaged { 1.0 } else { 0.0 };
        let a = smoothing_for(20.0);
        let step = REV_LIMIT_HZ / SAMPLE_RATE_HZ;
        for slot in out.iter_mut() {
            let level = self.level.step(target, a);
            // Square rather than sine: a limiter is an interruption, not a
            // note, and the abruptness is the whole character of it.
            let square = if self.phase < 0.5 { 1.0 } else { -1.0 };
            *slot += square * level * gain;
            self.phase = (self.phase + step).fract();
        }
    }
}

/// The slower, gentler pulse of a pit-lane speed limiter.
///
/// The rev strip's full-strip flash ([`crate::leds`]) is the part of this
/// that is hardware-verified against a Windows capture. The haptic here is
/// ours: G Hub renders the limiter on the lights alone. It is included
/// because a limiter you can feel is the point of a haptic wheel, and it is
/// separable via its own gain for anyone who disagrees.
pub struct PitLimiter {
    engaged: bool,
    phase: f32,
    level: OnePole,
}

const PIT_LIMIT_HZ: f32 = 10.0;

impl PitLimiter {
    pub fn new() -> Self {
        PitLimiter { engaged: false, phase: 0.0, level: OnePole::default() }
    }
}

impl Default for PitLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PitLimiter {
    fn id(&self) -> EffectId {
        EffectId::PitLimiter
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        self.engaged = tel.pit_limiter;
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        let target = if self.engaged { 1.0 } else { 0.0 };
        let a = smoothing_for(40.0);
        let step = PIT_LIMIT_HZ / SAMPLE_RATE_HZ;
        for slot in out.iter_mut() {
            let level = self.level.step(target, a);
            let square = if self.phase < 0.5 { 1.0 } else { -1.0 };
            *slot += square * level * 0.6 * gain;
            self.phase = (self.phase + step).fract();
        }
    }
}

/// A thump through the drivetrain when the gear changes.
pub struct GearShift {
    last_gear: Option<i8>,
    burst: Burst,
}

impl GearShift {
    pub fn new() -> Self {
        GearShift { last_gear: None, burst: Burst::default() }
    }
}

impl Default for GearShift {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for GearShift {
    fn id(&self) -> EffectId {
        EffectId::GearShift
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        match self.last_gear {
            // The first sample of a session establishes the gear; it is not
            // a change, and firing on it would thump every time a stream
            // starts.
            None => self.last_gear = Some(tel.gear),
            Some(prev) if prev != tel.gear => {
                self.last_gear = Some(tel.gear);
                // Into or out of neutral is a lighter event than a shift
                // under load between two driving gears.
                let engaged = prev != 0 && tel.gear != 0;
                let peak = if engaged { 0.9 } else { 0.5 };
                self.burst.fire(peak, 55.0, 35.0, 120.0);
            }
            Some(_) => {}
        }
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        self.burst.render(out, gain);
    }

    fn continuous(&self) -> bool {
        false
    }
}

/// The pulsing of an ABS pump modulating the brakes.
pub struct AbsClick {
    active: bool,
    brake: f32,
    phase: f32,
    level: OnePole,
}

/// Pump rate. Production ABS cycles at roughly this, and it is the rate
/// people recognize through the pedal.
const ABS_HZ: f32 = 15.0;

impl AbsClick {
    pub fn new() -> Self {
        AbsClick { active: false, brake: 0.0, phase: 0.0, level: OnePole::default() }
    }
}

impl Default for AbsClick {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for AbsClick {
    fn id(&self) -> EffectId {
        EffectId::Abs
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        self.active = tel.abs_active;
        self.brake = tel.brake.clamp(0.0, 1.0);
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        // Scale with pedal pressure where the format supplies it. A source
        // that reports the lamp but no pedal would otherwise be silent, so
        // treat an absent pedal as full effort rather than none.
        let effort = if self.brake > 0.0 { self.brake } else { 1.0 };
        let target = if self.active { effort } else { 0.0 };
        let a = smoothing_for(15.0);
        let step = ABS_HZ / SAMPLE_RATE_HZ;
        for slot in out.iter_mut() {
            let level = self.level.step(target, a);
            // A short pulse per cycle rather than a continuous tone: the
            // pump is felt as distinct hits, not a buzz.
            let pulse = if self.phase < 0.35 {
                (std::f32::consts::PI * self.phase / 0.35).sin()
            } else {
                0.0
            };
            *slot += pulse * level * gain;
            self.phase = (self.phase + step).fract();
        }
    }

    fn continuous(&self) -> bool {
        false
    }
}

/// The buzz of a driven axle breaking traction.
pub struct TractionLoss {
    level: f32,
    smooth: OnePole,
    noise: Noise,
    filter: OnePole,
}

/// Slip level assumed when a format reports only a traction-control lamp.
///
/// The lamp says the system intervened, not how hard. Half scale is a
/// deliberate middle: enough to feel, not enough to dominate, and better
/// than either extreme when the truth is unknown. A format that carries a
/// real slip channel overrides it.
const TC_LAMP_NOMINAL_SLIP: f32 = 0.5;

impl TractionLoss {
    pub fn new() -> Self {
        TractionLoss {
            level: 0.0,
            smooth: OnePole::default(),
            noise: Noise::new(0x5EED_1234),
            filter: OnePole::default(),
        }
    }
}

impl Default for TractionLoss {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for TractionLoss {
    fn id(&self) -> EffectId {
        EffectId::TractionLoss
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        // Interpreting a lamp as a slip level is this layer's job, not the
        // decoder's: the decoder reports what the packet said, and what a
        // lit lamp implies about grip is a judgement that belongs with the
        // thing rendering it.
        let lamp = if tel.traction_control { TC_LAMP_NOMINAL_SLIP } else { 0.0 };
        self.level = tel.wheel_slip.clamp(0.0, 1.0).max(lamp);
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        let a = smoothing_for(30.0);
        for slot in out.iter_mut() {
            let level = self.smooth.step(self.level, a);
            // Filtered noise: slip is broadband and irregular, unlike the
            // periodic effects around it. The filter keeps it in a band the
            // rim can move rather than asking it to chatter.
            let n = self.filter.step(self.noise.next(), 0.25);
            *slot += n * level * 0.8 * gain;
        }
    }
}

/// Surface texture under the tyres.
pub struct RoadBumps {
    amount: f32,
    smooth: OnePole,
    noise: Noise,
    filter: OnePole,
}

/// Speed at which surface texture reaches full strength, in m/s (~72 km/h).
/// Below it the effect scales down, because a rough surface at walking pace
/// is not felt the way it is at speed.
const ROAD_FULL_SPEED: f32 = 20.0;

impl RoadBumps {
    pub fn new() -> Self {
        RoadBumps {
            amount: 0.0,
            smooth: OnePole::default(),
            noise: Noise::new(0xB00B_5678),
            filter: OnePole::default(),
        }
    }
}

impl Default for RoadBumps {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RoadBumps {
    fn id(&self) -> EffectId {
        EffectId::RoadBumps
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        let speed = (tel.speed.abs() / ROAD_FULL_SPEED).clamp(0.0, 1.0);
        self.amount = tel.surface_roughness.clamp(0.0, 1.0) * speed;
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        let a = smoothing_for(50.0);
        for slot in out.iter_mut() {
            let level = self.smooth.step(self.amount, a);
            // Lower cutoff than slip: surface is felt as heave rather than
            // as chatter.
            let n = self.filter.step(self.noise.next(), 0.08);
            *slot += n * level * gain;
        }
    }
}

/// Wheels off the ground: attenuate the road rather than add to it.
pub struct Airborne {
    aloft: bool,
    depth: f32,
    duck: OnePole,
}

impl Airborne {
    /// `depth` 0..1 is how far the continuous layer is pulled down while
    /// airborne; 1.0 is silence.
    pub fn new(depth: f32) -> Self {
        Airborne { aloft: false, depth: depth.clamp(0.0, 1.0), duck: OnePole { y: 1.0 } }
    }
}

// The relay sets `Telemetry::airborne` from Assetto Corsa Competizione's
// wheel loads as of 0.30.0, so this layer can now run. Two things about it
// are still unverified and worth knowing before tuning anything: whether
// that game populates the field at all (the relay's gate is built so that
// either answer is safe, not so that one of them is right), and this
// layer's gain, which was chosen when nothing could reach the layer and has
// therefore never been heard by anyone.
impl Effect for Airborne {
    fn id(&self) -> EffectId {
        EffectId::Airborne
    }

    fn update(&mut self, tel: &Telemetry, block_ms: f32) {
        self.aloft = tel.airborne;
        // Ramp the duck rather than stepping it: an instant gain change is
        // a click, and takeoff and landing are exactly when the continuous
        // layer is loudest.
        let target = if self.aloft { 1.0 - self.depth } else { 1.0 };
        self.duck.step(target, smoothing_for_block(60.0, block_ms));
    }

    fn render(&mut self, _out: &mut [f32], _gain: f32) {
        // Contributes nothing by design; see `duck`. Its configured gain is
        // spent on duck depth at construction instead.
    }

    fn duck(&self) -> f32 {
        self.duck.y
    }
}

/// An impact.
pub struct Collision {
    burst: Burst,
    last_g: f32,
}

/// Impacts below this are kerbs and rumble strips, not crashes.
const COLLISION_MIN_G: f32 = 1.5;
/// Impact at which the effect is already at full strength.
const COLLISION_FULL_G: f32 = 12.0;

impl Collision {
    pub fn new() -> Self {
        Collision { burst: Burst::default(), last_g: 0.0 }
    }
}

impl Default for Collision {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Collision {
    fn id(&self) -> EffectId {
        EffectId::Collision
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        let g = tel.impact_g.max(0.0);
        // Fire on the rising edge only. A sustained scrape along a wall
        // reports a high g for many samples, and re-arming every sample
        // would turn one long contact into a continuous roar.
        if g >= COLLISION_MIN_G && g > self.last_g {
            let span = COLLISION_FULL_G - COLLISION_MIN_G;
            let scaled = ((g - COLLISION_MIN_G) / span).clamp(0.0, 1.0);
            // Bigger hits are lower and longer, the way mass sounds.
            let freq = 60.0 - 25.0 * scaled;
            let len = 90.0 + 160.0 * scaled;
            self.burst.fire(0.5 + 0.5 * scaled, freq, len * 0.4, len);
        }
        self.last_g = g;
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        self.burst.render(out, gain);
    }

    fn continuous(&self) -> bool {
        false
    }
}

/// A confirmation click when a drag-reduction wing (or any push-to-pass
/// equivalent) opens or closes.
pub struct Drs {
    last: Option<bool>,
    burst: Burst,
}

impl Drs {
    pub fn new() -> Self {
        Drs { last: None, burst: Burst::default() }
    }
}

impl Default for Drs {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Drs {
    fn id(&self) -> EffectId {
        EffectId::Drs
    }

    fn update(&mut self, tel: &Telemetry, _block_ms: f32) {
        match self.last {
            None => self.last = Some(tel.drs_active),
            Some(prev) if prev != tel.drs_active => {
                self.last = Some(tel.drs_active);
                // Opening is the event worth confirming; closing is usually
                // automatic and gets a lighter tick.
                let peak = if tel.drs_active { 0.7 } else { 0.4 };
                self.burst.fire(peak, 90.0, 18.0, 60.0);
            }
            Some(_) => {}
        }
    }

    fn render(&mut self, out: &mut [f32], gain: f32) {
        self.burst.render(out, gain);
    }

    fn continuous(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------
// Mixer
// ---------------------------------------------------------------------

/// Sums the effect layers into the stream buffer.
pub struct Mixer {
    effects: Vec<Box<dyn Effect>>,
    gains: EffectGains,
    continuous: Vec<f32>,
    transient: Vec<f32>,
}

impl Mixer {
    /// Build the full set. `cylinders` and `pitch_scale` configure the
    /// engine layer; see [`crate::synth::firing_frequency`].
    pub fn new(cylinders: u8, pitch_scale: f32, gains: EffectGains) -> Self {
        // Airborne's gain is a depth rather than a level: it scales how far
        // the duck pulls down, since the effect emits nothing to scale.
        let depth = gains.scale(EffectId::Airborne);
        let effects: Vec<Box<dyn Effect>> = vec![
            Box::new(EnginePulse::new(cylinders, pitch_scale)),
            Box::new(RevLimiter::new()),
            Box::new(PitLimiter::new()),
            Box::new(GearShift::new()),
            Box::new(AbsClick::new()),
            Box::new(TractionLoss::new()),
            Box::new(RoadBumps::new()),
            Box::new(Airborne::new(depth)),
            Box::new(Collision::new()),
            Box::new(Drs::new()),
        ];
        Mixer { effects, gains, continuous: Vec::new(), transient: Vec::new() }
    }

    /// Only the engine layer, for the `effects=off` case.
    pub fn engine_only(cylinders: u8, pitch_scale: f32, gains: EffectGains) -> Self {
        Mixer {
            effects: vec![Box::new(EnginePulse::new(cylinders, pitch_scale))],
            gains,
            continuous: Vec::new(),
            transient: Vec::new(),
        }
    }

    /// Render `count` samples for `tel` into `out`, replacing its contents.
    ///
    /// `intensity` (0..1) is the master level; 0 emits exact silence.
    pub fn render(&mut self, tel: &Telemetry, intensity: f32, count: usize, out: &mut Vec<f32>) {
        out.clear();
        if count == 0 {
            return;
        }
        let intensity = intensity.clamp(0.0, 1.0);
        let block_ms = count as f32 * 1000.0 / SAMPLE_RATE_HZ;

        self.continuous.clear();
        self.continuous.resize(count, 0.0);
        self.transient.clear();
        self.transient.resize(count, 0.0);

        // Update every effect before rendering any: ducking is a property of
        // the whole set for this block, so it has to be known before the
        // first sample of the continuous layer is written.
        let mut duck = 1.0f32;
        for effect in &mut self.effects {
            effect.update(tel, block_ms);
            duck *= effect.duck();
        }

        for effect in &mut self.effects {
            let gain = self.gains.scale(effect.id());
            let buf =
                if effect.continuous() { &mut self.continuous } else { &mut self.transient };
            effect.render(buf, gain);
            debug_assert_eq!(buf.len(), count, "effects add into the block, never resize it");
        }

        out.reserve(count);
        for i in 0..count {
            // Clamp rather than normalize: a mix that momentarily exceeds
            // full scale is a loud moment, not a reason to duck everything
            // else, and the stream's own encoding saturates here anyway.
            let mixed = self.continuous[i] * duck + self.transient[i];
            out.push((mixed * intensity).clamp(-1.0, 1.0));
        }
    }

    /// The gains this mixer was built with.
    pub fn gains(&self) -> EffectGains {
        self.gains
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::DEFAULT_CYLINDERS;

    /// Telemetry with the engine running and nothing else happening.
    fn running() -> Telemetry {
        Telemetry { rpm: 4000.0, max_rpm: 7000.0, throttle: 0.5, speed: 30.0, ..Default::default() }
    }

    fn all_gains(pct: u8) -> EffectGains {
        let mut g = EffectGains::default();
        for id in EffectId::ALL {
            g.set(id, pct);
        }
        g
    }

    /// Gains with exactly one effect audible.
    fn only(id: EffectId, pct: u8) -> EffectGains {
        let mut g = all_gains(0);
        g.set(id, pct);
        g
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Run `blocks` blocks of `len` samples and return everything rendered.
    /// Render `blocks` blocks of `block_ms` milliseconds each.
    ///
    /// `block_ms`, not a sample count: these were the same number only
    /// while the stream ran at 1 kHz, and every duration in these tests
    /// silently became a quarter of itself when it went to 4 kHz. The
    /// mixer's own comments already describe a block as "~1 ms", so this
    /// is what the call sites always meant.
    fn run(mixer: &mut Mixer, tel: &Telemetry, blocks: usize, block_ms: usize) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = Vec::new();
        for _ in 0..blocks {
            mixer.render(tel, 1.0, block_ms * crate::synth::SAMPLES_PER_MS, &mut buf);
            all.extend_from_slice(&buf);
        }
        all
    }

    #[test]
    fn every_effect_has_a_unique_key_that_round_trips() {
        let mut seen = Vec::new();
        for id in EffectId::ALL {
            assert_eq!(EffectId::from_key(id.key()), Some(id));
            assert!(!seen.contains(&id.key()), "duplicate key {}", id.key());
            seen.push(id.key());
        }
        assert_eq!(EffectId::from_key("no_such_effect"), None);
        // The gain array is indexed by discriminant; a reordering that
        // outgrew the array would silently alias two effects' gains.
        assert_eq!(seen.len(), EffectGains::default().gains.len());
    }

    #[test]
    fn the_engine_layer_is_unchanged_by_the_arrival_of_the_others() {
        // The regression that matters most here: someone upgrading with the
        // new effects unfed must hear exactly the engine they had before.
        let tel = running();
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 0.25, only(EffectId::Engine, 100));
        let mixed = run(&mut mixer, &tel, 40, 16);

        let mut synth = EngineSynth::new();
        let mut direct = Vec::new();
        let note = EngineNote {
            rpm: tel.rpm,
            throttle: tel.throttle,
            cylinders: DEFAULT_CYLINDERS,
            pitch_scale: 0.25,
        };
        synth.generate(&note, 1.0, mixed.len(), &mut direct);

        assert_eq!(mixed.len(), direct.len());
        for (i, (a, b)) in mixed.iter().zip(&direct).enumerate() {
            // Not bit-exact only because the mixer multiplies by intensity
            // after the synth's own gain rather than folding it in.
            assert!((a - b).abs() < 1e-6, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn an_inert_sample_is_exact_silence() {
        // Everything defaulted, engine stopped: nothing to render.
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, EffectGains::default());
        let out = run(&mut mixer, &Telemetry::default(), 20, 32);
        assert!(out.iter().all(|s| *s == 0.0), "peak {}", peak(&out));
    }

    #[test]
    fn zero_intensity_is_exact_silence_whatever_is_happening() {
        let tel = Telemetry {
            rpm: 7000.0,
            max_rpm: 7000.0,
            throttle: 1.0,
            speed: 50.0,
            pit_limiter: true,
            abs_active: true,
            traction_control: true,
            wheel_slip: 1.0,
            surface_roughness: 1.0,
            impact_g: 20.0,
            ..Default::default()
        };
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, EffectGains::default());
        let mut buf = Vec::new();
        mixer.render(&tel, 0.0, 64, &mut buf);
        assert!(buf.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn the_mix_never_leaves_full_scale() {
        // Every effect at once, all at full gain, engine on the limiter.
        let tel = Telemetry {
            rpm: 7000.0,
            max_rpm: 7000.0,
            throttle: 1.0,
            speed: 50.0,
            pit_limiter: true,
            abs_active: true,
            brake: 1.0,
            traction_control: true,
            wheel_slip: 1.0,
            surface_roughness: 1.0,
            impact_g: 20.0,
            ..Default::default()
        };
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, all_gains(100));
        let out = run(&mut mixer, &tel, 400, 8);
        assert!(out.iter().all(|s| (-1.0..=1.0).contains(s)), "peak {}", peak(&out));
        // And it is genuinely loud, not clamped to nothing.
        assert!(peak(&out) > 0.5);
    }

    #[test]
    fn the_rev_limiter_waits_for_the_engine_to_sit_on_the_stop() {
        let tel = Telemetry { rpm: 7000.0, max_rpm: 7000.0, throttle: 1.0, ..Default::default() };
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::RevLimiter, 100));
        // One block per millisecond, so blocks are the dwell's unit.
        let early = run(&mut mixer, &tel, (REV_LIMIT_DWELL_MS - 20) as usize, 1);
        assert_eq!(peak(&early), 0.0, "limiter fired before the dwell elapsed");
        let later = run(&mut mixer, &tel, 300, 1);
        assert!(peak(&later) > 0.3, "limiter never engaged: peak {}", peak(&later));
    }

    #[test]
    fn a_climbing_engine_does_not_trip_the_limiter_on_every_new_peak() {
        // OutGauge has no redline, so max_rpm is the running maximum and
        // rpm == max_rpm on every fresh peak all the way up the range. A
        // bare threshold would buzz continuously through an acceleration.
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::RevLimiter, 100));
        let mut out = Vec::new();
        let mut buf = Vec::new();
        for step in 0..600 {
            let rpm = 1000.0 + step as f32 * 10.0;
            // The defining feature of the learned redline: it equals rpm.
            let tel = Telemetry { rpm, max_rpm: rpm, throttle: 1.0, ..Default::default() };
            mixer.render(&tel, 1.0, 1, &mut buf);
            out.extend_from_slice(&buf);
        }
        assert_eq!(peak(&out), 0.0, "limiter buzzed while merely accelerating");
    }

    #[test]
    fn a_learned_redline_still_finds_the_limiter_once_the_car_sits_on_it() {
        // The other half of the guard: refusing to fire while the redline
        // moves must not mean never firing for a format that has to learn
        // one. Climb, then hold, exactly as OutGauge reports it.
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::RevLimiter, 100));
        let mut buf = Vec::new();
        let mut climbing = Vec::new();
        let mut redline = 0.0f32;
        for step in 0..400 {
            let rpm = 1000.0 + step as f32 * 15.0;
            redline = redline.max(rpm);
            mixer.render(
                &Telemetry { rpm, max_rpm: redline, throttle: 1.0, ..Default::default() },
                1.0,
                1,
                &mut buf,
            );
            climbing.extend_from_slice(&buf);
        }
        assert_eq!(peak(&climbing), 0.0, "fired on the way up");

        // Now on the stop: rpm holds, so the learned peak stops moving.
        let held = Telemetry { rpm: redline, max_rpm: redline, throttle: 1.0, ..Default::default() };
        let sitting = run(&mut mixer, &held, 400, 1);
        assert!(peak(&sitting) > 0.3, "never engaged once held: {}", peak(&sitting));
    }

    #[test]
    fn a_gear_change_thumps_but_the_first_sample_does_not() {
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::GearShift, 100));
        let third = Telemetry { gear: 3, ..running() };
        // Arriving already in third is not a shift into it.
        let settle = run(&mut mixer, &third, 200, 1);
        assert_eq!(peak(&settle), 0.0, "thumped on the first telemetry sample");

        let fourth = Telemetry { gear: 4, ..running() };
        let shift = run(&mut mixer, &fourth, 200, 1);
        assert!(peak(&shift) > 0.5, "no thump on the change: {}", peak(&shift));

        // And it decays rather than sustaining.
        let after = run(&mut mixer, &fourth, 400, 1);
        assert_eq!(peak(&after), 0.0, "the thump never ended");
    }

    #[test]
    fn shifting_into_neutral_is_gentler_than_a_shift_under_load() {
        let mut a = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::GearShift, 100));
        run(&mut a, &Telemetry { gear: 3, ..running() }, 5, 1);
        let engaged = peak(&run(&mut a, &Telemetry { gear: 4, ..running() }, 200, 1));

        let mut b = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::GearShift, 100));
        run(&mut b, &Telemetry { gear: 3, ..running() }, 5, 1);
        let neutral = peak(&run(&mut b, &Telemetry { gear: 0, ..running() }, 200, 1));

        assert!(neutral < engaged, "neutral {neutral} was not gentler than {engaged}");
        assert!(neutral > 0.0, "neutral produced nothing at all");
    }

    #[test]
    fn abs_pulses_only_while_it_is_modulating() {
        let braking = Telemetry { abs_active: true, brake: 1.0, ..running() };
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Abs, 100));
        assert!(peak(&run(&mut mixer, &braking, 300, 1)) > 0.3);

        let released = Telemetry { abs_active: false, brake: 0.0, ..running() };
        run(&mut mixer, &released, 300, 1); // let the smoother fall
        assert_eq!(peak(&run(&mut mixer, &released, 200, 1)), 0.0);
    }

    #[test]
    fn a_traction_lamp_alone_is_enough_to_feel_but_a_slip_channel_wins() {
        let mut lamp_only = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::TractionLoss, 100));
        let lamp = peak(&run(
            &mut lamp_only,
            &Telemetry { traction_control: true, ..running() },
            400,
            1,
        ));
        assert!(lamp > 0.1, "a lit lamp produced nothing: {lamp}");

        let mut full = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::TractionLoss, 100));
        let broken = peak(&run(
            &mut full,
            &Telemetry { traction_control: true, wheel_slip: 1.0, ..running() },
            400,
            1,
        ));
        assert!(broken > lamp, "full slip {broken} was not stronger than the lamp's {lamp}");
    }

    #[test]
    fn surface_texture_needs_both_a_surface_and_some_speed() {
        let rough_and_moving =
            Telemetry { surface_roughness: 1.0, speed: 30.0, ..Default::default() };
        let rough_but_parked =
            Telemetry { surface_roughness: 1.0, speed: 0.0, ..Default::default() };
        let smooth_at_speed =
            Telemetry { surface_roughness: 0.0, speed: 30.0, ..Default::default() };

        for (tel, want_felt) in
            [(rough_and_moving, true), (rough_but_parked, false), (smooth_at_speed, false)]
        {
            let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::RoadBumps, 100));
            let p = peak(&run(&mut mixer, &tel, 400, 1));
            assert_eq!(p > 0.02, want_felt, "roughness {} speed {} gave {p}", tel.surface_roughness, tel.speed);
        }
    }

    /// Time-dependent behaviour must not depend on how the caller chunks
    /// its rendering.
    ///
    /// This is the property the whole suite was missing: every other test
    /// renders one sample at a time, which is the one block size that made
    /// the old per-block arithmetic accidentally correct. The daemon
    /// renders ~50 ms at a time, so the airborne duck really took about
    /// three seconds to ramp in a game and the rev limiter wanted 7.5
    /// seconds of sustained limit instead of 150 ms, with nothing failing.
    #[test]
    fn the_same_wall_time_gives_the_same_result_at_any_block_size() {
        let duck_after = |blocks: usize, block_ms: usize| {
            let mut gains = all_gains(0);
            gains.set(EffectId::Engine, 100);
            gains.set(EffectId::Airborne, 100);
            let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, gains);
            let aloft = Telemetry { airborne: true, ..running() };
            run(&mut mixer, &aloft, blocks, block_ms);
            peak(&run(&mut mixer, &aloft, 20, 1))
        };
        // 240 ms of flight, chunked three ways.
        let fine = duck_after(240, 1);
        let daemonish = duck_after(5, 48);
        let coarse = duck_after(2, 120);
        assert!(
            (fine - daemonish).abs() < 0.05 && (fine - coarse).abs() < 0.05,
            "the duck depends on block size: 1 ms {fine}, 48 ms {daemonish}, 120 ms {coarse}",
        );
    }

    #[test]
    fn the_rev_limiter_engages_on_elapsed_time_not_on_block_count() {
        let engaged_after = |blocks: usize, block_ms: usize| {
            let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::RevLimiter, 100));
            let limited = Telemetry { rpm: 7900.0, max_rpm: 8000.0, ..running() };
            run(&mut mixer, &limited, blocks, block_ms);
            peak(&run(&mut mixer, &limited, 20, 1)) > 0.01
        };
        // REV_LIMIT_DWELL_MS is 150, so 300 ms must engage however it is cut.
        assert!(engaged_after(300, 1), "did not engage with 1 ms blocks");
        assert!(engaged_after(6, 50), "did not engage with 50 ms blocks (the daemon's size)");
    }

    #[test]
    fn going_airborne_quiets_the_road_but_not_an_impact() {
        let mut gains = all_gains(0);
        gains.set(EffectId::Engine, 100);
        gains.set(EffectId::Airborne, 100); // full duck depth
        gains.set(EffectId::Collision, 100);

        let grounded = Telemetry { airborne: false, ..running() };
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, gains);
        let on_road = peak(&run(&mut mixer, &grounded, 400, 1));
        assert!(on_road > 0.1);

        let aloft = Telemetry { airborne: true, ..running() };
        run(&mut mixer, &aloft, 400, 1); // let the duck ramp in
        let flying = peak(&run(&mut mixer, &aloft, 200, 1));
        assert!(flying < on_road * 0.1, "engine still audible aloft: {flying} vs {on_road}");

        // A landing is a transient and must survive the duck.
        let landing = Telemetry { airborne: true, impact_g: 10.0, ..running() };
        let hit = peak(&run(&mut mixer, &landing, 100, 1));
        assert!(hit > 0.5, "the impact was ducked away: {hit}");
    }

    #[test]
    fn a_scrape_along_a_wall_is_one_impact_not_a_continuous_roar() {
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Collision, 100));
        // Contact holds a high g for a long time.
        let contact = Telemetry { impact_g: 8.0, ..running() };
        let first = peak(&run(&mut mixer, &contact, 250, 1));
        assert!(first > 0.5, "no hit on contact: {first}");
        // Same g sustained: the burst must have decayed and not re-armed.
        let sustained = peak(&run(&mut mixer, &contact, 400, 1));
        assert_eq!(sustained, 0.0, "the scrape kept re-firing: {sustained}");
    }

    #[test]
    fn a_light_knock_is_below_the_collision_threshold() {
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Collision, 100));
        let kerb = Telemetry { impact_g: COLLISION_MIN_G - 0.1, ..running() };
        assert_eq!(peak(&run(&mut mixer, &kerb, 300, 1)), 0.0);
    }

    #[test]
    fn drs_ticks_on_both_edges_and_opening_is_the_firmer_one() {
        let mut mixer = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Drs, 100));
        let closed = Telemetry { drs_active: false, ..running() };
        assert_eq!(peak(&run(&mut mixer, &closed, 100, 1)), 0.0, "ticked on the first sample");

        let open = Telemetry { drs_active: true, ..running() };
        let opening = peak(&run(&mut mixer, &open, 100, 1));
        assert!(opening > 0.3, "no tick on opening: {opening}");

        let shutting = peak(&run(&mut mixer, &closed, 100, 1));
        assert!(shutting > 0.0 && shutting < opening, "closing {shutting} vs opening {opening}");
    }

    #[test]
    fn a_silenced_effect_does_not_resume_mid_event_when_turned_back_up() {
        // Gain must scale output without freezing state, or an effect muted
        // across an event restarts it audibly the moment the level returns.
        let mut burst = Burst::default();
        burst.fire(1.0, 50.0, 30.0, 100.0);
        // The burst is 100 ms long, so render 100 ms of it: as a bare
        // sample count this covered only a quarter of it at 4 kHz.
        let mut muted = vec![0.0f32; 100 * crate::synth::SAMPLES_PER_MS];
        burst.render(&mut muted, 0.0);
        assert!(muted.iter().all(|s| *s == 0.0), "gain 0 was not silent");
        assert!(!burst.active(), "the burst was paused rather than consumed");
    }

    #[test]
    fn one_effects_gain_does_not_touch_another() {
        let tel = Telemetry { pit_limiter: true, ..running() };
        let mut engine_only = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Engine, 100));
        let engine = run(&mut engine_only, &tel, 200, 1);

        let mut both = all_gains(0);
        both.set(EffectId::Engine, 100);
        both.set(EffectId::PitLimiter, 100);
        let mut pair = Mixer::new(DEFAULT_CYLINDERS, 1.0, both);
        let mixed = run(&mut pair, &tel, 200, 1);

        assert!(peak(&mixed) > peak(&engine), "adding the limiter changed nothing");
        // And with the limiter's own gain at zero, the engine is untouched.
        let mut zeroed = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Engine, 100));
        let again = run(&mut zeroed, &tel, 200, 1);
        assert_eq!(engine, again);
    }

    #[test]
    fn engine_only_leaves_out_everything_else() {
        let tel = Telemetry {
            pit_limiter: true,
            abs_active: true,
            brake: 1.0,
            traction_control: true,
            ..running()
        };
        let mut full = Mixer::engine_only(DEFAULT_CYLINDERS, 1.0, EffectGains::default());
        let bare = run(&mut full, &tel, 200, 1);

        let mut reference = Mixer::new(DEFAULT_CYLINDERS, 1.0, only(EffectId::Engine, 100));
        let engine = run(&mut reference, &tel, 200, 1);
        assert_eq!(bare, engine);
    }

    #[test]
    fn block_length_does_not_change_what_is_rendered() {
        // The daemon's block size follows scheduling jitter, so an effect
        // whose state advanced per block rather than per sample would drift
        // audibly under load.
        let tel = Telemetry { abs_active: true, brake: 1.0, ..running() };
        let mut a = Mixer::new(DEFAULT_CYLINDERS, 0.25, all_gains(100));
        let one_at_a_time = run(&mut a, &tel, 256, 1);
        let mut b = Mixer::new(DEFAULT_CYLINDERS, 0.25, all_gains(100));
        let in_chunks = run(&mut b, &tel, 8, 32);

        assert_eq!(one_at_a_time.len(), in_chunks.len());
        for (i, (x, y)) in one_at_a_time.iter().zip(&in_chunks).enumerate() {
            assert!((x - y).abs() < 1e-6, "sample {i} drifted: {x} vs {y}");
        }
    }

    #[test]
    fn gains_are_clamped_to_full_scale() {
        let mut g = EffectGains::default();
        g.set(EffectId::Engine, 250);
        assert_eq!(g.get(EffectId::Engine), 100);
    }
}
