# Logitech's real TrueForce texture, extracted from Windows captures

Source: `dev/captures/rs50_acc_2026-04-21/2026-04-21_trueforce_ace.pcapng`
(AC EVO on Windows, RS50, G HUB present). 119,028 stream packets, 250,140
audio samples at ~3983 Hz, analysed 2026-08-12. Parsing: type-0x01 packets,
new samples = last `byte[10]` slots of the 13-slot window at bytes 12+4i,
u16 LE offset-binary.

## What Windows sends that Proton does not

On Windows the ep3 stream carries BOTH the base force (`cur`, 1 kHz) and
real audio texture (`byte[10]=4` in 62,535 of 119,028 packets). Under Proton
the SDK's stream carries `cur` only, `byte[10]=0` always. The missing texture
is the audio slots.

**Who injects it (corrected 2026-08-13):** the injector is **G HUB the running
process**, synthesising the engine note from the game's Escape RPM and merging
it into ep3. It is NOT Logitech's OEM DirectInput driver
(`hidpp_forcefeedback_x64.dll`): this capture is on `c276` (native mode), and
that driver does not claim c276 at all (only c262/c268/c26e/c272; it
null-derefs on c276). The game itself calls `SetTorqueTF*` zero times. So the
texture is G HUB's synthesis, and there is no G HUB on Linux and no
non-synthesised signal to obtain - reproducing it means synthesising like G HUB
and fitting to this capture. The compat-mode(c272)/OEM-driver path is a dead
end for AC EVO texture (and c272 needs a physical wheel-OLED switch).

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

### Rigorous re-measurement 2026-08-13 (2048-pt windowed FFT, FS=3983 Hz)

251,299 samples reconstructed; overall rms **1.48% FS**, 88% non-zero.
Fundamental (firing freq) ranges 41-385 Hz. Median harmonic ratios and
amplitude, binned by f0:

| f0 band (Hz) | rms (%FS) | h2/h1 | h3/h1 | h4/h1 | h5/h1 |
|---|---|---|---|---|---|
| 40-140 (idle, f0 suppressed) | 0.67 | 0.38 | 0.22 | 0.16 | 0.14 |
| 140-190 | 0.48 | 0.11 | 0.08 | 0.05 | 0.02 |
| 190-240 | 0.57 | 0.15 | 0.09 | 0.08 | 0.03 |
| 240-290 | 1.27 | 0.23 | 0.13 | 0.08 | 0.07 |
| 290-360 | 1.61 | 0.27 | 0.25 | 0.07 | 0.05 |

Confirms the earlier table: h2 rises ~0.11->0.27 and h3 ~0.08->0.25 with revs,
amplitude climbs ~0.5%->1.6% FS. The idle band's high ratios reflect the
suppressed fundamental (energy sits at 2x/3x). Extraction:
`/tmp/tex_samples.txt` from the awk+numpy pipeline in the 2026-08-13 session.

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
