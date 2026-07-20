# logi-dd

Settings apps for the Logitech direct-drive wheels (RS50, G PRO): a terminal
app (`logi-dd`) and a desktop app (`logi-dd-gui`), native Linux stand-ins for
the parts of G HUB that configure the wheel. Both read and write the `wheel_*`
sysfs attributes the `hid-logitech-dd` driver exposes, with typed values,
validation, and mode awareness, so you do not have to `echo` values into sysfs
by hand. The workspace also builds `logi-ffb`, the DirectInput force-feedback
proxy (see below).

## Features

- **Every wheel setting in one place**, grouped into categories: Force
  feedback, Steering, Pedals, LIGHTSYNC, Profiles / mode, Info. Each row shows
  the live value read from the wheel; settings absent on your hardware are
  marked unavailable rather than hidden. Steering and Pedals have an Advanced
  toggle that reveals the full curve and filter set behind the simple sliders.
- **Typed, validated edits.** Percentages, ranges, enums, toggles and colours
  are parsed and range-checked before they reach the wheel, so an out-of-range
  value is rejected in the UI instead of erroring at the device.
- **Mode awareness.** Settings that only apply in desktop or onboard mode are
  flagged, and the app can switch the wheel between the two (the `d` key). A
  write that needs the other mode tells you so instead of failing silently.
- **A G HUB-style curve editor** for the pedal, steering and handbrake response
  curves: edit control points (input / output percent) plus lower and upper
  deadzones, with a live plot of the composed curve, then upload it.
- **A LIGHTSYNC slot editor**: per-LED colors (with an HSV picker in the GUI),
  effect, brightness and animation direction, composed per onboard slot.
- **Onboard profile renaming**: pick a slot and type a new name.
- **Combined pedals** toggle and per-pedal / handbrake sensitivity sliders.
- **A Setup section**: per-game TrueForce shim management over Steam/Proton
  game discovery (with an SDK directory picker), plus logi-ffb helper setup.
- **A Test section**: a live input monitor and guarded force-feedback
  simulations.

## Building

logi-dd is a Rust workspace of four crates (see Layout below): the
`logi-dd-core` library, the `logi-dd-tui` and `logi-dd-gui` frontends, and the
`ffb-proxy` crate that builds `logi-ffb`. It needs a Rust toolchain (edition
2021, Rust 1.88 or newer; the Slint GUI needs 1.92). The TUI needs
no system libraries beyond the standard terminal; the GUI additionally needs
`pkg-config` and the fontconfig headers (`libfontconfig-dev` on Debian/Ubuntu,
`fontconfig-devel` on Fedora, `fontconfig` on Arch).

```bash
cd userspace/logi-dd
cargo build --release
```

The binaries land at `userspace/logi-dd/target/release/{logi-dd,logi-dd-gui,logi-ffb}`.
Copy them somewhere on your `PATH` if you like, or run them in place.

The workspace crates share one version (`workspace.package` in `Cargo.toml`)
that follows the repository's release tag; bump it as part of cutting a release.

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

The TUI is a sidebar of views plus a content pane. Press `?` at any time for
the complete key list of the current context; that in-app overlay and the
footer render from the same table, so they are the authoritative reference.

**Global** (whenever no text entry is open)

| Key | Action |
|-----|--------|
| 1-7 | Jump straight to that sidebar view |
| Tab | Switch focus between the sidebar and the content pane |
| Esc | Close the topmost editor / overlay, else back to the sidebar |
| ? | The full key list for the current context |
| q | Quit |

**Sidebar focus**: Up / Down choose a view (its content loads live),
Enter / Right move focus into the content pane.

**Settings views** (content focus)

| Key | Action |
|-----|--------|
| Up / Down | Select a setting |
| Enter | Edit the selected setting (or apply / run it, for actions) |
| i | Explain the selected setting |
| a | Toggle sensitivity / full curve for the row's axis (Steering, Pedals) |
| d | Toggle desktop / onboard mode (on a saved-profile row: delete it) |
| r | Re-read all values from the wheel |

**Editing a value**: Left / Right nudge the value (or type, for text fields),
Enter commits, Esc cancels.

**Curve editor** (opens on a curve setting): Up / Down move between fields
(point, input, output, lower deadzone, upper deadzone), Left / Right adjust the
selected field, `+` / `-` add or delete a point, Enter uploads the curve, Esc
cancels.

**LED color picker** (opens on the LED colors row): Tab switches between the
strip and the palette, arrows move, Enter paints the selected LED (`a` paints
all, `p` the LED and its mirror pair, `x` opens hex entry), `w` writes the
strip to the wheel, Esc cancels.

**Setup and Info / Testing** are sectioned views with their own keys (per-game
shim install / remove, simulated-TrueForce daemon controls, guarded force
simulations); the footer shows the most useful ones and `?` lists them all.

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

- `crates/logi-dd-core` - the shared library: the setting registry, typed
  values, validation, the sysfs read/write layer, plus the `steam` (Steam /
  Proton game discovery), `evtest` (live input monitoring for the Test views),
  `lightsync` (composed per-slot LED model) and `shaping` (simple / advanced
  attribute split) modules. Reusable without either frontend.
- `crates/logi-dd-tui` - the terminal UI (ratatui + crossterm), builds the
  `logi-dd` binary.
- `crates/logi-dd-gui` - the desktop UI (Slint), builds the `logi-dd-gui`
  binary.
- `crates/ffb-proxy` - the `logi-ffb` DirectInput force-feedback proxy binary.

## Development without a wheel

Set `LOGI_DD_SYSFS_DIR=/path/to/dir` to point the apps at a directory of
plain `wheel_*` files instead of the real sysfs tree; both frontends then run
fully headless against it, no wheel or driver needed.
