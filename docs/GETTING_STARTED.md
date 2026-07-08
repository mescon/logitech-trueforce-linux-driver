# Getting Started: from download to racing

This is the guide for you if you own a **Logitech RS50** or **G PRO
Racing Wheel**, you run Linux, and you want to get into a sim with
working force feedback and TrueForce. It is one linear path; every
step links to the README for depth.

Time budget: about 15 minutes, plus one detail (the SDK DLLs) that
needs a copy of Logitech G HUB to source files from.

## 0. Will this work for me?

- **Wheels**: RS50 (`046d:c276` / `046d:c272`) and G PRO Racing Wheel
  (`046d:c272` Xbox/PC, `046d:c268` PS/PC). G920/G923 keep working
  through this module too, but the features described here target the
  direct-drive family. (G923 owners: your wheel speaks the same
  TrueForce protocol, and TrueForce under Proton may already work via
  steps 2-3 - unverified, testers wanted in [issue #27].)
- **Games, verified end-to-end**: Assetto Corsa Competizione and
  Assetto Corsa EVO under Proton, with simultaneous steering FFB and
  TrueForce. Other Logitech-SDK sims (Le Mans Ultimate, AMS2, Assetto
  Corsa, rFactor 2, iRacing) use the same SDK and are expected to
  behave the same; if you play one, your confirmation is wanted
  (open an issue, good or bad).
- **Everything else** (native Linux games, non-SDK titles): you get
  the standard force-feedback suite (constant, spring, damper,
  friction, periodic, rumble) with no extra setup beyond step 1.
- Honest expectations: see "State of the driver" in the README. Short
  version: the core works and is verified on real hardware; there is
  no GUI yet (settings are files you `echo` into, or Oversteer); and
  install is one command plus a couple of per-game Steam settings
  nobody can automate. No distro packages (AUR etc.) yet.

## 1. Install the driver

One command does it all - DKMS module, in-tree driver blacklist, udev
permissions, module load, and (if the SDK DLLs from step 2 are staged)
the TrueForce shim into every Steam prefix:

```bash
git clone https://github.com/mescon/logitech-trueforce-linux-driver.git
cd logitech-trueforce-linux-driver
sudo ./tools/setup.sh
```

It is idempotent - run it again after `git pull` or a kernel update
and it converges. It finishes with a diagnosis of every layer; you
can re-run that health check alone at any time, as your normal user:

```bash
./tools/setup.sh doctor
```

Every line should say PASS (warnings tell you exactly what to run).
Then replug the wheel's USB cable and check the kernel log:

```bash
sudo dmesg | grep -iE 'rs50|g pro'   # expect: "... Force feedback initialized"
# (log lines are tagged with your wheel model: "RS50 (native):",
#  "RS50 (G PRO compatibility mode):", or "G PRO:")
```

<details>
<summary>What setup.sh does, as manual steps (if you prefer to run them yourself)</summary>

```bash
sudo ./tools/dkms-update.sh
printf "blacklist hid-logitech-hidpp\nblacklist hid-logitech\n" | sudo tee /etc/modprobe.d/blacklist-hid-logitech-hidpp.conf
sudo depmod -a
sudo modprobe -r hid-logitech-hidpp 2>/dev/null; sudo modprobe hid-logitech-hidpp
./tools/install-tf-shim.sh --all-steam   # only with the SDK DLLs staged
```
</details>

> **Safety**: this is a direct-drive wheel producing up to 8 Nm. Keep
> hands clear (or hold the rim) whenever the driver loads, the wheel
> replugs, or profiles switch - it can rotate under power.

At this point every game with standard force feedback already works.
The rest of this guide is about TrueForce and the Proton sims.

## 2. Stage the Logitech SDK DLLs (TrueForce only)

TrueForce in the big sims is delivered by Logitech's own signed DLLs
running unmodified inside Proton. They are not redistributable, so
you supply them once, from any Logitech G HUB installation (a Windows
machine, or G HUB unpacked into a throwaway wine prefix). Four files,
placed at these exact paths inside the repo:

```
sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll
```

Then install them into your Steam prefixes (as your normal user,
not sudo):

```bash
./tools/install-tf-shim.sh --all-steam
```

Games installed later: re-run that command (it is idempotent), or
`--prefix /path/to/pfx` for non-Steam prefixes (Heroic, Lutris).

## 3. Per-game Steam setup

For each sim, in Steam:

1. Right-click the game -> Properties -> **Launch Options**:
   ```
   PROTON_ENABLE_HIDRAW=1 %command%
   ```
   Required: the SDK only finds the wheel through hidraw, which
   Proton exposes only with this set.
2. Properties -> **Controller** -> set to **Disable Steam Input** for
   this game, so the game sees the wheel directly instead of a
   virtual gamepad.

**(RS50, optional)** you can switch the wheel into "G PRO compatibility"
mode via its OLED menu, but as of 2026-07-08 this is no longer required:
the SDK also accepts the RS50's **native** identity (`046d:c276`), verified
end-to-end in AC EVO (usbmon-confirmed TrueForce stream). Native mode
additionally unlocks the full 2700 range. Compat mode remains a safe
fallback if a particular game's SDK build does not recognise the native
PID; if TrueForce does not engage in native, try compat and please open
an issue noting the game.

## 4. Set your steering range, then race

The wheel's compat-mode factory default is 90 degrees. Set what you
actually want (this survives game launches - see below):

```bash
H=$(ls -d /sys/class/hidraw/*/device/wheel_range | head -1 | xargs dirname)
echo 0   > "$H/wheel_profile"    # desktop mode
echo 900 > "$H/wheel_range"      # your preferred lock-to-lock degrees
echo 65  > "$H/wheel_strength"   # overall FFB strength, percent
```

In the game: load the "PRO Racing Wheel" controller preset (or bind
manually), and set the in-game steering lock / wheel rotation to the
same number of degrees.

**What to expect on launch:** some games (AC EVO confirmed) push the
wheel to 90 degrees once at session start through their own SDK
channel. The driver detects this and restores your range
automatically within about 20 seconds - you will see both events in
`dmesg` (`rotation range changed externally` followed by
`rotation range auto-restored`). You should never end up stuck at 90;
if you ever do, that is a bug we want reported.

## 5. When something is off

| Symptom | Fix |
|---|---|
| Anything at all | `./tools/setup.sh doctor` diagnoses every layer and names the fix |
| No `wheel_*` files, no FFB (wheel grabbed by `hid-generic`) | `sudo ./tools/rebind-wheel.sh` |
| A game stops seeing the wheel / hangs loading after the driver was reloaded | Quit the game, **restart Steam completely**, relaunch |
| Steering feels off-center | Hold the rim physically straight, then `echo 1 > "$H/wheel_calibrate_here"` |
| Rumble shakes the steering instead of buzzing the rim | Check `cat "$H/wheel_texture_route"` says `tf` (texture belongs on the haptic channel) |
| Reporting a bug | Include `dmesg | grep -iE 'rs50|g pro'` and `cat "$H/wheel_firmware"` output |

More in the README's Troubleshooting section. Settings reference:
`docs/SYSFS_API.md`.

## 6. Make the driver better by playing

The fastest way to improve this project is to race and report:

- Any sim from the "expected" list working (or not) moves the
  compatibility matrix - one sentence and a `wheel_firmware` output
  is enough.
- Real G PRO owners: your feel reports on texture routing
  (`wheel_texture_route` tf vs kf) and the new rev-light control
  (`echo 0-10 > wheel_rev_level`) are the top items we cannot test
  ourselves (issue #8).
- G923 owners: whether TrueForce works under Proton on your wheel is
  an open question only you can answer (issue #27).

[issue #27]: https://github.com/mescon/logitech-trueforce-linux-driver/issues/27

Enjoy the racing.
