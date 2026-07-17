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

> **Note**: this project was renamed from `logitech-rs50-linux-driver`
> because the driver covers the whole TrueForce direct-drive family, not
> just the RS50. Old GitHub links and clones redirect automatically. The
> kernel module is `hid-logitech-dd`; if you previously installed the
> full fork as `hid-logitech-hidpp`, the installer migrates you across.

You get the full evdev force-feedback suite (constant, spring, damper,
friction, inertia, periodic, ramp, rumble, gain), all buttons,
encoders, paddles, hat switch, 16-bit pedal axes, and G Hub-equivalent
settings (rotation range, FFB strength / damping / TRUEFORCE / filter,
pedal and handbrake curves, LIGHTSYNC LEDs). You configure the wheel with
**logi-dd**, an included terminal settings app with a G HUB-style curve
editor (`userspace/logi-dd/`), or directly through sysfs. **TrueForce
haptics work in supported sims under Proton** via Logitech's own signed
SDK DLLs - see the recipe below.

This is a fork of the in-kernel `hid-logitech-hidpp` driver, **scoped to
only the Logitech direct-drive wheels** - the RS50 and G PRO, across their
three USB IDs (`c276` RS50 native, `c272` G PRO Xbox/PC which the RS50 also
uses in compatibility mode, `c268` G PRO PS/PC). It installs as a
separate module, `hid-logitech-dd`, that binds *only* those wheels and
runs alongside the in-tree `hid-logitech-hidpp`, which keeps handling all
your other Logitech HID++ devices (mice, keyboards, G29 / G920 / G923
wheels, etc.) at its current, continuously-maintained version. No
blacklist, no shadowing - this driver owns only the hardware it improves.

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
- Per-pedal response curves, sensitivity and deadzones; combined-pedals mode
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
**Assetto Corsa Competizione** and **Assetto Corsa EVO** - full FFB,
TrueForce haptics, and complete button / paddle / encoder binding
all working through Logitech's own signed SDK DLLs running
unmodified under Proton. The same setup is expected to work for
Le Mans Ultimate, AMS2, Assetto Corsa (the original 2014 game),
rFactor 2, and iRacing - they all use the same SDK. The recipe below
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
tested on that exact model) · 🟡 implemented from captures, needs an
owner to validate · - not applicable.

| Capability | RS50 (`c276` native / `c272` compat) | G PRO Racing Wheel (`c272` Xbox-PC / `c268` PS-PC) |
|---|:--:|:--:|
| Steering, pedals, buttons, 8-way D-pad | ✅ | 🟢 |
| Force feedback (full evdev effect suite) | ✅ | 🟢 |
| TrueForce haptics (Proton + signed SDK) | ✅ | 🟢 |
| TrueForce texture routing for evdev effects (`wheel_texture_route`) | ✅ | 🟢 |
| Rotation range (90-2700°) | ✅ | 🟢 |
| FFB strength / damping / FFB filter (+ auto) | ✅ | 🟢 |
| TrueForce intensity / sensitivity / brake-force | ✅ | 🟢 |
| Pedal response curves / sensitivity / deadzones | ✅ | 🟢 |
| Combined-pedals mode (`wheel_combined_pedals`) | ✅ | 🟢 |
| RS Shifter & Handbrake (shift, digital + analog handbrake) | ✅ | 🟢 |
| LIGHTSYNC RGB LEDs (RS50 faceplate strip) | ✅ | - |
| Rev-light level (`wheel_rev_level`, real G PRO rim) | - | 🟡 needs a tester |
| Centre calibration | ✅ | 🟢 |
| Mode / profile switching | ✅ | 🟢 |

The RS50 is the development hardware, so its column is verified directly
in both native (`046d:c276`) and G PRO compatibility (`046d:c272`) modes,
including SDK game TrueForce under Proton in each. A real G PRO runs the
**same `hidpp_dd_ff_*` code path** as an RS50 in G PRO compatibility mode.
Its HID++ configuration protocol is now confirmed against real G PRO G Hub
captures ([issue #8](https://github.com/mescon/logitech-trueforce-linux-driver/issues/8)):
every wheel-config feature matches, differing only by a feature-index shift
the driver resolves at runtime. Those captures did not include the
force-feedback / TrueForce stream itself, so G PRO FFB and TrueForce are
expected to work but not yet verified end to end.
**G920 / G923** are handled by the
in-tree `hid-logitech-hidpp` driver (this scoped fork no longer claims
them); their standard HID++ FFB is unaffected, and the RS50/G-PRO-specific
`wheel_*` settings do not apply to them. The **G923**
speaks the same TrueForce stream protocol as the DD wheels (confirmed on
Windows by the TF4ALL project), and the udev rule now grants it hidraw
access so the SDK DLLs can reach it under Proton - TrueForce on a G923
is plausible but unverified on Linux; testers wanted in
[issue #27](https://github.com/mescon/logitech-trueforce-linux-driver/issues/27).

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

- **Assetto Corsa Competizione** and **Assetto Corsa EVO** - verified
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

### State of the driver

**Verified on RS50 hardware** (native and G PRO compatibility modes,
plus extensive USB captures): everything checked in the matrix above,
including full-lap ACC and AC EVO with simultaneous FFB and TrueForce.

**Expected, needs a field report:** a real G PRO (identical code path,
[issue #8](https://github.com/mescon/logitech-trueforce-linux-driver/issues/8))
and the other Logitech-SDK sims (Le Mans Ultimate, AMS2, Assetto Corsa,
rFactor 2, iRacing) - one confirmation each moves them into the verified
column.

**Not yet, vs G Hub on Windows:** no GUI (settings are sysfs, partly
Oversteer); two per-game Steam settings stay manual
(`PROTON_ENABLE_HIDRAW=1`, Steam Input off); no firmware updates and no
onboard-profile editing (slots select and read, not write).

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

**Short version** - one command covers steps 1-5 below (DKMS build,
migration off any old full-fork install, udev rule, module load,
TrueForce shim if the SDK DLLs are staged), and `doctor` verifies every
layer:

```bash
sudo ./tools/setup.sh        # install / update everything
./tools/setup.sh doctor      # health-check all layers, change nothing
```

The numbered steps below are what it does, kept for transparency and
for anyone who prefers manual control.

> **Prefer a native package?** Arch (AUR), Fedora/Nobara (COPR akmod),
> openSUSE (OBS), and Debian/Ubuntu (`.deb` from
> [Releases](https://github.com/mescon/logitech-trueforce-linux-driver/releases))
> all have one, and they rebuild on kernel upgrades. See the distro table in
> [Getting Started, step 1](docs/GETTING_STARTED.md#1-install-the-driver).

> **Atomic / immutable distros (Bazzite, Silverblue, Kinoite):** DKMS does
> not work on rpm-ostree systems. Build the module as a static kmod in a
> toolbox and layer it with `rpm-ostree install` instead - see
> [section 1a of the Getting Started guide](docs/GETTING_STARTED.md#1a-atomic--immutable-distros-bazzite-silverblue-kinoite).

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
   - Registers the source under `/usr/src/logitech-trueforce-1.0/`
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

4. **Reload the module and replug the wheel.**

   No blacklist is needed: `hid-logitech-dd` claims only the three
   direct-drive wheels, which the in-tree drivers do not, so the two
   coexist without conflict.

   > **Safety**: the RS50 can produce up to 8 Nm and may rotate
   > under power. Hold the rim or keep clear whenever you load or
   > reload the driver, replug the wheel, or switch profiles.

   ```bash
   sudo modprobe -r hid-logitech-dd 2>/dev/null
   sudo modprobe hid-logitech-dd
   ```
   Physically unplug then replug the wheel's USB cable (or reboot).
   `dmesg | grep -i "force feedback"` should show
   `Force feedback initialized`, prefixed with your wheel's model tag:
   `RS50 (native):`, `RS50 (G PRO compatibility mode):`, or `G PRO:`.

5. **Smoke test.**
   ```bash
   fftest /dev/input/by-id/*Logitech*event-joystick
   ```
   The wheel should respond to each effect in turn. `fftest` comes from
   the linuxconsoletools project: it's in the `linuxconsole` package on
   Arch-based distros (Arch, CachyOS, SteamOS) and `linuxconsoletools`
   on most others.

For ACC + TrueForce specifically, see "Recipe: SDK-aware sims (ACC,
AC EVO, ...) on RS50 or G PRO" further down. Other SDK-aware sims
follow the same recipe.

### Updating after `git pull`

```bash
sudo ./tools/dkms-update.sh
```
Then reload as in step 4. A reboot is only needed on UEFI Secure
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
sudo rmmod hid-logitech-dd 2>/dev/null
sudo insmod ./hid-logitech-dd.ko
```

## Recipe: SDK-aware sims (ACC, AC EVO, ...) on RS50 or G PRO

End-to-end verified against **Assetto Corsa Competizione** and
**Assetto Corsa EVO**: full FFB, TrueForce haptics, and complete
button / paddle / encoder binding, all delivered by Logitech's own
signed SDK DLLs running unmodified under Proton. The same recipe is
expected to work for the other Logitech-SDK-aware sims (Le Mans
Ultimate, AMS2, Assetto Corsa, rFactor 2, iRacing). The recipe
applies to both the RS50 and the G PRO Racing
Wheel for Xbox/PC. Step 1 is RS50-only and optional.

1. **(RS50, optional)** You can switch the wheel into "G PRO
   compatibility" mode via the OLED menu (it reboots and reappears as
   `046d:c272`), but as of 2026-07-08 this is not required: the SDK
   also accepts the RS50's **native** PID `046d:c276`, verified
   end-to-end in AC EVO (usbmon-confirmed TrueForce stream). Native
   mode additionally unlocks the full 2700 range. Use compat mode only
   as a fallback if a specific game's SDK build does not recognise the
   native PID.
2. Set the wheel's steering angle. The default range can be very small
   (compat mode's factory default is 90°), much too small to drive
   with. Two equivalent paths:
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
Windows G Hub, not Linux bugs:

- **The wheel self-centers when idle.** The firmware applies its own
  centering spring whenever no game is driving FFB; a game (or the TF
  SDK) overrides it. No host command disables it.
- **Factory steering angle is 90° in compat mode.** Set it with
  `wheel_profile=0` then `wheel_range=<degrees>`, or via the OLED. Some
  games (e.g. AC EVO) reset it to 90° on launch (firmware-side, not a
  range command from the game); if so, set the angle on the OLED after
  launch and pin the in-game range to match.
- **`wheel_profile=0`** enters desktop mode, where live SETs to
  `wheel_range`, `wheel_strength`, `wheel_trueforce`, `wheel_damping`,
  and `wheel_ffb_filter` take effect immediately. Selecting onboard
  slots 1-5 via `wheel_profile` is unreliable in compat mode; use the
  OLED menu instead.
- **`wheel_brake_force`, `wheel_sensitivity`, `wheel_ffb_filter_auto`**
  return `-EOPNOTSUPP` on this firmware; configure via G Hub or the OLED.

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

**Pedals:**

The pedal unit applies a hardware response curve to each axis it reports (the
same `0x80A4` mechanism as the steering wheel; verified on an RS50). Each pedal
`<p>` in {`throttle`, `brake`, `clutch`} has three attributes, all writing the
one curve the axis holds (last write wins):

| Attribute | Values | Description |
|-----------|--------|-------------|
| `wheel_<p>_curve` | `reset` or `in:out` pairs | Full response curve (like `wheel_response_curve`) |
| `wheel_<p>_sensitivity` | 0-100 (50=linear) | G HUB sensitivity slider |
| `wheel_<p>_deadzone` | `"lower upper"` % | Dead travel at each end (sum ≤ 99) |
| `wheel_combined_pedals` | 0-1 | Merge throttle+brake into one axis (legacy games; desktop only) |

The `logi-dd` app has a G HUB-style point editor for these curves. See
`docs/SYSFS_API.md` for the complete reference with examples.

The **RS Shifter & Handbrake** accessory works with no extra setup: plugged
into the wheel base, its inputs ride the wheel's existing report (sequential
shift = paddle buttons, digital handbrake = a face button, analog handbrake =
an axis, `ABS_Z` on the RS50). The analog handbrake can be shaped like the
pedals via `wheel_handbrake_curve` / `wheel_handbrake_sensitivity`. See
`docs/SYSFS_API.md` for the full input mapping.

### Oversteer Compatibility

The driver exposes the standard new-lg4ff attribute set for
[Oversteer](https://github.com/berarma/oversteer): `range` (to 2700°),
`gain`, `autocenter`, and `spring_level`/`damper_level`/`friction_level`,
verified through Oversteer against a live wheel. Oversteer's `combine_pedals`
control is not wired to a driver attribute, so Oversteer hides it; combine the
pedals through the `wheel_combined_pedals` sysfs attribute (or logi-dd) instead.

Full support needs a patch (`oversteer-logitech-trueforce.patch`, in this
repo): it adds RS50 detection (`046d:c276`), the 2700° range, and finds
the settings on the HID++ sibling interface (stock Oversteer looks on the
joystick interface and only unlocks `range` for the G PRO, leaving the
other controls greyed out). It applies cleanly to current Oversteer
master; upstreaming is planned.

```bash
# pip / system install:
cd "$(python3 -c 'import oversteer,os; print(os.path.dirname(os.path.dirname(oversteer.__file__)))')"
sudo patch -p1 < /path/to/oversteer-logitech-trueforce.patch

# or from git source:
git clone https://github.com/berarma/oversteer.git && cd oversteer
git apply /path/to/oversteer-logitech-trueforce.patch && sudo pip install .
```

Flatpak Oversteer is sandboxed and can't be patched in place; install from
source instead. Non-root access to the settings is handled by the driver's
udev rule (installed by `setup.sh`).

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

`userspace/logi-dd/` is **logi-dd**, a terminal settings app for the wheel: a
native-Linux stand-in for the parts of G HUB that configure force feedback,
rotation range, LEDs, profiles and pedal/handbrake response curves. It reads and
writes the `wheel_*` sysfs attributes with typed, validated edits and a G HUB-
style point-list curve editor, so you do not have to `echo` values into sysfs by
hand. Build and run it with a Rust toolchain:

```bash
cd userspace/logi-dd && cargo build --release
./target/release/logi-dd            # needs your user in the `input` group
```

See [`userspace/logi-dd/README.md`](userspace/logi-dd/README.md) for features,
keys and permissions.

`userspace/logi-dd/crates/ffb-proxy/` builds **logi-ffb**, a DirectInput
force-feedback proxy. It presents a virtual force-feedback wheel that mirrors the
real wheel's input and forwards the DirectInput effects a game sends onto the
real wheel's own force feedback, so sims that drive FFB through DirectInput over
hidraw (see "DirectInput force feedback and `PROTON_ENABLE_HIDRAW`" below) get
force feedback that would otherwise be lost. Prepend it to a game command:

```bash
cd userspace/logi-dd && cargo build --release
logi-ffb <game command>      # or paste `logi-ffb %command%` into Steam launch options
```

See [`userspace/logi-dd/crates/ffb-proxy/README.md`](userspace/logi-dd/crates/ffb-proxy/README.md)
for how it works and its requirements.

`userspace/libtrueforce/` is a native-Linux C reimplementation of
Logitech's TrueForce SDK. **You do not need it for the ACC + TF
recipe above** - that path runs Logitech's own signed DLLs through
Wine, which talk directly to our kernel driver. libtrueforce exists
for native-Linux applications that want to drive TrueForce without
going through Wine (for example a telemetry-driven haptic generator
or a custom test rig). It has its own README and tests under
`userspace/libtrueforce/`. The distribution packages (AUR, `.deb`,
COPR, OBS) build and install both **logi-dd** and **logi-ffb** to
`/usr/bin`; libtrueforce is not part of the regular install flow.

## Game compatibility

Games see the wheel as a standard Linux joystick with force feedback; no
special setup beyond binding controls in-game. Verified titles are listed
under [Verified game support](#verified-game-support); TrueForce in
SDK-aware sims needs the Proton recipe above. Some games want Steam Input
enabled as a gamepad, or `SDL_JOYSTICK_DEVICE=/dev/input/eventX`.

**DirectInput force feedback and `PROTON_ENABLE_HIDRAW`:** the wheel's report
descriptor carries no PID (force-feedback) collection on any of its three
interfaces. That costs nothing on Wine's SDL/evdev backend, which reaches force
feedback through evdev, nor for SDK-aware sims, whose force arrives over the SDK
path (which is why ACC and AC EVO keep full FFB with `PROTON_ENABLE_HIDRAW=1`).
It does affect a sim that drives FFB through **DirectInput** rather than the
SDK (Le Mans Ultimate, for example): with `PROTON_ENABLE_HIDRAW=1` Wine talks to
the wheel over hidraw, looks for a PID collection to send effects to, finds
none, and the game loses force feedback while its inputs keep working.

Two ways to get force feedback in those sims:

- **The simple one:** run them with `PROTON_ENABLE_HIDRAW=0`, which routes force
  feedback through evdev.
- **logi-ffb** (new in 0.15.0): prepend `logi-ffb %command%` to the game's
  launch (see "Userspace components"). It presents a virtual wheel that *does*
  advertise a PID collection, catches the DirectInput effects the game sends it,
  and forwards them to the real wheel's force feedback, so FFB works even on the
  hidraw path. The mechanism is validated on hardware (effects reach the wheel as
  real force); in-game validation with a DirectInput sim is still wanted, so if
  you have Le Mans Ultimate or a similar title, testing and reporting back is
  very welcome.

The `inject_pid` module parameter was an earlier, in-kernel attempt at the same
goal, injecting a PID output collection on interface 0. It does **not** work on
this wheel: the injected PID collection uses HID report IDs, but the joystick
report has none, and mixing the two makes the joystick input reports unparseable,
so no axis input reaches the game. It is off by default and superseded by
logi-ffb; there is no reason to enable it.

## Technical details

The wheel is a 3-interface USB device: interface 0 = joystick input,
interface 1 = HID++ 4.2 (config/settings), interface 2 = force-feedback
output (64-byte reports on ep 0x03). Unlike the belt-driven G920/G923
(FFB over HID++ 0x8123, 900°), the direct-drive wheels use a dedicated
FFB endpoint and reach 2700°, so the driver gates them on
`HIDPP_QUIRK_DD_FFB` and initialises FFB on interface 1 only (interface 0
has no HID++). Full protocol in
[`docs/PROTOCOL_SPECIFICATION.md`](docs/PROTOCOL_SPECIFICATION.md).

## Troubleshooting

### "Invalid code 768" messages during boot

These come from the HID descriptor declaring more buttons than
physically exist. **This driver filters them** (see `hidpp_dd_input_mapping`)
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
`hid-logitech-dd` module is not installed/loaded - run
`sudo ./tools/dkms-update.sh` (then reload the module), and retry.

### FFB not working

1. Verify the driver is *bound to the wheel*, not just loaded:
   `ls /sys/class/hidraw/*/device/wheel_range` should list a path. If it
   does not, see "stuck on hid-generic" above.
2. Check dmesg for errors: `dmesg | grep -iE 'rs50|g pro'`
3. Ensure you're testing with a game/app that supports FFB

### FFB "pulls the wrong way" / wheel feels unstable under Wine/Proton

If a game amplifies your steering instead of pushing back toward centre
(no self-centering when released), the `FF_CONSTANT` sign compensation is
in the wrong state. Wine/Proton deliver `FF_CONSTANT` inverted relative to
native evdev apps, so the driver inverts by default and Wine/Proton games
feel right out of the box. Native-evdev tools (`fftest`, SDL FF, raw
`EVIOCSFF`) then feel inverted - toggle it off:

```bash
WHEEL_DEV=$(dirname "$(ls -d /sys/class/hidraw/*/device/wheel_range | head -1)")
echo 1 | sudo tee "$WHEEL_DEV/wheel_ffb_constant_sign"   # invert (Wine/Proton, default)
echo 0 | sudo tee "$WHEEL_DEV/wheel_ffb_constant_sign"   # pass-through (native evdev)
```

Only `FF_CONSTANT` is affected. See [`docs/SYSFS_API.md`](docs/SYSFS_API.md).

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

- **Kernel driver** (`mainline/`), tooling, and everything else:
  **GPL-2.0-only** (same as the Linux kernel it derives from). Full text
  in [`COPYING`](COPYING).
- **libtrueforce** (`userspace/libtrueforce/`): **LGPL-2.1-or-later**, so
  native Linux apps (including closed-source ones) may link it while
  changes to the library itself stay open. Full text in
  [`userspace/libtrueforce/COPYING`](userspace/libtrueforce/COPYING).

Logitech's TrueForce SDK DLLs are **not** part of this project and are
not redistributed here; users supply them from their own Logitech G HUB
installation (see the Proton setup above).

## Acknowledgments

- RS50 USB protocol reverse-engineered using Wireshark captures from G Hub on Windows
- Based on [JacKeTUs/hid-logitech-hidpp](https://github.com/JacKeTUs/hid-logitech-hidpp) which adds G Pro wheel support and improved FFB
- Upstream Linux kernel [hid-logitech-hidpp driver](https://github.com/torvalds/linux/blob/master/drivers/hid/hid-logitech-hidpp.c) by Benjamin Tissoires and contributors
- [Oversteer](https://github.com/berarma/oversteer) by Bernat Arlandis for the wheel configuration GUI
