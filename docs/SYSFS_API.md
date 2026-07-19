# Logitech TrueForce Linux Driver - Sysfs API Reference

**Driver**: `hid-logitech-dd` (module), `logitech-dd` (hid_driver)
**Devices**:
- Logitech RS50 Direct Drive Wheel Base (USB `046d:c276`)
- Logitech G Pro Racing Wheel (USB `046d:c272` Xbox/PC, `046d:c268` PS/PC)

**Applies to**: the `hid-logitech-dd` driver in this repository.

Most of the attributes documented here are shared between the RS50 and G Pro (the two wheels share the settings code path). Attributes that are currently G Pro-only or RS50-only are called out inline.

---

## Overview

The driver exposes wheel configuration through sysfs attributes located at:
```
/sys/bus/hid/devices/<DEVICE_ID>/
```

Where `<DEVICE_ID>` is typically `0003:046D:C276.XXXX` (RS50), `0003:046D:C272.XXXX` (G Pro Xbox/PC), or `0003:046D:C268.XXXX` (G Pro PS/PC).

To find your device path:
```bash
find /sys/bus/hid/devices -name "*046D*C276*" 2>/dev/null   # RS50
find /sys/bus/hid/devices -name "*046D*C27[28]*" 2>/dev/null # G Pro (c272 or c268)
```

---

## Mode and Profile Control

### wheel_mode
**Access**: Read/Write
**Values**: `desktop`, `onboard`

Controls the operating mode of the wheel base.

- **Desktop mode** (profile 0): Settings controlled by host software. Sensitivity available.
- **Onboard mode** (profiles 1-5): Settings stored in wheel profiles. Brake force available.

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

Controls the active profile. Profile 0 is desktop mode; profiles 1-5 are onboard profiles.

```bash
# Read current profile
cat wheel_profile
# Output: 0-5

# Switch to onboard profile 3
echo 3 > wheel_profile

# Switch to desktop mode (profile 0)
echo 0 > wheel_profile
```

---

## Force Feedback Settings

### wheel_profile_names
**Access**: Read/Write

The five onboard slots' names, queried live from the wheel. Use this to
know which slot number `wheel_profile` should get.

```bash
cat wheel_profile_names
# 1: RACE
# 2: DRIFT
# 3: PROFILE 3
# 4: PROFILE 4
# 5: PROFILE 5
```

Writing renames one slot, as `<slot>:<name>` (feature `0x8137` fn4). The
wheel persists the name to its own NVM on the write; there is no separate
save step.

```bash
# Rename slot 3
echo "3:RACE" > wheel_profile_names
```

Slot is 1-5. On an RS50 the name may be up to **9 characters** (the same
length as its stock `PROFILE 3`), may contain spaces, and is stored
**uppercased** - `echo "3:Race"` reads back as `3: RACE`. A longer name is
refused by the wheel itself and surfaces as `-EIO`; a bad slot, an empty
name, or one over the 14-byte HID++ payload gives `-EINVAL`.

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

On the G PRO PID (`046d:c272` / `046d:c268`) - both real G PRO and
RS50-in-compat - the standard HID++ range feature is not advertised
at the index the native code expects; the driver falls back to
feature `0x8138` (index 0x18, fn 2) captured from G Hub. See
`docs/PROTOCOL_SPECIFICATION.md` section 5.1. The fallback
write only takes effect while the wheel is in desktop mode; the
wheel boots in onboard mode, so write `0` to `wheel_profile` first
to enter desktop mode and have subsequent range writes take effect
on the motor.

**External-change detection**: some game launches (observed with
Assetto Corsa EVO under Proton) reset the wheel's physical range to
90 degrees without any HID++ notification. The driver re-reads the
true range from the wheel every 20 seconds (paused while force
feedback is actively playing, so the synchronous query can never
stall the force stream; it catches up within one interval of the
effects stopping); if it changed externally,
the reported `wheel_range` value is updated to the real one, the
change is logged in dmesg (`rotation range changed externally`), and
`poll()`ers on the attribute are notified via `sysfs_notify()`.
When the external value is exactly 90 (the known SDK session-init
pathology - see `wheel_range_restore` below), the driver also
restores the previous range automatically.

### wheel_range_restore
**Access**: Read/Write
**Values**: `0` or `1`
**Default**: `1`

Automatic recovery from the launch-time 90-degree reset. Root cause
(usbmon-verified): some games' SDK sessions push an operating range
of 90 degrees once at session start via a TrueForce interface-2
packet, invisible to HID++. With this enabled, the driver restores
the pre-reset range automatically. Verified end-to-end against a
faithful reproduction of the game traffic: detection to restore in
under 100 ms once the poll samples the change.

Safety gates, each earned from a real incident:
- fires only for an EXTERNAL silent change landing exactly on 90;
  any other externally-set value (a game applying its configured
  steering lock: 540, 850, ...) is respected as legitimate intent;
- desktop mode only, and never an automatic mode switch;
- the wheel must be stationary (two encoder reads 50 ms apart);
  restores only ever widen the range, which cannot snap the wheel;
- at most 3 restores per session, then the driver logs and yields
  (`an external writer keeps changing the rotation range`);
- an explicit `wheel_range` write supersedes any pending restore
  and resets the strike counter.

`0` = detect-and-report only: the
change is still logged and `wheel_range` stays honest, but recovery
is manual (`wheel_profile=0` then `wheel_range=<degrees>` once FFB
is idle).

### wheel_strength
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the force feedback strength.

**Internal encoding**: The driver converts percentage to a 16-bit value where:
- 0% = 0x0000
- 100% = 0xFFFF (corresponds to 8.0 Nm max torque)

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

Sets the wheel damping (resistance when turning).

**Internal encoding**: The driver scales the 0-100 percentage to a 16-bit big-endian value (`value = percentage * 65535 / 100`) and writes it to page `0x8133` with SET fn=1. See `docs/PROTOCOL_SPECIFICATION.md` section 5.

```bash
# Read current damping
cat wheel_damping

# Set to 25%
echo 25 > wheel_damping
```

### wheel_trueforce
**Access**: Read/Write
**Values**: `0` to `100` (percentage)

Sets the TRUEFORCE bass shaker intensity.

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

Sets the brake pedal force threshold (load cell sensitivity).

**Note**: Returns `-EPERM` (Permission denied) in desktop mode.

```bash
# Set brake force to 75% (must be in onboard mode)
echo "onboard" > wheel_mode
echo 75 > wheel_brake_force
```

### wheel_sensitivity
**Access**: Read/Write
**Values**: `0` to `100` (percentage)
**Mode Restriction**: Writes only accepted in **desktop mode**; reads always succeed.

Shapes the steering response curve via feature `0x80A4`
(AxisResponseCurve): a 64-point cubic Bezier from (0,0) to (1,1) with
control points P1=(1-s, s), P2=(s, 1-s) for s = value/100. Values below
`50` soften the response near centre; values above `50` sharpen it. `50`
is the identity curve, so the driver reverts to the wheel's built-in
curve (as G Hub does) rather than uploading a flat line.

This is unrelated to LED brightness. Feature `0x8040` (behind
`wheel_led_brightness`) is brightness only; sensitivity and brightness are
fully independent.

Reads return the last value written, defaulting to `50`. The wheel has no
read-back for the slider, so this is a write-through cache. Writes in
onboard mode fail with `-EPERM`; if the wheel does not expose the
response-curve feature, writes return `-EOPNOTSUPP`.

```bash
# Sharpen the centre response (must be in desktop mode)
echo "desktop" > wheel_mode
echo 65 > wheel_sensitivity
```

### wheel_ffb_filter
**Access**: Read/Write
**Values**: `1` to `15` (filter level)

Sets the force feedback smoothing/filtering level. G Hub's labels are roughly Minimum (1), Low (7), Medium (11), Maximum (15).

Values outside `1..15` are clamped to that range.

```bash
# Read current filter level
cat wheel_ffb_filter

# Set to level 11 (G Hub "Medium")
echo 11 > wheel_ffb_filter
```

### wheel_ffb_filter_auto
**Access**: Read/Write
**Values**: `0` (manual), `1` (auto)

Enables automatic FFB filter adjustment based on game output.

The driver splits the two on-wire meanings of the filter flag byte across two sysfs writes: writing to `wheel_ffb_filter` stamps the "user set this level right now" bit, writing here toggles only the "auto mode" bit. See `docs/PROTOCOL_SPECIFICATION.md` section 5 (FFB Filter) for the bitfield decode.

```bash
# Enable auto filter
echo 1 > wheel_ffb_filter_auto

# Disable (use manual filter setting)
echo 0 > wheel_ffb_filter_auto
```

### wheel_ffb_constant_sign
**Access**: Read/Write
**Values**: `0` or `1`
**Default**: `1`

Controls whether the driver inverts the sign of every `FF_CONSTANT`
level before sending it to the wheel. This single toggle is what makes
the driver's FFB feel right under both Wine/Proton games and native
Linux apps - the two paths disagree on sign by one flip somewhere in
Wine's DirectInput-to-evdev translation layer, and the driver can't
tell them apart at runtime.

- `1` (default) - invert. Correct for Wine/Proton running Windows
  games (Assetto Corsa Competizione, etc.). If `FF_CONSTANT` feels
  like centring forces push the wheel *away* from centre instead of
  back toward it, the toggle is at the wrong setting.
- `0` - pass-through. Correct for native Linux evdev apps that
  upload effects with the convention documented in
  `Documentation/input/ff.rst` (direction=0x4000 east, positive
  level = rightward force). `fftest`, `ffcfstress`, and direct
  EVIOCSFF uploads from custom tools are in this category.

Only affects `FF_CONSTANT`. All other effect types (`FF_SPRING`,
`FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA`, `FF_RAMP`, `FF_PERIODIC`,
`FF_RUMBLE`) feel identical at either toggle value.

**Caveat for reverse driving**: the inversion is unconditional - it
does not look at the wheel's velocity or the car's gear. In sims that
correctly model the self-aligning torque flipping sign at negative
longitudinal velocity (most modern racing sims do), the chain is
"sim physics-inverts for reverse → Wine inverts again → driver inverts
again", which lands as physics-correct destabilising FFB the user
feels as the wheel actively pushing away from centre when reversing.
That is the real-car behaviour and not a driver bug, but it can feel
violent compared to a wheel without TF / direct-drive force. Lowering
`wheel_strength` is the only knob from the driver side; switching the
sign toggle off would make forward driving feel wrong without fixing
the reverse case.

A contributor cross-checked this on Windows (issue #8, AC EVO and AC):
the FFB "gets pretty violent in reverse" there too, with the same wheel
and game settings. So the strong reverse force is the sim's physics
surfacing through the wheel, identical to the Windows G Hub path, not a
sign error or double-inversion on our side.

```bash
# Playing Wine/Proton racing games: leave default
cat wheel_ffb_constant_sign    # -> 1

# Running native-evdev tools like fftest:
echo 0 | sudo tee wheel_ffb_constant_sign
```

The inversion is confirmed empirically on Assetto Corsa Competizione with this
wheel; a test harness in `tests/ff_matrix_test.c` cross-checks each toggle value
against native evdev expectations.

### wheel_spring_damping
**Access**: Read/Write
**Values**: `0` to `100` (percentage)
**Default**: `25`
**Availability**: all direct-drive wheels (RS50 native/compat and real G PRO - every family PID runs the same `hidpp_dd_ff_*` FFB path).

Synthetic damping applied to emulated `FF_SPRING` effects, as a
percentage of a `FF_DAMPER` running the spring's own coefficient.

The driver emulates condition effects host-side: it samples the wheel
position and pushes the computed force back over USB. That loop
latency on a low-friction direct-drive motor makes a stiff, undamped
game-uploaded spring ring - a growing back-and-forth oscillation that
ends with the wheel's over-torque failsafe cutting power (observed
live with Assetto Corsa EVO's map-load centring spring). Real wheels
damp the spring inside the firmware servo loop; this knob restores
that behaviour. `0` disables it. The damping
scales with the spring's own coefficient, so stiff springs get
proportionally stronger damping.

```bash
cat wheel_spring_damping     # -> 25
echo 40 > wheel_spring_damping
```

### wheel_texture_route
**Access**: Read/Write
**Values**: `tf` or `kf` (also accepts `1` / `0`)
**Default**: `tf`
**Availability**: all direct-drive wheels (RS50 native/compat and real G PRO - every family PID runs the same `hidpp_dd_ff_*` FFB path).

Selects where vibration-class effects - `FF_RUMBLE` and periodic
effects at 20 Hz or faster (period <= 50 ms) - are actuated:

- `tf` (default) - the driver streams them on the wheel's TrueForce
  audio-haptic channel (interface 2, ~1 kHz sample rate), the same
  physical path the Windows SDK uses for texture. Steering-shaping
  effects (`FF_CONSTANT`, conditions, slow periodics) stay on the
  force channel, so rumble no longer modulates the steering axis.
  This matches the Windows KF/TF split; the "gritty/notchy steering
  under rumble" reported in issue #8 is the `kf` behaviour.
- `kf` - legacy: everything is summed into the single steering
  force. Kept as a fallback and for A/B comparison.

The TrueForce session is brought up lazily: the first time a
texture-class effect actually plays, the driver replays the captured
68-packet init sequence twice (G Hub behaviour) and then streams
unified packets at 500 Hz while texture effects are active - each
packet carries the steering-force sum in its preamble and four
texture-audio window slots (2 kHz slot rate), the same shape AC EVO
streams (dmesg: `TrueForce texture channel ready`). Wheels that never see texture
effects never see TF traffic. If the init fails, texture effects
fall back to the steering channel - degraded feel, never lost - and
the driver retries on a later texture playback (up to 3 attempts per
session, logged in dmesg).

An effect's channel is decided when its playback starts and held for
the whole play cycle, so re-parametrising a playing effect across the
20 Hz crossover (or the session init completing mid-play) never yanks
a live effect between channels. Playbacks started before the session
is ready ride the steering channel for their duration; the next
playback moves to the TF stream.

Texture amplitude respects `FF_GAIN` and `wheel_strength` (the wheel
firmware scales steering forces by the strength setting itself but
plays TF samples at face value, so the driver applies strength to
texture in software for consistency), and is additionally capped at
half of full scale: above roughly 0.5-0.7 FS the wheel's DSP crosses
from vibration into pulling the steering axis, so the cap keeps a
synthetic full-scale rumble from hijacking steering torque. Real
games stream texture far below the cap.

Note for SDK games (ACC, AC EVO with the TrueForce shim): those
stream TrueForce themselves via hidraw and normally do not send
evdev rumble at the same time, so the two streams do not meet. If a
game somehow does both, set `kf` to keep the wheel's TF input to a
single writer.

```bash
cat wheel_texture_route      # -> tf
echo kf > wheel_texture_route   # A/B back to the legacy mixing
```

### wheel_calibrate
**Access**: Write-only (mode 0220)
**Values**: `0` to `65535` (raw encoder position)
**Availability**: RS50 and G Pro. Returns `-EOPNOTSUPP` if the wheel does not expose page `0x812C` on sub-device `0x05` (no known variant lacks it, but the driver does not assume).

Low-level primitive: writes the given raw 16-bit encoder value to adopt
as the new centre. The driver sends `10 05 <idx> 3D <hi> <lo> 00` to
HID++ sub-device `0x05`, feature page `0x812C` (see
`docs/PROTOCOL_SPECIFICATION.md` section 5 for the wire format).
Verified on RS50 from `2026-04-22_re_calibrate.pcapng`.

Use this only if you already have the raw encoder value you want to
make the centre (e.g., you read it via a HID++ GET yourself, or you
want to seed a specific reference value). For the common case ("make
the wheel's current physical position the new centre") use
`wheel_calibrate_here` below, which does the GET+SET internally.

```bash
# Low-level: you already have the raw encoder number you want.
echo 32768 > wheel_calibrate
```

### wheel_calibrate_here
**Access**: Write-only (mode 0220)
**Values**: any non-empty write triggers the operation
**Availability**: same as `wheel_calibrate`

One-shot "use the wheel's current physical position as the new centre".
The driver issues fn=1 GET to read the current raw encoder value, then
fn=3 SET with that value. Mirrors what G Hub does when the user clicks
Calibrate on Windows. Hold the wheel at the desired centre (typically
true centre), then write to this attribute.

```bash
# Hold the wheel at true physical centre, then:
echo 1 > wheel_calibrate_here
```

No state is stored in the driver; the wheel's firmware persists the new
centre across power cycles (same as G Hub on Windows).

---

## LIGHTSYNC LED Control

The RS50 has 10 RGB LEDs in a horizontal strip across the upper faceplate (an engine-RPM / shift indicator). The driver provides per-slot configuration with 5 custom slots (0-4).

> **Per-model availability**: the `wheel_led_*` LIGHTSYNC attributes exist on the RS50 in both native and G-PRO-compat enumeration (the RS50's faceplate LED-strip hardware doesn't change with the PID; verified live 2026-04-29). On a **real G PRO Racing Wheel** they are hidden: that rim has level-based rev lights with no per-LED RGB, exposed as `wheel_rev_level` instead (see its entry below).
>
> **G PRO PID (`046d:c272` / `046d:c268`)**: covers both real G PRO Racing Wheel and RS50-in-G-PRO-compat-mode. Both run through the same `hidpp_dd_ff_*` code path and expose the same wheel-config surface; the LED attributes differ per rim (see the per-model note above - the driver tells the two apart by USB product string). On the RS50 in compat mode, LIGHTSYNC works the same way as native - feature `0x807A` is advertised at the same index discovery picks up in native, and `wheel_led_*` writes drive the LED strip end-to-end (verified against the live wheel 2026-04-29). Wheel-config attributes that work via fallback feature paths (see `docs/PROTOCOL_SPECIFICATION.md` section 5.1): `wheel_range`, `wheel_strength`, `wheel_trueforce`, `wheel_damping`, `wheel_ffb_filter`, `wheel_profile` (write `0` to enter desktop mode), and `wheel_calibrate`. The remaining attributes (`wheel_brake_force`, `wheel_ffb_filter_auto`, `wheel_sensitivity`) are unsupported by this firmware: once their mode gating is satisfied the store returns `-EOPNOTSUPP` (note `wheel_brake_force` still returns `-EPERM` in desktop mode and `wheel_sensitivity` in onboard mode before that check). For those, configure via the wheel's OLED menu or via Windows G Hub on a Windows host.

### LED Control Workflow

1. **Select a slot**: `echo 2 > wheel_led_slot`
2. **Set direction** (optional): `echo 1 > wheel_led_direction`
3. **Set colors**: `echo "FF0000 FF0000 ... (10 colors)" > wheel_led_colors`
4. **Apply changes**: `echo 1 > wheel_led_apply`

Alternatively, use built-in effects via `wheel_led_effect`.

### wheel_led_slot
**Access**: Read/Write
**Values**: `0` to `4` (custom slot index)

Selects the active custom LED slot for configuration.

```bash
# Select slot 2
echo 2 > wheel_led_slot
```

### wheel_led_slot_name
**Access**: Read/Write
**Values**: String (max 8 characters)

Gets or sets the name of the currently selected LED slot. Names are stored on the device.

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

Gets or sets the brightness for the currently selected slot. Each slot can have its own brightness level, which is applied when the slot is activated via `wheel_led_apply`.

```bash
# Read current slot brightness
cat wheel_led_slot_brightness
# Output: brightness percentage (e.g., "75")

# Set slot brightness to 50%
echo 50 > wheel_led_slot_brightness
```

**Note**: This is per-slot brightness. Use `wheel_led_brightness` to set global brightness without changing slot settings.

### wheel_led_direction
**Access**: Read/Write
**Values**: `0` to `3`

Sets the LED animation direction for the current slot:
- `0` = Left to Right
- `1` = Right to Left
- `2` = Inside Out (center outward)
- `3` = Outside In (edges inward)

On the wire the `0x807B` config carries a 1-4 value; the driver translates
between this 0-3 enum and the device's 1-4 encoding internally (see
PROTOCOL_SPECIFICATION.md section 9.4.1).

```bash
# Set direction to Right-to-Left
echo 1 > wheel_led_direction
```

### wheel_led_colors
**Access**: Read/Write
**Format**: 10 space-separated hex RGB values (`RRGGBB`)

Sets all 10 LED colors for the current slot. LED1 is leftmost.

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

Brightness changed from the wheel itself (OLED menu) is tracked: the
driver consumes the official x8040 `brightnessChangeEvent` broadcast,
updates this attribute's value (and `wheel_sensitivity` when in
desktop mode, per the aliasing described there), and notifies
`poll()`ers on `wheel_led_brightness`.

### wheel_led_effect
**Access**: Read/Write
**Values**: `1` to `9`

Selects the LED animation mode. Values match feature 0x807A fn3 on
the wire:

| Value | Effect |
|---|---|
| `1` | Inside to out |
| `2` | Outside to in |
| `3` | Right to left |
| `4` | Left to right |
| `5` | Static/Custom (use custom slot colors) |
| `6`-`9` | Advertised by the wheel's supported-effect list (live-verified) but not yet visually labeled - try them and watch the LEDs |

Writing `5` re-applies the active slot's stored RGB so the new mode
has something to render; writes for other modes just switch the
effect and leave the cached colors untouched. Writes outside `1..9`
are clamped to the nearest end of the range.

Effect changed from outside the driver (G Hub-style tools, or the
wheel itself) is tracked: the driver consumes the LIGHTSYNC
effect-change broadcast, updates this attribute, and notifies
`poll()`ers.

```bash
# Use custom slot colors
echo 5 > wheel_led_effect

# Animate right to left
echo 3 > wheel_led_effect
```

### wheel_response_curve
**Access**: Read/Write
**Availability**: all direct-drive wheels (feature `0x80A4`)

The steering axis's 64-point response curve - the store behind G Hub's
**Sensitivity** slider. Write `reset` to revert to the built-in
(linear) curve, or 2-64 whitespace-separated `in:out` pairs (decimal
0-65535, strictly increasing `in`, non-decreasing `out`, starting at
`0:0` and ending at `65535:65535`). Fewer than 64 pairs are resampled
by linear interpolation to the 64 points the wheel stores.

```bash
# Soften the centre (slower response near straight-ahead)
echo "0:0 32768:16384 65535:65535" > wheel_response_curve

# Back to the built-in curve
echo reset > wheel_response_curve

cat wheel_response_curve
# 64/64 points loaded (0 = built-in curve)
```

The wheel applies this curve to the steering axis it reports to the PC (base
`0x80A4` axis 0, the same `wheel_sensitivity` uploads its slider curve to).
`wheel_sensitivity` and `wheel_response_curve` both write that one axis-0 curve,
so the last one written wins. The axis does not change until the wheel next
moves: it sends no HID reports while held still, so an upload appears to do
nothing until you nudge the wheel; `cat wheel_response_curve` reads the point
count back from the wheel and is the honest check. Use `reset` if steering feels
wrong after an upload. Whether curves persist across power cycles is untested.

### wheel_rev_level
**Access**: Read/Write
**Values**: `0`-`10` (number of rev LEDs lit)
**Availability**: real G PRO rim only. The attribute is present on every model,
but the store returns `-EOPNOTSUPP` on hardware without the level-based rev-light
feature (the RS50 uses the `wheel_led_*` LIGHTSYNC strip instead, which is hidden
on a real G PRO).

Rev-light level for the G PRO rim. The G PRO's rim lights are
level-based: the host commands how many LEDs are lit (0-10) and the
wheel's onboard profile owns colours, direction and scaling. Protocol
decoded from a G HUB capture by the TF4ALL project (see
`docs/PROTOCOL_SPECIFICATION.md` section 9). The first write arms the
feature. Writes return immediately: the driver coalesces them and
flushes only the newest level at G HUB's ~160 ms cadence (faster
bursts would starve the wheel's shared HID++ command processor), so a
fast telemetry feeder always shows the latest value with no queueing
lag. The
wheel holds a level for a while but reverts eventually - a telemetry
feeder should refresh at ~1 Hz or faster (natural for rev-light use).

**Status: implemented from captures, not yet validated on a real G PRO
- reports welcome (issue tracker).**

```bash
# Light 7 of 10 rev LEDs
echo 7 > wheel_rev_level
```

### wheel_serial
**Access**: Read-only
**Values**: 12-character device serial

The wheel's real serial number, read from HID++ DeviceInfo (feature
0x0003 fn2) at init. Matches the USB `iSerial` descriptor.

### wheel_firmware
**Access**: Read-only

Firmware versions read from DeviceInfo at init: the wheel base's
active main firmware and the motor unit's servo firmware (from
sub-device 0x05's own DeviceInfo).

```bash
cat wheel_firmware
# base: U1 65.03.B0038
# motor: SC 02.01.B0042
```

Include this output in bug reports - firmware-dependent behaviour
(feature index drift, settings quirks) is tracked against it.

### wheel_led_apply
**Access**: Write-only
**Values**: `1` (apply)

Applies the current slot configuration to the device.

```bash
# Apply current slot settings
echo 1 > wheel_led_apply
```

---

## Pedal Configuration

The pedal unit is a separate MCU (HID++ sub-device `0x02`) that applies a
64-point `0x80A4` response curve to each axis it reports to the PC. This was
verified live on an RS50 (2026-07-16) with a two-plateau throttle curve: the
reported axis dwelt at exactly the two programmed output values with an empty
gap between them, which a linear axis cannot do. So these are real shaping
controls, the same mechanism as the steering `wheel_response_curve`.

Each pedal `<p>` in {`throttle`, `brake`, `clutch`} exposes three attributes.
**They all write the single curve the axis holds, so the last write wins.** The
`_curve` attribute always reads back the wheel's true loaded-point count.

### wheel_&lt;p&gt;_curve
**Access**: Read/Write

The raw 64-point curve, identical in format to `wheel_response_curve`: write
`reset` for the built-in linear curve, or 2-64 whitespace-separated `in:out`
pairs (0-65535, strictly increasing `in`, non-decreasing `out`, starting `0:0`
and ending `65535:65535`; fewer than 64 pairs are resampled). Reads back
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
responsive early in its travel, below 50 less; `50` reverts to the built-in
linear curve. Generates the same symmetric-Bezier curve G HUB uploads. The
wheel stores only the resulting curve, so this reads back the last written
slider value, not a device query.

```bash
echo 70 > wheel_brake_sensitivity
```

### wheel_&lt;p&gt;_deadzone
**Access**: Read/Write, **Values**: `"<lower> <upper>"` percent

Dead travel at each end: output holds 0 until the pedal passes `lower`%, and
reaches full by `(100 - upper)`%. `lower + upper` must be at most 99. `"0 0"`
reverts to the built-in curve. Reads back the last written pair.

```bash
# 8% dead at the bottom, 5% saturation at the top of the clutch
echo "8 5" > wheel_clutch_deadzone
```

> Because sensitivity, deadzone and the raw curve all write the one hardware
> curve per axis, use one of them at a time per pedal. To combine a deadzone
> with a custom shape, author the whole thing as a single `_curve` upload (this
> is what the logi-dd editor does).

### wheel_combined_pedals
**Access**: Read/Write, **Values**: `0` (separate) / `1` (combined)
**Mode**: desktop only

G HUB's "combined pedals" toggle (feature `0x80D0`). When on, the wheel merges
the throttle and brake into a single centred axis for legacy games that expect
one pedal axis: released = centre, one pedal drives it up, the other down. The
brake's own axis goes silent. Off for any modern sim. Verified on an RS50: with
it on, `ABS_RX` re-centres to ~32768 and `ABS_RY` (the separate brake axis)
stops reporting.

```bash
echo 1 > wheel_combined_pedals   # merge (legacy games)
echo 0 > wheel_combined_pedals   # separate (default)
cat wheel_combined_pedals        # 0 or 1
```

### wheel_handbrake_curve / wheel_handbrake_sensitivity

Response-curve shaping for the **RS Shifter & Handbrake** in analog handbrake
mode. The handbrake drives a base axis (`0x80A4` axis 4, evdev `ABS_Z`), and the
driver shapes it with the same mechanism as the pedals, verified on an RS50.

- `wheel_handbrake_curve` - raw `in:out` points or `reset`, like `wheel_response_curve`.
- `wheel_handbrake_sensitivity` - the 0-100 G HUB slider (50 = linear).

Both write the one curve the axis holds; last write wins. The handbrake *input*
itself needs no configuration: connected to the wheel base, it works out of the
box as `ABS_Z`. These attributes only shape it.

```bash
echo 70 > wheel_handbrake_sensitivity
echo "0:0 26000:0 65535:65535" > wheel_handbrake_curve   # 40% dead travel
echo reset > wheel_handbrake_curve
```

### RS Shifter & Handbrake input mapping

The accessory rides the wheel's existing report, so its inputs reach evdev with
no driver change. By its mode switch:

| Mode | Action | evdev |
|------|--------|-------|
| Sequential shifter | shift up | `BTN_TOP2` |
| Sequential shifter | shift down | `BTN_PINKIE` |
| Digital handbrake | pull past point | `BTN_THUMB2` (face button) |
| Analog handbrake | pull | `ABS_Z` axis |

---

## Compatibility Attributes

These attributes provide compatibility with existing wheel management tools (e.g., Oversteer).
The sysfs filenames use standard Oversteer names (without the `wheel_` prefix).

**Note:** These aliases are created for every wheel this driver binds (RS50 and
G PRO). They exist so Oversteer, which looks for the new-lg4ff attribute names,
can drive the wheel; the same settings are also available under their `wheel_*`
names documented above.

These attributes follow the de-facto Linux wheel convention (the
new-lg4ff attribute names and scales) that Oversteer and similar tools
speak. Conformance was verified 2026-07-03 by driving every getter and
setter through Oversteer's own code against the live wheel. Note the
scales differ from the `wheel_*` attributes: tools expect raw device
units here, not percent.

### range
**Access**: Read/Write
**Values**: `90` to `2700` (degrees)

Same functionality as `wheel_range` (degrees on both).

### gain
**Access**: Read/Write
**Values**: `0` to `65535` (raw; the FF_GAIN scale)

Drives the same wheel strength setting as `wheel_strength`, but the
file speaks the raw 0-65535 scale tools expect (Oversteer shows it as
percent in its UI). `wheel_strength` keeps its human-friendly 0-100
percent scale; the two stay in sync.

### autocenter
**Access**: Read/Write
**Values**: `0` to `65535` (raw; the FF_AUTOCENTER scale)

A real, driver-emulated centring spring: while
nonzero, the wheel pulls itself toward centre with a damped spring
computed in the 500 Hz effect loop - firm within roughly the central
eighth of the axis, like hardware autocenter on other wheels. Also
reachable through the standard evdev `FF_AUTOCENTER` control, which
means games that write autocenter 0 before taking over force feedback
correctly disable it for their session. Useful for desk-driving
without a game, or as idle centring.

### spring_level / damper_level / friction_level
**Access**: Read/Write
**Values**: `0` to `100` (percent), default `100`

Global output scales for the emulated `FF_SPRING` / `FF_DAMPER` /
`FF_FRICTION` effect classes, matching the new-lg4ff semantics: 100 =
effects play as the game commanded, lower values tame that effect
class across all games, 0 mutes it. `damper_level` scales DAMPER
effects from games; the wheel's own firmware damping is `wheel_damping`.

---

## Debug Attributes

### wheel_hidpp_debug
**Access**: Read/Write, mode 0600 (root only)
**Availability**: Only present when the module is built with `CONFIG_HID_LOGITECH_HIDPP_DEBUG` (e.g. `make DEBUG=1`). Absent from default builds.

Raw HID++ command shell for protocol bring-up. Write `feature fn [params...]` (hex), read the last command's response.

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

## Protocol Details

For developers interested in the HID++ protocol details, see:
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
