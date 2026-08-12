# Logitech's real TrueForce texture, extracted from Windows captures

Source: `dev/captures/rs50_acc_2026-04-21/2026-04-21_trueforce_ace.pcapng`
(AC EVO on Windows, RS50, G HUB present). 119,028 stream packets, 250,140
audio samples at ~3983 Hz, analysed 2026-08-12. Parsing: type-0x01 packets,
new samples = last `byte[10]` slots of the 13-slot window at bytes 12+4i,
u16 LE offset-binary.

## What Windows sends that Proton does not

On Windows the ep3 stream carries BOTH the base force (`cur`, 1 kHz) and
real audio texture (`byte[10]=4` in 62,535 of 119,028 packets). Under Proton
tonight the SDK's stream carried `cur` only, `byte[10]=0` always. The
missing texture is the audio slots, and the injector on Windows is G HUB's
side (the OEM driver synthesises engine texture from the game's Escape RPM
stream; the game itself calls SetTorqueTF* zero times).

## The texture recipe (the tuning table)

An engine-firing-frequency harmonic stack, amplitude subtle:

| f0 (firing freq) | h2/h1 | h3/h1 | h4/h1 | h5/h1 |
|---|---|---|---|---|
| 140-190 Hz | 0.19 | 0.10 | 0.06 | 0.04 |
| 190-240 Hz | 0.19 | 0.13 | 0.08 | 0.05 |
| 240-290 Hz | 0.29 | 0.17 | 0.11 | 0.07 |
| 290-360 Hz | 0.28 | 0.33 | 0.08 | 0.07 |

- Amplitude: rms ~ 72 + 1.13 * f0 counts, i.e. **0.7% of fullscale at low
  revs rising to ~1.5% at high revs**. Occasional transients to ~2.3%.
- Harmonic falloff is much steeper than logi-tf-sim's current
  `HARMONIC_GAINS [1.0, 0.5, 0.25]`: the real h2 is ~0.2-0.3 and h3
  ~0.1-0.33 (h3 RISES with revs). Our synth is too bright and too loud
  relative to the real thing unless intensity is well below 100.
- At idle the fundamental may be suppressed (peaks at 2x/3x/4x of ~56.5 Hz
  for a ~850 rpm V8); at mid/high revs the fundamental dominates.

BeamNG on Windows (`g_pro_tf_2026-04-19/2026-04-19_trueforce_beamng.pcapng`)
carries 46,883 sample-packets and can cross-check the model on a second
title.

## Where to inject under Proton (open design question)

The SDK owns the ep3 stream (single writer) and fills only `cur`. Options:

1. **Kernel-side merge**: the stream passes through the kernel (hidraw
   write path); rewrite passing type-0x01 packets to add synth samples +
   count, exactly where Windows merges them. Base force stays bit-identical
   SDK output; texture is ours, tuned from this recipe, fed RPM via the
   existing wheel_rev_level/relay path.
2. **Own the stream**: drop the SDK, forward KF through libtrueforce and
   mix texture in userspace. Simpler layering, but replaces the SDK's
   bit-identical cur with our reconstruction.

Option 1 preserves the native base force exactly and matches the Windows
merge point. Not yet designed in detail.
