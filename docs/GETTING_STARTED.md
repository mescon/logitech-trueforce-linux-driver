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
  nobody can automate. An AUR package (`logitech-trueforce-dkms`) is
  published; other distros install from source.

## 1. Install the driver

One command does it all - DKMS module, migration off any old full-fork
install, udev permissions, module load, and (if the SDK DLLs from step 2 are staged)
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

> **On an atomic / immutable distro (Bazzite, Silverblue, Kinoite)?**
> `setup.sh` and DKMS do not apply there (DKMS needs a writable build tree,
> which `rpm-ostree` does not provide during a transaction). Follow
> [section 1a](#1a-atomic--immutable-distros-bazzite-silverblue-kinoite)
> instead, then rejoin at step 2.

<details>
<summary>What setup.sh does, as manual steps (if you prefer to run them yourself)</summary>

```bash
sudo ./tools/dkms-update.sh
# No blacklist needed: hid-logitech-dd claims only the direct-drive
# wheels, so it coexists with the in-tree drivers.
sudo modprobe -r hid-logitech-dd 2>/dev/null; sudo modprobe hid-logitech-dd
./tools/install-tf-shim.sh --all-steam   # only with the SDK DLLs staged
```
</details>

> **Safety**: this is a direct-drive wheel producing up to 8 Nm. Keep
> hands clear (or hold the rim) whenever the driver loads, the wheel
> replugs, or profiles switch - it can rotate under power.

At this point every game with standard force feedback already works.
The rest of this guide is about TrueForce and the Proton sims.

## 1a. Atomic / immutable distros (Bazzite, Silverblue, Kinoite)

On rpm-ostree systems the module ships as a **kmod RPM** you build once and
layer onto the base image. You build it in a `toolbox` (a mutable Fedora
container that shares the host kernel), then `rpm-ostree install` the result.
Verified on Fedora Silverblue 44 (kernel 7.1.3-200.fc44): the module builds,
layers, and loads, registering the `logitech-dd` driver with the wheel USB IDs.

**Build the kmod in a toolbox.** `$(uname -r)` inside the container is the host
kernel, so the matching `kernel-devel` is what you build against:

```bash
toolbox create -y logitech-build
toolbox enter logitech-build
```

Then, inside the toolbox:

```bash
sudo dnf install -y rpm-build make gcc kmodtool kernel-rpm-macros \
    elfutils-libelf-devel git kmod "kernel-devel-$(uname -r)"

git clone https://github.com/mescon/logitech-trueforce-linux-driver.git
cd logitech-trueforce-linux-driver

# Build the source tarball the spec expects, straight from this checkout:
VER=0.12.0
mkdir -p ~/rpmbuild/SOURCES
git archive --prefix=logitech-trueforce-linux-driver-$VER/ \
    -o ~/rpmbuild/SOURCES/logitech-trueforce-kmod-$VER.tar.gz HEAD

rpmbuild -bb packaging/akmods/logitech-trueforce-kmod.spec \
    --define "kernels $(uname -r)"
exit   # leave the toolbox
```

That produces two RPMs under `~/rpmbuild/RPMS/` (the toolbox home is your real
home): a `kmod-logitech-trueforce-<kernel>` module and a noarch
`logitech-trueforce-kmod-common` (the udev rule). If you ever bump `VER`, match
it to the `upstream_ver` line in the spec.

**Layer them onto the host and reboot** (run on the host, not in the toolbox):

```bash
sudo rpm-ostree install \
    ~/rpmbuild/RPMS/x86_64/kmod-logitech-trueforce-*.rpm \
    ~/rpmbuild/RPMS/noarch/logitech-trueforce-kmod-common-*.rpm
sudo systemctl reboot
```

After the reboot the module auto-loads when you plug the wheel in (or
`sudo modprobe hid-logitech-dd`). Confirm with `modinfo hid-logitech-dd`, then
continue at step 2 for TrueForce. There is no `setup.sh doctor` here; the udev
rule and module are installed by the RPMs directly.

> **After a kernel update, rebuild.** A static kmod does not rebuild itself the
> way DKMS does. When `rpm-ostree upgrade` brings in a new kernel, repeat the
> toolbox build (its `kernel-devel` tracks the new kernel automatically) and
> re-run `rpm-ostree install` on the fresh RPMs before rebooting into it.

> **Bazzite specifically:** Bazzite ships a custom uBlue kernel, so
> `kernel-devel-$(uname -r)` is not in the standard Fedora repos. Enable
> uBlue's akmods repo inside the toolbox first
> (`sudo dnf copr enable ublue-os/akmods`), then run the same steps. This path
> is verified on vanilla Fedora atomic; the Bazzite `kernel-devel` source is
> reported by uBlue but not yet confirmed here, so a Bazzite owner's report is
> welcome.

## 2. Stage the Logitech SDK DLLs (TrueForce only)

TrueForce in the big sims is delivered by Logitech's own signed DLLs
running unmodified inside Proton. They are not redistributable, so
you supply them once, from any Logitech G HUB installation (a Windows
machine, or G HUB unpacked into a throwaway wine prefix). Four files,
in Logitech's own `Logi/...` layout:

```
Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll
```

Place that `Logi/` tree under whichever of these the installer finds
first (highest precedence first):

- a directory you pass with `--sdk-dir <path>`,
- `$LOGITECH_TRUEFORCE_SDK_DIR`,
- the repo's `sdk/` subdirectory (i.e. `sdk/Logi/...`) if you cloned it,
- otherwise `~/.local/share/logitech-trueforce/sdk/` (the default when
  installed from the AUR, where there is no repo tree).

Then install them into your Steam prefixes (as your normal user,
not sudo):

```bash
# from a git checkout:
./tools/install-tf-shim.sh --all-steam
# installed from the AUR (command is on your PATH):
logitech-trueforce-install-shim --all-steam
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
