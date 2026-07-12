# logi-dd - Phase 1 design (core + TUI settings manager)

Status: design, awaiting review.
Date: 2026-07-12.

## 1. Overview

`logi-dd` is a native Linux control application for the Logitech direct-drive
racing wheels supported by the `hid-logitech-dd` kernel driver (RS50 `046d:c276`,
G PRO `046d:c272`/`046d:c268`). It is the "G HUB replacement" for these wheels:
one place to configure everything the wheel can do, with both a GUI and a TUI,
plus telemetry-driven behaviour (rev LEDs).

The heavy protocol reverse-engineering already lives in the driver, which exposes
~37 `wheel_*` sysfs attributes plus the evdev force-feedback surface. So the app
is, for the most part, a good interface over an already-rich control surface,
plus a few userspace-only features and telemetry.

The whole app is decomposed into phases, each with its own spec/plan/implementation:

- **Phase 1 (this spec):** `logi-dd-core` (settings library) + `logi-dd-tui`
  (terminal UI). Talks directly to sysfs. No daemon yet.
- **Phase 2:** a background daemon that owns the hardware (take-control handshake,
  broadcast/OLED-change sync, single source of truth) + `logi-dd-gui` (GTK4).
- **Phase 3:** button/paddle remapping, per-game auto-profile switching, LED
  pattern authoring, and telemetry integration (the existing `revlights`, and a
  TF4ALL-style telemetry->TrueForce path).

Out of scope for the whole app (documented as "not supported" until the protocol
is known): firmware update (DFU), OLED display configuration, dual-clutch
bite-point.

## 2. Phase 1 scope

**In scope**

- A Rust workspace named `logi-dd`.
- `logi-dd-core`: discover the bound wheel; model every driver-exposed setting;
  read current values; validate and write new values; understand mode gating
  (desktop vs onboard) and offer to switch when a write needs it.
- `logi-dd-tui`: a complete terminal UI (ratatui) that lets the user view and
  change every setting the driver exposes, grouped by category, with inline
  validation and clear errors.

**Explicitly out of scope for Phase 1** (later phases): the daemon, the GUI,
telemetry, button remapping, per-game auto-switching, LED pattern authoring,
onboard-profile authoring, direct evdev effect manipulation beyond what sysfs
already exposes.

**Success criteria:** on a machine with the driver loaded and a wheel bound, a
user can run `logi-dd-tui`, see the current state of every setting, and change
any settable one (with validation and, where needed, an offered mode switch),
and the change takes effect on the wheel.

## 3. Architecture

A Cargo workspace:

```
logi-dd/                      (repo path: userspace/logi-dd/)
  Cargo.toml                  (workspace)
  crates/
    logi-dd-core/             (library: device, settings model, io)
    logi-dd-tui/              (binary: ratatui frontend over the core)
```

`logi-dd-core` is designed so a future daemon can wrap it unchanged: it has no
UI and no global state; a `Device` handle owns the discovered paths and all
reads/writes go through it. In Phase 1 the TUI links the core directly and does
synchronous sysfs I/O. In Phase 2 the daemon links the same core and serves it
over an IPC boundary; the frontends then talk to the daemon instead. Nothing in
the core assumes which caller it has.

Rationale for the split: the core holds all the "what settings exist, how to
validate, how to talk to the hardware" knowledge in one tested place; frontends
stay thin. This keeps each unit understandable in isolation and lets the GUI and
TUI never drift out of sync because they share the model.

## 4. logi-dd-core design

### 4.1 Device discovery

`Device::discover() -> Result<Device, Error>`:

- Find the wheel's sysfs directory by globbing
  `/sys/class/hidraw/*/device/wheel_range` (the attribute that only this driver
  creates) and taking the containing directory. Error `NoWheel` if none.
- Read `wheel_serial` / `wheel_firmware` for identity; read `wheel_mode` for the
  current mode. Store the sysfs dir.
- (Phase 1 does not need the evdev node; strength/etc. are all sysfs.)

The `Device` exposes:

```
impl Device {
    fn discover() -> Result<Device, Error>;
    fn info(&self) -> DeviceInfo;                 // serial, firmware, model, mode
    fn read(&self, id: SettingId) -> Result<Value, Error>;
    fn write(&self, id: SettingId, v: &Value) -> Result<(), Error>;
    fn current_mode(&self) -> Result<Mode, Error>;
    fn ensure_desktop_mode(&self) -> Result<(), Error>;  // switch if needed
}
```

### 4.2 The settings model

Every driver knob is described by a static `SettingSpec`, and the set of specs is
a compile-time registry (`fn registry() -> &'static [SettingSpec]`). A spec is
data, not code, so the TUI (and later GUI) render generically from it.

```
struct SettingSpec {
    id: SettingId,             // enum, one per attribute
    attr: &'static str,        // sysfs filename, e.g. "wheel_strength"
    label: &'static str,       // "FFB strength"
    help: &'static str,        // one-line explanation
    category: Category,        // Ffb, Rotation, Sensitivity, Pedals, Leds,
                               //   Profiles, Calibration, TrueForce, Info
    kind: Kind,                // value type + constraints (below)
    access: Access,            // ReadWrite | ReadOnly | WriteOnlyAction
    mode_req: ModeReq,         // Any | DesktopOnly | OnboardOnly
}

enum Kind {
    Percent,                        // 0..=100
    IntRange { min, max, step, unit },   // e.g. range 90..=2700 step 10 "deg"
    Enum(&'static [&'static str]),  // e.g. led_direction: L->R, R->L, ...
    Toggle,                         // 0/1 with on/off labels
    Text { max_len },               // slot/profile names
    RgbStrip { leds: 10 },          // wheel_led_colors
    Curve,                          // response curves (see 4.4)
    Action,                         // write-only trigger (calibrate_here, led_apply)
}

enum Value { Percent(u8), Int(i32), Enum(u8), Bool(bool),
             Text(String), Rgb([Color;10]), Curve(Vec<(u16,u16)>), Trigger }
```

`parse`/`format` between a `Value` and the raw sysfs string live on `Kind`, with
the exact encodings taken from the driver's `SYSFS_API.md` (percent as decimal,
range as degrees, `wheel_led_colors` as ten space-separated `RRGGBB`, deadzones
as `"<lower> <upper>"`, curves as `in:out` pairs or `reset`, etc.). Validation is
`Kind::validate(&Value) -> Result<(), ValidationError>` and runs before any write.

### 4.3 Mode gating

Some settings only take effect (or are only accepted) in a specific mode:
`wheel_sensitivity` writes are desktop-only (`-EPERM` otherwise); `wheel_brake_force`
is onboard-only; several settings only physically apply in desktop mode. The
spec's `mode_req` captures this. `write()` checks it: if a write needs desktop
mode and the wheel is in onboard, it returns `Error::WrongMode { needed }` rather
than silently failing, so the frontend can offer `ensure_desktop_mode()` and
retry. `ensure_desktop_mode()` writes `wheel_mode = desktop` (i.e. `wheel_profile
= 0`) and re-reads to confirm.

### 4.4 Curves (sensitivity / response / pedal)

`wheel_sensitivity` is the simple 0-100 slider (50 = built-in). `wheel_response_curve`
and `wheel_pedal_response_curve` are full 64-point curves. Phase 1 exposes the
slider fully; the full curve editor is a richer UX best done in the GUI, so
Phase 1 renders these curve attributes as read-the-current-summary + a "reset"
action and the slider, and marks the point-by-point editor as a Phase 2 (GUI)
feature. This keeps the TUI honest without pretending to be a graph editor.

### 4.5 Errors

```
enum Error {
    NoWheel,                         // driver not loaded / not bound
    Io(std::io::Error),              // sysfs read/write failure (maps errno)
    WrongMode { needed: Mode },      // -EPERM: write needs a mode switch
    Unsupported,                     // -EOPNOTSUPP: attr absent on this firmware
    OutOfRange, Invalid,             // -ERANGE / -EINVAL, or local validation
    Parse(String),
}
```

sysfs write errno is mapped to these (EPERM->WrongMode/Unsupported per the attr,
EOPNOTSUPP->Unsupported, ERANGE->OutOfRange, EINVAL->Invalid) so the UI can show
a meaningful message, not a raw errno.

## 5. logi-dd-tui design

ratatui + crossterm. Runs as the normal user (sysfs is group-writable via the
udev rule); no root.

Layout: a two-pane view. Left: category list (FFB, Rotation, Sensitivity,
Pedals, LEDs, Profiles, Calibration, TrueForce, Info). Right: the settings in the
selected category, each showing label, current value, and (on focus) its help
line. A header shows the device (model, serial, firmware) and the current
**mode**, prominently, since mode governs what applies.

Interaction:

- Navigate categories/settings with arrow keys; `Enter`/`e` edits the focused
  setting. Editing is kind-aware: a percent/int gets a slider + numeric entry
  (left/right adjust, type to set); an enum/toggle cycles; text opens an input;
  RGB opens a per-LED colour editor; an action prompts confirm then triggers.
- On write, validation runs first; a `WrongMode` error pops a confirm: "This
  needs desktop mode. Switch now?" -> `ensure_desktop_mode()` then retry.
- `Unsupported` shows a clear "not available on this wheel/firmware" note and
  greys the setting.
- `r` refreshes all values from the device; a status line shows the last result
  / error.
- Read-only info (serial, firmware, profile names) is shown but not editable.
  Write-only actions (`wheel_calibrate_here`, `wheel_led_apply`) render as
  buttons.

Nothing is written speculatively: values only change on an explicit edit+commit.

## 6. Settings covered in Phase 1

Driven by the core registry; grouped as the TUI categories. Source of truth for
ranges/encodings is `docs/SYSFS_API.md`.

- **FFB:** `wheel_strength`, `wheel_damping`, `wheel_ffb_filter`,
  `wheel_ffb_filter_auto`, `wheel_spring_damping`, `wheel_ffb_constant_sign`.
- **Rotation:** `wheel_range`, `wheel_range_restore`.
- **Sensitivity:** `wheel_sensitivity` (slider); `wheel_response_curve`
  (summary + reset in Phase 1).
- **TrueForce:** `wheel_trueforce`, `wheel_texture_route`.
- **Pedals:** `wheel_brake_force`, `wheel_combined_pedals`,
  `wheel_{throttle,brake,clutch}_curve`, `wheel_{throttle,brake,clutch}_deadzone`,
  `wheel_pedal_response_curve` (summary + reset).
- **LEDs (RS50):** `wheel_led_brightness`, `wheel_led_effect`,
  `wheel_led_direction`, `wheel_led_colors`, `wheel_led_slot`,
  `wheel_led_slot_name`, `wheel_led_slot_brightness`, `wheel_led_apply`.
- **LEDs (G PRO):** `wheel_rev_level`.
- **Profiles/Mode:** `wheel_mode`, `wheel_profile`, `wheel_profile_names` (RO).
- **Calibration:** `wheel_calibrate_here`, `wheel_calibrate`.
- **Info (RO):** `wheel_serial`, `wheel_firmware`.

Attributes absent on a given wheel/firmware (e.g. LIGHTSYNC on a real G PRO, or
`-EOPNOTSUPP` settings) are detected at read time and hidden/greyed, so the TUI
adapts to the connected wheel rather than showing dead controls.

Deliberately deferred: the Oversteer-compat alias files (`gain`, `autocenter`,
`spring_level`/`damper_level`/`friction_level`, `range`, `combine_pedals`). They
exist only on the RS50 (skipped on the G PRO PID) and mostly duplicate `wheel_*`
attributes (`gain`=`wheel_strength`, `range`=`wheel_range`). Exposing them would
mean two controls for one setting and an inconsistent cross-wheel set. The one
with no `wheel_*` equivalent is `autocenter` (a host-side centring spring, which
G HUB itself does not expose on these DD wheels); it is revisited in Phase 2
alongside the evdev FF_GAIN/FF_AUTOCENTER controls.

## 7. Testing

- **Core unit tests:** `Kind::parse`/`format`/`validate` round-trips for every
  kind, against the exact strings `SYSFS_API.md` documents; boundary and invalid
  values; errno->Error mapping.
- **Core with a fake sysfs:** `Device` is parameterised over a small `SysfsIo`
  trait (real = files; test = an in-memory map), so read/write/mode-gating logic
  is tested without hardware. This is the main correctness surface.
- **TUI:** logic (which control for which kind, the mode-switch prompt flow) is
  unit-tested against the fake device; rendering is smoke-tested. No hardware in
  CI.
- **Manual hardware pass:** a short checklist on the RS50 (change one setting per
  category, confirm it takes effect), reusing the seat-test spirit.

## 8. Build & packaging

- Rust workspace under `userspace/logi-dd/`. `cargo build --release` yields the
  `logi-dd-tui` binary. No non-Rust runtime deps for Phase 1 (crossterm/ratatui
  only), so distribution is a single static-ish binary - which is why Rust was
  chosen (fits the existing AUR/COPR/OBS/Debian packaging story).
- CI: add a `cargo build`/`cargo test`/`cargo clippy` job for the workspace,
  alongside the existing kernel-module checks.

## 9. Non-goals recap

Daemon, GUI, telemetry, remapping, per-game switching, LED authoring,
onboard-profile authoring, firmware update, OLED config, dual-clutch bite-point.
Each is a later phase or documented as unsupported.

## 10. Future phases (context, not committed here)

- Phase 2: `logi-dd-daemon` (owns the wheel, take-control handshake, listens for
  the device's unsolicited broadcasts to stay in sync with OLED-side changes,
  serves the core over a local socket / D-Bus) + `logi-dd-gui` (GTK4 via
  gtk4-rs), with the full curve editors and LED authoring.
- Phase 3: button/paddle remapping, per-game auto-profile switching, and
  telemetry integration (fold in `revlights`; explore the TF4ALL
  telemetry->TrueForce model).
