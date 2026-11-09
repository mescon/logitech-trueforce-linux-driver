<img src="docs/images/logo.svg" align="right" width="120" alt="logi-wheel logo"/>

# Logitech TrueForce Linux Driver

[![Build](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/build.yml/badge.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/build.yml)
[![Userspace CI](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/logi-wheel.yml/badge.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/logi-wheel.yml)
[![CodeQL](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/codeql.yml/badge.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/mescon/logitech-trueforce-linux-driver/badge)](https://securityscorecards.dev/viewer/?uri=github.com/mescon/logitech-trueforce-linux-driver)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/14625/badge)](https://www.bestpractices.dev/projects/14625)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=mescon_logitech-rs50-linux-driver&metric=alert_status)](https://sonarcloud.io/dashboard?id=mescon_logitech-rs50-linux-driver)

[![Latest release](https://img.shields.io/github/v/release/mescon/logitech-trueforce-linux-driver)](https://github.com/mescon/logitech-trueforce-linux-driver/releases/latest)
[![AUR](https://img.shields.io/aur/version/logitech-trueforce-dkms?label=AUR)](https://aur.archlinux.org/packages/logitech-trueforce-dkms)
[![Fedora COPR](https://copr.fedorainfracloud.org/coprs/mescon/logitech-trueforce/package/logitech-trueforce-kmod/status_image/last_build.png)](https://copr.fedorainfracloud.org/coprs/mescon/logitech-trueforce/)
[![openSUSE OBS](https://build.opensuse.org/projects/home:mescon/packages/logitech-trueforce-dkms/badge.svg?type=default)](https://build.opensuse.org/package/show/home:mescon/logitech-trueforce-dkms)
[![Nix flake](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/nix.yml/badge.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/actions/workflows/nix.yml)
[![License: GPL-2.0](https://img.shields.io/badge/license-GPL--2.0-blue.svg)](https://github.com/mescon/logitech-trueforce-linux-driver/blob/master/COPYING)

This is a Linux kernel driver, plus userspace tools, for three Logitech
racing wheels: the direct-drive **RS50** and **G PRO Racing Wheel**, and the
gear-driven **G923**. The kernel driver is the part of Linux that talks to
the hardware. The userspace tools are normal programs that run on top of it.
Together they give you force feedback, TrueForce haptics, a live RPM
rev-light display, LIGHTSYNC LED control, and every wheel setting G HUB
offers. That includes sims running under Proton (Steam's layer for running
Windows games on Linux) and Wine (the project Proton is built on).

**This is the first TrueForce implementation on Linux.** TrueForce is
Logitech's high-frequency haptic layer. It is the engine note, road surface
and tyre texture you feel through the rim, on top of ordinary force feedback.
Before this project it did not exist on Linux at all. It arrives two ways:

- **Native**, in sims that support Logitech's SDK (the software kit Logitech
  gives game developers). The steering force is the game's own signal,
  carried to the wheel unchanged. The driver stack synthesises the engine
  texture on top from the game's live telemetry (the car data a game sends
  out: RPM, speed, and so on). That is how TrueForce texture works on every
  platform. On Windows too, the game supplies force and telemetry, and the
  driver stack supplies the texture. This driver does the same job in the
  kernel.
- **Simulated**, in everything else. Where a game has no TrueForce,
  `logi-tf-sim` synthesises it from that game's live telemetry: engine RPM,
  speed, surface. So your wheel still gets texture in titles that never
  shipped support for it.

The **G923** matters as much here as the direct-drive wheels. This is the
only Linux driver that gives it TrueForce. It is also the only one that gives
the PlayStation edition any force feedback at all. That edition had none on
Linux before this. See [G923 support](#g923-support).

> The older **G920** is already served by the in-tree `hid-logitech-hidpp`
> driver (the one that ships inside the kernel itself). It does not need
> this one.

## Getting started

Five steps, whichever of these wheels you own.

**1. Install it.** One command on most distributions. Find yours in
[Install](#install) below.

**2. Plug the wheel in and open Logi Wheel.** Run `logi-wheel-gui` for the
desktop app, or `logi-wheel` for the terminal one. It finds the wheel and
shows every setting that wheel supports.

**3. Set your games up from its Setup page.** The page lists the sims it
found across Steam, Lutris and Heroic. Each row has the buttons for that game:

- **Install relay** puts the telemetry relay into that game's Proton folder.
  The relay is what feeds simulated TrueForce and the rev lights in sims that
  keep their telemetry in shared memory (a block of memory the game lets other
  programs read). That is the Assetto Corsa family, iRacing, RaceRoom,
  rFactor 2 and Le Mans Ultimate.
- **Install TrueForce files** puts Logitech's own TrueForce files into that
  game's Proton folder. Use it for sims with TrueForce built in (Assetto Corsa
  Competizione, Assetto Corsa EVO) on a **direct-drive wheel**. It needs a
  one-time copy first; see below. A G923 does not use this route and can skip
  it, because that wheel does not answer Logitech's SDK.

  **With these files installed, this is the setup you want.** The game's own
  force reaches the wheel unchanged. The driver synthesises the engine
  texture from the game's live telemetry. That is the same division of labour
  the Windows driver stack uses, done here in the kernel.

  **Without them, do not switch the raw HID interface on.** HID is the USB
  protocol for input devices. The raw HID interface gives a game direct
  access to the wheel's USB reports. Logitech's SDK needs that access to
  deliver TrueForce. But switching it on also replaces the normal path force
  feedback arrives by. The raw interface offers no force feedback of its own.
  So with it on and the files missing, you get neither. `logi-launch` will
  not make that trade. It leaves the raw interface alone until the files are
  there, so force feedback keeps working and you get simulated TrueForce
  meanwhile.

> **The one-time copy: Logitech's TrueForce files.** Nobody may redistribute
> them, so you supply them yourself from a Windows G HUB installation.
>
> 1. Find the `Logi` folder in a Windows G HUB installation.
> 2. Copy the whole folder to `~/.local/share/logitech-trueforce/sdk`. Keep
>    its own layout, so you end up with:
>
>    ```
>    ~/.local/share/logitech-trueforce/sdk/Logi/Trueforce/<version>/trueforce_sdk_x64.dll
>    ~/.local/share/logitech-trueforce/sdk/Logi/wheel_sdk/<version>/logi_steering_wheel_x64.dll
>    ```
>
> 3. Open the Setup page's **TrueForce files** section. It shows the folder
>    it is reading and says whether it found the files. Check that before you
>    install them to a game.
>
> Whatever version numbers your G HUB shipped are the right ones. To keep the
> files somewhere else, set the folder on that page.

**4. Put one launch option on each sim.** In Steam: Properties, General,
Launch Options.

```
logi-launch %command%
```

That line is the same for every game and every wheel. For most people it is
the only setup step. `logi-launch` works out what this game needs on the
wheel you have plugged in, and does it. Older DirectInput sims get force
feedback (DirectInput is the older Windows game-controller interface). Sims
with their own TrueForce get that. Everything else gets simulated TrueForce
and live rev lights. For native-SDK sims like AC EVO it also sets up
the in-kernel texture merge and the telemetry rev-light chain. The SDK proxy
(a helper the game loads next to Logitech's SDK) relays the game's live rpm,
first-shift-light rpm and redline. `logi-rpm-bridge` then feeds them to the
wheel as engine texture (`wheel_texture_rpm`) and rev-light levels
(`wheel_rev_level`). Logi Wheel's Setup page shows what it will do for each
of your games. That differs by wheel. Full detail in
[docs/LAUNCH_OPTIONS.md](docs/LAUNCH_OPTIONS.md).

**5. Turn Steam Input off for the game.** In Steam: Properties, Controller,
"Disable Steam Input". This is the one thing `logi-launch` cannot do for
you. Left on, Steam Input hides the wheel behind a virtual gamepad, and force
feedback never arrives.

### Which wheel do I have?

```bash
lsusb | grep -i 046d
```

| Product id | Wheel | What you get |
|---|---|---|
| `c275`, `c276` | RS50 (both editions) | Everything: force feedback, the game's own TrueForce and simulated TrueForce, rev lights, every setting. The Xbox edition plugs in as `c275`. The driver switches it to `c276` automatically. After that it is the same wheel to everything here. Needs `usb_modeswitch` installed. |
| `c272` | RS50 in compatibility mode, **or** a G PRO (Xbox/PC) | Same as above. The two share this id. Logi Wheel names which one it found. |
| `c268` | G PRO Racing Wheel (PS/PC) | Same as above. |
| `c266`, `c267` | G923 PlayStation edition | Force feedback (no other Linux driver gives this wheel any), simulated TrueForce, rev lights, and the core settings. |
| `c26d`, `c26e` | G923 Xbox edition | Force feedback and TrueForce, both confirmed on a real unit. Rev lights are implemented and await an owner's confirmation in a game. Needs `usb_modeswitch` installed; see [G923 support](#g923-support). |

A wheel that plugs in as `c267`, `c26d` or `c275` is switched to its PC mode
automatically. So a different id after plugging in is expected, not a fault.
The two Xbox editions need `usb_modeswitch` (the tool that flips a USB
device between its modes) installed for that switch. On a system without
systemd, `sudo logi-wheel-modeswitch` does the switch by hand.

If nothing above matches, this driver is not for your wheel. The kernel's
own `hid-logitech-hidpp` already handles the **G920**. `hid-logitech-hidpp`
and `hid-lg4ff` handle the older G25/G27/G29.

## Logi Wheel

**Logi Wheel** is how you drive all of this. It is this project's answer to
G HUB: every setting your wheel supports, in one place. Wheels differ in what
they offer. A few settings only apply in one of the wheel's modes. So
anything unavailable is greyed out, with a note saying why. It runs as a
desktop app (`logi-wheel-gui`) or in a terminal (`logi-wheel`). Both are
built from the same core, so they behave identically.

![Logi Wheel](docs/images/logi-wheel.png)

It does more than settings. There is a G HUB-style curve editor for the
pedals. There is a LIGHTSYNC editor whose changes reach the wheel as you make
them. The Setup page finds your sims across Steam, Lutris and Heroic and
turns the per-game TrueForce helpers on and off. The Info / Testing page
shows live input and runs force tests you can cancel.

## What works

Force feedback, TrueForce haptics, LEDs, pedals, the RS Shifter & Handbrake,
the RS H-Shifter, and the full set of G HUB wheel settings all work. Force
feedback goes through evdev, the Linux input event interface, so games see a
standard wheel. The RS50 is the development hardware, so it is verified
directly. The G PRO runs the same code path and is expected to work. A few
items there await an owner's confirmation.

**Legend:** ✅ verified on hardware · 🟢 shares the verified code path, expected
to work · 🟡 needs a tester · `-` not applicable.

| Capability | RS50 | G PRO |
|---|:--:|:--:|
| Steering, pedals, buttons, D-pad | ✅ | 🟢 |
| Force feedback (full evdev effect suite) | ✅ | 🟢 |
| Force feedback in DirectInput sims (via `logi-ffb`) | 🟡 | 🟡 |
| TrueForce haptics (Proton + Logitech's signed SDK) | ✅ | 🟢 |
| Rotation range (90 to 2700°), strength, damping, filters | ✅ | 🟢 |
| Pedal response curves, sensitivity, deadzones, combined pedals | ✅ | 🟢 |
| RS Shifter & Handbrake (shift, digital + analog handbrake) | ✅ | 🟢 |
| RS H-Shifter (7 gates + reverse, works on the desktop, hot-plug) | ✅ | 🟢 |
| Racing Adapter on the base (legacy pedal sets + DB9 shifter, hot-plug) | ✅ | 🟢 |
| LIGHTSYNC RGB LEDs (slots, colors, direction; edits apply live) | ✅ (faceplate strip, see note) | 🟡 (rev lights) |
| RPM rev-light display (level fill; drawn center-out by default - the strip's pattern comes from the wheel's stored display config, separate from the level itself) | ✅ | 🟡 |
| Native texture merge + telemetry rev lights in AC EVO (relay triple -> logi-rpm-bridge -> wheel) | ✅ | 🟢 |
| Simulated TrueForce from game telemetry (`logi-tf-sim`) | ✅ (sweep-verified) | 🟢 |
| Centre calibration, mode / profile switching, computer-side profiles | ✅ | 🟢 |

> **If the strip stays dark, check which onboard profile is active.** The
> wheel stores five. A profile can keep the strip off entirely. Every LED
> write still reports success on such a profile, so the driver looks broken
> when the wheel is simply doing what that profile says. The four built-in
> sweeps also play a colour pattern held in the wheel's own firmware. The
> wheel never reports that pattern, so the app previews their motion but not
> their colours. Pick a custom slot to choose colours yourself.

USB IDs covered: RS50 (`046d:c276` native for both the PlayStation and the
Xbox edition, `046d:c272` compatibility mode),
G PRO Racing Wheel (`046d:c272` Xbox/PC, `046d:c268` PS/PC), and the G923
(`046d:c266`/`c267` PlayStation edition, `046d:c26d`/`c26e` Xbox edition,
see [G923 support](#g923-support)).

## What's included

Everything below is built from this repository:

- **The kernel driver** (`hid-logitech-dd`) makes the wheel work. Games get
  standard Linux force feedback. Every wheel setting appears under
  `/sys/.../wheel_*`, in sysfs, the kernel's settings file tree. Your other
  Logitech devices stay on their usual drivers. It drives the G923 too,
  through a separate force-feedback engine suited to that wheel (see
  [G923 support](#g923-support)).

- **Logi Wheel**, described [above](#logi-wheel), is the app you configure the
  wheel with. `logi-wheel-gui` is the desktop version. Its LIGHTSYNC editor
  sends per-slot colors and animation direction to the wheel immediately. It
  also has computer-side profile presets, a Setup page for your sims, and an
  Info / Testing page with a rotating wheel diagram, button and pedal readouts
  and cancelable force tests. `logi-wheel` is the terminal version. It has
  the same settings and the same G HUB-style curve editor, so you never have
  to `echo` values into sysfs by hand.

- **logi-ffb** restores force feedback in DirectInput sims under Wine/Proton
  (Le Mans Ultimate, for example). One launch option and it works: see
  [Force feedback in games](#force-feedback-in-games).

- **logi-tf-sim** gives you TrueForce engine haptics and a live RPM rev-light
  display in games with no native TrueForce. It uses the game's own UDP
  telemetry (car data the game sends out over the network). It auto-detects
  the supported games (DiRT Rally 2.0 and the classic Codemasters format,
  Automobilista 2 / Project CARS 2, F1, BeamNG.drive and EA Sports WRC).
  Enable and tune it per game from the Setup page.

- **libtrueforce** lets native Linux apps drive TrueForce without Wine. It is
  a C library that reimplements Logitech's TrueForce SDK. Optional; the
  Proton recipe does not need it.

The distribution packages install the driver plus the `logi-wheel`,
`logi-wheel-gui`, `logi-ffb` and `logi-tf-sim` tools. They also install the
TrueForce shim installer (the shim puts Logitech's TrueForce files into a
game's Proton folder) and the helpers games load themselves: the SDK proxy,
the telemetry relay and the truck-sim plugin. The idea is simple. After you install a
package, the only thing you still need is Logitech's own DLLs (Windows
library files), which cannot be redistributed. `libtrueforce` has its own
build under `userspace/libtrueforce/`.

## G923 support

The G923 gets something no other Linux driver offers: **force feedback and
TrueForce on the same wheel**. TrueForce was never available for it on Linux
before. The PlayStation edition had no force feedback at all, because the
in-tree drivers do not cover it.

It is gear-driven and speaks an older Logitech protocol. So inside the driver
it takes a different path from the RS50 and G PRO above. The result is the
same: real forces in games, engine haptics through the rim, working rev
lights, and the settings app.

Both editions are verified on hardware. An owner confirmed the Xbox edition
in issue #27, on a wheel none of the maintainers own (see
[CREDITS.md](CREDITS.md)).

**PlayStation edition** (`046d:c266`/`c267`) is fully supported and verified on
hardware:

- **Force feedback** in games. The launch option `logi-launch %command%`
  works here too. The wrapper detects the classic wheel and applies its plan:
  raw HID stays off, and the telemetry engine note comes on where the game
  supports it. To do it by hand instead, just turn Steam Input off. Never set
  `PROTON_ENABLE_HIDRAW` for this wheel. That variable is meant for the
  direct-drive wheels, and it disables the G923's force feedback.
- **TrueForce** engine haptics through `logi-tf-sim`, driven by game telemetry.
  Logitech's own SDK cannot deliver TrueForce here. It hands the haptics to
  G HUB, which does not exist on Linux. So this driver streams them itself.
- **Rev lights**, driven from telemetry, or controllable as ordinary Linux
  LEDs. The packaged udev rules (udev is the service that sets device
  permissions) cover their brightness files, so a normal user can drive them.
- **Pedals the right way round.** This wheel sends all three pedals inverted:
  255 released, 0 fully pressed. HID has no way to mark an axis as inverted.
  So every layer passes that on, and a game reads every pedal as fully
  applied at rest. The driver turns them the right way up. Raw HID is
  untouched, because a Wine or Proton game reading the wheel that way expects
  the Windows convention.
- **Settings**: rotation range, force strength, autocenter and combined pedals,
  through `logi-wheel` or Oversteer.

Since 0.29.0 that engine note also covers **Assetto Corsa Competizione and
Assetto Corsa EVO**, which is new. Those two have real TrueForce of their
own. But it travels through a Logitech SDK the G923 does not answer, so it
never reached the wheel. Instead, their telemetry is read from the game's
shared memory and synthesized into an engine note (see
[docs/SHARED_MEMORY_RELAY.md](docs/SHARED_MEMORY_RELAY.md)). Confirmed on a
c266: stationary in the pit box with the engine revving, the wheel buzzes.
Force feedback alone cannot do that.

**Xbox edition** (`046d:c26e`) has **force feedback and TrueForce**, both
confirmed on a real unit, and **rev lights**. Neither the forces nor the
haptics existed for this wheel on Linux before this project.

The rev lights follow RPM in a game. An owner confirmed that in Assetto
Corsa EVO. With Logitech's TrueForce files in a game's prefix (the private
Windows-style folder Proton keeps for each game), `logi-launch %command%`
puts this edition on the same route as a direct-drive wheel. The game's own
force and TrueForce travel over raw HID. The simulated daemon (the
background service) does the rev lights and screen only. The strip exposes the same five
`::RPM1`..`::RPM5` LED devices the PlayStation edition does. So anything
that drives those works here unchanged. Two owners found that this wheel
gives force feedback and simulated TrueForce together on the driver's own
force engine (`g923_xbox_dd_engine=1`, raw HID off). The [G923 wiki
page](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/G923)
has that recipe.

This edition ships in a console-only mode that Linux cannot use. It has to
be switched into PC mode. The packages handle it: a udev rule does the switch
on plug-in, using `usb_modeswitch`. That tool comes in as a recommended
dependency on Debian, Ubuntu, Fedora and openSUSE. **On Arch it is an
optional dependency**, so install it yourself there:

```bash
sudo pacman -S usb_modeswitch
```

The RS50 Xbox edition (`046d:c275`) uses the same rule and the same helper.
`sudo logi-wheel-modeswitch` switches either wheel by hand. That is the
answer on a system without systemd.

If the wheel never leaves console mode, the out-of-tree `xone` driver has
probably claimed it first.

One known limitation: some sims lock the steering to 90 degrees (45 each
way). That is not the wheel or this driver. Logitech's TrueForce SDK asks
G HUB for your wheel's rotation over a local pipe. Nothing answers that under
Proton, so the game falls back to 90, the minimum of the wheel's legal range.
The [Troubleshooting](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Troubleshooting) wiki page has the workaround, and the shim that
aims to remove the need for one. Tracked in #27.

Two notes for the curious. The wheel plugs in as `c267` (PlayStation) or
`c26d` (Xbox), and the driver switches it to its PC mode automatically. The
G923's sysfs settings use the classic Oversteer-compatible names rather than
the `wheel_*` names the direct-drive wheels use. The
[wiki](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/G923)
has the details, including the LED devices and the driver-precedence rule.

## Install

Pick your distribution in the table. The full step-by-step is on the
[**Installation**](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Installation)
wiki page. The one-time TrueForce SDK setup is on
[**Force feedback in games**](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Force-Feedback-in-Games).
Most packages use DKMS, which rebuilds the driver module for each new kernel.

| Distribution | Install |
|---|---|
| Arch, CachyOS, Manjaro | `paru -S logi-wheel-gui` (from the AUR, the Arch User Repository, with paru or your own AUR helper; pulls `logi-wheel` and the driver. Headless box: `paru -S logi-wheel`). Or use the [signed binary repo](#arch-linux-binary-repository) below, which does not depend on the AUR being reachable. |
| Debian, Ubuntu, Mint, Pop!_OS | download the `.deb`s from [Releases](https://github.com/mescon/logitech-trueforce-linux-driver/releases), then `sudo apt install ./logitech-trueforce-dkms_*.deb ./logi-wheel_*.deb ./logi-wheel-gui_*.deb` (skip the gui one on a headless box) |
| Fedora, Nobara | COPR (Fedora's community package service) akmod: `sudo dnf copr enable mescon/logitech-trueforce && sudo dnf install akmod-logitech-trueforce logi-wheel-gui` (headless box: `logi-wheel` instead of `logi-wheel-gui`) |
| openSUSE | OBS (openSUSE's build service) repo `home:mescon` (see the [Installation](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Installation) page) |
| NixOS | add this repo as a flake input, import `nixosModules.default`, and set `hardware.logitech-trueforce.enable = true;` That builds the module against your own kernel and installs the tools, the udev rules and the helpers. Full snippet on the [Installation](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Installation) page. |
| From source (any distro) | `git clone` this repo, then `sudo ./tools/setup.sh`. That does the DKMS build, the udev rules, the helper scripts, and the apps themselves (`logi-wheel`, `logi-ffb`, `logi-tf-sim`, and the window when it can be built). It needs `cargo`, and stops before touching anything if it is missing: an app left over from an older build next to a new module is a fault, not an install (`--without-apps` skips them on purpose). Every app and the module carry the same build stamp, shown by `--version`. `./tools/setup.sh doctor` fails when they were not built from the same source. The window also needs `pkg-config` and the fontconfig headers: `libfontconfig-dev` on Debian/Ubuntu, `fontconfig-devel` on Fedora, `fontconfig` on Arch. Without them everything else still installs. |

Then:

1. Run the install command for your distribution.
2. Plug in the wheel.
3. Check `dmesg` for a line naming your wheel model.

The AUR and Debian packages are DKMS-based, so they rebuild automatically on
kernel upgrades. The NixOS module builds against whichever kernel your
configuration selects, and is rebuilt with the system. The packages install
a udev rule, so settings are writable right away. No group membership needed.

### Arch Linux binary repository

The AUR is the normal route. When it is up, nothing here is needed. But it
has had long outages. During those, no Arch user could get anything newer
than whatever was last pushed. So every release also publishes a signed
pacman repository as release assets. GitHub serves it, so it works whenever
GitHub does.

1. Import the signing key and sign it locally:

   ```bash
   curl -fsSL -O https://github.com/mescon/logitech-trueforce-linux-driver/releases/latest/download/logitech-trueforce-signing-key.asc
   sudo pacman-key --add logitech-trueforce-signing-key.asc
   sudo pacman-key --lsign-key 4B5BDD7802723B289FA934CACD77C00A443B9E79
   ```

   Before you confirm the signing, check that the fingerprint `--lsign-key`
   prints matches the one in that command. We use the full fingerprint
   rather than a short id, because short ids can be collided, and this is a
   key you are telling your package manager to trust. It signs nothing but
   this repository's packages.

2. Add the repository at the end of `/etc/pacman.conf`:

   ```ini
   # /etc/pacman.conf, at the end
   [logitech-trueforce]
   Server = https://github.com/mescon/logitech-trueforce-linux-driver/releases/latest/download
   ```

3. Install:

   ```bash
   sudo pacman -Sy logi-wheel-gui    # headless box: logi-wheel
   ```

The key is served from the release rather than a keyserver, so that
installing depends on nothing beyond GitHub. If you would rather use a
keyserver, `sudo pacman-key --recv-keys 4B5BDD7802723B289FA934CACD77C00A443B9E79 --keyserver keyserver.ubuntu.com`
works once it has propagated.

Updates then arrive through `pacman -Syu` like anything else. `latest/download`
always resolves to the newest release, so the `Server` line never needs
changing.

The packages are the same ones the AUR recipe builds, from the same tag,
built and signed in public CI. If you would rather not add a third-party
repository, the individual `.pkg.tar.zst` files are attached to every
release. They install with `sudo pacman -U <url>`.

**On a Steam Deck this repository is the route**, not the AUR. SteamOS ships
libraries without the files needed to compile against them, so an AUR build
fails before it reaches anything of ours. SteamOS also has a read-only root
filesystem, and a SteamOS update replaces it, so you have to repeat the
install afterwards. The wiki's
[SteamOS section](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Installation#steamos-steam-deck)
has the full sequence, including the kernel headers the module needs.

## Force feedback in games

**`logi-launch %command%` already does everything in this section.** It is
covered here so you can check what it does, and because some people would
rather set things up themselves.

The recipe is per game **and per wheel**. The same title needs opposite
settings on an RS50 and a G923. That is why one launch option that works it
out beats a line copied from someone else's post. Every game and wheel this
project knows about is in **[docs/GAME_SETUP.md](docs/GAME_SETUP.md)**. It is
generated from the same registry the app uses, so the two cannot disagree.

- **Native and most Proton sims:** force feedback works out of the box. Games
  see a standard Linux wheel. No setup beyond binding controls in game.

- **TrueForce haptics** (the high-frequency texture layer, on top of normal
  FFB) in SDK-aware sims. Stage Logitech's signed SDK DLLs into the game's
  Proton prefix, from the app's Setup page or with `tools/install-tf-shim.sh`.
  Then launch with `logi-launch %command%`. It turns raw HID on for the
  wheels that want it, and stages the proxy that answers the SDK's rotation
  question. Leave `PROTON_ENABLE_HIDRAW` to the launcher. With Logitech's
  stock library alone, the game's steering and force feedback stop on track,
  and the launcher refuses to set it in that state. The DLLs are the one
  part nobody can automate, because Logitech's files cannot be
  redistributed. The one-time recipe is on the
  [Force feedback in games](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Force-Feedback-in-Games)
  wiki page. Verified end to end on **Assetto Corsa Competizione** and
  **Assetto Corsa EVO**.

  **On a G923 the recipe is different, not absent.** That wheel does not
  answer the TrueForce SDK, so `PROTON_ENABLE_HIDRAW` must stay unset. There
  it does not add TrueForce; it takes away the force feedback you already
  had. For haptics, turn the game on under Simulated TrueForce instead.
  `logi-tf-sim` synthesizes an engine note from the game's own telemetry.
  That is confirmed working on a G923 in ACC and AC EVO. The shim's `--proxy`
  option does not help in those two. Both check the library's signature and
  refuse the unsigned proxy, so it carries nothing there. See
  [G923 support](#g923-support).

  **Where the SDK files go.** Copy the `Logi` folder out of a Windows G HUB
  install into `~/.local/share/logitech-trueforce/sdk`. Keep its own layout,
  so you end up with:

  ```
  ~/.local/share/logitech-trueforce/sdk/Logi/Trueforce/<version>/trueforce_sdk_x64.dll
  ~/.local/share/logitech-trueforce/sdk/Logi/wheel_sdk/<version>/logi_steering_wheel_x64.dll
  ```

  Whatever version numbers your G HUB shipped are the right ones. Nothing
  here assumes a particular pair. To keep them somewhere else, set
  `LOGITECH_TRUEFORCE_SDK_DIR` to that directory, or pass `--sdk-dir` to the
  installer, or set the folder on the app's Setup page. `setup.sh doctor`
  prints the directory it checked. If it disagrees with you, that line tells
  you where it looked.

  **Side effects to know about.** With `PROTON_ENABLE_HIDRAW` set, some games
  read the wheel's raw HID reports instead of the normal Linux input layer.
  The two use opposite conventions for a control at rest. Nothing is wrong
  with the wheel or the driver. All of it goes away if you unset the
  variable.

  - **Pedals read the other way up**: resting registers as fully pressed.
    Turn on the game's own "invert axis" option for the affected pedals.
    Seen on Assetto Corsa EVO.
  - **A control can read as permanently held**, for example a constant "up"
    that makes menus impossible to navigate. Rebinding in the game usually
    clears it. Reported on Assetto Corsa EVO under GE-Proton9-27.

  If either makes a game unusable, and you would rather have force feedback
  than the game's own TrueForce, tell the wrapper so for that game in
  `~/.config/logi-wheel/games.conf`:

  ```
  3058630  hidraw=0 tfsim=1 relay=ac-evo
  ```

- **DirectInput sims** (Le Mans Ultimate, for example): `logi-launch` routes
  these through `logi-ffb` for you. By hand it is `logi-ffb %command%` in
  the game's Steam launch options. `logi-ffb` presents a virtual wheel the
  game can drive force feedback on, and passes the forces through to the real
  one. Do not set `PROTON_ENABLE_HIDRAW` yourself; `logi-ffb` handles it. The
  game sees a "logi-ffb Virtual Wheel" (its own name and IDs, not the real
  wheel's), so it may need a one-time manual binding. Hardware-validated, but
  still waiting on an in-game tester. If you run such a sim, reports are very
  welcome.

**The one line to put in a game's launch options** is:

```
logi-launch %command%
```

That is the whole setup for most people. It works out what the game needs on
the wheel you actually have, and applies it. That means the raw-HID setting
only where that wheel wants it, and the `logi-ffb` proxy for DirectInput
games. It also means the simulated-TrueForce daemon, and the telemetry relay
inside the game's Proton prefix for the sims that need one. Games with their own TrueForce keep it and
get nothing layered on top. It knows 29 titles by their Steam appid (Steam's
number for a game), and you can teach it others. Full details, including how
to add a game yourself, are in [docs/LAUNCH_OPTIONS.md](docs/LAUNCH_OPTIONS.md).

- **Simulated TrueForce** for games without native support. Enable the game
  in Setup's "Simulated TrueForce" panel, and switch on the game's own UDP
  telemetry setting. `logi-tf-sim` then plays engine haptics from live RPM
  and throttle, and drives the rev LEDs to match. Intensity and felt rev rate
  (pitch) are tunable. A built-in test sweep lets you feel the effect without
  launching a game; the app asks before running it. The sweep refuses, and
  names what has the wheel, if a game or the daemon is already streaming to
  it. The wheel's TrueForce endpoint takes one writer at a time, and two at
  once buzz rather than blend. Hardware-verified with those test sweeps;
  in-game reports welcome.

  Two kinds of game need a helper before they can feed it. **`sudo
  ./tools/setup.sh` installs both for you**, into every Proton prefix and
  into both truck sims. The settings app can also install the relay per game
  from its Setup page. So there is normally nothing to fetch or copy. They
  are also packaged and downloadable if you would rather place them yourself.
  Euro Truck Simulator 2 and American Truck Simulator use a native Linux
  plugin ([docs/SCS_PLUGIN.md](docs/SCS_PLUGIN.md)). iRacing, RaceRoom, the
  Assetto Corsa family, rFactor 2 and Le Mans Ultimate publish to shared
  memory, and a small in-prefix relay forwards it
  ([docs/SHARED_MEMORY_RELAY.md](docs/SHARED_MEMORY_RELAY.md)). The Assetto
  Corsa family's decoders were confirmed against running games. The others
  are written against published layouts, but nobody has driven them yet.
  That is why those titles carry the provisional marker in the table.

  Beyond the engine note there is a **haptic effects layer**: both limiters,
  gear shifts, the ABS pump, traction loss, surface texture, impacts and DRS.
  Tune it in Setup, under "Simulated TrueForce". In the GUI: "Extra effects",
  then "Adjust individual levels". In the terminal app: `x` toggles the layer
  and `l` lists it, with `[` `]` to pick a layer and `v` to set its level.

  How much of it you feel depends on what your game's telemetry carries. Each
  slider tells you which games feed it. Only the engine note and the rev
  limiter work in every supported game. The gear, the pedals and the ABS and
  traction lamps come from OutGauge (a telemetry format), which among these
  games means BeamNG.drive. So the pit limiter, gear shifts, ABS and traction
  are silent elsewhere. Surface texture, airborne, impacts and DRS have no
  source at all yet. The effects are written; the missing piece is a decoder
  field.

  All of this applies only to games you switched simulated TrueForce on for.

  Games with built-in TrueForce (ACC, Assetto Corsa EVO) normally get their
  effects from the game itself. None of this affects them. **The G923 is the
  exception**, and the reason simulated TrueForce covers those two titles at
  all. Their TrueForce goes through a Logitech SDK that wheel does not
  answer, so on a G923 it never arrives. A synthesized engine note is the
  difference between haptics and silence. On a direct-drive wheel, keep using
  the shim and leave these switches alone.

  The same settings live in `tf-sim.conf` as `effects=0/1` and
  `effect_<layer>=0-100`, where `<layer>` is one of `engine`, `rev_limiter`,
  `pit_limiter`, `gear_shift`, `abs`, `traction_loss`, `road_bumps`,
  `airborne`, `collision`, `drs`. `effect_airborne` is a depth rather than a
  level: it sets how far the road is quieted with the wheels off the ground.
  Only the engine layer is hardware-validated so far. If something feels
  wrong, turn that one layer down, and please say so in an issue.

  The base's screen can show the car while a session runs, from the same
  telemetry as the rev lights. Turn on the screen switch on the Setup page
  (`screen=1` in `tf-sim.conf`). It shows the gear and the speed by default.
  The template (`screen.template`) is any `wheel_oled` frame with `{gear}`,
  `{speed}`, `{speed_mph}`, `{rpm}` and the gauge values `{rpm_pct}`,
  `{throttle_pct}`, `{brake_pct}` filled in. So
  `J|Lap {gear}|{speed}|{rpm}|km/h` is four rows of text, and
  `G|{gear}|{speed}` is the gear-and-speed screen. The screen goes back to
  the wheel's own menu when the session ends. It works in the lights-only
  mode too, so a strength of zero gives rev lights and a dashboard with no
  haptics at all. Direct-drive wheels only; the G923 has no screen.

  The easiest way to set all of this is the screen editor on either app's
  lights page ("Choose what to show..."). It has a preview and a list of
  ready-made screens, from gear and speed to gauges and messages of your own.
  "Show now" puts one on the screen. "Use during games" makes it the
  dashboard. The design behind any preset is open for changes.

  With a G923 and a direct-drive wheel plugged in together, say which one
  simulated TrueForce should drive. The Setup page has a picker, or set
  `wheel=dd` or `wheel=g923` in `tf-sim.conf`. Left on `auto`, it takes a
  G923 whenever it finds one, so the direct-drive wheel would never get it.

## Configuring the wheel

Run **logi-wheel-gui** (or **logi-wheel** in a terminal) and edit settings
live: rotation range, force-feedback strength and filters, TrueForce level,
LIGHTSYNC LEDs, profiles, and per-pedal / steering response curves through a
G HUB-style curve editor.

![logi-wheel-gui curve editor](docs/images/logi-wheel-curve-editor.png)

**More than one wheel plugged in?** Both apps manage all of them. The window
puts a button for each next to the title. The terminal app switches with
`w`. The settings you see, the values you write and the tests all belong to
the wheel currently picked. Wheels of the same model are numbered so you can
tell them apart. Nothing is shared between them: each wheel keeps its own
settings on its own hardware.

Games are scoped the same way. `logi-launch` resolves one wheel when it
starts (`--wheel` picks it on a mixed rig) and hands that choice to
everything it runs. So the engine texture, the rev lights and the force
feedback all go to the wheel you are playing. Quitting the game undoes only
what that session armed on it. Another session on the other wheel is left
alone.

The **RS Shifter & Handbrake** is supported too. Plug it into the wheel base
and its settings appear on their own. Unplug it and they go away again. The
app also reads the physical three-position switch on its base and greys out
whatever does not apply. Below, it is in analog-handbrake mode, so the
handbrake curve is live while the two trigger points are not.

![logi-wheel-gui RS Shifter and Handbrake settings](docs/images/logi-wheel-accessory.png)

The **RS H-Shifter** works the same way. Plug it into an accessory port on
the base, and its seven gates and reverse appear as buttons 12 to 20 (button
19 is not used). The driver also turns on the base's routing for it in
desktop mode, so the gates work outside a game. The Info / Testing page
shows which gate is in. Plug the shifter straight into the computer by USB
instead, and the page reads it there too, with the mode shown next to the
gate.

The **Logitech Racing Adapter** is recognised on the base as well. It takes
a G29, G920 or G923 pedal set and a DB9 Driving Force Shifter. The driver
turns on the base's routing for the adapter in desktop mode, like the
H-Shifter. The Info / Testing page also reads the adapter and the RS Shifter
& Handbrake on their own USB cables: the three pedals and the 6+R shifter
on the adapter, the handbrake lever and the shift direction on the Shifter
& Handbrake.

The Info / Testing page doubles as a live input tester (does this button
reach the computer?). The Setup page manages the game helpers:

![logi-wheel-gui Info / Testing](docs/images/logi-wheel-info-testing.png)

![logi-wheel-gui Setup](docs/images/logi-wheel-setup.png)

To build the apps yourself:

```bash
cd userspace/logi-wheel && cargo build --release
./target/release/logi-wheel-gui    # desktop app; ./target/release/logi-wheel for the TUI
```

**logi-wheel is the recommended way to configure these wheels**: it covers
every setting. Everything it sets is also a plain sysfs attribute under
`/sys/class/hidraw/hidrawX/device/wheel_*`, so you can script it directly.
The complete reference is [**docs/SYSFS_API.md**](docs/SYSFS_API.md). If you
already run [Oversteer](https://github.com/berarma/oversteer) across a
collection of Logitech wheels, it recognizes the basics here too, because
the attribute names it expects are exposed as well.

## Verified game support

**Assetto Corsa Competizione** and **Assetto Corsa EVO** run under Proton
with steering, force feedback and TrueForce at once on a direct-drive wheel.
Put `logi-launch %command%` in the launch options and turn Steam Input off.
It applies the rest. Most other sims work out of the box with standard force
feedback, or need `logi-ffb` in their launch options. The full per-game
table, and which needs what, is in [docs/GAME_SETUP.md](docs/GAME_SETUP.md)
and on the
[Force feedback in games](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Force-Feedback-in-Games)
wiki page.

Two game-side behaviors (rotation-range reset at session start, and keeping
hands clear during AC EVO map loads) are covered on the
[Troubleshooting](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Troubleshooting) wiki page.

## Troubleshooting

**Filing a bug? Start with this**, and paste what it prints:

```bash
logi-wheel --report
```

### The wheel never shows up, or drops off the bus

If the wheel is missing from `lsusb`, or appears and vanishes, read the
kernel log before you suspect the driver:

```bash
sudo dmesg | grep -i usb | tail -n 30
```

Lines like `disabled by hub (EMI?)`, `device descriptor read/64, error -71`
or `unable to enumerate USB device` mean the USB link itself failed. That
happens before any driver runs. The usual cause is a front-panel USB port or
a hub ([#141](../../issues/141)). To fix it:

1. Move the wheel to a port on the back of the computer, on the mainboard.
2. Power the wheel off and on.

### No force feedback once the raw HID interface is on

Some games drive force feedback the older Windows way. That needs a part of
the HID protocol these wheels do not implement. On the raw HID interface
those games have nowhere to send force. So force feedback goes silent while
everything else keeps working.

**There is no fix for this yet.** The driver has an `inject_pid` option that
adds the missing part. It does deliver force feedback, but it also breaks
steering and pedals on these wheels. So it is off and should stay off. The
reason is structural. These wheels send their input reports without a report
id. The added part declares several ids. After that, the kernel expects an
id on every report and misreads the ones the wheel is sending correctly.
Making it usable needs the driver to rewrite incoming reports too.

What works today is to leave the raw interface off for that game. That
keeps force feedback and loses the game's own TrueForce. In
`~/.config/logi-wheel/games.conf`:

```
3058630  hidraw=0
```


## Documentation

The [**project wiki**](https://github.com/mescon/logitech-trueforce-linux-driver/wiki)
is the friendliest place to start. It has a **Users** section (install, force
feedback in games, configuring the wheel, simulated TrueForce,
troubleshooting) and a **Developers** section (architecture, the sysfs API,
the protocol specification, libtrueforce, and the internals of `logi-ffb`
and the simulated-TrueForce daemon).

The `wheel_*` attribute reference for scripting,
[**docs/SYSFS_API.md**](docs/SYSFS_API.md), ships in the repo, so it always
matches your installed version. The protocol and button-mapping references
live under [`docs/`](docs/) as well. Two of them are worth knowing.
[**docs/FEATURE_MATRIX.md**](docs/FEATURE_MATRIX.md) lists what each wheel
reports it can do against what this driver uses.
[**docs/STATUS.md**](docs/STATUS.md) says plainly which claims here are
measured, which are argued from the code, and which are neither, along with
the problems we know about and have not fixed.

## Verifying a release

Assets on a [release](https://github.com/mescon/logitech-trueforce-linux-driver/releases)
are signed with the project's GnuPG key (GnuPG is the standard open-source
signing tool). Its fingerprint is
`4B5B DD78 0272 3B28 9FA9 34CA CD77 C00A 443B 9E79`. Its public half is
attached to each release as `logitech-trueforce-signing-key.asc`. Up to
0.41.0 that covers the Arch packages and the repository database, which
pacman verifies once the key is trusted. From 0.42.0 every asset has a
detached `.sig`. To check one by hand:

```bash
gpg --import logitech-trueforce-signing-key.asc
gpg --fingerprint 4B5BDD7802723B289FA934CACD77C00A443B9E79   # must match the line above
gpg --verify logi-wheel-0.41.0-1-x86_64.pkg.tar.zst.sig logi-wheel-0.41.0-1-x86_64.pkg.tar.zst
```

A release is from this project when it verifies against that key. A change
of key would be announced here and in the release notes.

From 0.42.0 each release also carries a CycloneDX SBOM per binary (a
software bill of materials: the list of what went into it) and a SLSA
provenance statement, `release-assets.intoto.jsonl`. The statement lists
every asset by SHA-256, and the release workflow itself signs it through
Sigstore. [slsa-verifier](https://github.com/slsa-framework/slsa-verifier)
checks that an asset is the one the workflow published, from this
repository, at that tag:

```bash
slsa-verifier verify-artifact logi-wheel_0.42.0-1_amd64.deb \
  --provenance-path release-assets.intoto.jsonl \
  --source-uri github.com/mescon/logitech-trueforce-linux-driver \
  --source-tag v0.42.0
```

Only the latest release is supported. See [SECURITY.md](SECURITY.md) for
the policy and the reporting process.

## Project documents

- [CONTRIBUTING.md](CONTRIBUTING.md): how to take part and what a change needs
- [GOVERNANCE.md](GOVERNANCE.md): roles, access, continuity, secrets
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- [SECURITY.md](SECURITY.md): reporting and disclosure;
  [docs/SECURITY_ASSURANCE.md](docs/SECURITY_ASSURANCE.md): threat model
  and dependency policy; [security/openvex.json](security/openvex.json)
- [docs/ROADMAP.md](docs/ROADMAP.md): what changes next;
  [docs/STATUS.md](docs/STATUS.md): what works today
- [CREDITS.md](CREDITS.md): who did the work
- Architecture: the wiki's
  [Architecture](https://github.com/mescon/logitech-trueforce-linux-driver/wiki/Architecture)
  page

## Contributing

Contributions are welcome: code, testing on hardware this project cannot
reach (a real G PRO, a DirectInput sim with `logi-ffb`), and USB captures of
wheel variants that are not yet fully supported. The kernel driver is a fork
of [JacKeTUs/hid-logitech-hidpp](https://github.com/JacKeTUs/hid-logitech-hidpp).
Changes that apply to other Logitech devices are worth contributing upstream
too. Open an issue with your kernel version, distribution, and relevant
`dmesg` output. `tools/setup.sh report` prints all of that in one block.

Most of the project can be checked without a wheel. The Rust workspace has
its own tests (`cargo test --workspace` in `userspace/logi-wheel`). The parts
of the kernel driver that are pure arithmetic, the force maths and the
texture synthesis, are header-only files. They have userspace harnesses
under [`tests/`](tests/README.md) that build with nothing but a C compiler.
CI runs all of it on every push. Changes that add functionality come with
tests for it, in the same change. What needs hardware is listed in
[docs/STATUS.md](docs/STATUS.md).

## License

- **Kernel driver** (`mainline/`), tooling, and everything else: **GPL-2.0-only**
  (see [`COPYING`](COPYING)).
- **libtrueforce** (`userspace/libtrueforce/`): **LGPL-2.1-or-later**, so native
  Linux apps may link it while changes to the library itself stay open.

Logitech's TrueForce SDK DLLs are not part of this project and are not
redistributed here. You supply them from your own G HUB installation.

## Protocol and SDK notes

What this project has worked out about the hardware and Logitech's own
software. Kept current as things are proven or disproven:

- [**docs/PROTOCOL_SPECIFICATION.md**](docs/PROTOCOL_SPECIFICATION.md): the
  wheel's HID++ surface (HID++ is Logitech's extension of HID): features,
  settings, LEDs, force feedback.
- [**docs/TRUEFORCE_PROTOCOL.md**](docs/TRUEFORCE_PROTOCOL.md): the haptic
  stream, the operating-range packet, and why a sim under Proton is told the
  wheel turns 90 degrees.
- [**docs/SDK_ABI_NOTES.md**](docs/SDK_ABI_NOTES.md): how to call Logitech's
  SDK correctly, taken from its machine code. What is verified and what is
  still assumed are marked as such.
- [**docs/SYSFS_API.md**](docs/SYSFS_API.md): every attribute this driver
  exposes.

## Acknowledgments

Most of what this driver knows about the hardware came from people who
tested wheels the maintainers do not own and reported what happened. They
are credited individually in **[CREDITS.md](CREDITS.md)**. That is where the
work that never shows up in a commit log is recorded.

The projects this one is built on:

- Based on [JacKeTUs/hid-logitech-hidpp](https://github.com/JacKeTUs/hid-logitech-hidpp),
  which adds G PRO wheel support and improved force feedback.
- Upstream Linux [hid-logitech-hidpp](https://github.com/torvalds/linux/blob/master/drivers/hid/hid-logitech-hidpp.c)
  by Benjamin Tissoires and contributors.
- [Oversteer](https://github.com/berarma/oversteer) by Bernat Arlandis, prior art
  for Linux wheel configuration; the Oversteer-compatible attribute names are
  exposed for it.
- [new-lg4ff](https://github.com/berarma/new-lg4ff), also by Bernat Arlandis:
  source of the classic force-feedback engine that drives the PlayStation G923
  here.
- [TF4ALL](https://github.com/Mhytee/Trueforce-For-All) by Mhytee, a Windows
  SimHub plugin whose protocol analysis (issue #20) confirmed the G923 shares
  the RS50/G PRO TrueForce stream protocol.
