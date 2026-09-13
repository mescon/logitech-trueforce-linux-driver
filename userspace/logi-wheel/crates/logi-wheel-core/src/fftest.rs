// SPDX-License-Identifier: GPL-2.0-only
//! The Test view's two guarded force-feedback simulations: shared step
//! tables, kernel `ff_effect` construction, capability-based skipping and
//! the sequence runner both front-ends drive.
//!
//! Both front-ends used to upload and play exactly one canned effect
//! (a single `FF_CONSTANT` for "force", a single `FF_PERIODIC`/`FF_SINE`
//! for "texture") for a fixed 2 s. That let the wheel pin to one side and
//! stop, which is not a useful demonstration of anything the driver
//! actually implements. This module replaces each with a short sequence
//! of labelled steps exercising the palette the kernel driver advertises.
//!
//! The force sequence ([`FORCE_SEQUENCE`]) was hardware-tested on a live
//! G923 and re-tuned from that feedback: the old six alternating 0.45 s
//! pulses (left/right, repeated three times) walked the wheel back and
//! forth for no real reason - one left step and one right step of equal
//! duration already self-cancel positionally, so the sequence now plays
//! exactly one of each - and every amplitude and duration was raised
//! (~30% to ~60% of range; 0.45 s pulses to 1.8-4 s steps) because the
//! owner reported the old ones as "very very faint" and "very very
//! short". The table also grew to cover every effect type the driver
//! advertises for both engines (`mainline/hid-logitech-hidpp.c`'s
//! `hidpp_dd_ff_effects`/DD `set_bit` list, and `mainline/dd-lg4ff.c`'s
//! `dd_lg4ff_wheel_effects` for the G923's classic engine): `FF_CONSTANT`,
//! `FF_RAMP`, `FF_SPRING`, `FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA`,
//! `FF_PERIODIC` with the sine/square/triangle/sawtooth waveforms, an
//! envelope (attack/fade) demo, a two-effect mix that exercises the
//! classic engine's slot-0 summing, `FF_GAIN`, and `FF_AUTOCENTER`. The
//! DD wheels (RS50, G PRO) advertise every one of those; the G923's
//! classic engine advertises the same set minus `FF_FRICTION`
//! (hardware-probed on a live G923, 2026-07-27), so the friction step is
//! the one row a G923 run always shows as skipped. `FF_SAW_DOWN` stays
//! out of the table too; see [`FF_SAW_UP`]'s doc comment for why.
//!
//! What lives here vs. per front-end:
//! - the step tables ([`FORCE_SEQUENCE`], [`TEXTURE_SEQUENCE`]) and the
//!   `ff_effect` byte layout ([`build_ff_effect`]) are the one shared
//!   source of truth;
//! - the sequencing itself (capability filtering, each step's own
//!   countdown, upload/play/wait/stop/erase per step, cancellation,
//!   ENODEV handling) is also shared, via [`run_sequence`] against the
//!   [`FfDevice`] trait;
//! - only the actual file descriptor and the `ioctl`/`libc` calls that
//!   implement `FfDevice` stay in each front-end (this crate stays
//!   dependency-free and never opens a device node), mirroring how
//!   `evtest` keeps event decoding here while the open fd stays out;
//! - so does the rendered plan's state machine ([`StepState`],
//!   [`SequenceProgress`]): a front-end confirms a sequence, builds a
//!   [`SequenceProgress`] with every row `Pending` and shows the whole
//!   plan immediately, then folds each [`SequenceEvent`] the running
//!   sequence reports into it. Both front-ends render the exact same rows
//!   from the exact same source; only the widgets differ.
//!
//! Force/direction math is not invented here: `direction` values and the
//! condition-effect coefficient signs are exactly what
//! `mainline/hid-logitech-hidpp.c` expects (`hidpp_dd_project_constant`'s
//! `direction=0x4000` == east convention, and `hidpp_dd_condition_force`'s
//! restoring-force sign, which needs a POSITIVE coefficient on both
//! sides - a negative pair would build an anti-spring, not a centering
//! one) - for the DD engine. The G923's classic (lg4ff-style) engine does
//! not share that convention: see [`Side`] and [`resolve_direction`].
//! Every step table here stores a logical [`Side`] (`Right`/`Left`/`None`),
//! never a raw evdev direction value directly, and [`build_ff_effect`]
//! resolves it against the playing wheel's [`WheelModel`] at upload time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::device::WheelModel;
use crate::evtest::EVENT_SIZE;

// ---------------------------------------------------------------------------
// evdev force-feedback constants (`linux/input-event-codes.h`).
// ---------------------------------------------------------------------------

/// `EV_FF`, the evdev event type for force-feedback play/stop/gain events.
pub const EV_FF: u16 = 0x15;

/// `ff_effect.type` values this crate builds. Verified against
/// `mainline/hid-logitech-hidpp.c` (which builds against the real kernel
/// header), `mainline/dd-lg4ff.c` (the G923's classic engine) and the
/// ffb-proxy crate's `sink` module.
pub const FF_PERIODIC: u16 = 0x51;
pub const FF_CONSTANT: u16 = 0x52;
pub const FF_SPRING: u16 = 0x53;
/// `ff_condition_effect`, same struct as [`FF_SPRING`]/[`FF_DAMPER`]/
/// [`FF_INERTIA`] (see [`StepEffect::Friction`]): resistance to motion
/// that does not scale with speed, unlike [`FF_DAMPER`]. Advertised by
/// the DD engine; the G923's classic engine does not advertise it
/// (hardware-probed on a live G923, 2026-07-27), so a friction step is
/// the one row a G923 run always skips.
pub const FF_FRICTION: u16 = 0x54;
pub const FF_DAMPER: u16 = 0x55;
pub const FF_INERTIA: u16 = 0x56;
pub const FF_RAMP: u16 = 0x57;
/// `ff_periodic_effect.waveform` values [`StepEffect::Periodic`] uses.
pub const FF_SQUARE: u16 = 0x58;
/// A linear ramp up then down each period: smoother than [`FF_SQUARE`]'s
/// instant flips, more evenly shaped than [`FF_SAW_UP`]'s one-way ramp.
pub const FF_TRIANGLE: u16 = 0x59;
pub const FF_SINE: u16 = 0x5a;
pub const FF_SAW_UP: u16 = 0x5b;

// No `FF_SAW_DOWN` (0x5c) constant here on purpose: both engines
// advertise it (see the module doc and `mainline/dd-lg4ff.c`'s
// `dd_lg4ff_wheel_effects`), but `FORCE_SEQUENCE` does not carry a
// dedicated step for it. `FF_SAW_UP`'s step already conveys the
// "ratcheting" shape a sawtooth makes; a mirrored ramp-down step would
// exercise a different `ff_effect.type` bit but not teach the user
// anything a hand on the rim can actually distinguish, the way sine vs.
// square vs. triangle vs. sawtooth can. `ffb-proxy`'s `sink` module
// still defines and uses `FF_SAW_DOWN` for real game effects; this
// crate's test suite just has no step that needs its own constant for it.

/// Not an effect type: the `EV_FF` code for the device-gain write.
pub const FF_GAIN: u16 = 0x60;
/// Not an effect type: the `EV_FF` code for the device-autocenter write.
/// Lives in the same code space as the effect types above, so
/// [`ff_type_supported`] (backed by the same `EVIOCGBIT(EV_FF, ...)`
/// bitmap) gates it exactly the same way.
pub const FF_AUTOCENTER: u16 = 0x61;
/// Highest legal bit `EVIOCGBIT(EV_FF, ...)` can report (`FF_MAX` in
/// `linux/input-event-codes.h`; `FF_CNT` is one past it).
pub const FF_MAX: u16 = 0x7f;

/// Size of the union embedded at the end of `struct ff_effect`, sized to
/// fit the largest member (`ff_periodic_effect`, 32 bytes on a 64-bit
/// kernel).
pub const FF_UNION_SIZE: usize = 32;

/// Bytes an `EVIOCGBIT(EV_FF, ...)` capability query needs to cover every
/// bit up to [`FF_MAX`] (`(FF_MAX + 1) / 8`, rounded up, with headroom).
pub const FF_BITS_LEN: usize = 32;

/// evdev direction for "east" (`0x4000`, i.e. 90 degrees on evdev's
/// circular direction scale). On the DD engine this is the documented
/// convention for "a positive level/magnitude produces a positive
/// (rightward) force" (see `hidpp_dd_project_constant`'s doc comment -
/// direction 0 produces zero force for a signed level, which is why
/// every directional step here resolves to one of these two raw values,
/// never 0 - see [`Side`]/[`resolve_direction`]).
const DIRECTION_EAST: u16 = 0x4000;
/// evdev direction for "west" (`0xC000`, 270 degrees): flips the sign of
/// the same positive level/magnitude relative to [`DIRECTION_EAST`].
/// Condition effects (spring/damper/inertia) ignore `direction` entirely
/// (`hidpp_dd_condition_force` reads only the condition fields), so their
/// steps use [`Side::None`], which resolves to 0.
const DIRECTION_WEST: u16 = 0xC000;

/// Which side of center a directional step pulls the wheel toward,
/// independent of engine. Resolved to a raw evdev `direction` value only
/// at upload time, via [`resolve_direction`], because the DD and G923
/// (classic/lg4ff) engines disagree on which raw value means which side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
    /// Condition effects (spring/damper/inertia) and device-level state
    /// (autocenter): direction is meaningless, always resolves to 0.
    None,
}

/// Resolve `side` to the raw `ff_effect.direction` value for a wheel of
/// `model`. Two force engines, two conventions:
/// - the DD engine (RS50, G PRO, and `Unknown` - assumed DD-shaped, same
///   as everywhere else `WheelModel` gates DD vs. classic behavior) uses
///   `hidpp_dd_project_constant`'s documented convention as-is:
///   [`DIRECTION_EAST`] (0x4000) is rightward, [`DIRECTION_WEST`] (0xC000)
///   is leftward.
/// - the G923's classic (lg4ff-style) engine does the opposite in
///   practice, hardware-verified 2026-07-27 on the owner's G923 across
///   three separate measurements (the original constant-force
///   validation, the TrueForce mirror sign test's baseline push, and this
///   sequence's own step 1): an effect with `direction=0x4000` and a
///   POSITIVE level rotates the wheel LEFT, not right. So a G923's
///   "right" step must carry [`DIRECTION_WEST`] and its "left" step
///   [`DIRECTION_EAST`] - the DD engine's values, swapped.
pub fn resolve_direction(side: Side, model: WheelModel) -> u16 {
    match (side, model) {
        (Side::None, _) => 0,
        (Side::Right, WheelModel::G923) => DIRECTION_WEST,
        (Side::Right, _) => DIRECTION_EAST,
        (Side::Left, WheelModel::G923) => DIRECTION_EAST,
        (Side::Left, _) => DIRECTION_WEST,
    }
}

/// ~30% of the `i16` force/magnitude range and ~30% of the `u16`
/// saturation range - the levels [`TEXTURE_SEQUENCE`] still uses (the
/// owner's "very very faint" feedback was about the force sequence's old
/// alternating pulses, not the texture progression, so texture keeps its
/// original moderate amplitude unchanged).
pub const SIM_LEVEL_30: i16 = 9830;
pub const SIM_SATURATION_30: u16 = 19661;

/// ~60% of the `i16` force/magnitude range and ~60% of the `u16`
/// saturation range: [`FORCE_SEQUENCE`]'s retuned amplitude, clearly
/// feelable but not maximal (every step at this level tells the user, in
/// its label, to hold the rim).
pub const SIM_LEVEL_60: i16 = 19660;
pub const SIM_SATURATION_60: u16 = 39321;

/// The reduced-gain half of the gain demo (`FF_GAIN`, ~30% of the
/// 0..=0xFFFF device-gain scale) - a distinct device-level scalar from
/// [`SIM_SATURATION_30`], even though it happens to share the same
/// numeric value (both are "~30% of a `u16` range").
pub const SIM_GAIN_DEMO_LOW: i32 = 19661;
/// Full device gain (`0xFFFF`), both what [`run_sequence`] sets before
/// the first step and what the gain demo always restores to afterward.
pub const SIM_GAIN_FULL: i32 = 0xFFFF;
/// The autocenter demo's strength (`FF_AUTOCENTER`, ~60% of the
/// 0..=0xFFFF device scale) - a distinct device-level scalar from
/// [`SIM_SATURATION_60`], even though it happens to share the same
/// numeric value.
pub const SIM_AUTOCENTER_LEVEL: i32 = 39321;

/// How often a step's playback wait re-checks the cancel flag.
pub const SIM_CANCEL_POLL: Duration = Duration::from_millis(10);

/// The lead-in countdown [`run_sequence`] runs before a step, ticking
/// `ticks` times down to 1, each tick held out for `tick`. Every real
/// table uses one of [`STEP_COUNTDOWN_LONG`] or [`STEP_COUNTDOWN_SHORT`]
/// (see their doc comments) via [`SimStep::countdown`); [`Countdown::NONE`]
/// is only for tests that want to skip the wait entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Countdown {
    pub ticks: u64,
    pub tick: Duration,
}

impl Countdown {
    /// No countdown at all: zero ticks, so [`run_sequence`] moves straight
    /// from a step's `Step` event into uploading it. Only meant for tests
    /// that do not care about countdown behavior and want to stay fast.
    pub const NONE: Countdown = Countdown { ticks: 0, tick: Duration::ZERO };
}

/// The countdown before a step that needs the user to actually DO
/// something once it starts playing (spring, damper, inertia,
/// autocenter - see each step's `SimStep::countdown` in [`FORCE_SEQUENCE`]):
/// a full "3, 2, 1" over 3 real seconds, same as every step used before
/// this table grew. Longer sequences make the countdown's total cost
/// matter more, so it is no longer applied uniformly - see
/// [`STEP_COUNTDOWN_SHORT`] for the other half of that split.
pub const STEP_COUNTDOWN_LONG: Countdown = Countdown { ticks: 3, tick: Duration::from_secs(1) };

/// The countdown before a passive step, where the user only has to hold
/// the rim and feel what plays: a single "1" tick held out for 1.5 s.
/// Still a real, visible countdown (nothing plays with zero warning), just
/// shorter than [`STEP_COUNTDOWN_LONG`] - one tick rather than three
/// separate one-second ticks, so as not to claim a false second-by-second
/// granularity for a wait shorter than a second per tick would allow.
pub const STEP_COUNTDOWN_SHORT: Countdown = Countdown { ticks: 1, tick: Duration::from_millis(1500) };

// ---------------------------------------------------------------------------
// `struct ff_effect` (`linux/input.h`), mirrored field for field.
// ---------------------------------------------------------------------------

/// The trailing `union` of the kernel's `struct ff_effect`, modeled as a
/// plain byte array (never a live typed reference into misaligned
/// fields), the same convention the ffb-proxy crate's `sink` module uses.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct FfUnion(pub [u8; FF_UNION_SIZE]);

/// Mirrors the kernel's `struct ff_effect`: `type`, `id`, `direction`,
/// trigger (button+interval), replay (length+delay), then the union.
/// `#[repr(C)]` with the union's own `align(8)` is what makes
/// `size_of::<FfEffect>()` match the kernel's 48 bytes (pinned by a test
/// below); a front-end's `EVIOCSFF`/`EVIOCRMFF` request numbers are
/// computed from this size, so a layout mismatch fails the ioctl outright
/// rather than silently.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FfEffect {
    pub type_: u16,
    pub id: i16,
    pub direction: u16,
    pub trigger_button: u16,
    pub trigger_interval: u16,
    pub replay_length: u16,
    pub replay_delay: u16,
    pub u: FfUnion,
}

/// Encode one `struct input_event` (64-bit ABI, zeroed timestamp; the
/// kernel fills timestamps in itself for written `EV_FF` events).
pub fn encode_ff_event(code: u16, value: i32) -> [u8; EVENT_SIZE] {
    let mut b = [0u8; EVENT_SIZE];
    b[16..18].copy_from_slice(&EV_FF.to_le_bytes());
    b[18..20].copy_from_slice(&code.to_le_bytes());
    b[20..24].copy_from_slice(&value.to_le_bytes());
    b
}

/// Whether `bits` (an `EVIOCGBIT(EV_FF, ...)` result, any length) marks
/// `ff_type` as supported. Pure bit test, no I/O: the ioctl call itself
/// stays in the front-end's [`FfDevice`] implementation. Also used for
/// [`FF_GAIN`]/[`FF_AUTOCENTER`], which live in the same `EV_FF` code
/// space as the effect types (see [`StepAction::supported`]).
pub fn ff_type_supported(bits: &[u8], ff_type: u16) -> bool {
    let idx = usize::from(ff_type);
    bits.get(idx / 8).is_some_and(|b| b & (1 << (idx % 8)) != 0)
}

// ---------------------------------------------------------------------------
// Step table.
// ---------------------------------------------------------------------------

/// `ff_envelope` (`linux/input.h`): a linear attack ramp in from
/// `attack_level` over `attack_length` ms, then a linear fade out to
/// `fade_level` over `fade_length` ms at the effect's end. All-zero (see
/// [`ENVELOPE_NONE`]) disables both and every effect plays at its flat
/// magnitude for its whole duration, which is what every step except the
/// envelope demo uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Envelope {
    pub attack_length: u16,
    pub attack_level: u16,
    pub fade_length: u16,
    pub fade_level: u16,
}

/// The disabled envelope: every field zero. A named constant (rather than
/// `Envelope::default()`) because the step tables below are `const`s and
/// a derived `Default` impl is not `const fn`.
pub const ENVELOPE_NONE: Envelope = Envelope { attack_length: 0, attack_level: 0, fade_length: 0, fade_level: 0 };

/// One playable effect's kind and parameters, sized to build exactly one
/// [`FfEffect`] variant's union. Field layouts (and the sign convention
/// for the condition effects) are exactly what
/// `mainline/hid-logitech-hidpp.c` expects; see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEffect {
    /// `ff_constant_effect`: a steady pull, optionally attack/fade-shaped
    /// by `envelope` (see [`ENVELOPE_NONE`] for "no shaping, flat level").
    Constant { level: i16, envelope: Envelope },
    /// `ff_ramp_effect`: force rising (or falling) linearly over the
    /// step's duration.
    Ramp { start: i16, end: i16 },
    /// `ff_condition_effect`: a centering spring. `right_coeff`/
    /// `left_coeff` must both be positive (see the module doc) - a
    /// restoring spring, not an anti-spring on one side.
    Spring { right_coeff: i16, left_coeff: i16, right_sat: u16, left_sat: u16 },
    /// `ff_condition_effect`: resistance proportional to turning speed.
    /// Same positive-both-sides requirement as [`StepEffect::Spring`].
    Damper { right_coeff: i16, left_coeff: i16, right_sat: u16, left_sat: u16 },
    /// `ff_condition_effect`: resistance to motion that does NOT scale
    /// with speed, unlike [`StepEffect::Damper`] - a constant drag the
    /// instant the wheel moves at all, rather than growing the faster it
    /// turns. Same struct and sign requirement as [`StepEffect::Spring`]/
    /// [`StepEffect::Damper`] (see `hidpp_dd_ff_effect_tick`'s
    /// `FF_FRICTION` case, a Karnopp-style model). Only the DD engine
    /// advertises `FF_FRICTION`; the G923's classic engine does not
    /// (hardware-probed live), so this is the step a G923 run skips.
    Friction { right_coeff: i16, left_coeff: i16, right_sat: u16, left_sat: u16 },
    /// `ff_condition_effect`: resistance proportional to turning
    /// acceleration - the simulated mass a quick flick of the wheel has
    /// to fight against. Same struct and sign requirement as
    /// [`StepEffect::Spring`]/[`StepEffect::Damper`]/
    /// [`StepEffect::Friction`] (all four share `ff_condition_effect` -
    /// see `hidpp_dd_ff_effect_tick`'s `FF_INERTIA` case and
    /// `dd_lg4ff_wheel_effects`).
    Inertia { right_coeff: i16, left_coeff: i16, right_sat: u16, left_sat: u16 },
    /// `ff_periodic_effect`: a waveform vibration (sine/square/triangle/
    /// sawtooth - see [`FF_SINE`]/[`FF_SQUARE`]/[`FF_TRIANGLE`]/
    /// [`FF_SAW_UP`]), optionally attack/fade-shaped by `envelope` same as
    /// [`StepEffect::Constant`].
    Periodic { waveform: u16, period_ms: u16, magnitude: i16, envelope: Envelope },
}

impl StepEffect {
    /// The `ff_effect.type` value this variant uploads as (what
    /// `EVIOCGBIT(EV_FF, ...)` capability bit gates it, and what
    /// [`build_ff_effect`] sets `type_` to).
    pub fn ff_type(&self) -> u16 {
        match self {
            StepEffect::Constant { .. } => FF_CONSTANT,
            StepEffect::Ramp { .. } => FF_RAMP,
            StepEffect::Spring { .. } => FF_SPRING,
            StepEffect::Damper { .. } => FF_DAMPER,
            StepEffect::Friction { .. } => FF_FRICTION,
            StepEffect::Inertia { .. } => FF_INERTIA,
            StepEffect::Periodic { .. } => FF_PERIODIC,
        }
    }
}

/// What a step actually does when it plays. Most steps are the plain
/// case ([`StepAction::Effect`]: upload, play, wait, stop, erase exactly
/// one effect - what every step did before this table grew); a few
/// exercise device-level state or need two effects live at once. Every
/// kind unconditionally cleans up whatever it touched - stopping/erasing
/// an uploaded effect, restoring gain, resetting autocenter - before
/// [`run_sequence`] moves on, on every exit including a mid-step cancel;
/// see the `run_*_step` functions next to [`run_sequence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAction {
    /// Upload, play, wait the step's duration, stop, erase.
    Effect(StepEffect),
    /// Upload and play both effects at once for the step's duration, then
    /// stop and erase both. The only step that exercises the classic
    /// engine's slot-0 mixing (constant/ramp/periodic sum in slot 0;
    /// conditions get their own slots 1-3 - see `dd-lg4ff.c`'s per-slot
    /// effect table) rather than one effect at a time like every other
    /// step.
    Mixed(StepEffect, StepEffect),
    /// Play `effect` for the first half of the step's duration at full
    /// device gain, then the second half at `demo_gain`, so the user can
    /// feel `FF_GAIN` change the same force mid-play. Full gain
    /// ([`SIM_GAIN_FULL`]) is restored unconditionally afterward,
    /// including on a cancel in either half.
    GainDemo { effect: StepEffect, demo_gain: i32 },
    /// Not an uploaded effect at all: a device-level `FF_AUTOCENTER`
    /// write to `level`, held for the step's duration, then reset to 0
    /// unconditionally afterward, including on cancel.
    Autocenter { level: i32 },
}

impl StepAction {
    /// Whether every `ff_effect.type`/device capability this action needs
    /// is present in `bits` (an `EVIOCGBIT(EV_FF, ...)` result). `FF_GAIN`
    /// and `FF_AUTOCENTER` live in the same `EV_FF` code space as the
    /// effect types, so [`ff_type_supported`] covers device-level state
    /// too, not just uploaded effects.
    fn supported(&self, bits: &[u8]) -> bool {
        match self {
            StepAction::Effect(e) => ff_type_supported(bits, e.ff_type()),
            StepAction::Mixed(a, b) => {
                ff_type_supported(bits, a.ff_type()) && ff_type_supported(bits, b.ff_type())
            }
            StepAction::GainDemo { effect, .. } => {
                ff_type_supported(bits, effect.ff_type()) && ff_type_supported(bits, FF_GAIN)
            }
            StepAction::Autocenter { .. } => ff_type_supported(bits, FF_AUTOCENTER),
        }
    }
}

/// One step of a test sequence: what to do, for how long, in which
/// logical direction, how long to count down first, and the human label
/// a front-end shows while it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimStep {
    /// Shown by both front-ends while this step plays (see
    /// [`step_status_text`]).
    pub label: &'static str,
    pub action: StepAction,
    /// `ff_effect.replay.length` for [`StepAction::Effect`]/
    /// [`StepAction::Mixed`] (both effects get the same length), and how
    /// long [`run_sequence`] holds the step's device-level state
    /// ([`StepAction::GainDemo`]/[`StepAction::Autocenter`]) or plays it
    /// before moving on (or ending, if `cancel` flips first).
    pub duration_ms: u16,
    /// The logical side this step pulls toward; [`Side::None`] for the
    /// condition effects and the autocenter demo (direction is
    /// meaningless to both - see the module doc). Resolved to a raw
    /// `ff_effect.direction` value by [`build_ff_effect`], which needs
    /// the playing wheel's model to do it (see [`resolve_direction`]).
    pub direction: Side,
    /// How long [`run_sequence`] counts down before this step plays -
    /// [`STEP_COUNTDOWN_LONG`] for steps that need the user to actually
    /// turn the wheel once it starts, [`STEP_COUNTDOWN_SHORT`] for steps
    /// where holding the rim is enough.
    pub countdown: Countdown,
}

/// Build the kernel `ff_effect` for `effect`, `direction` and
/// `duration_ms` on a wheel of `model` (which resolves `direction`'s
/// logical [`Side`] to the raw value this model's engine expects - see
/// [`resolve_direction`]), id `-1` (a fresh upload; the kernel assigns
/// one). The core builder both [`build_ff_effect`] and every
/// [`StepAction`] variant's own upload(s) go through.
fn effect_to_ff(effect: &StepEffect, direction: Side, duration_ms: u16, model: WheelModel) -> FfEffect {
    let mut u = [0u8; FF_UNION_SIZE];
    match *effect {
        StepEffect::Constant { level, envelope } => {
            // ff_constant_effect: level:i16 @0, envelope @2 (attack_length
            // @2, attack_level @4, fade_length @6, fade_level @8).
            u[0..2].copy_from_slice(&level.to_le_bytes());
            u[2..4].copy_from_slice(&envelope.attack_length.to_le_bytes());
            u[4..6].copy_from_slice(&envelope.attack_level.to_le_bytes());
            u[6..8].copy_from_slice(&envelope.fade_length.to_le_bytes());
            u[8..10].copy_from_slice(&envelope.fade_level.to_le_bytes());
        }
        StepEffect::Ramp { start, end } => {
            // ff_ramp_effect: start_level:i16 @0, end_level:i16 @2
            // (envelope, zeroed, follows - this crate never shapes a
            // ramp's own attack/fade).
            u[0..2].copy_from_slice(&start.to_le_bytes());
            u[2..4].copy_from_slice(&end.to_le_bytes());
        }
        StepEffect::Spring { right_coeff, left_coeff, right_sat, left_sat }
        | StepEffect::Damper { right_coeff, left_coeff, right_sat, left_sat }
        | StepEffect::Friction { right_coeff, left_coeff, right_sat, left_sat }
        | StepEffect::Inertia { right_coeff, left_coeff, right_sat, left_sat } => {
            // ff_condition_effect: right_saturation:u16 @0,
            // left_saturation:u16 @2, right_coeff:i16 @4, left_coeff:i16
            // @6, deadband:u16 @8 (0) and center:i16 @10 (0) - a true
            // center, exactly what "pulls back to center" needs.
            u[0..2].copy_from_slice(&right_sat.to_le_bytes());
            u[2..4].copy_from_slice(&left_sat.to_le_bytes());
            u[4..6].copy_from_slice(&right_coeff.to_le_bytes());
            u[6..8].copy_from_slice(&left_coeff.to_le_bytes());
        }
        StepEffect::Periodic { waveform, period_ms, magnitude, envelope } => {
            // ff_periodic_effect: waveform:u16 @0, period:u16 @2,
            // magnitude:i16 @4, offset:i16 @6 (0), phase:u16 @8 (0),
            // envelope @10 (attack_length @10, attack_level @12,
            // fade_length @14, fade_level @16).
            u[0..2].copy_from_slice(&waveform.to_le_bytes());
            u[2..4].copy_from_slice(&period_ms.to_le_bytes());
            u[4..6].copy_from_slice(&magnitude.to_le_bytes());
            u[10..12].copy_from_slice(&envelope.attack_length.to_le_bytes());
            u[12..14].copy_from_slice(&envelope.attack_level.to_le_bytes());
            u[14..16].copy_from_slice(&envelope.fade_length.to_le_bytes());
            u[16..18].copy_from_slice(&envelope.fade_level.to_le_bytes());
        }
    }
    FfEffect {
        type_: effect.ff_type(),
        id: -1,
        direction: resolve_direction(direction, model),
        trigger_button: 0,
        trigger_interval: 0,
        replay_length: duration_ms,
        replay_delay: 0,
        u: FfUnion(u),
    }
}

/// Build the kernel `ff_effect` for `step` on a wheel of `model`. Only
/// meaningful for a [`StepAction::Effect`] step (every real single-effect
/// step in both tables); panics on any other [`StepAction`] variant,
/// which never happens outside a test calling this directly on a
/// known-Effect step, since [`run_sequence`] itself calls [`effect_to_ff`]
/// per action kind rather than through this function.
pub fn build_ff_effect(step: &SimStep, model: WheelModel) -> FfEffect {
    match &step.action {
        StepAction::Effect(effect) => effect_to_ff(effect, step.direction, step.duration_ms, model),
        other => panic!("build_ff_effect: step {:?} is not a single Effect action ({other:?})", step.label),
    }
}

/// The force-feedback sequence, hardware-tested and re-tuned on a live
/// G923 (2026-07-27): raised amplitude (~30% to [`SIM_LEVEL_60`]/
/// [`SIM_SATURATION_60`], ~60% of range - clearly feelable, not maximal),
/// longer per-step durations (1.8-4 s, was 0.45 s pulses), and the old
/// six alternating left/right pulses collapsed to exactly one left step
/// and one right step (repeating them taught nothing extra: one left
/// pulse and one right pulse of equal duration already self-cancel
/// positionally).
///
/// Covers every `FF_*` effect type the DD engine advertises
/// (`hid-logitech-hidpp.c`'s DD `set_bit` list): constant, ramp, all four
/// condition effects (spring/damper/friction/inertia), four periodic
/// waveforms (sine/square/triangle/sawtooth), an envelope (attack/fade)
/// demo, a two-effect mix (the only step that exercises the classic
/// engine's slot-0 summing rather than one effect at a time), a gain demo
/// (`FF_GAIN`) and an autocenter demo (`FF_AUTOCENTER`).
///
/// The G923's classic engine (`dd-lg4ff.c`'s `dd_lg4ff_wheel_effects`)
/// advertises that same set minus `FF_FRICTION` (hardware-probed on a
/// live G923, 2026-07-27), so on a G923 the friction step is always
/// skipped - the one row [`ff_type_supported`]'s capability check ever
/// takes out of this table. `FF_SAW_DOWN` gets no step of its own: see
/// [`FF_SAW_UP`]'s doc comment for why one sawtooth direction is enough.
///
/// Every step's own [`SimStep::countdown`] is [`STEP_COUNTDOWN_LONG`] if
/// the step needs the user to actually turn the wheel once it starts
/// (spring, damper, friction, inertia, autocenter), else
/// [`STEP_COUNTDOWN_SHORT`] (holding the rim is enough). Total run time
/// (all fifteen steps' own durations plus their countdowns, nothing else)
/// is pinned by a test below to stay under the task's ~70 s budget for a
/// DD wheel where nothing is skipped, and comes in lower still on a G923,
/// where the friction step's own duration and countdown are never spent.
///
/// Condition effects (spring/damper/friction/inertia) always play one at
/// a time here - each is its own step, uploaded, played, stopped and
/// erased before the next step's countdown ever starts (see
/// [`run_effect_step`]) - so at most one of the classic engine's three
/// condition slots (1-3; slot 0 is constant/ramp/periodic) is ever in use
/// at once. Nothing in this table risks the 3-slot ceiling; a future step
/// that plays a condition effect concurrently with another (the way
/// [`StepAction::Mixed`] does for constant+periodic) would need to check
/// that explicitly.
pub const FORCE_SEQUENCE: &[SimStep] = &[
    SimStep {
        label: "Constant force, left - hold the rim and feel it pull",
        action: StepAction::Effect(StepEffect::Constant { level: SIM_LEVEL_60, envelope: ENVELOPE_NONE }),
        duration_ms: 1800,
        direction: Side::Left,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Constant force, right - hold the rim and feel it pull",
        action: StepAction::Effect(StepEffect::Constant { level: SIM_LEVEL_60, envelope: ENVELOPE_NONE }),
        duration_ms: 1800,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Ramp, rising force - hold the rim as it builds",
        action: StepAction::Effect(StepEffect::Ramp { start: 0, end: SIM_LEVEL_60 }),
        duration_ms: 1800,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Spring / centering - turn the wheel and release, it pulls back to center",
        action: StepAction::Effect(StepEffect::Spring {
            right_coeff: SIM_LEVEL_60,
            left_coeff: SIM_LEVEL_60,
            right_sat: SIM_SATURATION_60,
            left_sat: SIM_SATURATION_60,
        }),
        duration_ms: 3500,
        direction: Side::None,
        countdown: STEP_COUNTDOWN_LONG,
    },
    SimStep {
        label: "Damper - turn the wheel, feel the resistance to motion",
        action: StepAction::Effect(StepEffect::Damper {
            right_coeff: SIM_LEVEL_60,
            left_coeff: SIM_LEVEL_60,
            right_sat: SIM_SATURATION_60,
            left_sat: SIM_SATURATION_60,
        }),
        duration_ms: 3500,
        direction: Side::None,
        countdown: STEP_COUNTDOWN_LONG,
    },
    SimStep {
        label: "Friction - turn the wheel, feel a steady drag that doesn't grow with speed (unlike the damper)",
        action: StepAction::Effect(StepEffect::Friction {
            right_coeff: SIM_LEVEL_60,
            left_coeff: SIM_LEVEL_60,
            right_sat: SIM_SATURATION_60,
            left_sat: SIM_SATURATION_60,
        }),
        duration_ms: 3500,
        direction: Side::None,
        countdown: STEP_COUNTDOWN_LONG,
    },
    SimStep {
        label: "Inertia - turn the wheel quickly, feel the simulated mass (FF_INERTIA)",
        action: StepAction::Effect(StepEffect::Inertia {
            right_coeff: SIM_LEVEL_60,
            left_coeff: SIM_LEVEL_60,
            right_sat: SIM_SATURATION_60,
            left_sat: SIM_SATURATION_60,
        }),
        duration_ms: 3500,
        direction: Side::None,
        countdown: STEP_COUNTDOWN_LONG,
    },
    SimStep {
        label: "Sine vibration - hold the rim and feel a smooth buzz",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 25,
            magnitude: SIM_LEVEL_60,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Square wave - hold the rim, a notchier, harsher buzz than the sine",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SQUARE,
            period_ms: 25,
            magnitude: SIM_LEVEL_60,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Sawtooth - hold the rim, a ratcheting texture",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SAW_UP,
            period_ms: 25,
            magnitude: SIM_LEVEL_60,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Triangle wave - hold the rim, smoother ramps than the square, more even than the sawtooth",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_TRIANGLE,
            period_ms: 25,
            magnitude: SIM_LEVEL_60,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Envelope demo - hold the rim, the vibration fades in and out smoothly",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 25,
            magnitude: SIM_LEVEL_60,
            envelope: Envelope { attack_length: 500, attack_level: 0, fade_length: 500, fade_level: 0 },
        }),
        duration_ms: 2200,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Mixed effects - hold the rim, a steady pull with a vibration on top",
        action: StepAction::Mixed(
            StepEffect::Constant { level: SIM_LEVEL_60, envelope: ENVELOPE_NONE },
            StepEffect::Periodic { waveform: FF_SINE, period_ms: 25, magnitude: SIM_LEVEL_60, envelope: ENVELOPE_NONE },
        ),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Gain demo - hold the rim, the same pull at full strength then about 30 percent",
        action: StepAction::GainDemo {
            effect: StepEffect::Constant { level: SIM_LEVEL_60, envelope: ENVELOPE_NONE },
            demo_gain: SIM_GAIN_DEMO_LOW,
        },
        duration_ms: 3000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Autocenter demo - turn the wheel and release, it centers on its own (FF_AUTOCENTER)",
        action: StepAction::Autocenter { level: SIM_AUTOCENTER_LEVEL },
        duration_ms: 4000,
        direction: Side::None,
        countdown: STEP_COUNTDOWN_LONG,
    },
];

/// The TrueForce texture sequence: a frequency progression through four
/// `FF_PERIODIC`/`FF_SINE` steps (10 Hz through 100 Hz) at the same
/// moderate amplitude, so the user can feel that the wheel reproduces a
/// range rather than one fixed tone. Untouched by the force sequence's
/// re-tune (the owner's "faint"/"short"/"back and forth" feedback was
/// about the constant-force test, not this one) apart from picking up
/// [`STEP_COUNTDOWN_SHORT`] for every step, now that countdown length is
/// per-step data: every step here is passive (hold the rim and feel the
/// frequency), the same category the retuned force sequence's passive
/// steps use. Nominal effect playback alone is 8 s; with the short
/// countdown's 1.5 s lead-in before each of the four steps, the whole run
/// takes about 14 s end to end.
pub const TEXTURE_SEQUENCE: &[SimStep] = &[
    SimStep {
        label: "Low rumble (~10 Hz) - a slow, heavy pulse",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 100,
            magnitude: SIM_LEVEL_30,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Buzz (~25 Hz) - a coarse, gritty texture",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 40,
            magnitude: SIM_LEVEL_30,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "Mid-high texture (~50 Hz) - a finer grain",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 20,
            magnitude: SIM_LEVEL_30,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
    SimStep {
        label: "High-frequency texture (~100 Hz) - a fine, tight buzz",
        action: StepAction::Effect(StepEffect::Periodic {
            waveform: FF_SINE,
            period_ms: 10,
            magnitude: SIM_LEVEL_30,
            envelope: ENVELOPE_NONE,
        }),
        duration_ms: 2000,
        direction: Side::Right,
        countdown: STEP_COUNTDOWN_SHORT,
    },
];

/// Which sequence a confirmed simulation plays; replaces the old
/// `SimKind` both front-ends kept locally (`ConstantForce`/`Texture`),
/// now backed by [`FORCE_SEQUENCE`]/[`TEXTURE_SEQUENCE`] instead of one
/// fixed effect each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimKind {
    Force,
    Texture,
}

impl SimKind {
    pub fn label(self) -> &'static str {
        match self {
            SimKind::Force => "force feedback",
            SimKind::Texture => "TrueForce texture",
        }
    }

    /// The step table this kind plays, in order.
    pub fn steps(self) -> &'static [SimStep] {
        match self {
            SimKind::Force => FORCE_SEQUENCE,
            SimKind::Texture => TEXTURE_SEQUENCE,
        }
    }
}

/// The status text a front-end shows while `step` (0-based `row` of
/// `total`) plays. Shared so both front-ends' status lines read
/// identically.
pub fn step_status_text(row: usize, total: usize, step: &SimStep) -> String {
    format!("step {}/{total}: {}", row + 1, step.label)
}

// ---------------------------------------------------------------------------
// Per-step progress: the state machine both front-ends render the same
// way (a persistent list of every step, pending through done/skipped),
// fed by [`SequenceEvent`]s from a running (or finished) [`run_sequence`].
// ---------------------------------------------------------------------------

/// One step's state in a rendered plan. Exactly the five states the task
/// calls for: waiting its turn, ticking down before it plays, playing,
/// finished (fully, or stopped early by a cancel that still cleaned up),
/// or skipped for lacking device support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    /// Counting down before this step plays; ticks from
    /// [`Countdown::ticks`] down to 1.
    Countdown(u64),
    Playing,
    Done,
    Skipped,
}

impl StepState {
    /// A short, state-only word (never relying on color alone to carry
    /// meaning, since some renderers show these next to a color swatch):
    /// "pending", "3..." while counting down, "playing", "done",
    /// "skipped".
    pub fn status_text(&self) -> String {
        match self {
            StepState::Pending => "pending".to_string(),
            StepState::Countdown(secs) => format!("{secs}..."),
            StepState::Playing => "playing".to_string(),
            StepState::Done => "done".to_string(),
            StepState::Skipped => "skipped".to_string(),
        }
    }
}

/// The whole plan's live progress: one [`StepState`] per row of the step
/// table currently playing (or just finished), plus which row is
/// currently active. Lives here, not per front-end, so the GUI and TUI
/// render the exact same rows from the exact same source - only the
/// widgets differ. A front-end builds one with [`SequenceProgress::new`]
/// the moment a sequence is confirmed (so the full plan renders before
/// anything plays), then folds every [`SequenceEvent`] the running
/// sequence reports into it with [`SequenceProgress::apply`], and keeps
/// showing the result after the run ends - nothing here ever reverts a
/// row back to `Pending`, which is what keeps a finished step's row on
/// screen instead of it disappearing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceProgress {
    pub states: Vec<StepState>,
    /// The row a `Countdown` or `Step` event last pointed at; cleared back
    /// to `None` once that row's `Done` event lands. `None` before
    /// anything starts and once the whole run has ended.
    pub current: Option<usize>,
}

impl SequenceProgress {
    /// One row per entry in `steps`, all `Pending`, nothing active yet.
    pub fn new(steps: &[SimStep]) -> Self {
        SequenceProgress { states: vec![StepState::Pending; steps.len()], current: None }
    }

    /// Fold one [`SequenceEvent`] in: mark the row(s) it names with the
    /// matching [`StepState`]. Unknown rows (out of range for whatever
    /// `steps` this was built from) are ignored rather than panicking -
    /// defensive only, every real event's row always fits the table it
    /// came from.
    pub fn apply(&mut self, event: &SequenceEvent) {
        match *event {
            SequenceEvent::Skipped(rows) => {
                for &(row, _) in rows {
                    if let Some(s) = self.states.get_mut(row) {
                        *s = StepState::Skipped;
                    }
                }
            }
            SequenceEvent::Countdown { row, seconds_left, .. } => {
                if let Some(s) = self.states.get_mut(row) {
                    *s = StepState::Countdown(seconds_left);
                }
                self.current = Some(row);
            }
            SequenceEvent::Step { row, .. } => {
                if let Some(s) = self.states.get_mut(row) {
                    *s = StepState::Playing;
                }
                self.current = Some(row);
            }
            SequenceEvent::Done { row, .. } => {
                if let Some(s) = self.states.get_mut(row) {
                    *s = StepState::Done;
                }
                self.current = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The sequence runner.
// ---------------------------------------------------------------------------

/// An error from one [`FfDevice`] operation. `Gone` (`ENODEV`: the wheel
/// was unplugged mid-run) ends the sequence quietly; `Other` surfaces its
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    Gone,
    Other(String),
}

impl DeviceError {
    fn into_end(self) -> SequenceEnd {
        match self {
            DeviceError::Gone => SequenceEnd::DeviceGone,
            DeviceError::Other(msg) => SequenceEnd::Failed(msg),
        }
    }
}

/// What [`run_sequence`] needs from an open device connection. Each
/// front-end implements this over its own `std::fs::File` + `libc`
/// `ioctl`s (`EVIOCSFF`/`EVIOCRMFF`/`EVIOCGBIT`); this crate never opens a
/// device node, so none of that plumbing lives here.
pub trait FfDevice {
    /// Set the overall device gain (0..=0xFFFF). Called once before the
    /// first step (a zero gain, left by another tool or a fresh power-up,
    /// would make every step silently do nothing), and again by the gain
    /// demo step to restore full gain afterward.
    fn set_gain(&mut self, value: i32) -> Result<(), DeviceError>;
    /// Set the device's autocenter strength (0..=0xFFFF, `FF_AUTOCENTER`).
    /// Only ever called by the autocenter demo step: `level` at the
    /// start, `0` unconditionally afterward.
    fn set_autocenter(&mut self, value: i32) -> Result<(), DeviceError>;
    /// Upload `effect` (`EVIOCSFF`), returning the kernel-assigned id.
    fn upload(&mut self, effect: &FfEffect) -> Result<i16, DeviceError>;
    /// Start (`value != 0`) or stop (`value == 0`) an uploaded effect.
    fn play(&mut self, id: i16, value: i32) -> Result<(), DeviceError>;
    /// Erase a previously uploaded effect (`EVIOCRMFF`). Best-effort by
    /// design: [`run_sequence`] calls this as unconditional cleanup after
    /// every step, including ones that already failed, so there is
    /// nothing further to recover from an erase error.
    fn erase(&mut self, id: i16);
    /// The `EV_FF` capability bitmap (`EVIOCGBIT(EV_FF, ...)`, at least
    /// [`FF_BITS_LEN`] bytes), used to skip steps the device does not
    /// advertise.
    fn ff_bits(&mut self) -> [u8; FF_BITS_LEN];
}

/// How a run of [`run_sequence`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceEnd {
    /// Every runnable step played out its full duration.
    Completed,
    /// `cancel` flipped before or during a step; already-uploaded effects
    /// were stopped and erased first, and any device-level state (gain,
    /// autocenter) that step had already changed was restored.
    Cancelled,
    /// The device disappeared mid-run (`ENODEV`); ended quietly, no error.
    DeviceGone,
    /// A device operation failed for some other reason.
    Failed(String),
}

/// The outcome of one whole sequence run: how it ended, how many steps
/// actually played (fully or partially, if cancelled mid-step), and which
/// steps were skipped for lacking device support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceOutcome {
    pub end: SequenceEnd,
    pub ran: usize,
    pub skipped: Vec<&'static str>,
}

impl SequenceOutcome {
    /// A one-line human summary a front-end can show as the final status
    /// (it already showed [`step_status_text`] for each step along the
    /// way; this is just how the run ended).
    pub fn summary(&self) -> String {
        let base = match &self.end {
            SequenceEnd::Completed => "finished".to_string(),
            SequenceEnd::Cancelled => "stopped".to_string(),
            SequenceEnd::DeviceGone => "wheel disconnected, stopped".to_string(),
            SequenceEnd::Failed(msg) => format!("error: {msg}"),
        };
        if self.skipped.is_empty() {
            base
        } else {
            format!("{base} (not supported by this wheel, skipped: {})", self.skipped.join(", "))
        }
    }
}

/// What [`run_sequence`] reports back through `on_event`, in the order
/// they can happen: the skip list once up front (only if non-empty, and
/// always before the first `Countdown`, so a rendered plan can mark those
/// rows before anything else happens), then a `Countdown`/`Step`/`Done`
/// triple per runnable step. `row` is always 0-based into the `steps`
/// slice `run_sequence` was given, and `total` is always `steps.len()` -
/// both stay stable across skips, so a front-end's rendered plan (one row
/// per table entry, in table order) never needs to renumber anything.
#[derive(Debug, Clone, Copy)]
pub enum SequenceEvent<'a> {
    /// The full skip list, determined once up front from the device's
    /// `EVIOCGBIT` capability query: each unsupported step's row and
    /// label.
    Skipped(&'a [(usize, &'static str)]),
    /// Row `row` is counting down before it plays; `seconds_left` ticks
    /// from the countdown's tick count down to 1.
    Countdown { row: usize, total: usize, step: &'a SimStep, seconds_left: u64 },
    /// Row `row` is now playing.
    Step { row: usize, total: usize, step: &'a SimStep },
    /// Row `row` finished: it played out its full duration, or was
    /// stopped early by a cancel that arrived mid-play - either way it
    /// was cleanly stopped and erased first, so its row is done either
    /// way.
    Done { row: usize, total: usize },
}

/// Run `steps` against `device`, resolving each step's logical [`Side`]
/// against `model` (see [`resolve_direction`] - this is the one place the
/// DD/G923 direction-sign divergence actually matters to playback).
///
/// The device's [`FfDevice::ff_bits`] capability query determines the
/// full skip list up front (reported once via [`SequenceEvent::Skipped`]
/// before anything else); every other row then gets its own
/// [`SimStep::countdown`] lead-in, plays via the [`StepAction`]-specific
/// `run_*_step` function below (always cleaning up whatever it touched -
/// effect slot, gain, autocenter - before returning), and only then does
/// the next row's countdown start, so nothing leaks between steps.
///
/// Cancellable at any point: before a row's countdown starts, during a
/// countdown tick, or during a row's play wait - in every case the
/// sequence stops right there (with whatever was already uploaded or
/// changed cleanly restored) rather than continuing to the next row.
pub fn run_sequence(
    device: &mut impl FfDevice,
    steps: &[SimStep],
    model: WheelModel,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(SequenceEvent),
) -> SequenceOutcome {
    let bits = device.ff_bits();
    let total = steps.len();
    let mut skip_mask = vec![false; total];
    let mut skipped_rows: Vec<(usize, &'static str)> = Vec::new();
    let mut skipped_labels: Vec<&'static str> = Vec::new();
    for (row, step) in steps.iter().enumerate() {
        if !step.action.supported(&bits) {
            skip_mask[row] = true;
            skipped_rows.push((row, step.label));
            skipped_labels.push(step.label);
        }
    }
    if !skipped_rows.is_empty() {
        on_event(SequenceEvent::Skipped(&skipped_rows));
    }

    if skipped_labels.len() == total {
        return SequenceOutcome { end: SequenceEnd::Completed, ran: 0, skipped: skipped_labels };
    }

    if let Err(e) = device.set_gain(SIM_GAIN_FULL) {
        return SequenceOutcome { end: e.into_end(), ran: 0, skipped: skipped_labels };
    }

    let mut ran = 0;
    for (row, step) in steps.iter().enumerate() {
        if skip_mask[row] {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            return SequenceOutcome { end: SequenceEnd::Cancelled, ran, skipped: skipped_labels };
        }

        for seconds_left in (1..=step.countdown.ticks).rev() {
            on_event(SequenceEvent::Countdown { row, total, step, seconds_left });
            if wait_out(step.countdown.tick, cancel) == WaitOutcome::Cancelled {
                return SequenceOutcome { end: SequenceEnd::Cancelled, ran, skipped: skipped_labels };
            }
        }

        on_event(SequenceEvent::Step { row, total, step });

        let step_result = match &step.action {
            StepAction::Effect(effect) => run_effect_step(device, step, effect, model, cancel),
            StepAction::Mixed(a, b) => run_mixed_step(device, step, a, b, model, cancel),
            StepAction::GainDemo { effect, demo_gain } => {
                run_gain_demo_step(device, step, effect, *demo_gain, model, cancel)
            }
            StepAction::Autocenter { level } => run_autocenter_step(device, *level, step.duration_ms, cancel),
        };

        let wait_outcome = match step_result {
            Ok(w) => w,
            Err(e) => return SequenceOutcome { end: e.into_end(), ran, skipped: skipped_labels },
        };

        ran += 1;
        on_event(SequenceEvent::Done { row, total });

        if wait_outcome == WaitOutcome::Cancelled {
            return SequenceOutcome { end: SequenceEnd::Cancelled, ran, skipped: skipped_labels };
        }
    }

    SequenceOutcome { end: SequenceEnd::Completed, ran, skipped: skipped_labels }
}

/// [`StepAction::Effect`]: the common single-effect case, unchanged from
/// before this table grew a `StepAction` enum - upload, play, wait, stop,
/// erase, always all four regardless of how the wait ended.
fn run_effect_step<D: FfDevice>(
    device: &mut D,
    step: &SimStep,
    effect: &StepEffect,
    model: WheelModel,
    cancel: &AtomicBool,
) -> Result<WaitOutcome, DeviceError> {
    let ff = effect_to_ff(effect, step.direction, step.duration_ms, model);
    let id = device.upload(&ff)?;
    let played = device.play(id, 1);
    let wait_outcome = if played.is_ok() {
        wait_out(Duration::from_millis(u64::from(step.duration_ms)), cancel)
    } else {
        WaitOutcome::Completed
    };
    let _ = device.play(id, 0);
    device.erase(id);
    played?;
    Ok(wait_outcome)
}

/// [`StepAction::Mixed`]: upload both effects, play both, wait once, stop
/// both, erase both - always all of that regardless of how far it got
/// (a failed second upload still stops/erases the first; a failed play
/// still stops/erases whichever of the two started).
fn run_mixed_step<D: FfDevice>(
    device: &mut D,
    step: &SimStep,
    a: &StepEffect,
    b: &StepEffect,
    model: WheelModel,
    cancel: &AtomicBool,
) -> Result<WaitOutcome, DeviceError> {
    let ff_a = effect_to_ff(a, step.direction, step.duration_ms, model);
    let ff_b = effect_to_ff(b, step.direction, step.duration_ms, model);
    let id_a = device.upload(&ff_a)?;
    let id_b = match device.upload(&ff_b) {
        Ok(id) => id,
        Err(e) => {
            let _ = device.play(id_a, 0);
            device.erase(id_a);
            return Err(e);
        }
    };

    let played_a = device.play(id_a, 1);
    let played_b = if played_a.is_ok() { device.play(id_b, 1) } else { Ok(()) };
    let wait_outcome = if played_a.is_ok() && played_b.is_ok() {
        wait_out(Duration::from_millis(u64::from(step.duration_ms)), cancel)
    } else {
        WaitOutcome::Completed
    };

    let _ = device.play(id_a, 0);
    let _ = device.play(id_b, 0);
    device.erase(id_a);
    device.erase(id_b);

    played_a?;
    played_b?;
    Ok(wait_outcome)
}

/// [`StepAction::GainDemo`]: play `effect` at full gain for the first
/// half of the step's duration, then at `demo_gain` for the second half.
/// Stops and erases the effect, then restores [`SIM_GAIN_FULL`],
/// unconditionally - whatever happened above (full duration, a cancel in
/// either half, or a failed play/gain write) - so the device is never
/// left at the demo's reduced gain.
fn run_gain_demo_step<D: FfDevice>(
    device: &mut D,
    step: &SimStep,
    effect: &StepEffect,
    demo_gain: i32,
    model: WheelModel,
    cancel: &AtomicBool,
) -> Result<WaitOutcome, DeviceError> {
    let ff = effect_to_ff(effect, step.direction, step.duration_ms, model);
    let id = device.upload(&ff)?;

    let half = step.duration_ms / 2;
    let rest = step.duration_ms - half;

    let mut result = device.play(id, 1).map(|()| WaitOutcome::Completed);
    if let Ok(WaitOutcome::Completed) = result {
        let first_half = wait_out(Duration::from_millis(u64::from(half)), cancel);
        result = if first_half == WaitOutcome::Cancelled {
            Ok(WaitOutcome::Cancelled)
        } else {
            device.set_gain(demo_gain).map(|()| wait_out(Duration::from_millis(u64::from(rest)), cancel))
        };
    }

    let _ = device.play(id, 0);
    device.erase(id);
    // Unconditional: never leave the device at the demo's reduced gain,
    // whatever happened above.
    let restore = device.set_gain(SIM_GAIN_FULL);

    let outcome = result?;
    restore?;
    Ok(outcome)
}

/// [`StepAction::Autocenter`]: set autocenter to `level`, hold for
/// `duration_ms`, then reset to 0 - unconditionally, whatever happened
/// above - so the device is never left with autocenter enabled.
fn run_autocenter_step<D: FfDevice>(
    device: &mut D,
    level: i32,
    duration_ms: u16,
    cancel: &AtomicBool,
) -> Result<WaitOutcome, DeviceError> {
    let set = device.set_autocenter(level);
    let wait_outcome = if set.is_ok() {
        wait_out(Duration::from_millis(u64::from(duration_ms)), cancel)
    } else {
        WaitOutcome::Completed
    };
    // Unconditional: never leave autocenter enabled, whatever happened.
    let reset = device.set_autocenter(0);
    set?;
    reset?;
    Ok(wait_outcome)
}

/// How a playback (or gap) wait ended: it ran out `duration`, or `cancel`
/// flipped first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Completed,
    Cancelled,
}

/// Sleep out `duration` in [`SIM_CANCEL_POLL`] ticks, returning early as
/// soon as `cancel` flips (including if it is already set on entry, which
/// returns immediately with no sleep at all - what makes cancellation
/// tests fast).
fn wait_out(duration: Duration, cancel: &AtomicBool) -> WaitOutcome {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return WaitOutcome::Cancelled;
        }
        std::thread::sleep(SIM_CANCEL_POLL.min(duration));
    }
    WaitOutcome::Completed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unwrap a step's single [`StepEffect`], panicking (with a message
    /// naming the step) on any other [`StepAction`] - a test-only
    /// convenience for the many steps that are still the plain
    /// single-effect case.
    fn single_effect(step: &SimStep) -> &StepEffect {
        match &step.action {
            StepAction::Effect(e) => e,
            other => panic!("{}: expected a single Effect action, got {other:?}", step.label),
        }
    }

    // -----------------------------------------------------------------
    // ff_effect layout / capability bits.
    // -----------------------------------------------------------------

    #[test]
    fn ff_effect_layout_matches_kernel_abi() {
        assert_eq!(std::mem::size_of::<FfEffect>(), 48);
        assert_eq!(std::mem::align_of::<FfEffect>(), 8);
        let e = build_ff_effect(&FORCE_SEQUENCE[0], WheelModel::Rs50);
        let union_offset = (&e.u as *const _ as usize) - (&e as *const _ as usize);
        assert_eq!(union_offset, 16);
    }

    #[test]
    fn ff_type_supported_reads_the_bitmap() {
        let mut bits = [0u8; FF_BITS_LEN];
        bits[FF_CONSTANT as usize / 8] |= 1 << (FF_CONSTANT as usize % 8);
        assert!(ff_type_supported(&bits, FF_CONSTANT));
        assert!(!ff_type_supported(&bits, FF_RAMP));
        // A too-short slice never panics, just reports unsupported.
        assert!(!ff_type_supported(&[], FF_CONSTANT));
    }

    #[test]
    fn envelope_bytes_land_at_the_offsets_the_kernel_expects() {
        // ff_constant_effect: level @0 (2 bytes), then ff_envelope's four
        // u16 fields packed at @2, @4, @6, @8.
        let constant = effect_to_ff(
            &StepEffect::Constant {
                level: 100,
                envelope: Envelope { attack_length: 111, attack_level: 222, fade_length: 333, fade_level: 444 },
            },
            Side::Right,
            0,
            WheelModel::Rs50,
        );
        assert_eq!(u16::from_le_bytes([constant.u.0[2], constant.u.0[3]]), 111, "attack_length");
        assert_eq!(u16::from_le_bytes([constant.u.0[4], constant.u.0[5]]), 222, "attack_level");
        assert_eq!(u16::from_le_bytes([constant.u.0[6], constant.u.0[7]]), 333, "fade_length");
        assert_eq!(u16::from_le_bytes([constant.u.0[8], constant.u.0[9]]), 444, "fade_level");

        // ff_periodic_effect: waveform/period/magnitude/offset/phase (10
        // bytes), then the same four envelope fields at @10, @12, @14, @16.
        let periodic = effect_to_ff(
            &StepEffect::Periodic {
                waveform: FF_SINE,
                period_ms: 25,
                magnitude: 100,
                envelope: Envelope { attack_length: 555, attack_level: 0, fade_length: 666, fade_level: 0 },
            },
            Side::Right,
            0,
            WheelModel::Rs50,
        );
        assert_eq!(u16::from_le_bytes([periodic.u.0[10], periodic.u.0[11]]), 555, "attack_length");
        assert_eq!(u16::from_le_bytes([periodic.u.0[14], periodic.u.0[15]]), 666, "fade_length");
    }

    // -----------------------------------------------------------------
    // Step tables: exactly what we intend to upload.
    // -----------------------------------------------------------------

    #[test]
    fn force_sequence_has_the_fifteen_specified_steps_in_order() {
        assert_eq!(FORCE_SEQUENCE.len(), 15);
        let types: Vec<Option<u16>> = FORCE_SEQUENCE
            .iter()
            .map(|s| match &s.action {
                StepAction::Effect(e) => Some(e.ff_type()),
                _ => None,
            })
            .collect();
        assert_eq!(
            types,
            vec![
                Some(FF_CONSTANT), // left
                Some(FF_CONSTANT), // right
                Some(FF_RAMP),
                Some(FF_SPRING),
                Some(FF_DAMPER),
                Some(FF_FRICTION),
                Some(FF_INERTIA),
                Some(FF_PERIODIC), // sine
                Some(FF_PERIODIC), // square
                Some(FF_PERIODIC), // sawtooth
                Some(FF_PERIODIC), // triangle
                Some(FF_PERIODIC), // envelope demo
                None,              // mixed: constant + sine
                None,              // gain demo
                None,              // autocenter demo
            ]
        );
        assert!(matches!(FORCE_SEQUENCE[12].action, StepAction::Mixed(..)), "row 12 is the mixed step");
        assert!(matches!(FORCE_SEQUENCE[13].action, StepAction::GainDemo { .. }), "row 13 is the gain demo");
        assert!(matches!(FORCE_SEQUENCE[14].action, StepAction::Autocenter { .. }), "row 14 is the autocenter demo");
    }

    #[test]
    fn force_sequence_opens_with_exactly_one_left_step_and_one_right_step() {
        // Replaces the old six alternating 0.45s pulses: repeating them
        // taught nothing extra (one left step and one right step of equal
        // duration already self-cancel positionally), and the owner
        // reported the repetition as pointless ("I wonder why we pull
        // back and forth so many times").
        let left = single_effect(&FORCE_SEQUENCE[0]);
        let right = single_effect(&FORCE_SEQUENCE[1]);
        assert_eq!(FORCE_SEQUENCE[0].direction, Side::Left);
        assert_eq!(FORCE_SEQUENCE[1].direction, Side::Right);
        assert!(FORCE_SEQUENCE[0].label.contains("left"));
        assert!(FORCE_SEQUENCE[1].label.contains("right"));
        for (step, effect) in [(&FORCE_SEQUENCE[0], left), (&FORCE_SEQUENCE[1], right)] {
            let StepEffect::Constant { level, .. } = effect else { panic!("expected a Constant step") };
            assert_eq!(*level, SIM_LEVEL_60, "raised amplitude");
            // Raised duration: 1.8-2s, was a 0.45s pulse.
            assert!((1800..=2000).contains(&step.duration_ms), "duration_ms={}", step.duration_ms);
        }
        // No third left or right step anywhere in the table.
        assert!(
            FORCE_SEQUENCE[2..].iter().all(|s| !matches!(&s.action, StepAction::Effect(StepEffect::Constant { .. }))
                || s.direction == Side::Right && matches!(s.action, StepAction::GainDemo { .. })),
            "no leftover alternating pulses past row 1"
        );
    }

    #[test]
    fn force_sequence_amplitude_is_raised_to_the_specified_55_to_65_percent_band() {
        // The owner's "very very faint" feedback: raised from ~30% to
        // ~60% of range, still short of maximal.
        let pct = f64::from(SIM_LEVEL_60) / f64::from(i16::MAX) * 100.0;
        assert!((55.0..=65.0).contains(&pct), "SIM_LEVEL_60 is {pct}% of i16::MAX");
        let sat_pct = f64::from(SIM_SATURATION_60) / f64::from(u16::MAX) * 100.0;
        assert!((55.0..=65.0).contains(&sat_pct), "SIM_SATURATION_60 is {sat_pct}% of u16::MAX");
    }

    #[test]
    fn force_sequence_ramp_rises_from_zero_to_the_raised_level() {
        let StepEffect::Ramp { start, end } = single_effect(&FORCE_SEQUENCE[2]) else {
            panic!("expected a Ramp step");
        };
        assert_eq!(*start, 0);
        assert_eq!(*end, SIM_LEVEL_60);
    }

    #[test]
    fn force_sequence_spring_damper_friction_inertia_use_a_true_restoring_pair_and_the_long_countdown() {
        // Spring (3), damper (4), friction (5), inertia (6): grouped
        // together, right next to each other, each its own step.
        for row in [3, 4, 5, 6] {
            let step = &FORCE_SEQUENCE[row];
            assert_eq!(step.direction, Side::None, "row {row}: condition effects ignore direction");
            assert_eq!(step.countdown, STEP_COUNTDOWN_LONG, "row {row}: an active 'turn the wheel' step");
            assert!(step.duration_ms >= 3500 && step.duration_ms <= 4000, "row {row} duration_ms={}", step.duration_ms);
            let (right_coeff, left_coeff, right_sat, left_sat) = match single_effect(step) {
                StepEffect::Spring { right_coeff, left_coeff, right_sat, left_sat }
                | StepEffect::Damper { right_coeff, left_coeff, right_sat, left_sat }
                | StepEffect::Friction { right_coeff, left_coeff, right_sat, left_sat }
                | StepEffect::Inertia { right_coeff, left_coeff, right_sat, left_sat } => {
                    (*right_coeff, *left_coeff, *right_sat, *left_sat)
                }
                other => panic!("row {row}: expected a condition effect, got {other:?}"),
            };
            // Both coefficients must be positive and equal: a negative
            // pair would build an anti-spring/anti-damper on one side
            // instead of a symmetric centering force (see
            // hidpp_dd_condition_force's sign convention in the module
            // doc).
            assert!(right_coeff > 0);
            assert!(left_coeff > 0);
            assert_eq!(right_coeff, left_coeff);
            assert!(right_sat > 0);
            assert_eq!(right_sat, left_sat);
        }
        assert!(matches!(single_effect(&FORCE_SEQUENCE[5]), StepEffect::Friction { .. }), "row 5 is friction");
        assert!(FORCE_SEQUENCE[5].label.to_lowercase().contains("friction"));
        // The task's distinguishing point: the label must call out how
        // friction differs from the damper (constant drag vs. speed-
        // scaled resistance), not just name the effect.
        assert!(FORCE_SEQUENCE[5].label.contains("damper"), "label: {}", FORCE_SEQUENCE[5].label);
        assert!(FORCE_SEQUENCE[6].label.contains("FF_INERTIA"));
    }

    #[test]
    fn force_sequence_periodic_steps_cover_sine_square_sawtooth_and_triangle() {
        let waveforms: Vec<u16> = [7, 8, 9, 10]
            .iter()
            .map(|&row| match single_effect(&FORCE_SEQUENCE[row]) {
                StepEffect::Periodic { waveform, .. } => *waveform,
                other => panic!("row {row}: expected Periodic, got {other:?}"),
            })
            .collect();
        assert_eq!(waveforms, vec![FF_SINE, FF_SQUARE, FF_SAW_UP, FF_TRIANGLE]);
        assert!(FORCE_SEQUENCE[8].label.contains("notchier") && FORCE_SEQUENCE[8].label.contains("harsher"));
        assert!(FORCE_SEQUENCE[9].label.contains("ratchet"));
        // The triangle step's label must place it relative to both its
        // neighbors, per the task: smoother than the square, more even
        // than the sawtooth.
        assert!(FORCE_SEQUENCE[10].label.to_lowercase().contains("triangle"));
        assert!(FORCE_SEQUENCE[10].label.contains("smoother") && FORCE_SEQUENCE[10].label.contains("square"));
        assert!(FORCE_SEQUENCE[10].label.contains("even") && FORCE_SEQUENCE[10].label.contains("sawtooth"));
        for row in [7, 8, 9, 10] {
            assert!((2000..=2500).contains(&FORCE_SEQUENCE[row].duration_ms), "row {row}");
            assert_eq!(FORCE_SEQUENCE[row].countdown, STEP_COUNTDOWN_SHORT, "row {row}: passive, hold the rim");
        }
    }

    #[test]
    fn force_sequence_envelope_demo_shapes_a_periodic_effect_with_a_nonzero_attack_and_fade() {
        let StepEffect::Periodic { envelope, .. } = single_effect(&FORCE_SEQUENCE[11]) else {
            panic!("expected the envelope demo to be a Periodic step");
        };
        assert!(envelope.attack_length > 0, "attack must be nonzero to actually fade in");
        assert!(envelope.fade_length > 0, "fade must be nonzero to actually fade out");
        assert!(FORCE_SEQUENCE[11].label.contains("fade"));
        assert!((2000..=2500).contains(&FORCE_SEQUENCE[11].duration_ms));
    }

    #[test]
    fn force_sequence_mixed_step_plays_a_constant_and_a_periodic_together() {
        let StepAction::Mixed(a, b) = &FORCE_SEQUENCE[12].action else { panic!("expected a Mixed step") };
        assert!(matches!(a, StepEffect::Constant { .. }), "first effect is the steady pull");
        assert!(matches!(b, StepEffect::Periodic { .. }), "second effect is the vibration on top");
        assert!(FORCE_SEQUENCE[12].label.contains("vibration on top"));
    }

    #[test]
    fn force_sequence_gain_demo_uses_a_reduced_gain_well_below_full() {
        let StepAction::GainDemo { effect, demo_gain } = &FORCE_SEQUENCE[13].action else {
            panic!("expected a GainDemo step")
        };
        assert!(matches!(effect, StepEffect::Constant { .. }));
        assert!(*demo_gain > 0 && *demo_gain < SIM_GAIN_FULL / 2, "clearly reduced, not just slightly");
        assert!(FORCE_SEQUENCE[13].label.contains("30 percent"));
    }

    #[test]
    fn force_sequence_autocenter_demo_uses_the_long_countdown_and_a_device_level_event() {
        let StepAction::Autocenter { level } = &FORCE_SEQUENCE[14].action else {
            panic!("expected an Autocenter step")
        };
        assert!(*level > 0 && *level < SIM_GAIN_FULL, "a real but non-maximal strength");
        assert_eq!(FORCE_SEQUENCE[14].direction, Side::None);
        assert_eq!(FORCE_SEQUENCE[14].countdown, STEP_COUNTDOWN_LONG, "an active 'turn the wheel' step");
        assert_eq!(FORCE_SEQUENCE[14].duration_ms, 4000, "the task's specified ~4s hold");
        assert!(FORCE_SEQUENCE[14].label.contains("turn the wheel"));
    }

    #[test]
    fn force_sequence_countdown_is_long_only_for_the_turn_the_wheel_steps() {
        let long_rows: Vec<usize> =
            FORCE_SEQUENCE.iter().enumerate().filter(|(_, s)| s.countdown == STEP_COUNTDOWN_LONG).map(|(i, _)| i).collect();
        // Spring, damper, friction, inertia, autocenter - exactly the
        // steps that need the user to actually turn the wheel once it
        // starts.
        assert_eq!(long_rows, vec![3, 4, 5, 6, 14]);
        assert!(
            FORCE_SEQUENCE.iter().enumerate().filter(|(i, _)| !long_rows.contains(i)).all(|(_, s)| s.countdown == STEP_COUNTDOWN_SHORT),
            "every other row uses the short, passive countdown"
        );
    }

    /// Every step's own duration plus its own countdown, nothing else (no
    /// separate pre-sequence wait exists) - what a user actually
    /// experiences end to end for the rows in `rows`.
    fn total_ms_for(rows: impl Iterator<Item = usize>) -> u64 {
        rows.map(|row| {
            let s = &FORCE_SEQUENCE[row];
            let countdown_ms = s.countdown.ticks * u64::try_from(s.countdown.tick.as_millis()).unwrap();
            u64::from(s.duration_ms) + countdown_ms
        })
        .sum()
    }

    #[test]
    fn force_sequence_total_time_stays_under_the_tasks_70s_budget_on_a_dd_wheel() {
        // A DD wheel (RS50/G PRO) skips nothing: every one of the fifteen
        // rows plays, friction included.
        let total_ms = total_ms_for(0..FORCE_SEQUENCE.len());
        assert!(total_ms < 70_000, "total_ms={total_ms}, must stay under the task's ~70s budget");
        // Comfortably under, not just barely - regression guard against a
        // future step quietly pushing it over, and against one quietly
        // shrinking the table back down.
        assert!(total_ms > 60_000, "total_ms={total_ms} looks suspiciously short for 15 steps");
    }

    #[test]
    fn force_sequence_total_time_is_lower_still_on_a_g923_which_skips_friction() {
        // A G923 run never spends the friction row's own duration or
        // countdown: `run_sequence` skips it outright (see
        // `run_sequence_skips_friction_on_a_g923_like_bitmap_lacking_ff_
        // friction` below for the runner-level proof), so the G923's real
        // total is the DD total minus exactly that row.
        let dd_total_ms = total_ms_for(0..FORCE_SEQUENCE.len());
        let friction_row_ms = total_ms_for(std::iter::once(5));
        let g923_total_ms = dd_total_ms - friction_row_ms;
        assert!(g923_total_ms < dd_total_ms, "skipping a row must shorten the run");
        assert!(g923_total_ms < 65_000, "g923_total_ms={g923_total_ms}");
        assert!(g923_total_ms > 55_000, "g923_total_ms={g923_total_ms} looks suspiciously short");
    }

    #[test]
    fn resolve_direction_is_swapped_on_the_g923_but_not_on_dd_wheels() {
        // DD wheels (RS50, G PRO, and Unknown - treated as DD-shaped, same
        // as everywhere else WheelModel gates DD vs. classic behavior)
        // keep today's values: hidpp_dd_project_constant's documented
        // convention, direction=0x4000 is rightward.
        for model in [WheelModel::Rs50, WheelModel::GPro, WheelModel::Unknown] {
            assert_eq!(resolve_direction(Side::Right, model), 0x4000, "{model:?} right");
            assert_eq!(resolve_direction(Side::Left, model), 0xC000, "{model:?} left");
        }
        // The G923's classic (lg4ff-style) engine does the opposite in
        // practice (hardware-verified 2026-07-27): its "left" pulls with
        // the DD engine's "right" value, and vice versa.
        assert_eq!(resolve_direction(Side::Left, WheelModel::G923), 0x4000);
        assert_eq!(resolve_direction(Side::Right, WheelModel::G923), 0xC000);
        // Condition effects and autocenter ignore direction on every model.
        for model in [WheelModel::Rs50, WheelModel::GPro, WheelModel::Unknown, WheelModel::G923] {
            assert_eq!(resolve_direction(Side::None, model), 0, "{model:?} condition effect");
        }
    }

    #[test]
    fn build_ff_effect_resolves_the_force_sequences_left_and_right_directions_per_model() {
        // The task's exact acceptance check: on a G923 the "left" step
        // must carry 0x4000 and "right" must carry 0xC000; on an RS50
        // (the primary DD device) it is the reverse - today's values,
        // unchanged.
        let left = &FORCE_SEQUENCE[0];
        let right = &FORCE_SEQUENCE[1];
        assert_eq!(build_ff_effect(left, WheelModel::Rs50).direction, 0xC000, "Rs50 left: today's value");
        assert_eq!(build_ff_effect(right, WheelModel::Rs50).direction, 0x4000, "Rs50 right: today's value");
        assert_eq!(build_ff_effect(left, WheelModel::G923).direction, 0x4000, "G923 left: swapped");
        assert_eq!(build_ff_effect(right, WheelModel::G923).direction, 0xC000, "G923 right: swapped");
    }

    #[test]
    fn texture_sequence_progresses_through_rising_frequencies_at_one_amplitude() {
        assert!((3..=4).contains(&TEXTURE_SEQUENCE.len()));
        let mut last_period = u16::MAX;
        for step in TEXTURE_SEQUENCE {
            let StepEffect::Periodic { waveform, period_ms, magnitude, .. } = single_effect(step) else {
                panic!("expected every texture step to be Periodic");
            };
            assert_eq!(*waveform, FF_SINE);
            assert_eq!(*magnitude, SIM_LEVEL_30, "one moderate amplitude throughout, unchanged by the retune");
            assert!(*period_ms < last_period, "each step's frequency must rise (period must fall)");
            last_period = *period_ms;
            assert_eq!(step.countdown, STEP_COUNTDOWN_SHORT, "every texture step is passive");
        }
        let total_ms: u32 = TEXTURE_SEQUENCE.iter().map(|s| u32::from(s.duration_ms)).sum();
        assert!((6_000..=10_000).contains(&total_ms), "total_ms={total_ms}");
    }

    #[test]
    fn build_ff_effect_matches_the_step_it_was_built_from() {
        let e = build_ff_effect(&FORCE_SEQUENCE[1], WheelModel::Rs50);
        assert_eq!(e.type_, FF_CONSTANT);
        assert_eq!(e.id, -1, "fresh upload, kernel assigns the id");
        assert_eq!(e.direction, 0x4000, "Rs50 right step: today's convention");
        assert_eq!(e.replay_length, FORCE_SEQUENCE[1].duration_ms);
        assert_eq!(i16::from_le_bytes([e.u.0[0], e.u.0[1]]), SIM_LEVEL_60);

        let spring = build_ff_effect(&FORCE_SEQUENCE[3], WheelModel::Rs50);
        assert_eq!(spring.type_, FF_SPRING);
        assert_eq!(u16::from_le_bytes([spring.u.0[0], spring.u.0[1]]), SIM_SATURATION_60);
        assert_eq!(i16::from_le_bytes([spring.u.0[4], spring.u.0[5]]), SIM_LEVEL_60, "right_coeff");
        assert_eq!(i16::from_le_bytes([spring.u.0[6], spring.u.0[7]]), SIM_LEVEL_60, "left_coeff");
    }

    #[test]
    #[should_panic(expected = "not a single Effect action")]
    fn build_ff_effect_panics_on_a_non_effect_step() {
        build_ff_effect(&FORCE_SEQUENCE[14], WheelModel::Rs50); // the autocenter demo
    }

    #[test]
    fn sim_kind_maps_to_the_matching_table() {
        assert_eq!(SimKind::Force.steps(), FORCE_SEQUENCE);
        assert_eq!(SimKind::Texture.steps(), TEXTURE_SEQUENCE);
    }

    // -----------------------------------------------------------------
    // The runner, against a mock FfDevice.
    // -----------------------------------------------------------------

    /// A device double that never touches a real fd: counts uploads and
    /// erases (to catch a leaked effect slot), records every play (id,
    /// value), gain and autocenter call, and can be told which
    /// `ff_effect.type`s it supports and to "disappear" (ENODEV) after a
    /// given number of uploads.
    struct MockDevice {
        supported: Vec<u16>,
        uploads: usize,
        erases: usize,
        plays: Vec<(i16, i32)>,
        gain_calls: Vec<i32>,
        autocenter_calls: Vec<i32>,
        next_id: i16,
        gone_after_uploads: Option<usize>,
        /// If set, `erase` flips this once its call count reaches the
        /// given number - used to exercise the between-steps cancel check
        /// (the top-of-loop check ahead of the next row's countdown)
        /// without any real sleeping.
        cancel_after_erases: Option<(usize, std::sync::Arc<AtomicBool>)>,
    }

    impl MockDevice {
        fn supporting(types: &[u16]) -> Self {
            MockDevice {
                supported: types.to_vec(),
                uploads: 0,
                erases: 0,
                plays: Vec::new(),
                gain_calls: Vec::new(),
                autocenter_calls: Vec::new(),
                next_id: 0,
                gone_after_uploads: None,
                cancel_after_erases: None,
            }
        }
    }

    impl FfDevice for MockDevice {
        fn set_gain(&mut self, value: i32) -> Result<(), DeviceError> {
            self.gain_calls.push(value);
            Ok(())
        }

        fn set_autocenter(&mut self, value: i32) -> Result<(), DeviceError> {
            self.autocenter_calls.push(value);
            Ok(())
        }

        fn upload(&mut self, effect: &FfEffect) -> Result<i16, DeviceError> {
            self.uploads += 1;
            if self.gone_after_uploads == Some(self.uploads) {
                return Err(DeviceError::Gone);
            }
            let id = self.next_id;
            self.next_id += 1;
            let _ = effect;
            Ok(id)
        }

        fn play(&mut self, id: i16, value: i32) -> Result<(), DeviceError> {
            self.plays.push((id, value));
            Ok(())
        }

        fn erase(&mut self, _id: i16) {
            self.erases += 1;
            if let Some((n, cancel)) = &self.cancel_after_erases {
                if self.erases == *n {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }

        fn ff_bits(&mut self) -> [u8; FF_BITS_LEN] {
            let mut bits = [0u8; FF_BITS_LEN];
            for t in &self.supported {
                bits[usize::from(*t) / 8] |= 1 << (usize::from(*t) % 8);
            }
            bits
        }
    }

    /// Every effect type plus FF_GAIN/FF_AUTOCENTER, i.e. "supports
    /// everything" - what most runner tests want so nothing is skipped.
    const ALL_TYPES: &[u16] = &[
        FF_CONSTANT,
        FF_RAMP,
        FF_SPRING,
        FF_DAMPER,
        FF_FRICTION,
        FF_INERTIA,
        FF_PERIODIC,
        FF_GAIN,
        FF_AUTOCENTER,
    ];

    /// Tiny local steps (duration/gap 0, [`Countdown::NONE`]) so runner
    /// tests exercise the exact same code paths as the real sequences
    /// without sleeping out their real durations.
    const QUICK_STEPS: &[SimStep] = &[
        SimStep {
            label: "one",
            action: StepAction::Effect(StepEffect::Constant { level: 100, envelope: ENVELOPE_NONE }),
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        },
        SimStep {
            label: "two",
            action: StepAction::Effect(StepEffect::Ramp { start: 0, end: 100 }),
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        },
        SimStep {
            label: "three",
            action: StepAction::Effect(StepEffect::Periodic {
                waveform: FF_SINE,
                period_ms: 10,
                magnitude: 100,
                envelope: ENVELOPE_NONE,
            }),
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        },
    ];

    /// The model every runner test plays against; the runner's own model-
    /// resolution logic is covered separately (`resolve_direction_is_
    /// swapped_on_the_g923_but_not_on_dd_wheels`,
    /// `build_ff_effect_resolves_the_force_sequences_left_and_right_
    /// directions_per_model`), so these tests just need any fixed model to
    /// exercise the upload/play/stop/erase machinery.
    const RUNNER_TEST_MODEL: WheelModel = WheelModel::Rs50;

    #[test]
    fn run_sequence_completes_every_step_with_no_leaked_effect_slot() {
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Step { row, total, step } = ev {
                seen.push((row, total, step.label));
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 3);
        assert!(outcome.skipped.is_empty());
        assert_eq!(seen, vec![(0, 3, "one"), (1, 3, "two"), (2, 3, "three")]);
        assert_eq!(device.uploads, 3);
        assert_eq!(device.erases, 3, "every upload must be erased - no leaked slot");
        // Each step plays (id, 1) then stops (id, 0), in order.
        assert_eq!(device.plays, vec![(0, 1), (0, 0), (1, 1), (1, 0), (2, 1), (2, 0)]);
    }

    #[test]
    fn run_sequence_skips_steps_the_device_does_not_advertise() {
        // Only FF_CONSTANT is supported: the Ramp and Periodic steps must
        // be skipped, not attempted (and never leaked, since they are
        // never uploaded at all). Rows stay 0-based into the full table,
        // so "two" is row 1 and "three" is row 2 even though row 0 ("one")
        // is the only one that actually runs.
        let mut device = MockDevice::supporting(&[FF_CONSTANT]);
        let cancel = AtomicBool::new(false);
        let mut skip_report = None;
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Skipped(rows) = ev {
                skip_report = Some(rows.to_vec());
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 1);
        assert_eq!(outcome.skipped, vec!["two", "three"]);
        assert_eq!(skip_report, Some(vec![(1, "two"), (2, "three")]));
        assert_eq!(device.uploads, 1);
        assert_eq!(device.erases, 1);
        assert!(outcome.summary().contains("not supported"));
    }

    #[test]
    fn run_sequence_skipping_every_step_runs_nothing_and_still_completes() {
        let mut device = MockDevice::supporting(&[]);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 0);
        assert_eq!(outcome.skipped.len(), 3);
        assert_eq!(device.uploads, 0);
        assert_eq!(device.erases, 0);
    }

    #[test]
    fn run_sequence_cancelled_before_it_starts_runs_nothing() {
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(true);
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(outcome.ran, 0);
        assert_eq!(device.uploads, 0);
    }

    #[test]
    fn run_sequence_cancelled_mid_step_stops_and_erases_then_ends() {
        // Flip `cancel` as soon as the second step's upload is about to
        // start (its Step event); the runner must still finish that
        // step's own upload/play/stop/erase cleanly (it already
        // committed to it) but never reach the third step.
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Step { row, step, .. } = ev {
                seen.push(step.label);
                if row == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(outcome.ran, 2, "the interrupted step counts as ran: it was stopped and erased cleanly");
        assert_eq!(seen, vec!["one", "two"], "the third step's Step event never fires");
        assert_eq!(device.uploads, 2);
        assert_eq!(device.erases, 2, "no leaked slot on a mid-step cancel");
    }

    #[test]
    fn run_sequence_cancelled_between_steps_ends_before_the_next_one_starts() {
        // No Step event ever sets `cancel`; instead the mock flips it the
        // moment the first step's erase happens, exercising the
        // between-steps cancel check (the top-of-loop check ahead of the
        // next row's countdown) rather than a step's own play-wait or
        // countdown-tick wait.
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut device = MockDevice::supporting(ALL_TYPES);
        device.cancel_after_erases = Some((1, cancel.clone()));
        let mut seen = Vec::new();
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Step { step, .. } = ev {
                seen.push(step.label);
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(outcome.ran, 1);
        assert_eq!(seen, vec!["one"], "the second step never starts");
        assert_eq!(device.uploads, 1);
        assert_eq!(device.erases, 1);
    }

    #[test]
    fn run_sequence_ends_quietly_on_device_gone_mid_upload() {
        let mut device = MockDevice::supporting(ALL_TYPES);
        device.gone_after_uploads = Some(2); // the second step's upload
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::DeviceGone);
        assert_eq!(outcome.ran, 1, "only the first step ever completed");
        assert_eq!(device.uploads, 2, "the failed upload still counts as attempted");
        assert_eq!(device.erases, 1, "nothing to erase for the upload that never succeeded");
        assert!(!outcome.summary().contains("error"), "ENODEV must not read as a generic error");
    }

    #[test]
    fn run_sequence_surfaces_a_non_enodev_failure() {
        struct FailGain;
        impl FfDevice for FailGain {
            fn set_gain(&mut self, _v: i32) -> Result<(), DeviceError> {
                Err(DeviceError::Other("permission denied".to_string()))
            }
            fn set_autocenter(&mut self, _v: i32) -> Result<(), DeviceError> {
                unreachable!()
            }
            fn upload(&mut self, _e: &FfEffect) -> Result<i16, DeviceError> {
                unreachable!()
            }
            fn play(&mut self, _id: i16, _v: i32) -> Result<(), DeviceError> {
                unreachable!()
            }
            fn erase(&mut self, _id: i16) {
                unreachable!()
            }
            fn ff_bits(&mut self) -> [u8; FF_BITS_LEN] {
                let mut b = [0u8; FF_BITS_LEN];
                b[FF_CONSTANT as usize / 8] |= 1 << (FF_CONSTANT as usize % 8);
                b
            }
        }
        let mut device = FailGain;
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, &QUICK_STEPS[..1], RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Failed("permission denied".to_string()));
        assert_eq!(outcome.ran, 0);
        assert!(outcome.summary().contains("permission denied"));
    }

    #[test]
    fn step_status_text_names_the_step_and_its_position() {
        let text = step_status_text(1, 15, &FORCE_SEQUENCE[1]);
        assert_eq!(text, "step 2/15: Constant force, right - hold the rim and feel it pull");
    }

    // -----------------------------------------------------------------
    // The mixed step: two effects uploaded, played, and erased together.
    // -----------------------------------------------------------------

    #[test]
    fn mixed_step_uploads_and_erases_both_effects() {
        let mixed_only: &[SimStep] = &[SimStep {
            label: "mixed",
            action: StepAction::Mixed(
                StepEffect::Constant { level: 100, envelope: ENVELOPE_NONE },
                StepEffect::Periodic { waveform: FF_SINE, period_ms: 10, magnitude: 100, envelope: ENVELOPE_NONE },
            ),
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, mixed_only, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 1);
        assert_eq!(device.uploads, 2, "both effects uploaded");
        assert_eq!(device.erases, 2, "both effects erased, no leaked slot");
        // Both play (id, 1), then both stop (id, 0).
        assert_eq!(device.plays, vec![(0, 1), (1, 1), (0, 0), (1, 0)]);
    }

    #[test]
    fn mixed_step_is_skipped_when_either_effect_type_is_unsupported() {
        // Only FF_CONSTANT: the mixed step needs FF_PERIODIC too, so it
        // must be skipped rather than uploading just the constant half.
        let mixed_only: &[SimStep] = &[SimStep {
            label: "mixed",
            action: StepAction::Mixed(
                StepEffect::Constant { level: 100, envelope: ENVELOPE_NONE },
                StepEffect::Periodic { waveform: FF_SINE, period_ms: 10, magnitude: 100, envelope: ENVELOPE_NONE },
            ),
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(&[FF_CONSTANT]);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, mixed_only, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.ran, 0);
        assert_eq!(outcome.skipped, vec!["mixed"]);
        assert_eq!(device.uploads, 0);
    }

    // -----------------------------------------------------------------
    // The friction step: capability-gated exactly like every other step,
    // which is what makes it play on a DD wheel and skip on a G923.
    // -----------------------------------------------------------------

    /// A one-step table shaped like `FORCE_SEQUENCE`'s friction row, but
    /// zero-duration/zero-countdown so a test can actually run it through
    /// `run_sequence` without sleeping out real time.
    const FRICTION_ONLY: &[SimStep] = &[SimStep {
        label: "friction",
        action: StepAction::Effect(StepEffect::Friction {
            right_coeff: 100,
            left_coeff: 100,
            right_sat: 100,
            left_sat: 100,
        }),
        duration_ms: 0,
        direction: Side::None,
        countdown: Countdown::NONE,
    }];

    #[test]
    fn run_sequence_plays_friction_on_a_dd_like_bitmap_that_advertises_ff_friction() {
        // DD wheels (RS50/G PRO) advertise FF_FRICTION alongside every
        // other condition effect - ALL_TYPES models exactly that.
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, FRICTION_ONLY, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 1, "friction plays when the device advertises it");
        assert!(outcome.skipped.is_empty());
        assert_eq!(device.uploads, 1);
        assert_eq!(device.erases, 1, "no leaked slot");
    }

    #[test]
    fn run_sequence_skips_friction_on_a_g923_like_bitmap_lacking_ff_friction() {
        // The G923's classic engine advertises the same set as a DD wheel
        // minus FF_FRICTION (hardware-probed on a live G923): every
        // ALL_TYPES entry except that one.
        let g923_like: Vec<u16> = ALL_TYPES.iter().copied().filter(|&t| t != FF_FRICTION).collect();
        let mut device = MockDevice::supporting(&g923_like);
        let cancel = AtomicBool::new(false);
        let mut skip_report = None;
        let outcome = run_sequence(&mut device, FRICTION_ONLY, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Skipped(rows) = ev {
                skip_report = Some(rows.to_vec());
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Completed, "an all-skipped run still completes cleanly");
        assert_eq!(outcome.ran, 0, "friction never plays without FF_FRICTION");
        assert_eq!(outcome.skipped, vec!["friction"]);
        assert_eq!(skip_report, Some(vec![(0, "friction")]));
        assert_eq!(device.uploads, 0, "never even attempted");
    }

    #[test]
    fn force_sequences_own_friction_row_is_supported_on_dd_bits_and_skipped_on_g923_bits() {
        // Same check, but against the real FORCE_SEQUENCE row (5) and
        // StepAction::supported directly, rather than a stand-in table -
        // confirms the actual shipped step, not just a lookalike.
        let friction_row = &FORCE_SEQUENCE[5];
        assert!(matches!(single_effect(friction_row), StepEffect::Friction { .. }));

        let mut dd_bits = [0u8; FF_BITS_LEN];
        for &t in ALL_TYPES {
            dd_bits[usize::from(t) / 8] |= 1 << (usize::from(t) % 8);
        }
        assert!(friction_row.action.supported(&dd_bits), "DD wheels advertise FF_FRICTION");

        let mut g923_bits = dd_bits;
        g923_bits[usize::from(FF_FRICTION) / 8] &= !(1 << (usize::from(FF_FRICTION) % 8));
        assert!(!friction_row.action.supported(&g923_bits), "G923 lacks FF_FRICTION, must be skipped");
    }

    // -----------------------------------------------------------------
    // The gain demo: restores full gain on completion and on cancel.
    // -----------------------------------------------------------------

    #[test]
    fn gain_demo_step_plays_full_then_reduced_then_restores_full() {
        let gain_only: &[SimStep] = &[SimStep {
            label: "gain demo",
            action: StepAction::GainDemo {
                effect: StepEffect::Constant { level: 100, envelope: ENVELOPE_NONE },
                demo_gain: 1000,
            },
            duration_ms: 0,
            direction: Side::Right,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, gain_only, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 1);
        assert_eq!(device.uploads, 1);
        assert_eq!(device.erases, 1);
        // gain_calls[0] is run_sequence's own up-front "ensure full gain";
        // then the demo's own reduced gain, then its restore.
        assert_eq!(device.gain_calls, vec![SIM_GAIN_FULL, 1000, SIM_GAIN_FULL]);
    }

    #[test]
    fn gain_demo_step_restores_full_gain_even_when_cancelled_mid_step() {
        // Cancel during the countdown-free wait: since duration_ms is 0
        // here, `wait_out` returns Completed immediately for both halves
        // (nothing to cancel mid-wait with a zero duration), so instead
        // cancel from inside the Step event itself, before the runner
        // even calls into run_gain_demo_step - the point is just that the
        // *up-front* gain (full) is untouched and no reduced gain is ever
        // left set.
        let gain_only: &[SimStep] = &[SimStep {
            label: "gain demo",
            action: StepAction::GainDemo {
                effect: StepEffect::Constant { level: 100, envelope: ENVELOPE_NONE },
                demo_gain: 1000,
            },
            duration_ms: 200,
            direction: Side::Right,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let cancel_clone = &cancel;
        let outcome = run_sequence(&mut device, gain_only, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Step { .. } = ev {
                cancel_clone.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        // The demo never got to lower the gain (the very first wait
        // caught the cancel), but full gain is still the last call:
        // run_gain_demo_step's restore always runs.
        assert_eq!(*device.gain_calls.last().unwrap(), SIM_GAIN_FULL);
        assert!(!device.gain_calls.contains(&1000), "cancelled before the reduced half ever played");
    }

    // -----------------------------------------------------------------
    // The autocenter demo: resets to 0 on completion and on cancel.
    // -----------------------------------------------------------------

    #[test]
    fn autocenter_step_sets_the_level_then_resets_to_zero() {
        let autocenter_only: &[SimStep] = &[SimStep {
            label: "autocenter",
            action: StepAction::Autocenter { level: 40000 },
            duration_ms: 0,
            direction: Side::None,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, autocenter_only, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(outcome.ran, 1);
        assert_eq!(device.autocenter_calls, vec![40000, 0]);
        // Never an uploaded effect: no upload/erase at all.
        assert_eq!(device.uploads, 0);
        assert_eq!(device.erases, 0);
    }

    #[test]
    fn autocenter_step_resets_to_zero_even_when_cancelled_mid_step() {
        let autocenter_only: &[SimStep] = &[SimStep {
            label: "autocenter",
            action: StepAction::Autocenter { level: 40000 },
            duration_ms: 200,
            direction: Side::None,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let cancel_clone = &cancel;
        let outcome = run_sequence(&mut device, autocenter_only, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Step { .. } = ev {
                cancel_clone.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(device.autocenter_calls, vec![40000, 0], "reset to zero even on a mid-step cancel");
    }

    #[test]
    fn autocenter_step_is_skipped_when_the_device_does_not_advertise_ff_autocenter() {
        let autocenter_only: &[SimStep] = &[SimStep {
            label: "autocenter",
            action: StepAction::Autocenter { level: 40000 },
            duration_ms: 0,
            direction: Side::None,
            countdown: Countdown::NONE,
        }];
        let mut device = MockDevice::supporting(&[FF_CONSTANT]);
        let cancel = AtomicBool::new(false);
        let outcome = run_sequence(&mut device, autocenter_only, RUNNER_TEST_MODEL, &cancel, |_| {});
        assert_eq!(outcome.ran, 0);
        assert_eq!(outcome.skipped, vec!["autocenter"]);
        assert!(device.autocenter_calls.is_empty(), "never touched: the step never ran");
    }

    // -----------------------------------------------------------------
    // The per-step countdown: ticks down before every step, including
    // the first, and is cancellable mid-tick just like a step's own
    // play-wait. Each step now carries its own Countdown (see
    // `SimStep::countdown`), rather than one shared value for every step.
    // -----------------------------------------------------------------

    #[test]
    fn run_sequence_counts_down_before_every_step_including_the_first() {
        let steps: &[SimStep] = &[SimStep { countdown: Countdown { ticks: 3, tick: Duration::ZERO }, ..QUICK_STEPS[0] }];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut events: Vec<String> = Vec::new();
        let outcome = run_sequence(&mut device, steps, RUNNER_TEST_MODEL, &cancel, |ev| match ev {
            SequenceEvent::Countdown { row, seconds_left, .. } => events.push(format!("countdown row={row} secs={seconds_left}")),
            SequenceEvent::Step { row, .. } => events.push(format!("step row={row}")),
            SequenceEvent::Done { row, .. } => events.push(format!("done row={row}")),
            SequenceEvent::Skipped(_) => {}
        });
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(
            events,
            vec![
                "countdown row=0 secs=3",
                "countdown row=0 secs=2",
                "countdown row=0 secs=1",
                "step row=0",
                "done row=0",
            ],
            "the very first step gets a countdown lead-in, same as every other one"
        );
    }

    #[test]
    fn run_sequence_uses_each_steps_own_countdown_not_a_shared_one() {
        // Row 0 gets three visible ticks, row 1 gets exactly one - each
        // step's own Countdown, not a single value applied uniformly.
        let steps: &[SimStep] = &[
            SimStep { countdown: Countdown { ticks: 3, tick: Duration::ZERO }, ..QUICK_STEPS[0] },
            SimStep { countdown: Countdown { ticks: 1, tick: Duration::ZERO }, ..QUICK_STEPS[1] },
        ];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut counts = [0u32; 2];
        let outcome = run_sequence(&mut device, steps, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Countdown { row, .. } = ev {
                counts[row] += 1;
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(counts, [3, 1]);
    }

    #[test]
    fn run_sequence_cancelled_mid_countdown_never_plays_that_step() {
        // Cancel as soon as the countdown reaches 2; the tick's own
        // nonzero wait must catch it before the step ever uploads.
        let steps: &[SimStep] = &[
            SimStep { countdown: Countdown { ticks: 3, tick: Duration::from_millis(20) }, ..QUICK_STEPS[0] },
            QUICK_STEPS[1],
            QUICK_STEPS[2],
        ];
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut seen: Vec<u64> = Vec::new();
        let outcome = run_sequence(&mut device, steps, RUNNER_TEST_MODEL, &cancel, |ev| {
            if let SequenceEvent::Countdown { seconds_left, .. } = ev {
                seen.push(seconds_left);
                if seconds_left == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(outcome.ran, 0, "the step never played: it was still counting down");
        assert_eq!(seen, vec![3, 2], "the tick after the cancel never fires");
        assert_eq!(device.uploads, 0, "cancelling mid-countdown must never upload the effect");
    }

    // -----------------------------------------------------------------
    // SequenceProgress: the rendered-plan state machine both front-ends
    // fold every SequenceEvent into.
    // -----------------------------------------------------------------

    #[test]
    fn sequence_progress_starts_pending_and_matches_the_step_table() {
        let progress = SequenceProgress::new(FORCE_SEQUENCE);
        assert_eq!(progress.states.len(), FORCE_SEQUENCE.len(), "one row per step, no more, no less");
        assert!(progress.states.iter().all(|s| *s == StepState::Pending));
        assert_eq!(progress.current, None);
    }

    #[test]
    fn sequence_progress_follows_one_step_through_countdown_play_and_done() {
        let mut progress = SequenceProgress::new(QUICK_STEPS);
        progress.apply(&SequenceEvent::Countdown { row: 0, total: 3, step: &QUICK_STEPS[0], seconds_left: 3 });
        assert_eq!(progress.states[0], StepState::Countdown(3));
        assert_eq!(progress.current, Some(0));
        assert_eq!(progress.states[1], StepState::Pending, "other rows are untouched");

        progress.apply(&SequenceEvent::Countdown { row: 0, total: 3, step: &QUICK_STEPS[0], seconds_left: 1 });
        assert_eq!(progress.states[0], StepState::Countdown(1));

        progress.apply(&SequenceEvent::Step { row: 0, total: 3, step: &QUICK_STEPS[0] });
        assert_eq!(progress.states[0], StepState::Playing);
        assert_eq!(progress.current, Some(0));

        progress.apply(&SequenceEvent::Done { row: 0, total: 3 });
        assert_eq!(progress.states[0], StepState::Done);
        assert_eq!(progress.current, None, "nothing active once the step is done");

        // A finished row's state is never reverted: the whole point is
        // that a completed step stays visible as done, not pending, once
        // the run has moved on (or ended).
        assert_eq!(progress.states[0], StepState::Done);
    }

    #[test]
    fn sequence_progress_marks_skipped_rows_without_touching_others() {
        let mut progress = SequenceProgress::new(QUICK_STEPS);
        progress.apply(&SequenceEvent::Skipped(&[(1, "two"), (2, "three")]));
        assert_eq!(progress.states, vec![StepState::Pending, StepState::Skipped, StepState::Skipped]);
    }

    #[test]
    fn sequence_progress_reflects_a_full_run_including_a_skip() {
        // Fold every event a real run_sequence call against QUICK_STEPS
        // (with only FF_CONSTANT/FF_PERIODIC supported, skipping the
        // Ramp) would report, and check the final rendered plan matches
        // exactly what actually happened: row 0 done, row 1 skipped
        // (never touched otherwise), row 2 done, nothing left active.
        let mut device = MockDevice::supporting(&[FF_CONSTANT, FF_PERIODIC]);
        let cancel = AtomicBool::new(false);
        let mut progress = SequenceProgress::new(QUICK_STEPS);
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            progress.apply(&ev);
        });
        assert_eq!(outcome.end, SequenceEnd::Completed);
        assert_eq!(progress.states, vec![StepState::Done, StepState::Skipped, StepState::Done]);
        assert_eq!(progress.current, None);
    }

    #[test]
    fn sequence_progress_keeps_a_cancelled_run_s_last_states_visible() {
        // Mirrors run_sequence_cancelled_mid_step_stops_and_erases_then_ends:
        // row 0 finishes, row 1 is interrupted mid-play by its own cancel
        // (still cleanly stopped and erased, so it reads as done), row 2
        // never starts and stays pending - none of that gets cleared away
        // once the run ends, which is the point of the whole feature.
        let mut device = MockDevice::supporting(ALL_TYPES);
        let cancel = AtomicBool::new(false);
        let mut progress = SequenceProgress::new(QUICK_STEPS);
        let outcome = run_sequence(&mut device, QUICK_STEPS, RUNNER_TEST_MODEL, &cancel, |ev| {
            progress.apply(&ev);
            if let SequenceEvent::Step { row, .. } = ev {
                if row == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        });
        assert_eq!(outcome.end, SequenceEnd::Cancelled);
        assert_eq!(progress.states, vec![StepState::Done, StepState::Done, StepState::Pending]);
    }

    #[test]
    fn step_state_status_text_never_relies_on_color_alone() {
        // Every state renders as a distinct word (or a ticking number),
        // never just a color swatch - the user is colorblind, so text is
        // the primary signal, color only ever a secondary one.
        assert_eq!(StepState::Pending.status_text(), "pending");
        assert_eq!(StepState::Countdown(3).status_text(), "3...");
        assert_eq!(StepState::Playing.status_text(), "playing");
        assert_eq!(StepState::Done.status_text(), "done");
        assert_eq!(StepState::Skipped.status_text(), "skipped");
    }
}
