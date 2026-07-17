# logi-dd

A terminal settings app for the Logitech direct-drive wheels (RS50, G PRO),
a native Linux stand-in for the parts of G HUB that configure the wheel. It
reads and writes the `wheel_*` sysfs attributes the `hid-logitech-dd` driver
exposes, with typed values, validation, and mode awareness, so you do not have
to `echo` values into sysfs by hand.

## Features

- **Every wheel setting in one place**, grouped into categories: Force
  feedback, Steering, Pedals, LEDs, Profiles / mode, Info. Each row shows the
  live value read from the wheel; settings absent on your hardware are marked
  unavailable rather than hidden.
- **Typed, validated edits.** Percentages, ranges, enums, toggles and colours
  are parsed and range-checked before they reach the wheel, so an out-of-range
  value is rejected in the UI instead of erroring at the device.
- **Mode awareness.** Settings that only apply in desktop or onboard mode are
  flagged, and the app can switch the wheel between the two (the `d` key). A
  write that needs the other mode tells you so instead of failing silently.
- **A G HUB-style curve editor** for the pedal, steering and handbrake response
  curves: edit control points (input / output percent) plus lower and upper
  deadzones, with a live plot of the composed curve, then upload it.
- **Onboard profile renaming**: pick a slot and type a new name.
- **Combined pedals** toggle and per-pedal / handbrake sensitivity sliders.

## Building

logi-dd is a Rust workspace: a `logi-dd-core` library and the `logi-dd-tui`
crate that builds the `logi-dd` binary. It needs a Rust toolchain (edition
2021, Rust 1.74 or newer) and no system libraries beyond the standard terminal.

```bash
cd userspace/logi-dd
cargo build --release
```

The binary lands at `userspace/logi-dd/target/release/logi-dd`. Copy it
somewhere on your `PATH` if you like, or run it in place.

## Running

```bash
./target/release/logi-dd
```

logi-dd finds the wheel automatically (it looks for the driver's sysfs
attributes). Writing settings needs permission to the `wheel_*` sysfs files,
which the driver's udev rule makes group-writable by the `input` group. Add
yourself to that group once:

```bash
sudo usermod -aG input "$USER"     # then log out and back in
```

Without it, reads work but writes return "permission denied"; running under
`sudo` also works but is not needed once you are in the `input` group. If no
wheel is found, logi-dd prints `no wheel found` and exits (check the driver is
loaded and bound).

## Keys

**Main view**

| Key | Action |
|-----|--------|
| Up / Down | Select a setting |
| Left / Right | Switch category |
| Enter | Edit the selected setting (or run it, for actions) |
| d | Toggle desktop / onboard mode |
| r | Re-read all values from the wheel |
| q | Quit |

**Editing a value**: Left / Right nudge the value (or type for text fields),
Enter commits, Esc cancels.

**Curve editor** (opens on a curve setting): Up / Down move between fields
(point, input, output, lower deadzone, upper deadzone), Left / Right adjust the
selected field, `+` / `-` add or delete a point, Enter uploads the curve, Esc
cancels.

## DirectInput force feedback (`logi-ffb`)

DirectInput games under Wine/Proton that need `PROTON_ENABLE_HIDRAW=1` get no
force feedback on the real wheel, because its HID descriptor has no PID
(force-feedback) collection. The `logi-ffb` binary, built from the
`ffb-proxy` crate in this workspace, fixes that: it presents a virtual
force-feedback wheel and forwards effects onto the real wheel's existing
kernel evdev FF interface, so the hidraw path gets the same force feedback
the native path already has.

Usage is a single prepended command, or the same string pasted into a Steam
title's launch options:

```
logi-ffb %command%
```

See [`crates/ffb-proxy/README.md`](crates/ffb-proxy/README.md) for how it
works, build instructions, and the standalone `--daemon` mode.

## Layout

- `crates/logi-dd-core` - the library: the setting registry, typed values,
  validation, and the sysfs read/write layer. Reusable without the TUI.
- `crates/logi-dd-tui` - the terminal UI (ratatui + crossterm) and the curve
  editor.
- `crates/ffb-proxy` - the `logi-ffb` DirectInput force-feedback proxy binary.
