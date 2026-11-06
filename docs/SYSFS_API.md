# Logitech TrueForce Linux Driver - Sysfs API Reference

**Driver**: `hid-logitech-dd` (module), `logitech-dd` (hid_driver)
**Devices**:
- Logitech RS50 Direct Drive Wheel Base (USB `046d:c276`)
- Logitech G Pro Racing Wheel (USB `046d:c272` Xbox/PC, `046d:c268` PS/PC)
- Logitech G923 (USB `046d:c266`/`c267` PlayStation edition, `046d:c26e` Xbox edition)

**Applies to**: the `hid-logitech-dd` driver in this repository.

This document lists the driver's sysfs attributes. Sysfs is the kernel's settings file tree under `/sys`. Each attribute is a small file. You read a file to get a value and write to it to set one.

The RS50 and the G Pro share most of the attributes here, because the two wheels share the settings code path. Where an attribute is G Pro-only or RS50-only, the text says so.

The **G923 is different**. None of the `wheel_*` attributes below exist on it. Its classic force-feedback engine has its own, smaller attribute set: `range`, `gain`, `autocenter`, `combine_pedals` and `ffb_output`. It also has rev-LED devices under `/sys/class/leds/`. See [G923 Attributes](#g923-attributes-playstation-edition-c266c267) below.

---

## Overview

The driver puts the wheel settings in sysfs attributes under this directory:
```
/sys/bus/hid/devices/<DEVICE_ID>/
```

`<DEVICE_ID>` is typically `0003:046D:C276.XXXX` (RS50), `0003:046D:C272.XXXX` (G Pro Xbox/PC), or `0003:046D:C268.XXXX` (G Pro PS/PC).

The driver talks to the wheel with HID++, Logitech's configuration protocol over USB. Each HID++ setting lives in a feature, named by a hex id such as `0x8137`. A feature has numbered functions, written `fn` in this document.

To find your device path, run one of these commands:
```bash
find /sys/bus/hid/devices -name "*046D*C276*" 2>/dev/null   # RS50
find /sys/bus/hid/devices -name "*046D*C27[28]*" 2>/dev/null # G Pro (c272 or c268)
```

---

## Mode and Profile Control

### wheel_mode
**Access**: Read/Write
**Values**: `desktop`, `onboard`

Selects the operating mode of the wheel base.

- **Desktop mode** (profile 0): The host software controls the settings. Sensitivity is available.
- **Onboard mode** (profiles 1-5): The wheel stores the settings in its profiles. Brake force is available.

```bash
# Read current mode
cat wheel_mode
# Output: "desktop" or "onboard"

# Switch to onboard mode
echo "onboard" > wheel_mode

# Switch to desktop mode
echo "desktop" > wheel_mode
```

### wheel_profile
**Access**: Read/Write
**Values**: `0` (desktop), `1-5` (onboard profiles)

Selects the active profile. Profile 0 is desktop mode. Profiles 1-5 are onboard profiles.

```bash
# Read current profile
cat wheel_profile
# Output: 0-5

# Switch to onboard profile 3
echo 3 > wheel_profile

# Switch to desktop mode (profile 0)
echo 0 > wheel_profile
```

### Editing an onboard slot

There is no separate "write a whole profile" attribute or protocol. To edit a slot:

1. Make the slot active: write its number to `wheel_profile`.
2. Write the per-setting attributes that the rest of this page uses: `wheel_range`, `wheel_strength`, `wheel_trueforce`, `wheel_damping`, `wheel_ffb_filter`/`wheel_ffb_filter_auto`, `wheel_brake_force`, `wheel_led_effect`/`wheel_led_brightness`.
3. Write the slot's own name with `wheel_profile_names`.

The wheel saves each write to the active slot's own storage **immediately**. This is the same as every other write on this page. There is no commit or save step afterwards. See the "Onboard Profile Authoring" section of `docs/PROTOCOL_SPECIFICATION.md` for the wire-level evidence.

```bash
# Author slot 3: activate it, then set its values one at a time.
echo 3 > wheel_profile
echo 540 > wheel_range
echo 80 > wheel_strength
echo "3:RACE NITE" > wheel_profile_names
```

Two things follow from "immediate write, no commit step":

- **Activating a slot changes the wheel's live feel right away.** As soon
  as `wheel_profile` lands on a slot, the motor runs the strength, damping
  and range that the slot already has stored. This happens before you write
  anything else. Never do this while driving.
- **A half-finished edit cannot be rolled back on the wire.** If a script
  or tool stops partway through a slot edit, everything written so far
  stays written. There is nothing to "cancel". A caller that wants an undo
  path must read every value it is about to touch *before* it writes
  anything. It then has something to write back if it needs to revert.
  The "Edit onboard slot" flow in the `logi-wheel` GUI and TUI does exactly
  this: it takes a snapshot on entry, and an explicit Revert action replays
  it. See `logi_wheel_core::onboard` in the userspace crate for the
  reusable implementation.

One more thing matters when you script this. The wheel can switch to a
different slot from its own OLED menu at any time. (The OLED is the small
display on the wheel base.) This can happen in the middle of a
multi-attribute edit like the one above. Re-read `wheel_profile` before
you trust that a write landed on the slot you intended. Do not assume
nothing changed underneath you.

---

## Force Feedback Settings

### wheel_profile_names
**Access**: Read/Write

The names of the five onboard slots, read live from the wheel. Use this
to find the slot number to write to `wheel_profile`.

```bash
cat wheel_profile_names
# 1: RACE
# 2: DRIFT
# 3: PROFILE 3
# 4: PROFILE 4
# 5: PROFILE 5
```

A write renames one slot. The format is `<slot>:<name>` (feature `0x8137`
fn4). The wheel saves the name to its own NVM (non-volatile memory, the
wheel's permanent storage) on the write. There is no separate save step.

```bash
# Rename slot 3
echo "3:RACE" > wheel_profile_names
```

The slot is 1-5. On an RS50 the name may be up to **9 characters**, the
same length as its stock `PROFILE 3`. It may contain spaces. The wheel
stores it **uppercased**: `echo "3:Race"` reads back as `3: RACE`. The
wheel itself refuses a longer name, and the write returns `-EIO`. A bad
slot, an empty name, or a name over the 14-byte HID++ payload gives
`-EINVAL`.

### wheel_range
**Access**: Read/Write
**Values**: `90` to `2700` (degrees)

Sets the steering wheel rotation range.

```bash
# Read current range
cat wheel_range
# Output: degrees (e.g., "900")

# Set to 540 degrees
echo 540 > wheel_range
```

On the G923 Xbox edition under `g923_xbox_dd_engine=1`, the driver reads
and writes the range through the wheel's classic force-feedback feature
(0x8123). The default path uses the same feature. The range is clamped to
`180` to `900`, because it is a gear-driven wheel. The attribute reports
what the wheel answered at probe, not a direct-drive default. (Probe is the
moment the driver first binds to the wheel.)

This range governs the ordinary DirectInput/evdev axis and the soft
endstop. (DirectInput is the Windows game input API. Evdev is the Linux
input event interface.) It does **not** limit the axis that Logitech's
TrueForce SDK sees over raw HID. (TrueForce is Logitech's audio-driven
haptic channel. The SDK is the software kit games use to drive it. Raw
HID, or hidraw, is direct device access that bypasses the input layer.)
That path always presents the wheel's full mechanical range, 2700 on the
RS50, whatever this attribute is set to. So in a game that runs with TrueForce (raw HID
on), set the game's steer lock or rotation to the wheel's maximum. Then
let the game apply each car's own ratio. That is the usual direct-drive
setup. Matching this value in the game is only correct on the
non-TrueForce path.

On the G PRO PID (`046d:c272` / `046d:c268`), the standard HID++ range
feature is not advertised at the index the native code expects. This
applies both to a real G PRO and to an RS50 in compat mode. (Compat mode
is when the RS50 enumerates with the G PRO's product id.) The driver falls
back to feature `0x8138` (index 0x18, fn 2), captured from G Hub. (G Hub
is Logitech's Windows configuration app.) See
`docs/PROTOCOL_SPECIFICATION.md` section 5.1. The fallback write only
takes effect while the wheel is in desktop mode. The wheel boots in
onboard mode. So write `0` to `wheel_profile` first to enter desktop mode.
Range writes after that take effect on the motor.

**External-change detection**: some game launches reset the wheel's
physical range to 90 degrees without any HID++ notification. This was
observed with Assetto Corsa EVO under Proton. (Proton is Valve's
Wine-based layer that runs Windows games on Linux.) The driver re-reads
the true range from the wheel every 20 seconds. It pauses this while force
feedback is actively playing, so the synchronous query can never stall the
force stream. It catches up within one interval after the effects stop.
If the range changed externally, the driver does three things:

1. It updates the reported `wheel_range` value to the real one.
2. It logs the change in dmesg, the kernel log (`rotation range changed externally`).
3. It wakes any program that waits on the attribute with `poll()`, via `sysfs_notify()`.

When the external value is exactly 90, the driver also restores the
previous range automatically. The value 90 is the known SDK session-init
problem; see `wheel_range_restore` below.

### wheel_range_restore
**Access**: Read/Write
**Values**: `0` or `1`
**Default**: `1`

Automatic recovery from the launch-time 90-degree reset. The root cause
is verified with usbmon, the kernel's USB traffic capture. Some games'
SDK sessions push an operating range of 90 degrees once at session start.
They send it as a TrueForce interface-2 packet, which HID++ cannot see.
With this attribute enabled, the driver restores the pre-reset range
automatically. This is verified end-to-end against a faithful
reproduction of the game traffic. Detection to restore takes under 100 ms
once the poll samples the change.

Safety gates, each one earned from a real incident:
- The restore fires only for an EXTERNAL silent change that lands
  exactly on 90. The driver respects any other externally-set value as
  legitimate intent, for example a game applying its configured steering
  lock (540, 850, ...).
- It works in desktop mode only. It never switches the mode automatically.
- The wheel must be stationary (two encoder reads 50 ms apart). The
  encoder is the sensor that measures the wheel's position. A restore
  only ever widens the range, which cannot snap the wheel.
- The driver does at most 3 restores per session. Then it logs
  `an external writer keeps changing the rotation range` and yields.
- An explicit `wheel_range` write supersedes any pending restore and
  resets the strike counter.

`0` = detect-and-report only. The driver still logs the change, and
`wheel_range` stays honest. Recovery is manual. To recover by hand:

1. Wait until FFB (force feedback) is idle.
2. Write `0` to `wheel_profile`.
3. Write the degrees you want to `wheel_range`.

The same opt-out also governs the push-triggered one-shot restore. The
driver performs that restore when the interceptor sees the SDK's
type-`0x0e` range push directly (`docs/TRUEFORCE_PROTOCOL.md`). So `0`
means detect-only for both mechanisms, not just the poll-based one.

Both mechanisms act only on a push of exactly 90 degrees. That is the
value the SDK sends when nothing answered its rotation question. Any other
pushed value is a game applying its own steering lock through an SDK that
was answered (the launcher's proxy does that). The driver leaves such a
value in place. The game's setting then moves the wheel as it does on
Windows.

### wheel_strength
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the force feedback strength.

**Internal encoding**: The driver converts the percentage to a 16-bit value:
- 0% = 0x0000
- 100% = 0xFFFF (this is the 8.0 Nm maximum torque)

```bash
# Read current strength
cat wheel_strength
# Output: percentage (e.g., "75")

# Set to 50%
echo 50 > wheel_strength
```

### wheel_damping
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the wheel damping, the resistance you feel when you turn the wheel.

**Internal encoding**: The driver scales the 0-100 percentage to a 16-bit big-endian value (`value = percentage * 65535 / 100`). It writes the value to page `0x8133` with SET fn=1. See `docs/PROTOCOL_SPECIFICATION.md` section 5.

```bash
# Read current damping
cat wheel_damping

# Set to 25%
echo 25 > wheel_damping
```

### wheel_trueforce
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the TRUEFORCE bass shaker intensity. This is the strength of the vibration the wheel plays from TrueForce audio.

```bash
# Read current TRUEFORCE level
cat wheel_trueforce

# Enable at 80%
echo 80 > wheel_trueforce

# Disable
echo 0 > wheel_trueforce
```

### wheel_brake_force
**Access**: Read/Write
**Values**: `0` to `100` (percentage)
**Mode Restriction**: **Onboard mode only**

Sets the brake pedal force threshold. This is the sensitivity of the load cell, the pressure sensor in the brake pedal.

**Note**: A write returns `-EPERM` (Permission denied) in desktop mode.

```bash
# Set brake force to 75% (must be in onboard mode)
echo "onboard" > wheel_mode
echo 75 > wheel_brake_force
```

### wheel_sensitivity
**Access**: Read/Write
**Values**: `0` to `100` (percentage)
**Mode Restriction**: Writes only accepted in **desktop mode**; reads always succeed.

Bends the steering response curve through feature `0x80A4`
(AxisResponseCurve). A response curve maps the wheel's real position to
the position it reports. The curve is a 64-point cubic Bezier from (0,0)
to (1,1). (A Bezier is a smooth curve defined by control points.) The
control points are P1=(1-s, s) and P2=(s, 1-s), where s = value/100.
Values below `50` soften the response near centre. Values above `50`
sharpen it. `50` is the identity curve. For `50` the driver reverts to
the wheel's built-in curve, as G Hub does, instead of uploading a flat
line.

This is unrelated to LED brightness. Feature `0x8040`, behind
`wheel_led_brightness`, is brightness only. Sensitivity and brightness
are fully independent.

A read returns the last value written. The default is `50`. The wheel has
no read-back for the slider, so this is a write-through cache. A write in
onboard mode fails with `-EPERM`. If the wheel does not expose the
response-curve feature, a write returns `-EOPNOTSUPP`.

```bash
# Sharpen the centre response (must be in desktop mode)
echo "desktop" > wheel_mode
echo 65 > wheel_sensitivity
```

### wheel_ffb_filter
**Access**: Read/Write
**Values**: `1` to `15` (filter level)

Sets the force feedback smoothing (filter) level. G Hub's labels are roughly Minimum (1), Low (7), Medium (11), Maximum (15).

The driver clamps values outside `1..15` to that range.

```bash
# Read current filter level
cat wheel_ffb_filter

# Set to level 11 (G Hub "Medium")
echo 11 > wheel_ffb_filter
```

### wheel_ffb_filter_auto
**Access**: Read/Write
**Values**: `0` (manual), `1` (auto)

Enables automatic FFB filter adjustment based on the game's output.

The filter flag byte on the wire has two meanings. The driver splits them across two sysfs writes. A write to `wheel_ffb_filter` stamps the "user set this level right now" bit. A write here toggles only the "auto mode" bit. See `docs/PROTOCOL_SPECIFICATION.md` section 5 (FFB Filter) for the bitfield decode.

```bash
# Enable auto filter
echo 1 > wheel_ffb_filter_auto

# Disable (use manual filter setting)
echo 0 > wheel_ffb_filter_auto
```

### wheel_ffb_game_gain

Read-only. The force-feedback strength the running game has asked for,
as a percentage. This is the `FF_GAIN` value that arrives through evdev
when a game's own force-feedback slider moves. It is 100 until a game
lowers it.

This is not a setting, and it is not the wheel's own strength. It is the
game's value. The driver publishes it so that everything the wheel does on
that game's behalf can follow it. `logi-tf-sim` scales its synthesized
haptics by this value. So a title with no TrueForce of its own still obeys
the slider a driver reaches for. `follow_game_gain` in `tf-sim.conf` turns
that off.

```bash
cat "$H/wheel_ffb_game_gain"     # 100 with nothing running
```

### wheel_ffb_constant_sign
**Access**: Read/Write
**Values**: `0` or `1`
**Default**: `1`

Selects whether the driver inverts the sign of every `FF_CONSTANT`
level before it sends the level to the wheel. (`FF_CONSTANT` is the evdev
constant-force effect type.) This single toggle makes the driver's FFB
feel right under both Wine/Proton games and native Linux apps. The two
paths disagree on sign by one flip somewhere in Wine's
DirectInput-to-evdev translation layer. The driver cannot tell them apart
at runtime.

- `1` (default) - invert. Correct for Wine/Proton running Windows
  games (Assetto Corsa Competizione, etc.). If `FF_CONSTANT` feels
  like centring forces push the wheel *away* from centre instead of
  back toward it, the toggle is at the wrong setting.
- `0` - pass-through. Correct for native Linux evdev apps that
  upload effects with the convention in `Documentation/input/ff.rst`
  (direction=0x4000 east, positive level = rightward force).
  `fftest`, `ffcfstress`, and direct EVIOCSFF uploads from custom
  tools are in this category.

This only affects `FF_CONSTANT`. All other effect types (`FF_SPRING`,
`FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA`, `FF_RAMP`, `FF_PERIODIC`,
`FF_RUMBLE`) feel identical at either toggle value.

**Caveat for reverse driving**: the inversion is unconditional. It does
not look at the wheel's velocity or the car's gear. Most modern racing
sims correctly model the self-aligning torque flipping sign at negative
longitudinal velocity. In those sims the chain is: the sim physics
inverts for reverse, then Wine inverts again, then the driver inverts
again. The result is physics-correct destabilising FFB. The user feels
it as the wheel actively pushing away from centre when reversing. That is
the real-car behaviour and not a driver bug. But it can feel violent
compared to a wheel without TF / direct-drive force. Lowering
`wheel_strength` is the only knob on the driver side. Switching the sign
toggle off would make forward driving feel wrong without fixing the
reverse case.

A contributor cross-checked this on Windows (issue #8, AC EVO and AC).
The FFB "gets pretty violent in reverse" there too, with the same wheel
and game settings. So the strong reverse force is the sim's physics
surfacing through the wheel. It is identical to the Windows G Hub path.
It is not a sign error or a double inversion on our side.

```bash
# Playing Wine/Proton racing games: leave default
cat wheel_ffb_constant_sign    # -> 1

# Running native-evdev tools like fftest:
echo 0 | sudo tee wheel_ffb_constant_sign
```

The inversion is confirmed by testing on Assetto Corsa Competizione with
this wheel. A test harness in `tests/ff_matrix_test.c` cross-checks each
toggle value against native evdev expectations.

### wheel_spring_damping
**Access**: Read/Write
**Values**: `0` to `100` (percentage)
**Default**: `25`
**Availability**: all direct-drive wheels (RS50 native/compat and real G PRO - every family PID runs the same `hidpp_dd_ff_*` FFB path).

Synthetic damping that the driver applies to emulated `FF_SPRING`
effects. It is a percentage of a `FF_DAMPER` that runs at the spring's
own coefficient.

The driver emulates condition effects on the host side. It samples the
wheel position and pushes the computed force back over USB. That loop
has latency. On a low-friction direct-drive motor, the latency makes a
stiff, undamped spring from a game ring. The ring is a growing
back-and-forth oscillation. It ends when the wheel's over-torque
failsafe cuts power. This was observed live with Assetto Corsa EVO's
map-load centring spring. Real wheels damp the spring inside the
firmware servo loop. This knob restores that behaviour. `0` disables it.
The damping scales with the spring's own coefficient, so stiff springs
get proportionally stronger damping.

```bash
cat wheel_spring_damping     # -> 25
echo 40 > wheel_spring_damping
```

### wheel_texture_route
**Access**: Read/Write
**Values**: `tf` or `kf` (also accepts `1` / `0`)
**Default**: `tf`
**Availability**: all direct-drive wheels (RS50 native/compat and real G PRO - every family PID runs the same `hidpp_dd_ff_*` FFB path).

Selects where the driver plays vibration-class effects. These are
`FF_RUMBLE` and periodic effects at 20 Hz or faster (period <= 50 ms).

- `tf` (default) - the driver streams them on the wheel's TrueForce
  audio-haptic channel (interface 2, ~1 kHz sample rate). This is the
  same physical path the Windows SDK uses for texture. Steering
  effects (`FF_CONSTANT`, conditions, slow periodics) stay on the
  force channel. So rumble does not modulate the steering axis.
  This matches the Windows KF/TF split. The "gritty/notchy steering
  under rumble" reported in issue #8 is the `kf` behaviour.
- `kf` - legacy. The driver sums everything into the single steering
  force. It is kept as a fallback and for A/B comparison.

The driver brings up the TrueForce session lazily. The first time a
texture-class effect actually plays, the driver replays the captured
68-packet init sequence twice. This is G Hub behaviour. It then streams
unified packets at 1 kHz while texture effects are active. Each packet
carries the steering-force sum in its preamble and four texture-audio
window slots (2 kHz slot rate). This is the same layout AC EVO streams.
The kernel log shows `TrueForce texture channel ready`. A wheel that
never sees texture effects never sees TF traffic. If the init fails,
texture effects fall back to the steering channel. The feel is degraded,
never lost. The driver retries on a later texture playback, up to 3
attempts per session, and logs each attempt in dmesg.

The driver decides an effect's channel when its playback starts. It holds
that channel for the whole play cycle. So re-parametrising a playing
effect across the 20 Hz crossover never yanks a live effect between
channels. The same is true when the session init completes mid-play.
Playbacks that start before the session is ready ride the steering
channel for their duration. The next playback moves to the TF stream.

Texture amplitude respects `FF_GAIN` and `wheel_strength`. The wheel
firmware scales steering forces by the strength setting itself, but it
plays TF samples at face value. So the driver applies strength to texture
in software for consistency. The amplitude is also capped at half of
full scale. Above roughly 0.5-0.7 FS the wheel's DSP (its signal
processor) crosses from vibration into pulling the steering axis. The
cap keeps a synthetic full-scale rumble from hijacking steering torque.
Real games stream texture far below the cap.

Note for SDK games (ACC, AC EVO with the TrueForce shim): those games
stream TrueForce themselves via hidraw. They normally do not send evdev
rumble at the same time, so the two streams do not meet. If a game
somehow does both, set `kf`. That keeps the wheel's TF input to a single
writer.

```bash
cat wheel_texture_route      # -> tf
echo kf > wheel_texture_route   # A/B back to the legacy mixing
```

### wheel_tf_merge
**Access**: Read/Write
**Values**: `0` or `1`
**Default**: `0`
**Availability**: all direct-drive wheels (RS50 native/compat and real G PRO - every family PID runs the same `hidpp_dd_ff_*` FFB path). Backed by the native texture-merge interceptor on interface 2. If the interceptor is not installed (interface 2 not yet bound, or already torn down), reads give `0` and writes fail with `-ENODEV`.

Master switch for the native texture merge. When `1`, the driver splices
engine texture into outgoing SDK stream packets on interface 2 that
carry no audio samples. The texture is synthesised from
`wheel_texture_rpm` and fitted to the Windows capture in
`docs/TF_TEXTURE_RECIPE.md`. The driver never modifies the base force
bytes. With no fresh RPM (200 ms staleness window), or below 300 rpm,
the stream passes through untouched. Turning the switch off also zeroes
the internal sample-debt counter. So re-enabling starts clean instead of
bursting queued samples.

```bash
cat wheel_tf_merge           # -> 0
echo 1 > wheel_tf_merge
```

### wheel_texture_rpm
**Access**: Read/Write
**Values**: store `"<rpm> <max_rpm>"` as plain unsigned decimals, rpm units (e.g. `6500 14000`); each capped at 30000, larger values rejected with `-EINVAL`. Show prints `rpm max_rpm age_ms`.
**Default**: `0 0 0`
**Availability**: same as `wheel_tf_merge`, including the NULL-shim fallback (reads back `0 0 0`, writes fail with `-ENODEV`).

Live engine RPM feed for the texture merge. Normally logi-rpm-bridge
writes it at roughly 60 Hz from the game's telemetry relay. You can also
write it by hand for bench tests. `age_ms` on read is how long ago the
value was last written. `wheel_tf_merge` checks this age against its
200 ms staleness window.

logi-rpm-bridge also drives `wheel_rev_level` from the same telemetry.
The default mapping is a full rev bar: LED 1 lights as soon as rpm > 0,
and all 10 are lit at the limiter. This works for legacy 28-byte relay
senders too. `LOGI_REV_MODE=shift` selects the dash band instead: dark
below the first-shift-light rpm, level 1 exactly there, 10 at the
limiter. The dash band needs the 32-byte relay form that carries the
first-shift-light rpm. `LOGI_REV_SYSFS` overrides the LED target
attribute. `LOGI_RPM_PORT` overrides the UDP port. The strip darkens on
telemetry loss (1 s) and on bridge exit.

```bash
cat wheel_texture_rpm        # -> 6500 14000 12
echo "6500 14000" > wheel_texture_rpm
```

### wheel_texture_intensity
**Access**: Read/Write
**Values**: `0` to `200` (percent)
**Default**: `100`
**Availability**: same as `wheel_tf_merge`, including the NULL-shim fallback (reads back `0`, writes fail with `-ENODEV`).

Texture amplitude as a percentage of the capture fit. `0` silences
the texture. `200` doubles it.

```bash
cat wheel_texture_intensity  # -> 100
echo 150 > wheel_texture_intensity
```

### wheel_texture_cylinders
**Access**: Read/Write
**Values**: `1` to `16`
**Default**: `4`
**Availability**: same as `wheel_tf_merge`, including the NULL-shim fallback (reads back `0`, writes fail with `-ENODEV`).

Cylinder count for the firing-frequency model that drives the texture
synthesis (`f0 = rpm/60 * cylinders/2`). Treat it as a feel knob, not a
realism setting. Rim excursion falls as 1/f^2. So higher counts push the
firing frequency, across the rev range, above what a direct-drive rim can
physically express. The texture then fades out of reach.

```bash
cat wheel_texture_cylinders  # -> 4
echo 6 > wheel_texture_cylinders
```

### wheel_calibrate
**Access**: Write-only (mode 0220)
**Values**: `0` to `65535` (raw encoder position)
**Availability**: RS50 and G Pro. Returns `-EOPNOTSUPP` if the wheel does not expose page `0x812C` on sub-device `0x05` (no known variant lacks it, but the driver does not assume).

Low-level primitive. It writes the given raw 16-bit encoder value, which
the wheel adopts as the new centre. The driver sends
`10 05 <idx> 3D <hi> <lo> 00` to HID++ sub-device `0x05`, feature page
`0x812C`. (A sub-device is a HID++ address for one part of the wheel,
such as the motor unit.) See `docs/PROTOCOL_SPECIFICATION.md` section 5
for the wire format. Verified on RS50 from
`2026-04-22_re_calibrate.pcapng`.

Use this only if you already have the raw encoder value you want as the
centre. For example, you read it via a HID++ GET yourself, or you want
to seed a specific reference value. For the common case, "make the
wheel's current physical position the new centre", use
`wheel_calibrate_here` below. It does the GET+SET internally.

```bash
# Low-level: you already have the raw encoder number you want.
echo 32768 > wheel_calibrate
```

### wheel_calibrate_here
**Access**: Write-only (mode 0220)
**Values**: any non-empty write triggers the operation
**Availability**: same as `wheel_calibrate`

One-shot "use the wheel's current physical position as the new centre".
The driver issues fn=1 GET to read the current raw encoder value. It
then issues fn=3 SET with that value. This mirrors what G Hub does when
the user clicks Calibrate on Windows. To use it:

1. Hold the wheel at the centre you want (typically true centre).
2. Write to this attribute.

```bash
# Hold the wheel at true physical centre, then:
echo 1 > wheel_calibrate_here
```

The driver stores no state. The wheel's firmware keeps the new centre
across power cycles, the same as G Hub on Windows.

---

## LIGHTSYNC LED Control

The RS50 has 10 RGB LEDs in a horizontal strip across the upper faceplate. (RGB means each LED mixes red, green and blue, so it can show any colour.) The strip is an engine-RPM / shift indicator. The driver provides per-slot configuration with 5 custom slots (0-4).

> **Per-model availability**: the `wheel_led_*` LIGHTSYNC attributes exist on the RS50 in both native and G-PRO-compat enumeration. The RS50's faceplate LED-strip hardware does not change with the PID (the USB product id). Verified live 2026-04-29. On a **real G PRO Racing Wheel** they are hidden. That rim has level-based rev lights with no per-LED RGB. The driver exposes them as `wheel_rev_level` instead (see its entry below).
>
> **G PRO PID (`046d:c272` / `046d:c268`)**: covers both the real G PRO Racing Wheel and the RS50 in G PRO compat mode. Both run through the same `hidpp_dd_ff_*` code path and expose the same wheel-config attributes. The LED attributes differ per rim (see the per-model note above). The driver tells the two apart by the USB product string. On the RS50 in compat mode, LIGHTSYNC works the same way as native. Feature `0x807A` is advertised at the same index that discovery picks up in native, and `wheel_led_*` writes drive the LED strip end-to-end (verified against the live wheel 2026-04-29). These wheel-config attributes work via fallback feature paths (see `docs/PROTOCOL_SPECIFICATION.md` section 5.1): `wheel_range`, `wheel_strength`, `wheel_trueforce`, `wheel_damping`, `wheel_ffb_filter`, `wheel_profile` (write `0` to enter desktop mode), and `wheel_calibrate`. This firmware does not support the remaining attributes (`wheel_brake_force`, `wheel_ffb_filter_auto`, `wheel_sensitivity`). Once their mode gating is satisfied, the store returns `-EOPNOTSUPP`. Note that `wheel_brake_force` still returns `-EPERM` in desktop mode, and `wheel_sensitivity` in onboard mode, before that check. For those settings, use the wheel's OLED menu or G Hub on a Windows host.

### LED Control Workflow

1. **Select a slot**: `echo 2 > wheel_led_slot`
2. **Set direction** (optional): `echo 1 > wheel_led_direction`
3. **Set colors**: `echo "FF0000 FF0000 ... (10 colors)" > wheel_led_colors`
4. **Apply changes**: `echo 1 > wheel_led_apply`

Or use the built-in effects through `wheel_led_effect`.

### wheel_led_slot
**Access**: Read/Write
**Values**: `0` to `4` (custom slot index)

Selects the active custom LED slot and renders it on the strip. On the
wire, a custom slot IS effect value `5 + slot` (see `wheel_led_effect`).
So a write to this attribute equals a write of effect `5 + slot`. The
driver applies the slot's stored config and switches the strip to it.
This is hardware-verified in both directions, 2026-07-20. The
slot-scoped attributes (`wheel_led_slot_name`, `wheel_led_colors`,
`wheel_led_direction`, `wheel_led_slot_brightness`) target the slot
selected here.

```bash
# Select slot 2 (CUSTOM 3) and render it
echo 2 > wheel_led_slot
```

### wheel_led_slot_name
**Access**: Read/Write
**Values**: String (max 8 characters)

Gets or sets the name of the currently selected LED slot. The device stores the names.

```bash
# Read current slot name
cat wheel_led_slot_name
# Output: "CUSTOM 1" (or user-defined name)

# Set a custom name for the slot
echo "Racing" > wheel_led_slot_name
```

### wheel_led_slot_brightness
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Gets or sets the brightness for the currently selected slot. Each slot can have its own brightness level. The wheel applies it when you activate the slot with `wheel_led_apply`.

```bash
# Read current slot brightness
cat wheel_led_slot_brightness
# Output: brightness percentage (e.g., "75")

# Set slot brightness to 50%
echo 50 > wheel_led_slot_brightness
```

**Note**: This is per-slot brightness. Use `wheel_led_brightness` to set the global brightness without changing slot settings.

### wheel_led_direction
**Access**: Read/Write
**Values**: `0` to `3`

Sets the LED animation direction for the current slot:
- `0` = Left to Right
- `1` = Right to Left
- `2` = Inside Out (center outward)
- `3` = Outside In (edges inward)

On the wire, the `0x807B` config carries a 1-4 value. The driver
translates between this 0-3 enum and the device's 1-4 encoding
internally (see PROTOCOL_SPECIFICATION.md section 9.4.1).

```bash
# Set direction to Right-to-Left
echo 1 > wheel_led_direction
```

### wheel_led_colors
**Access**: Read/Write
**Format**: 10 space-separated hex RGB values (`RRGGBB`)

Sets all 10 LED colors for the current slot. LED1 is the leftmost LED.

```bash
# Set all LEDs to red
echo "FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000" > wheel_led_colors

# Rainbow effect (example)
echo "FF0000 FF7F00 FFFF00 7FFF00 00FF00 00FF7F 00FFFF 007FFF 0000FF 7F00FF" > wheel_led_colors

# Read current colors
cat wheel_led_colors
# Output: "RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB"
```

### wheel_led_brightness
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the global LED brightness.

```bash
# Set brightness to 50%
echo 50 > wheel_led_brightness
```

The driver tracks brightness changes made from the wheel itself (OLED
menu). It consumes the official x8040 `brightnessChangeEvent` broadcast,
updates this attribute's value, and wakes `poll()` waiters on
`wheel_led_brightness`. It does not touch `wheel_sensitivity`. The two
were once thought to alias in desktop mode. That was disproved on
hardware. The driver's own handler carries a note that the read here
must never touch sensitivity. The two are fully independent, as stated
above.

### wheel_led_effect
**Access**: Read/Write
**Values**: `1` to `9`

Selects the LED animation mode. The values match feature 0x807A fn3 on
the wire:

| Value | Effect |
|---|---|
| `1` | Inside to out |
| `2` | Outside to in |
| `3` | Left to right |
| `4` | Right to left |
| `5`-`9` | The five custom slots: `5` = CUSTOM 1 .. `9` = CUSTOM 5 |

Values `5`-`9` ARE the custom slots (decoded 2026-07-20). There is no
separate slot-activate command. Selecting a slot is selecting its
effect number. A write of `5` renders the ACTIVE slot. The driver
re-applies the stored RGB of `wheel_led_slot` and sends `5 + slot` on
the wire. So through this attribute, `5` always means "custom mode with
the currently selected slot". A write of `6`-`9` selects that specific
slot's image directly, without moving `wheel_led_slot`. Prefer
`wheel_led_slot` (or `5`), so the slot-scoped attributes keep targeting
the rendered slot. A write of an animated mode (1-4) just switches the
effect and leaves the cached colors untouched. The driver clamps a write
outside `1..9` to the nearest end of the range.

The driver tracks effect changes made from outside the driver, by G
Hub-style tools or by the wheel itself. It consumes the LIGHTSYNC
effect-change broadcast, updates this attribute, and wakes `poll()`
waiters.

```bash
# Render the active custom slot
echo 5 > wheel_led_effect

# Render CUSTOM 3 (slot 2) directly
echo 7 > wheel_led_effect

# Animate left to right
echo 3 > wheel_led_effect
```

### wheel_oled
**Access**: Read/Write
**Availability**: wheels with feature `0x8130` (the RS50 and G PRO base OLED); reads `unsupported` elsewhere

Puts a frame on the base's Dynamic OLED, or hands the screen back. Write
`<layout>|<field>|<field>...`. The layout is a letter `A` to `J` (or `0`
to `9`). The fields follow in **payload order**. For layouts D and E,
payload order is not the order the screen draws them. `wheel_oled_layouts`
lists what each layout takes. The driver pads text fields with spaces to
their width. A text field must not exceed its width (`EMSGSIZE`). Number
fields take `0` to `255`. More fields than the layout has is `E2BIG`.
Fewer fields leaves the rest blank. Write `off` to hand the screen back to
the wheel's own menu immediately.

| layout | fields, in write order | draws |
|---|---|---|
| A | none | black frame |
| B | none | the firmware's own test composition |
| C | fill | a gauge |
| D | fill, mark, text(11) | gauge with a label |
| E | fill, mark, text(3), text(7) | gauge; the 3-char draws right, the 7-char left |
| F | text(1), text(3) | medium gear, very large value |
| G | text(1), text(3) | very large gear, medium value |
| H | text(21), text(10) | small row over a larger row |
| I | text(19), text(10), text(19), text(10) | four rows, 2 and 4 larger, right-aligned |
| J | same as I | four rows, every row centred |

The panel does not hold a frame. The firmware returns to its own menu
after under two seconds of silence. So the driver resends the current
frame every 50 ms for as long as one is up. A read gives the last write,
or `off`. Two renderer rules are worth knowing. Both were watched on an
RS50. First, a value on a wide large row (H's lower row, I's large rows)
draws its first character pinned left and the rest right-aligned. So
`112%` shows as `1` and `12%` with a gap. A leading space gives a clean
right-aligned value instead. Second, no large-font layout centres. So
J's rows are the largest centred text the panel has. The full protocol
is in `PROTOCOL_SPECIFICATION.md` 12.3.

```bash
echo 'G|3|142' > wheel_oled          # gear and speed
echo 'J|Lap 12|1:42.7|Best|1:41.9' > wheel_oled
echo off > wheel_oled
```

These are non-force writes to the HID++ endpoint during play. They are
the same class as the rev lights, which have run alongside force and
texture on these wheels since 0.35.0. On a title that pushes DirectInput
force to HID++ next to its own TrueForce stream, they can cut that force
(12.5). The rev lights carry the identical caveat.

### wheel_oled_layouts
**Access**: Read-only
**Availability**: as `wheel_oled`

One line per layout the wheel reports. Each line has the index, the
letter, the field kinds in write order, and then the text widths. The
widths are listed as the wheel's own descriptor lists them, which is
drawn order. E is the one layout where drawn order differs from write
order. Its 3-character field is written first and drawn on the right. On
an RS50:

```
0 A
1 B
2 C num
3 D num num text 11
4 E num num text text 7,3
5 F text text 1,3
6 G text text 1,3
7 H text text 21,10
8 I text text text text 19,10,19,10
9 J text text text text 19,10,19,10
```

### wheel_response_curve
**Access**: Read/Write
**Availability**: all direct-drive wheels (feature `0x80A4`)

The steering axis's 64-point response curve. This is the store behind
G Hub's **Sensitivity** slider. Write `reset` to revert to the built-in
(linear) curve. Or write 2-64 whitespace-separated `in:out` pairs. The
values are decimal 0-65535. `in` must strictly increase. `out` must not
decrease. The pairs must start at `0:0` and end at `65535:65535`. If you
write fewer than 64 pairs, the driver resamples them by linear
interpolation to the 64 points the wheel stores.

```bash
# Soften the centre (slower response near straight-ahead)
echo "0:0 32768:16384 65535:65535" > wheel_response_curve

# Back to the built-in curve
echo reset > wheel_response_curve

cat wheel_response_curve
# 64/64 points loaded (0 = built-in curve)
```

The wheel applies this curve to the steering axis it reports to the PC.
That is base `0x80A4` axis 0, the same axis `wheel_sensitivity` uploads
its slider curve to. `wheel_sensitivity` and `wheel_response_curve` both
write that one axis-0 curve. So the last one written wins. The axis does
not change until the wheel next moves. The wheel sends no HID reports
while held still. So an upload appears to do nothing until you nudge the
wheel. `cat wheel_response_curve` reads the point count back from the
wheel and is the honest check. Use `reset` if steering feels wrong after
an upload. Whether curves persist across power cycles is untested.

### wheel_rev_level
**Access**: Read/Write
**Values**: `0`-`10` (number of rev LEDs lit)
**Availability**: real G PRO rim AND the RS50's LIGHTSYNC strip
(hardware-verified 2026-07-20, re-verified with the current wire form
2026-08-14). On the RS50 the display renders **center-out by
default**. So a 10-LED strip shows 5 mirrored visual steps across a
0-10 level sweep. The display pattern is a separate concern from the
level protocol. The level write says how much of the bar is lit. The
wheel's stored display config says how that amount is drawn.
Reconfiguring the pattern mid-session (feature `0x807B`) is NOT part
of G HUB's own mid-session vocabulary. It is untested during live
TrueForce sessions, so do not assume it is safe there. A written
level holds until the next write. Write `wheel_led_effect` to restore
the normal idle pattern.

Rev-light level for the G PRO rim. The G PRO's rim lights are
level-based. The host commands how many LEDs are lit (0-10). The
wheel's onboard profile owns colours, direction and scaling. The
protocol was decoded from G HUB captures (see
`docs/PROTOCOL_SPECIFICATION.md` section 9). No arming is needed. From
the onboard power-on state, the strip renders bare levels exactly
(hardware-proven 2026-08-14). The earlier belief that "the first write
arms the feature" is disproven. The driver's direct-drive path sends no
arm burst. Writes return immediately. The driver coalesces them and
flushes only the newest level, at a ~10 ms floor. That floor is
comfortably above G HUB's own measured ~60 Hz rev-light rate. Faster
bursts would starve the wheel's shared HID++ command processor. So a
fast telemetry feeder always shows the latest value with no queueing
lag. The wheel holds a level for a while but reverts eventually. A
telemetry feeder should refresh at ~1 Hz or faster, which is natural
for rev-light use.

Level writes coexist safely with a live native TrueForce SDK session.
This has been true since the driver moved to the bare fn2+fn6 wire
form. Base FFB + TrueForce texture + telemetry-driven levels are
hardware-validated running together (2026-08-14). The old arm-burst
form did NOT coexist. It killed the SDK session at init and killed wheel
input mid-session.

**Status: hardware-validated on the RS50 (2026-07-20, live 0-10-0 sweep
demos; fill semantics per the note above). Not yet validated on a real
G PRO. Reports are welcome (issue tracker).**

```bash
# Light 7 of 10 rev LEDs
echo 7 > wheel_rev_level
```

### wheel_serial
**Access**: Read-only
**Values**: 12-character device serial

The wheel's real serial number. The driver reads it from HID++
DeviceInfo (feature 0x0003 fn2) at init. It matches the USB `iSerial`
descriptor.

### wheel_firmware
**Access**: Read-only

Firmware versions, read from DeviceInfo at init. The first is the wheel
base's active main firmware. The second is the motor unit's servo
firmware, from sub-device 0x05's own DeviceInfo.

```bash
cat wheel_firmware
# base: U1 65.03.B0038
# motor: SC 02.01.B0042
```

Include this output in bug reports. Firmware-dependent behaviour
(feature index drift, settings quirks) is tracked against it.

### wheel_accessory
**Access**: Read-only
**Values**: accessory device name, or `none`

Presence signal for the base's optional accessories: the **RS Shifter &
Handbrake** and the **RS H-Shifter**. They plug into the base's USB-A
ports. The attribute reports the RS Shifter & Handbrake's own HID++
device name, or `RS H-Shifter`, or `none` when neither is attached. With
both attached, it names the RS Shifter & Handbrake. Unlike the
pedal/handbrake attributes below, this never returns an error. It exists
precisely so apps can check for an accessory without inferring presence
from another attribute's error code.

The driver finds both accessories at probe time and re-checks on demand.
A read queues a presence re-check in the background. So an accessory
plugged in or pulled after the module loaded shows up within a few reads.
An RS H-Shifter that arrives this way in desktop mode gets its gate
routing enabled at the same time, as it would have at probe.

```bash
cat wheel_accessory
# RS Shifter & Handbrake
```

The driver exposes all three of the accessory's G HUB settings:

| G HUB slider | sysfs attribute |
|---|---|
| Shift Sensitivity | `wheel_shift_actuation` |
| Handbrake Actuation | `wheel_handbrake_actuation` |
| Handbrake Sensitivity | `wheel_handbrake_sensitivity` (+ `wheel_handbrake_curve`) |

The first two are trigger POINTS on the lever's travel. They are named
`_actuation` for that reason, even though G HUB calls the shifter one
"sensitivity". In this driver, a `*_sensitivity` attribute means a
generator that applies a response curve to an axis. These two are not
that.

### wheel_led_apply
**Access**: Write-only
**Values**: `1` (apply)

Applies the current slot configuration to the device.

```bash
# Apply current slot settings
echo 1 > wheel_led_apply
```

---


### wheel_reset
**Access**: Write-only (`1`)

Re-enumerates the wheel over USB from inside the driver. This is the
software equivalent of unplugging and replugging it. Logitech's TrueForce
SDK can leave the wheel's engine latched after a session that ended
without its teardown, for example a hard-killed or crashed game. The next
SDK session then opens but never streams. The wheel's steering and force
go dead on track. A power cycle clears it. So does this attribute.
`logi-launch` writes it before a TrueForce session, so the ritual is
unnecessary (`LOGI_TF_RESET=0` opts out).

```bash
echo 1 > wheel_reset   # the wheel drops off the bus and comes back in ~2 s
```

The driver queues the reset to a workqueue, a kernel background job
queue. So the write returns immediately, and the driver re-probes
cleanly. The hid ids change across the reset. USB wheels only.

## Pedal Configuration

A pedal curve is a 64-point `0x80A4` response curve. It is the same
mechanism as the steering `wheel_response_curve`. The driver applies it on
the **wheel base** at axes 1/2/3 (throttle/brake/clutch).

It is not applied on the pedal unit. The pedal MCU (the pedal unit's own
microcontroller) has its own `0x80A4` store. It accepts an upload and
reports it back as loaded. But it never applies the curve to the axis it
reports to the PC. Anything written there is silently inert. This was
proven on an RS50 (2026-07-28) with a step curve. A step curve makes the
band between its two output plateaus unreachable if it is applied. Loaded
on the pedal MCU, the axis swept straight through that band. The same
curve on base axis 1 pinned the axis to the plateau exactly, with the
pedals untouched.

These attributes still require a pedal unit to be present. The base has
axes 1/2/3 regardless. But a curve on an axis that nothing drives is
pointless. So they return `EOPNOTSUPP` when no pedal unit answered
discovery.

Each pedal `<p>` in {`throttle`, `brake`, `clutch`} exposes three
attributes. **They all write the single curve the axis holds, so the last
write wins.** The `_curve` attribute always reads back the wheel's true
loaded-point count.

### wheel_&lt;p&gt;_curve
**Access**: Read/Write

The raw 64-point curve. The format is identical to `wheel_response_curve`.
Write `reset` for the built-in linear curve. Or write 2-64
whitespace-separated `in:out` pairs (0-65535, strictly increasing `in`,
non-decreasing `out`, starting `0:0` and ending `65535:65535`). The driver
resamples fewer than 64 pairs. A read gives
`"<loaded>/<max> points loaded"`.

```bash
# Dead-until-30%, then linear, on the throttle
echo "0:0 19660:0 65535:65535" > wheel_throttle_curve
echo reset > wheel_throttle_curve
cat wheel_throttle_curve      # e.g. "64/64 points loaded (0 = built-in curve)"
```

### wheel_&lt;p&gt;_sensitivity
**Access**: Read/Write, **Values**: `0`-`100` (`50` = linear)

G HUB's simple sensitivity slider. Values above 50 make the pedal more
responsive early in its travel. Values below 50 make it less responsive.
`50` reverts to the built-in linear curve. The driver generates the same
symmetric-Bezier curve G HUB uploads. The wheel stores only the resulting
curve. So this reads back the last written slider value, not a device
query.

```bash
echo 70 > wheel_brake_sensitivity
```

### wheel_&lt;p&gt;_deadzone
**Access**: Read/Write, **Values**: `"<lower> <upper>"` percent

Dead travel at each end. The output holds 0 until the pedal passes
`lower`%. It reaches full by `(100 - upper)`%. `lower + upper` must be at
most 99. `"0 0"` reverts to the built-in curve. A read gives the last
written pair.

```bash
# 8% dead at the bottom, 5% saturation at the top of the clutch
echo "8 5" > wheel_clutch_deadzone
```

> Sensitivity, deadzone and the raw curve all write the one hardware curve
> per axis. So use one of them at a time per pedal. To combine a deadzone
> with a custom curve, write the whole thing as a single `_curve` upload.
> This is what the logi-wheel editor does.

### wheel_combined_pedals
**Access**: Read/Write, **Values**: `0` (separate) / `1` (combined)
**Mode**: desktop only

G HUB's "combined pedals" toggle (feature `0x80D0`). When on, the wheel
merges the throttle and brake into a single centred axis. This is for
legacy games that expect one pedal axis. Released = centre. One pedal
drives the axis up, the other down. The brake's own axis goes silent. Keep
it off for any modern sim. Verified on an RS50: with it on, `ABS_RX`
re-centres to ~32768, and `ABS_RY` (the separate brake axis) stops
reporting.

```bash
echo 1 > wheel_combined_pedals   # merge (legacy games)
echo 0 > wheel_combined_pedals   # separate (default)
cat wheel_combined_pedals        # 0 or 1
```

### wheel_accessory_mode
**Access**: Read-only
**Values**: `shifter`, `digital-handbrake`, `analog-handbrake`

Which of its three jobs the **RS Shifter & Handbrake** is currently doing.
The driver reads it live from feature `0x1B30` on the accessory. The unit
has a physical three-position switch. Only one mode is active at a time.
So most of its settings apply to only one of them. A read returns
`EOPNOTSUPP` when no accessory is attached.

```bash
cat wheel_accessory_mode
# analog-handbrake
```

The desktop and terminal apps use this to grey out the settings that belong
to a different mode. They also say which mode each setting needs. The
greying is presentation only. The attributes below stay writable in every
mode. Their values persist across a mode change. So setting one up before
you flip the switch is a reasonable thing to do.

An unrecognised value reads as `unknown (N)`. The apps treat that as
"cannot tell" and hide nothing.

### wheel_shift_actuation / wheel_handbrake_actuation
**Access**: Read/Write
**Values**: `1`-`100` (G HUB's own range; it refuses `0`)

The two trigger points of the **RS Shifter & Handbrake**. They live on
feature `0x80B1` (BANDED_AXIS) on the accessory itself. Both return
`EOPNOTSUPP` when no accessory is attached.

- `wheel_shift_actuation` - how far you must push the sequential shifter
  before a shift registers. A write sets both of the shifter's bands, one
  per direction, symmetric about centre. G HUB calls this "Shift
  Sensitivity". It is a trigger point, not a response curve, hence the
  name here.
- `wheel_handbrake_actuation` - how far you must pull the handbrake before
  the digital handbrake button (`BTN_THUMB2`) fires. It applies in
  digital-handbrake mode. The analog mode uses the handbrake curve
  instead.

```bash
echo 30 > wheel_shift_actuation        # shift triggers early
echo 70 > wheel_handbrake_actuation    # handbrake fires late in the pull
```

Neither is mode-gated. G HUB's UI will not let you change these in onboard
mode. But the wire accepts them in either mode (tested).

**A deliberate difference from G HUB.** The handbrake scale overflows 16
bits above about 69%. G HUB truncates there: it writes 75% as a point
*shorter* than 50%. The wheel accepts the full value. So this driver
writes the true one and stays monotonic across the whole 1-100 range.
Above ~69%, values set here and values set in G HUB will not correspond.

### wheel_handbrake_curve / wheel_handbrake_sensitivity

The response curve for the **RS Shifter & Handbrake** in analog handbrake
mode. The handbrake drives a base axis (`0x80A4` axis 4, evdev `ABS_Z`).
The driver bends it with the same mechanism as the pedals. Verified on an
RS50.

- `wheel_handbrake_curve` - raw `in:out` points or `reset`, like `wheel_response_curve`.
- `wheel_handbrake_sensitivity` - the 0-100 G HUB slider (50 = linear).

Both write the one curve the axis holds. The last write wins. The
handbrake *input* itself needs no configuration. Connected to the wheel
base, it works out of the box as `ABS_Z`. These attributes only bend it.

```bash
echo 70 > wheel_handbrake_sensitivity
echo "0:0 26000:0 65535:65535" > wheel_handbrake_curve   # 40% dead travel
echo reset > wheel_handbrake_curve
```

### RS Shifter & Handbrake input mapping

The accessory rides the wheel's existing report. So its inputs reach evdev
with no driver change. By its mode switch:

| Mode | Action | evdev |
|------|--------|-------|
| Sequential shifter | shift up | `BTN_TOP2` |
| Sequential shifter | shift down | `BTN_PINKIE` |
| Digital handbrake | pull past point | `BTN_THUMB2` (face button) |
| Analog handbrake | pull | `ABS_Z` axis |

Use `wheel_accessory` (above) to check whether the accessory is attached.
Use `wheel_accessory_mode` to see which of its three jobs it is currently
doing. All three of its G HUB settings are implemented:
`wheel_shift_actuation`, `wheel_handbrake_actuation` and
`wheel_handbrake_sensitivity` (with `wheel_handbrake_curve`).

---

## Compatibility Attributes

These attributes provide compatibility with existing wheel management tools, for example Oversteer, a Linux wheel configuration tool.
The sysfs filenames use the standard Oversteer names, without the `wheel_` prefix.

**Note:** The driver creates these aliases for every direct-drive wheel it
binds (RS50 and G PRO). Oversteer looks for the new-lg4ff attribute names.
(new-lg4ff is the community driver for classic Logitech wheels.) The
aliases exist so Oversteer can drive the wheel. The same settings are also
available under their `wheel_*` names documented above. On the **G923**
the classic names are not aliases. They are the primary (and only)
attributes, with classic semantics and scales. See the next section.

These attributes follow the de-facto Linux wheel convention that Oversteer
and similar tools speak: the new-lg4ff attribute names and scales.
Conformance was verified 2026-07-03 by driving every getter and setter
through Oversteer's own code against the live wheel. Note that the scales
differ from the `wheel_*` attributes. Tools expect raw device units here,
not percent.

### range
**Access**: Read/Write
**Values**: `90` to `2700` (degrees)

The same function as `wheel_range`. Both use degrees.

### gain
**Access**: Read/Write
**Values**: `0` to `65535` (raw; the FF_GAIN scale)

Drives the same wheel strength setting as `wheel_strength`. But this file
speaks the raw 0-65535 scale that tools expect. Oversteer shows it as
percent in its UI. `wheel_strength` keeps its human-friendly 0-100 percent
scale. The two stay in sync.

### autocenter
**Access**: Read/Write
**Values**: `0` to `65535` (raw; the FF_AUTOCENTER scale)

A real, driver-emulated centring spring. While nonzero, the wheel pulls
itself toward centre with a damped spring. The driver computes the spring
in the 1 kHz effect loop. The spring is firm within roughly the central
eighth of the axis, like hardware autocenter on other wheels. It is also
reachable through the standard evdev `FF_AUTOCENTER` control. So a game
that writes autocenter 0 before it takes over force feedback correctly
disables it for its session. Useful for desk-driving without a game, or
as idle centring.

### spring_level / damper_level / friction_level / inertia_level
**Access**: Read/Write
**Values**: `0` to `400` (percent), default `100`

Global output scales for the emulated `FF_SPRING` / `FF_DAMPER` /
`FF_FRICTION` / `FF_INERTIA` effect classes. They match the new-lg4ff
semantics. 100 = effects play as the game commanded. Lower values tame
that effect class across all games. 0 mutes it. `damper_level` scales
DAMPER effects from games. The wheel's own firmware damping is
`wheel_damping`.

new-lg4ff stops at 100. This engine accepts up to 400. Its gains sit
below what the wheels' firmware renders for the same effect, by amounts
that differ per wheel. A G923 Xbox edition owner measured the engine's
damper at 0.61 of the firmware's ([#87](../../issues/87)). A G PRO owner
runs 2.25x damper and 4x spring against theirs ([#89](../../issues/89)).
Until those measurements settle the defaults, the levels are how to try
them:

```bash
echo 225 > damper_level     # the G PRO numbers from #89
echo 400 > spring_level
```

The driver clamps the summed force to the motor's range after it adds
every effect. So a large level saturates instead of wrapping.
`inertia_level` has no new-lg4ff counterpart. The engine's inertia
estimate is band-limited (a 50 ms one-pole on the velocity). So a steady
acceleration reads as itself, and the encoder's quantisation no longer
arrives as grain.

---

## G923 Attributes (PlayStation edition, c266/c267)

The G923 PlayStation edition runs a classic force-feedback engine ported
from berarma's new-lg4ff. It does not use the direct-drive `hidpp_dd_ff_*`
path. So it exposes the classic lg4ff attribute names as its primary
attributes. Oversteer speaks them natively. None of the `wheel_*`
attributes above exist on this wheel. The attributes live on the wheel's
interface-0 HID device:

```bash
find /sys/bus/hid/devices -name "*046D*C266*" 2>/dev/null   # G923 PS (after mode switch)
```

The wheel enumerates as `046d:c267` in PlayStation mode. The driver
switches it to classic mode (`046d:c266`) automatically. So `c266` is the
device you configure. All attributes are hardware-verified on a c266
except where noted.

### range
**Access**: Read/Write
**Values**: `40` to `900` (degrees)

Steering rotation range. A write of `0` selects the maximum (900). The
driver ignores out-of-range values. The write succeeds but changes
nothing.

```bash
echo 540 > range
```

### gain
**Access**: Read/Write
**Values**: `0` to `65535`
**Default**: `65535`

Master gain for all effects the classic engine plays. The engine
multiplies it with the per-application `FF_GAIN` from evdev.

### autocenter
**Access**: Read/Write
**Values**: `0` to `65535` (spring strength; `0` = off)
**Default**: `0`

Hardware autocenter spring. Writes go through the same `FF_AUTOCENTER`
callback that games use. So a game that disables autocenter before it
takes over force feedback behaves correctly.

### combine_pedals
**Access**: Read/Write
**Values**: `0`, `1`, `2`

Merges two pedals into one centred axis for legacy games. The driver does
this by rewriting the wheel's input report:

- `0` - separate pedals (default)
- `1` - throttle and brake combined into the throttle axis
- `2` - throttle and clutch combined into the throttle axis

The driver clamps values above `2` to `2`.

### ffb_output
**Access**: Read-only
**Values**: roughly `-32768` to `32767` (`0` = no force)

The classic engine's current net force output, after gain. The engine
updates it once per effect-loop tick. `logi-tf-sim` polls this to mirror
live force feedback into the G923's TrueForce stream while simulated
TrueForce is running. An active stream makes the wheel follow the stream's
force field instead of the classic path. So the mirror is what keeps force
feedback alive during streaming.

### Rev LEDs

The five rev-light pairs are standard Linux LED class devices. There is
one device per mirrored pair, outermost first:

```
/sys/class/leds/<hid-device>::RPM1 .. ::RPM5
# e.g. /sys/class/leds/0003:046D:C266.0005::RPM1/brightness
```

Write `1`/`0` to each `brightness` file to switch a pair on or off. Any
LED tool (or a telemetry feeder) that speaks the standard `leds` class
works. Under the hood this is the classic G29-family LED command, not the
DD wheels' HID++ feature. So `wheel_rev_level` does not exist here.

### Xbox edition (c26e)

The G923 Xbox edition routes through the driver's HID++ 0x8123
(G920-style) force-feedback path instead of the classic engine. So it
exposes `range` (clamped to `180`-`900`) and `range_restore`. It exposes
none of the classic engine's `gain`/`autocenter`/`combine_pedals`, and
none of its rev-LED devices. Force feedback and TrueForce are both
hardware-confirmed on this edition (issue #27, 2026-07-30).

#### range_restore

**Access**: Read/Write, mode 0664
**Default**: `1`, on (matching `wheel_range_restore` on the direct-drive
wheels)

Puts the rotation range back when a game moves it.

A game that reaches the Logitech SDK directly pushes its own configured
steering rotation at the wheel. On Linux, direct SDK access means
`PROTON_ENABLE_HIDRAW=1`. The push is a TrueForce type-`0x0e` packet. It
bypasses the HID++ range feature entirely, so nothing is notified. A
title configured for 90 degrees silently locks the wheel to 90, with a
soft stop there. The SDK sends the push once at session start. So putting
the range back sticks for that session.

On this edition specifically, that push does not appear to reach the
wheel. An owner's rim keeps its full travel in ACC's config screen. The
limit shows up only on track. That means the game is clamping its own
steering, and the wheel is not being reconfigured (see
`docs/TRUEFORCE_PROTOCOL.md`). So this attribute is expected to find
nothing to do there. It is on regardless. The cost of watching is a read
every two seconds. The cost of not watching is an unusable wheel for
anyone whose game does write the range.

While on, the driver re-reads the range the wheel is actually enforcing
every two seconds. If a game has moved it, the driver re-applies the
configured one. It gives up after three restores and logs once. So a
program that genuinely wants a different range wins, instead of being
fought forever. A write of `1` again forgives that count.

**The risk.** The driver writes a range while a game holds the wheel. On the direct-drive wheels, doing that at the
wrong moment desynchronised the centre violently. That is why their
equivalent first checks that the rim is still and near centre. It uses an
encoder read that this wheel does not offer. The check here reads the
reported steering axis instead. It refuses to write unless the rim is
within 3% of centre. This is believed equivalent, but it has not been
proven on this hardware. Watch the wheel the first time it fires. Write
`0` to turn it off if the centre moves.

Doing it by hand is a complete alternative, because the push is one-shot:

```bash
# after the game has started and taken the wheel
echo 900 > /sys/class/hidraw/hidrawN/device/range
```

---

## Debug Attributes

### wheel_hidpp_debug
**Access**: Read/Write, mode 0600 (root only)
**Availability**: Only present when the module is built with `CONFIG_HID_LOGITECH_HIDPP_DEBUG` (e.g. `make DEBUG=1`). Absent from default builds.

Raw HID++ command shell for protocol bring-up. Write `feature fn [params...]` in hex. Read the last command's response.

```bash
# Send fn 0x5c to feature 0x0b with three zero params
echo "0b 5c 00 00 00" > wheel_hidpp_debug

# Read the response
cat wheel_hidpp_debug
```

---

## Error Codes

| Error | Meaning |
|-------|---------|
| `-ENODEV` | Device not found or driver not ready |
| `-EPERM` | Operation not permitted in current mode |
| `-EINVAL` | Invalid value provided |
| `-ERANGE` | Value out of range (e.g. `wheel_calibrate` > 65535, or an active LED slot index out of range) |
| `-EOPNOTSUPP` | Feature not supported by device |
| `-EIO` | Communication error with device |

---

## Example Scripts

### Quick Setup Script
```bash
#!/bin/bash
# Set up RS50 for racing

DEVICE=$(find /sys/bus/hid/devices -name "*046D*C276*" | head -1)
cd "$DEVICE" || exit 1

# Force feedback settings
echo 900 > wheel_range        # 900 degrees
echo 75 > wheel_strength      # 75% force
echo 20 > wheel_damping       # 20% damping
echo 50 > wheel_trueforce     # 50% TRUEFORCE

# LED: Red theme
echo 0 > wheel_led_slot
echo "FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000 FF0000" > wheel_led_colors
echo 1 > wheel_led_apply

echo "RS50 configured!"
```

### Mode Switch Script
```bash
#!/bin/bash
# Toggle between desktop and onboard mode

DEVICE=$(find /sys/bus/hid/devices -name "*046D*C276*" | head -1)
MODE=$(cat "$DEVICE/wheel_mode")

if [ "$MODE" = "desktop" ]; then
    echo "onboard" > "$DEVICE/wheel_mode"
    echo "Switched to onboard mode"
else
    echo "desktop" > "$DEVICE/wheel_mode"
    echo "Switched to desktop mode"
fi
```

---

## Module parameters

Set a parameter at load time (`modprobe hid-logitech-dd stream_yield=N`).
Or, where the mode allows, set it live under
`/sys/module/hid_logitech_dd/parameters/`. All of them default to the
right thing. Each exists so a behaviour that was introduced to fix
something can be turned off again. That matters if it ever misbehaves on
hardware nobody here has.

| Parameter | Default | What it does |
|---|---|---|
| `stream_yield` | `Y` | Stay off the TrueForce stream while another process is writing it. The endpoint carries one packet per millisecond. So two streamers halve each other and square-modulate the motor at 500 Hz (see the one-writer section of TRUEFORCE_PROTOCOL.md). While yielding, the driver still applies its own force by writing it into the owner's packet. It does this only when an effect is actually running here, so a game's own force is never overwritten. `N` restores the old behaviour of streaming regardless. |
| `kf_idle_gate` | `Y` | Stop the 1 kHz zero-force keepalive after 500 ms of exact-zero force. This keeps an idle wheel silent instead of holding a stream open. `N` streams always. |
| `g923_pedal_invert` | `Y` | Report the G923's pedals the way round Linux expects: 0 released and full scale pressed. The wheel sends all three inverted (255 released, 0 fully pressed). It does this on every id it has, both editions included. Nothing in HID marks an axis as inverted, so no layer corrects it by itself. The driver applies this at the input layer only. Raw HID stays exactly as the wheel sent it, because a Wine or Proton game that reads the wheel over raw HID expects the Windows convention. `N` passes the axes through as sent, for anyone who already inverted them inside their games. It does not apply to the direct-drive wheels, which report these axes the normal way up. It also does not apply while `combine_pedals` is on. That mode merges throttle and brake into one bidirectional axis, where inverting would swap them. |
| `g923_xbox_dd_engine` | `N` | Experimental, and load-time only. Drives the G923 Xbox edition's force feedback from this driver's own engine. The default downloads the effects into the wheel's firmware over HID++ 0x8123 instead. The reason to want it is simulated TrueForce. The firmware sums the effects itself, so the driver never knows the force being produced. A TrueForce stream, whose torque field takes the motor, can then only carry a zero. That is why `logi-tf-sim` refuses that wheel by default. With this engine, the driver sums the force here and puts it into the stream with the texture, the way it does on the direct-drive wheels. The daemon then streams without an override. Untested on hardware: nobody working on this owns that wheel. Its rev lights are unaffected and stay on their own LED device. The direct-drive `wheel_rev_level` that appears with this parameter does nothing on this wheel, which speaks the other rev-light dialect. Condition effects (spring, damper, friction, inertia) stay silent until the wheel's steering position reaches this engine. That needs that edition's report layout and is not done. |
| `inject_pid` | `0` (off) | Appends a PID force-feedback collection to interface 0's report descriptor. (PID is the USB HID force-feedback class.) **Leave it off.** The injected collection declares report ids this wheel does not use. The wheel's input reports are then misparsed, and steering and pedals stop working. It was briefly a default in 0.34.0, and that is exactly what happened (#59, #63). `1` is a dry run. `2` actuates. |

`disable_tap_to_click` also exists. It is inherited from the in-tree driver
this one forked. It concerns a K400 touchpad and nothing about a wheel.

---

## Protocol Details

For the HID++ protocol details, see:
- `PROTOCOL_SPECIFICATION.md` - Full protocol documentation
- `dev/docs/CAPTURE_ANALYSIS_*.md` - USB capture analysis

### Feature Pages Used

| Page | Index Var | Description |
|------|-----------|-------------|
| 0x8040 | idx_brightness | LED Brightness / Sensitivity |
| 0x807A | idx_lightsync | LIGHTSYNC Effects |
| 0x807B | idx_rgb_config | RGB Zone Configuration |
| 0x812C | idx_calibrate | Centre Calibration (RS50 + G Pro, sub-device 0x05) |
| 0x8133 | idx_damping | Wheel Damping |
| 0x8134 | idx_brakeforce | Brake Force |
| 0x8136 | idx_strength | FFB Strength |
| 0x8137 | idx_profile | Profile/Mode Switching |
| 0x8138 | idx_range | Rotation Range |
| 0x8139 | idx_trueforce | TRUEFORCE |
| 0x8140 | idx_filter | FFB Filter |
