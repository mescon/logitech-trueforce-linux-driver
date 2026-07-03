# Logitech TrueForce Linux Driver

[![Build Status](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/build.yml/badge.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/build.yml)
[![License: GPL v2](https://img.shields.io/badge/License-GPL_v2-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-2.0.en.html)
[![Linux](https://img.shields.io/badge/Linux-5.15%2B-green.svg)](https://kernel.org/)
[![Static Analysis](https://img.shields.io/badge/Static_Analysis-sparse%20%2B%20smatch-blueviolet.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/build.yml)
[![Language](https://img.shields.io/badge/Language-C_(Kernel)-orange.svg)](https://www.kernel.org/doc/html/latest/process/coding-style.html)
[![GitHub last commit](https://img.shields.io/github/last-commit/mescon/logitech-trueforce-linux-driver)](https://github.com/mescon/logitech-trueforce-linux-driver/commits/master)
[![GitHub issues](https://img.shields.io/github/issues/mescon/logitech-trueforce-linux-driver)](https://github.com/mescon/logitech-trueforce-linux-driver/issues)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/pulls)

> **Warning**
> This driver is under active development and may contain bugs or incomplete features. Use at your own risk. This disclaimer will be removed once the driver reaches a stable release.

> **New here with a wheel and a sim to race?** Start with the
> step-by-step guide: [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md)
> - from download to driving in about 15 minutes.

Linux kernel driver for **Logitech TrueForce direct-drive racing
wheels**:

- **Logitech RS50** (`046d:c276`)
- **Logitech G PRO Racing Wheel for Xbox/PC** (`046d:c272`)
- **Logitech G PRO Racing Wheel for PS/PC** (`046d:c268`)

> **Note**: this project was previously named
> `logitech-rs50-linux-driver`. It was renamed 2026-07-02 because the
> driver covers the whole TrueForce direct-drive family, not just the
> RS50. Old GitHub links and clones redirect automatically; nothing
> changes for installed systems (the kernel module and DKMS package
> were always named `hid-logitech-hidpp`).

You get the full evdev force-feedback suite (constant, spring, damper,
friction, inertia, periodic, ramp, rumble, gain), all buttons,
encoders, paddles, hat switch, 16-bit pedal axes, and G Hub-equivalent
settings (rotation range, FFB strength / damping / TRUEFORCE / filter,
pedal curves, LIGHTSYNC LEDs) exposed via sysfs. **TrueForce haptics
work in supported sims under Proton** via Logitech's own signed SDK
DLLs - see the recipe below.

This is a patched fork of the in-kernel `hid-logitech-hidpp` module
that replaces it. Other Logitech HID++ devices (mice, keyboards, G29 /
G920 / G923 wheels, etc.) keep working through the same module.

## What works

**Force feedback** (full evdev suite, all routed to wheel torque):
`FF_CONSTANT`, `FF_SPRING`, `FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA`,
`FF_PERIODIC` (sine, square, triangle, saw-up, saw-down), `FF_RAMP`,
`FF_RUMBLE`, `FF_GAIN`. Verified with `fftest`, the in-tree
`tests/ff_matrix_test`, and across multiple sims (ACC, AC, BeamNG,
AMS2, Le Mans Ultimate, iRacing, Dirt Rally, ETS2).

**Inputs**: all buttons including the G1 (logo) button, both
encoder rotaries with click, both shifter paddles, 8-direction D-pad
as a hat switch, 16-bit wheel axis (up to 2700° on RS50), 16-bit
pedal axes (throttle, brake, clutch). Button table further down.

**G Hub-equivalent settings via sysfs** at
`/sys/class/hidraw/hidrawX/device/wheel_*`. Native mode (RS50 in
its native `046d:c276` enumeration):

- Rotation range (90 to 2700°)
- FFB strength, damping, TRUEFORCE level, FFB filter (with auto)
- Sensitivity (desktop mode) and brake-force (onboard mode)
- Per-pedal response curves and deadzones, combined-pedals mode
- LIGHTSYNC LED slots, colors, effects, direction, brightness
- Mode + profile switching (desktop / onboard 1-5)
- Centre calibration (`wheel_calibrate`, `wheel_calibrate_here`)

Compat mode (RS50 or G PRO enumerated as `046d:c272` / `046d:c268`)
exposes a reduced HID++ feature set, but the same wheel-config
attributes (range, strength, trueforce, damping, FFB filter,
calibration, plus LIGHTSYNC) all work via fallback feature paths
decoded from G Hub captures. The wheel boots in onboard mode in
compat; write `0` to `wheel_profile` to enter desktop mode and
have live SETs take effect on the motor. See "Compat-mode
behavior" below for caveats.

**TrueForce in Proton sims**: end-to-end verified against
**Assetto Corsa Competizione** and **Assetto Corsa EVO** — full FFB,
TrueForce haptics, and complete button / paddle / encoder binding
all working through Logitech's own signed SDK DLLs running
unmodified under Proton. The same setup is expected to work for
Le Mans Ultimate, AMS2, Assetto Corsa (the original 2014 game),
rFactor 2, and iRacing — they all use the same SDK. The recipe below
covers the setup. Our driver passes the SDK's raw writes through
unchanged; no shim, no DLL injection.

Only ACC and AC EVO are verified; the rest are expected to work because
they share the SDK, not confirmed. This project does not ship an
rFactor 2 plugin: rFactor 2's TrueForce support is a separate
third-party component (for example the community TF4ALL SimHub plugin),
not something provided here, and it is untested with this setup.

See [`docs/SYSFS_API.md`](docs/SYSFS_API.md) for the complete sysfs
reference.

## Compatibility matrix

What to expect per wheel. **Legend:** ✅ verified on hardware · 🟢
supported, expected to work (shares the verified code path, not yet
tested on that exact model) · — not applicable.

| Capability | RS50 (`c276` native / `c272` compat) | G PRO Racing Wheel (`c272` Xbox-PC / `c268` PS-PC) |
|---|:--:|:--:|
| Steering, pedals, buttons, 8-way D-pad | ✅ | 🟢 |
| Force feedback (full evdev effect suite) | ✅ | 🟢 |
| TrueForce haptics (Proton + signed SDK) | ✅ | 🟢 |
| TrueForce texture routing for evdev effects (`wheel_texture_route`) | ✅ | 🟢 |
| Rotation range (90–2700°) | ✅ | 🟢 |
| FFB strength / damping / FFB filter (+ auto) | ✅ | 🟢 |
| TrueForce intensity / sensitivity / brake-force | ✅ | 🟢 |
| LIGHTSYNC RGB LEDs | ✅ | 🟢 |
| Centre calibration | ✅ | 🟢 |
| Mode / profile switching | ✅ | 🟢 |

The RS50 is the development hardware, so its column is verified directly.
A real G PRO runs the **same `hidpp_dd_ff_*` code path** as an RS50 in G PRO
compatibility mode (which *is* verified), so it is expected to work; we
just do not have one to confirm against. **G920 / G923** keep working as
a drop-in through the inherited upstream HID++ `0x8123` FFB path, but the
RS50/G-PRO-specific `wheel_*` settings and TrueForce do not apply to them.

### Force-feedback effect types

All effects are routed to the wheel's single direct-drive motor
(software-emulated on top of its constant-force endpoint), verified with
`fftest`, the in-tree `tests/ff_matrix_test`, and in-game:

| Effect | Notes |
|---|---|
| `FF_CONSTANT` | Direct torque (the steering/centring force). |
| `FF_SPRING` | Use this for auto-centring. Synthetic damping (`wheel_spring_damping`) keeps stiff springs stable on the direct-drive motor. |
| `FF_DAMPER`, `FF_FRICTION`, `FF_INERTIA` | Condition effects, sampled from live wheel motion. |
| `FF_RAMP` | Linear force ramp. |
| `FF_PERIODIC` | sine / square / triangle / saw-up / saw-down. 20 Hz and faster ride the TrueForce texture channel by default (`wheel_texture_route`). |
| `FF_RUMBLE` | Streams on the TrueForce texture channel by default, so it vibrates the rim without shaking the steering axis. |
| `FF_GAIN` | Global force scaling. |
| `FF_AUTOCENTER` | Driver-emulated damped centring spring (also via the `autocenter` sysfs); games can disable it per session as usual. |

### Verified game support

- **Assetto Corsa Competizione** and **Assetto Corsa EVO** — verified
  end-to-end under Proton: **steering, full FFB, and TrueForce all at the
  same time**, with `PROTON_ENABLE_HIDRAW=1` and Steam Input disabled.
- Other Logitech-SDK sims (Le Mans Ultimate, AMS2, Assetto Corsa,
  rFactor 2, iRacing) share the same SDK and are expected to work; not
  yet confirmed.

> **Caveats:**
> - Some sims (AC EVO observed) reset the wheel's rotation range to
>   **90° once at session start**, via the game's own SDK path (a
>   TrueForce operating-range packet, usbmon-verified). The driver
>   detects this within 20 seconds and **restores your range
>   automatically** (`wheel_range_restore`, default on, heavily
>   safety-gated - see `docs/SYSFS_API.md`), logging both the
>   external change and the restore in dmesg. Also check the game's
>   own steering-rotation setting (AC EVO: "Steering lock") - once
>   touched and re-applied, the game pushes its configured value
>   itself. The in-game FFB gain is the master force control;
>   `wheel_strength` is the wheel-side multiplier.
> - AC EVO's **map-load centring force** has once been observed ringing
>   the wheel into its over-torque failsafe (the base shuts itself off;
>   power-cycle to recover). Instrumented sessions show AC EVO drives
>   all its forces through the Logitech SDK stream rather than the
>   kernel FFB path, so this is game/SDK-side behaviour; if it occurs,
>   lower the in-game FFB gain or `wheel_strength`. Keep hands clear
>   during map loads as a precaution.
> - If a game stops seeing the wheel (dead bindings, hung map loads)
>   after the driver was reloaded while Steam ran: **restart Steam
>   fully** - its device list goes stale across driver reloads.

### State of the driver (v0.10.0, 2026-07-03)

An honest calibration of what "supported" means today:

**Verified on hardware** (one RS50, G PRO compatibility mode, plus
extensive USB-capture cross-checks): everything marked with a check in
the matrix above, including full-lap gameplay in ACC and AC EVO with
simultaneous steering FFB and TrueForce.

**Expected but awaiting independent confirmation:**
- A **real G PRO Racing Wheel** runs the identical code path and
  protocol (byte-verified against contributor captures), but no
  field report has confirmed the new texture routing or range
  auto-restore on one yet. If you have a G PRO, your report is the
  most valuable thing you can contribute (issue #8).
- **Other Logitech-SDK sims** (Le Mans Ultimate, AMS2, Assetto Corsa,
  rFactor 2, iRacing) link against the same SDK as the two verified
  titles and should behave identically. One confirmation each is all
  it takes to move them into the verified column.

**Not there yet, compared to G Hub on Windows:**
- No GUI; configuration is sysfs (and partially Oversteer).
- Setup is manual: DKMS module, per-prefix TrueForce shim install,
  `PROTON_ENABLE_HIDRAW=1`, Steam Input off. Documented, not
  automatic.
- No firmware updates (SecureDFU untouched by design), no onboard
  profile *editing* (slots can be selected and their names read, not
  written), no per-game automatic profiles, and the response-curve /
  Sensitivity upload feature is protocol-mapped but not implemented.

## Button Mapping

![RS50 Button Layout](rs-wheel-hub-button-layout.png)

Buttons use sequential indices matching Windows DirectInput for cross-platform compatibility.

| Index | Button |
|-------|--------|
| 0 | A |
| 1 | X |
| 2 | B |
| 3 | Y |
| 4 | Right Paddle / Gear Right |
| 5 | Left Paddle / Gear Left |
| 6 | RT (Right Trigger) |
| 7 | LT (Left Trigger) |
| 8 | Camera/View |
| 9 | Menu |
| 10 | RSB (Right Stick) |
| 11 | LSB (Left Stick) |
| 21 | Right Encoder CW |
| 22 | Right Encoder CCW |
| 23 | Right Encoder Push |
| 24 | Left Encoder CW |
| 25 | Left Encoder CCW |
| 26 | Left Encoder Push |
| 27 | G1 (Logitech logo) |

D-pad reports as hat switch (ABS_HAT0X / ABS_HAT0Y).

Note: Indices 12-20 are gaps in the HID descriptor (unused).

## Installation

This is the path most users want: from a fresh clone to a working
wheel with full force feedback and (optionally) TrueForce in
SDK-aware sims under Proton.

### Prerequisites

- Linux kernel 5.15 or newer (tested through 7.1)
- Kernel headers for the running kernel
- `dkms`, `make`, `gcc` or `clang`
- **For TrueForce in Proton sims only**: `winegcc` (ships with Wine
  on most distros), and a copy of Logitech G HUB on Windows from
  which to source four signed SDK DLLs. Skip these if you only want
  standard force feedback. You will still get full FFB
  (constant force, spring, damper, periodic, etc.) in every game.

There are no userspace components you need to compile by hand. The
install script below handles the kernel module, the udev rule, and
the SDK DLL installation into your wine prefixes.

### Steps

1. **Clone the repo.**
   ```bash
   git clone https://github.com/mescon/logitech-trueforce-linux-driver.git
   cd logitech-trueforce-linux-driver
   ```

2. **(TrueForce only) Stage the Logitech SDK DLLs.** These are
   Logitech's own Authenticode-signed binaries. We do **not**
   redistribute them; you must supply your own copies from a
   Logitech G HUB installation on Windows (or G HUB unpacked into a
   throwaway wine prefix on Linux). Place exactly these four files
   at exactly these paths inside the repo:
   ```
   sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
   sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
   sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
   sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll
   ```
   See `sdk/README.md` for more detail. If any are missing the SDK
   DLL install step in (3) prints a warning and is skipped; the
   kernel driver itself still installs fine.

3. **Run the installer.**
   ```bash
   sudo ./tools/dkms-update.sh
   ```
   This:
   - Registers the source under `/usr/src/hid-logitech-hidpp-1.0/`
     and runs `dkms install` so the kernel module rebuilds
     automatically on every kernel update.
   - Installs `udev/70-logitech-trueforce.rules`, which hands `wheel_*`
     sysfs and the wheel's hidraw nodes to your session user (no
     `sudo` needed for Oversteer or `echo > wheel_*`).
   - If `winegcc` is available **and** the SDK DLLs are staged
     (step 2), copies the four DLLs into every Steam wine prefix
     it finds and registers the two CLSIDs in each prefix's
     `system.reg` so SDK-aware sims (ACC, Le Mans Ultimate, AMS2,
     AC, rF2, iRacing) load TrueForce.

4. **Blacklist conflicting in-tree drivers** (one-time):
   ```bash
   printf "blacklist hid-logitech-hidpp\nblacklist hid-logitech\n" | sudo tee /etc/modprobe.d/blacklist-hid-logitech-hidpp.conf
   sudo depmod -a
   ```
   `hid-logitech-hidpp` is the upstream version without RS50 / G PRO
   compat support, which our module replaces. `hid-logitech` (lg4ff)
   is for older wheels (G25/G27/G29) and matches the RS50, sending
   incorrect FFB commands that crash the wheel firmware on
   reconnect. Blacklisting `hid-logitech` does **not** affect G920
   or G923 (those use HID++, handled by our driver), but if you
   also use a G25/G27/G29 you will lose lg4ff FFB on that older
   wheel.

5. **Reload the module and replug the wheel.**

   > **Safety**: the RS50 can produce up to 8 Nm and may rotate
   > under power. Hold the rim or keep clear whenever you load or
   > reload the driver, replug the wheel, or switch profiles.

   ```bash
   sudo modprobe -r hid-logitech-hidpp 2>/dev/null
   sudo modprobe hid-logitech-hidpp
   ```
   Physically unplug then replug the wheel's USB cable (or reboot).
   `dmesg | grep -i "force feedback"` should show
   `Force feedback initialized`, prefixed with your wheel's model tag:
   `RS50 (native):`, `RS50 (G PRO compatibility mode):`, or `G PRO:`.

6. **Smoke test.**
   ```bash
   fftest /dev/input/by-id/*Logitech*event-joystick
   ```
   The wheel should respond to each effect in turn. `fftest` comes from
   the linuxconsoletools project: it's in the `linuxconsole` package on
   Arch-based distros (Arch, CachyOS, SteamOS) and `linuxconsoletools`
   on most others.

For ACC + TrueForce specifically, see "Recipe: ACC + TrueForce on
RS50 or G PRO Racing Wheel" further down. Other SDK-aware sims
follow the same recipe.

### Updating after `git pull`

```bash
sudo ./tools/dkms-update.sh
```
Then reload as in step 5. A reboot is only needed on UEFI Secure
Boot systems if the MOK key needs re-enrollment.

### Adding TrueForce to a Wine prefix created later

For Steam games installed after step 3, or for non-Steam Wine
prefixes (Heroic, Lutris, bottled wine):
```bash
./tools/install-tf-shim.sh --all-steam            # every Steam prefix
./tools/install-tf-shim.sh --prefix /path/to/pfx  # a single prefix
```
Run as your normal user, not `sudo`.

### `input` group membership

Most desktop distros put interactive users in `input` automatically
via systemd-logind `uaccess`. If `echo > wheel_*` returns
`EACCES`:
```bash
sudo usermod -aG input "$USER"
# log out and back in
```

### Build without DKMS (developers)

```bash
cd mainline
make
sudo rmmod hid-logitech-hidpp 2>/dev/null
sudo insmod ./hid-logitech-hidpp.ko
```

## Recipe: SDK-aware sims (ACC, AC EVO, ...) on RS50 or G PRO

End-to-end verified against **Assetto Corsa Competizione** and
**Assetto Corsa EVO**: full FFB, TrueForce haptics, and complete
button / paddle / encoder binding, all delivered by Logitech's own
signed SDK DLLs running unmodified under Proton. The same recipe is
expected to work for the other Logitech-SDK-aware sims (Le Mans
Ultimate, AMS2, Assetto Corsa, rFactor 2, iRacing). The recipe
applies to both the RS50 and the G PRO Racing
Wheel for Xbox/PC. Step 1 is RS50-only.

1. **(RS50 only)** Switch the wheel into "G PRO compatibility" mode
   via the OLED menu. The wheel reboots and reappears as
   `046d:c272`, which is the PID ACC's TrueForce check accepts.
2. Set the wheel's steering angle. The compat-mode factory default
   is 90°, much too small to drive with. Two equivalent paths:
   - **From Linux (recommended)**: enter desktop mode and set the
     range live via sysfs:
     ```bash
     H=$(ls -d /sys/class/hidraw/*/device/wheel_range | head -1 | xargs dirname)
     echo 0   > "$H/wheel_profile"   # desktop mode
     echo 540 > "$H/wheel_range"     # 540 degrees lock-to-lock
     ```
   - From the OLED: edit the active onboard profile's stored
     steering angle. Each onboard profile carries its own.
3. Stage the four Logitech-signed SDK DLLs under `sdk/Logi/` in the
   repo. We do not redistribute these; copy them out of a Logitech
   G HUB install on Windows (or G HUB unpacked into a throwaway
   wine prefix on Linux):
   ```
   sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
   sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
   sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
   sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll
   ```
4. Install the DLLs into your Wine prefixes (idempotent, run as
   your normal user, **not** sudo):
   ```bash
   ./tools/install-tf-shim.sh --all-steam
   ```
5. Steam launch options: `PROTON_ENABLE_HIDRAW=1 %command%`.
   Required: the TF SDK only sees the wheel through hidraw nodes
   that Wine exposes when this is set.
6. In the game, Settings → Controls → load the "PRO Racing Wheel
   for Xbox/PC" preset (or the closest match), bind axes and
   buttons, then set the in-game Wheel Rotation / steering lock to
   match the angle you set in step 2. If a gamepad is plugged in,
   unplug or disable it during binding so the game's auto-bind
   does not pick it up over the wheel.

Other Logitech-SDK-aware sims (Le Mans Ultimate, AMS2, Assetto
Corsa, rFactor 2, iRacing) follow the same recipe.

## Compat-mode behavior

A few things look wrong but are firmware-side defaults that match
Windows G Hub on the same wheel. Listed here so you do not chase
them as Linux issues:

- **The wheel "wants to stay centered"** when no game is sending
  FFB. The firmware applies its own self-centering spring whenever
  it is idle. There is no known host command to disable it. Once a
  game (or the TF SDK) starts driving FFB, that overrides it.
- **Default steering angle is 90°** out of the factory in compat
  mode, not 1080°. Set it from Linux via `wheel_profile=0` then
  `wheel_range=<degrees>`, or from the OLED by editing the active
  onboard profile's stored steering angle. Some games (Assetto Corsa
  EVO has been reported) appear to reset the wheel back to 90° on
  launch every time even when the user has set a wider range
  beforehand. Inspecting the SDK's HID++ traffic shows the games
  themselves do **not** write any range-set command - they never
  even query the range feature - so the reset is wheel-firmware-side,
  triggered by something in the game's open / acquire / DInput claim
  path. If you hit this, set the angle via the OLED on the wheel
  base after launching the game (the OLED change takes effect live)
  and pin the in-game wheel range to the same value so the firmware
  has no reason to re-clamp.
- **Mode and slot semantics**:
  - Writing `0` to `wheel_profile` enters desktop mode (verified
    against motor behaviour: subsequent live SETs to `wheel_range`,
    `wheel_strength`, `wheel_trueforce`, `wheel_damping`, and
    `wheel_ffb_filter` take effect on the motor immediately).
  - Writing `1..5` to `wheel_profile` is intended to select onboard
    slot N but the byte encoding our driver currently sends is wrong
    in compat mode; it triggers a profile-broadcast cascade and the
    wheel can land on an unintended slot. Use the OLED menu to
    select an onboard slot until that path is fixed.
- **`wheel_brake_force`, `wheel_sensitivity`, `wheel_ffb_filter_auto`**
  return `-EOPNOTSUPP` on this firmware regardless of mode.
  Configure them via Windows G Hub or the wheel's OLED menu.

## Usage

### Test Force Feedback

```bash
# Find your device
ls /dev/input/by-id/ | grep -i logi

# Test FFB (requires linuxconsole package)
fftest /dev/input/by-id/usb-Logitech_RS50*-event-joystick
```

### Configure Settings via sysfs

Settings are exposed at `/sys/class/hidraw/hidrawX/device/` (where X varies by system).

```bash
# Find your wheel's hidraw device
WHEEL_DEV=$(ls -d /sys/class/hidraw/*/device/wheel_range 2>/dev/null | head -1 | xargs dirname)
echo "Wheel found at: $WHEEL_DEV"

# Example: Set rotation to 900 degrees
echo 900 | sudo tee $WHEEL_DEV/wheel_range

# Example: Set FFB strength to 80%
echo 80 | sudo tee $WHEEL_DEV/wheel_strength

# Example: Set LED slot to CUSTOM 1 (slot 0)
echo 0 | sudo tee $WHEEL_DEV/wheel_led_slot

# Example: Set custom rainbow colors for all 10 LEDs (hex RGB triplets)
echo "ff0000 ff7f00 ffff00 00ff00 00ffff 0000ff 7f00ff ff00ff ff0080 ffffff" | sudo tee $WHEEL_DEV/wheel_led_colors
echo 1 | sudo tee $WHEEL_DEV/wheel_led_apply
```

### Available sysfs Attributes

**Mode and Profile:**

| Attribute | Range | Description |
|-----------|-------|-------------|
| `wheel_mode` | desktop/onboard | Operating mode (Desktop or Onboard profiles) |
| `wheel_profile` | 0-5 | Active profile (0=Desktop, 1-5=Onboard profiles) |

**Force Feedback:**

| Attribute | Range | Description |
|-----------|-------|-------------|
| `wheel_range` | 90-2700 | Rotation range in degrees |
| `wheel_strength` | 0-100 | FFB strength percentage |
| `wheel_damping` | 0-100 | Damping percentage |
| `wheel_trueforce` | 0-100 | TRUEFORCE audio-haptic level |
| `wheel_sensitivity` | 0-100 | Wheel sensitivity (Desktop mode only) |
| `wheel_brake_force` | 0-100 | Brake pedal load cell threshold (Onboard mode only) |
| `wheel_ffb_filter` | 1-15 | FFB smoothing level |
| `wheel_ffb_filter_auto` | 0-1 | Auto FFB filter (0=off, 1=on) |
| `wheel_calibrate` | 0-65535 (write-only) | Raw encoder value to adopt as the new centre (RS50 and G Pro). |

**LIGHTSYNC LED Control:**

| Attribute | Range | Description |
|-----------|-------|-------------|
| `wheel_led_slot` | 0-4 | Active custom slot (CUSTOM 1-5) |
| `wheel_led_slot_name` | string | Slot name (max 8 chars, stored on device) |
| `wheel_led_slot_brightness` | 0-100 | Per-slot brightness (applied when slot activated) |
| `wheel_led_direction` | 0-3 | Animation direction (0=L→R, 1=R→L, 2=In→Out, 3=Out→In) |
| `wheel_led_colors` | hex | 10 space-separated RGB hex values (LED1-LED10) |
| `wheel_led_effect` | 1-5 | LED effect (1-4 = animated modes, 5 = static/custom slot colors) |
| `wheel_led_brightness` | 0-100 | Global LED brightness percentage |
| `wheel_led_apply` | (write) | Apply current slot config to device |

**Pedal Configuration:**

| Attribute | Range | Description |
|-----------|-------|-------------|
| `wheel_combined_pedals` | 0-1 | Combined pedals mode |
| `wheel_throttle_curve` | 0-2 | Throttle response curve (0=linear, 1=low sens, 2=high sens) |
| `wheel_brake_curve` | 0-2 | Brake response curve |
| `wheel_clutch_curve` | 0-2 | Clutch response curve |
| `wheel_throttle_deadzone` | "L U" | Throttle deadzone (lower% upper%) |
| `wheel_brake_deadzone` | "L U" | Brake deadzone |
| `wheel_clutch_deadzone` | "L U" | Clutch deadzone |

See `docs/SYSFS_API.md` for complete API documentation with examples.

### Oversteer Compatibility

The driver exposes the standard wheel attribute set (new-lg4ff
names and scales) for [Oversteer](https://github.com/berarma/oversteer)
compatibility, all verified through Oversteer's own code against the
live wheel:
- `range` - rotation range (up to 2700°)
- `gain` - FFB strength (raw 0-65535 scale)
- `autocenter` - driver-emulated damped centring spring (raw 0-65535)
- `spring_level` / `damper_level` / `friction_level` - per-effect-class
  output scales (0-100)
- `combine_pedals` - combined pedals mode

**Note:** Oversteer requires a patch for full support (native RS50
detection, and attribute discovery for all three PIDs - stock
Oversteer looks for the settings on the joystick interface, but this
driver exposes them on the HID++ sibling interface). The patch ships
in this repo and applies cleanly to current Oversteer master; the
full round trip (detect, read settings, set range) is verified
against a live wheel as of 2026-07-03. Upstreaming is planned; until
merged, apply it manually.

The patch (`oversteer-logitech-trueforce.patch`) adds:
- RS50 device detection (USB ID `046d:c276`)
- 2700° rotation range support (range slider marks at 1800/2700)
- Correct pedal axis mapping
- udev permissions for the full settings set (`gain`, `autocenter`,
  `spring_level`/`damper_level`/`friction_level`, `combine_pedals`) on
  the RS50 **and both G PRO variants** (`c268`/`c272`) - stock
  Oversteer only unlocks `range` for the G PRO, so without the patch
  the other controls stay greyed out even though this driver provides
  them

#### Applying the Patch

**Option 1: System package / pip install**

```bash
# Find where Oversteer is installed
python3 -c "import oversteer; print(oversteer.__file__)"
# Usually: /usr/lib/python3.x/site-packages/oversteer/__init__.py

# Apply patch (adjust path as needed)
cd /usr/lib/python3.x/site-packages/
sudo patch -p1 < /path/to/oversteer-logitech-trueforce.patch
```

**Option 2: From git source**

```bash
git clone https://github.com/berarma/oversteer.git
cd oversteer
git apply /path/to/oversteer-logitech-trueforce.patch
sudo pip install .
```

**Option 3: Flatpak**

Flatpak apps are sandboxed, so you need to extract, patch, and reinstall:

```bash
# Export the installed Flatpak to a bundle
flatpak build-bundle ~/.local/share/flatpak/repo oversteer.flatpak \
  io.github.berarma.Oversteer

# Unfortunately, Flatpak bundles can't be easily patched.
# For Flatpak users, the recommended approach is to:
# 1. Uninstall the Flatpak version
flatpak uninstall io.github.berarma.Oversteer

# 2. Install from source with the patch applied (Option 2 above)

# 3. Or wait for the upstream patch to be merged and Flatpak updated
```

#### udev Rule (Required for non-root access)

Create `/etc/udev/rules.d/99-oversteer-rs50.rules`:

```
SUBSYSTEM=="usb", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c276", MODE="0666", TAG+="uaccess"
```

Then reload:
```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Documentation

In-repo references for users and contributors:

- [`docs/SYSFS_API.md`](docs/SYSFS_API.md) - every `wheel_*` sysfs
  attribute, with examples and per-mode availability (native vs.
  compat).
- [`docs/PROTOCOL_SPECIFICATION.md`](docs/PROTOCOL_SPECIFICATION.md) -
  HID++ feature catalog for both native and compat modes, the
  dedicated-endpoint FFB protocol, and the G PRO compat-mode
  feature decoding.
- [`docs/TRUEFORCE_PROTOCOL.md`](docs/TRUEFORCE_PROTOCOL.md) -
  interface-2 audio-haptic stream layout (init sequence, sample
  framing, gain/damping commands).
- [`sdk/README.md`](sdk/README.md) - inventory of Logitech's Windows
  SDK artifacts we reference, plus the DLL-staging layout
  consumed by `tools/install-tf-shim.sh`.

## Userspace components

`userspace/libtrueforce/` is a native-Linux C reimplementation of
Logitech's TrueForce SDK. **You do not need it for the ACC + TF
recipe above** - that path runs Logitech's own signed DLLs through
Wine, which talk directly to our kernel driver. libtrueforce exists
for native-Linux applications that want to drive TrueForce without
going through Wine (for example a telemetry-driven haptic generator
or a custom test rig). It has its own README and tests under
`userspace/libtrueforce/`. Nothing in `userspace/` is built or
installed by the regular install flow.

## Game Compatibility

The driver works with any game that supports Linux force feedback:

| Game | Status | Notes |
|------|--------|-------|
| **Native Linux** | ✓ | F1, Dirt Rally 2.0, Euro Truck Simulator 2 |
| **Proton/Steam** | ✓ | Assetto Corsa, ACC, iRacing, etc. |
| **Wine** | ✓ | Most racing games via Proton |

Games detect the wheel as a standard Linux joystick with FF support. No special configuration needed beyond setting up controls in-game.

### Proton Tips

- Enable "Steam Input" → "Gamepad with Joystick Trackpad" for some games
- Some games may need `SDL_JOYSTICK_DEVICE=/dev/input/eventX` environment variable

### inject_pid module parameter

The driver carries an experimental kernel-side path that injects a
USB HID PID Page 0x0F output collection into interface 0's
descriptor and translates the resulting DirectInput PID FFB writes
into our evdev FFB pipeline. It exists for racing games that have
**no** Logitech SDK integration and rely on standard DInput PID
force feedback (older sims, indie games, fftest-style standalone
tools). For all SDK-aware sims listed above it is unused, because
the SDK bypasses DInput FFB entirely.

Default: `inject_pid=0` (off). The two non-zero values:

```bash
sudo modprobe -r hid_logitech_hidpp
# Dry-run: inject the descriptor and intercept PID output reports,
# but do NOT actuate the wheel - logs every intercepted report.
# Use this first to confirm the game is hitting our shim.
sudo modprobe hid_logitech_hidpp inject_pid=1

# Actuate: descriptor + intercept + drive the wheel. Bench-tested
# only; not yet verified end-to-end against a real non-SDK game.
sudo modprobe hid_logitech_hidpp inject_pid=2
```

This path is intended for **Proton's default joystick layer**
(without `PROTON_ENABLE_HIDRAW`). Setting `PROTON_ENABLE_HIDRAW=1`
is **not** required and is unrelated; it routes a different game
class (the SDK-aware sims covered by the recipe above) and does
not interact with `inject_pid`.

## Technical Details

The RS50 is a multi-interface USB device:
- **Interface 0**: Joystick input (30-byte reports) - No HID++ support
- **Interface 1**: HID++ 4.2 protocol (configuration, settings, feature discovery)
- **Interface 2**: Force feedback output (64-byte reports on endpoint 0x03)

### Architecture Difference: RS50 vs G920/G923

| Aspect | G920/G923 (Belt-driven) | RS50 (Direct-drive) |
|--------|-------------------------|---------------------|
| FFB Protocol | HID++ Feature 0x8123 | Dedicated USB endpoint |
| FFB Commands | Via HID++ FAP messages | Raw HID output reports (01 XX) |
| Interface Layout | Unified | 3 separate interfaces |
| Max Rotation | 900° | 2700° |

**Critical Implementation Detail:** The RS50 driver must initialize FFB only on Interface 1 (HID++), not Interface 0 (joystick). Interface 0 lacks HID++ support and attempting FFB initialization there causes joystick input to fail. The driver uses `HIDPP_QUIRK_DD_FFB` to differentiate from the standard G920 code path.

See `docs/PROTOCOL_SPECIFICATION.md` for complete protocol documentation.

## Troubleshooting

### "Invalid code 768" messages during boot

These come from the HID descriptor declaring more buttons than
physically exist. **This driver filters them** (see `rs50_input_mapping`)
so they never reach userspace - which means if you *do* see them, the
wheel is being handled by `hid-generic`, not this driver. That happens
when the wheel enumerates before the module loads. See "Wheel has no FFB
/ no `wheel_*` (stuck on hid-generic)" below.

### Wheel has no FFB / no `wheel_*` (stuck on hid-generic)

If the wheel works as a plain joystick but has no force feedback and no
`wheel_*` sysfs (and you see "Invalid code 768" in dmesg), `hid-generic`
claimed it before this module was loaded. Fix it with:

```bash
sudo ./tools/rebind-wheel.sh
```

This loads the module and rebinds every wheel interface from
`hid-generic` to this driver. If it reports the bind failed, the
in-kernel `hid-logitech-hidpp` is loaded instead of this fork - run
`sudo ./tools/dkms-update.sh` so the DKMS build shadows it, then retry.

### FFB not working

1. Verify the driver is *bound to the wheel*, not just loaded:
   `ls /sys/class/hidraw/*/device/wheel_range` should list a path. If it
   does not, see "stuck on hid-generic" above.
2. Check dmesg for errors: `dmesg | grep -iE 'rs50|g pro'`
3. Ensure you're testing with a game/app that supports FFB

### FFB "pulls the wrong way" / wheel feels unstable under Wine/Proton

If a racing game feels like the wheel wants to *amplify* your steering
input instead of pushing back toward centre ("tips over" when nudged,
no self-centering when released), the `FF_CONSTANT` sign compensation
is probably in the wrong state for your app.

Wine and Proton's DirectInput-to-evdev translation lands
`FF_CONSTANT` at the driver with the sign inverted relative to what
native Linux evdev apps produce (this has been empirically confirmed
against Assetto Corsa Competizione; we have not pinned down the
exact Wine source location). The driver compensates by default, so
Wine/Proton games feel right out of the box. Native-evdev tools
(`fftest`, `ffcfstress`, games using SDL's FF path directly, and
anyone uploading via raw EVIOCSFF) see that compensation as an
unwanted flip and will feel inverted.

Toggle via sysfs:

```bash
# Resolve the wheel's sysfs dir once (the attributes live on the HID++
# interface, not necessarily hidraw0; this finds the right one):
WHEEL_DEV=$(dirname "$(ls -d /sys/class/hidraw/*/device/wheel_range | head -1)")

# Default: invert (correct for Wine/Proton games)
echo 1 | sudo tee "$WHEEL_DEV/wheel_ffb_constant_sign"

# Pass-through (correct for fftest, SDL FF, custom evdev apps)
echo 0 | sudo tee "$WHEEL_DEV/wheel_ffb_constant_sign"
```

If `WHEEL_DEV` comes back empty, the driver is not bound to the wheel
(check `lsmod | grep hid_logitech_hidpp` and that the wheel is in PC
mode), or your build predates this attribute (it was added later in the
0.9 series) - pull latest and rebuild.

Only `FF_CONSTANT` is affected. SPRING, DAMPER, FRICTION, INERTIA,
RAMP, PERIODIC, and RUMBLE all feel identical at either toggle
value.

See `docs/SYSFS_API.md` for details, including the ongoing
investigation into where the flip actually lives.

### Settings not persisting

sysfs settings are volatile and reset on driver reload. For persistent settings, add commands to a udev rule or startup script.

### Wine/Proton: HIDRAW=0 vs HIDRAW=1

Wine's HID stack has two paths it can take for the wheel, and the
right one depends on the game:

- **`PROTON_ENABLE_HIDRAW=0`** (default): Wine routes the joystick
  interface via SDL. Suitable for native-Linux-style FFB games where
  no Logitech-specific SDK is involved - input flows cleanly and
  evdev FFB works through our driver.
- **`PROTON_ENABLE_HIDRAW=1`**: Wine exposes all wheel hidraw nodes
  to the Windows side. **Required for any game that uses the
  Logitech TrueForce SDK** (ACC, LMU, AMS2, AC, iRacing) - the SDK
  finds the wheel via Windows HID enumeration which only sees
  hidraw devices Wine has explicitly exposed.

If you see no FFB *and* the game's "wheel detection" or TrueForce
check says no Logitech wheel is present, you probably need
`PROTON_ENABLE_HIDRAW=1` plus the steps in the SDK-aware-sims
recipe above.

If the game just doesn't see any wheel at all (no FFB, ghost
inputs), Wine may be holding the device through a different
backend - the legacy fallback is to hide the wheel from Wine's
hidraw layer entirely:

```bash
# Steam launch options:
PROTON_ENABLE_HIDRAW=0 %command%

# Or globally hide the wheel from any Wine prefix:
echo 'SUBSYSTEM=="hidraw", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c276", MODE="0000"' | \
  sudo tee /etc/udev/rules.d/99-hide-rs50-from-wine.rules
sudo udevadm control --reload-rules
```

**Solution 2: Use SDL instead of Wine's dinput**

Some games work better with SDL's joystick handling:

```bash
# Steam launch options:
SDL_JOYSTICK_HIDAPI=0 %command%
```

**Solution 3: Check hidraw permissions**

If Oversteer or sysfs settings don't work, Wine may have grabbed the hidraw device:

```bash
# Find your wheel's hidraw device number
ls -la /sys/class/hidraw/*/device/wheel_range 2>/dev/null

# Check who has the device open (replace X with your hidraw number)
sudo lsof /dev/hidrawX

# If wine processes are listed, close them or use Solution 1
```

## Contributing

Contributions are welcome! There are several ways to help:

**Code contributions:** This driver is forked from [JacKeTUs/hid-logitech-hidpp](https://github.com/JacKeTUs/hid-logitech-hidpp) with RS50-specific additions. If your changes apply to other Logitech devices, please consider contributing upstream as well.

**Testing:** Try the driver and report issues. Include your kernel version, distribution, and any relevant dmesg output.

**USB captures:** If you own a Logitech wheel variant that isn't yet fully supported and want to help with reverse-engineering, open an issue and we will share the contributor capture tooling.

## License

GPL-2.0-only (same as the Linux kernel)

## Acknowledgments

- RS50 USB protocol reverse-engineered using Wireshark captures from G Hub on Windows
- Based on [JacKeTUs/hid-logitech-hidpp](https://github.com/JacKeTUs/hid-logitech-hidpp) which adds G Pro wheel support and improved FFB
- Upstream Linux kernel [hid-logitech-hidpp driver](https://github.com/torvalds/linux/blob/master/drivers/hid/hid-logitech-hidpp.c) by Benjamin Tissoires and contributors
- [Oversteer](https://github.com/berarma/oversteer) by Bernat Arlandis for the wheel configuration GUI
