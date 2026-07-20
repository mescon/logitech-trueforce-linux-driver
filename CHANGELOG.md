# Changelog

This project follows a loose semver: major versions mark API-breaking
changes to the sysfs surface, minor versions add supported wheels or
new attributes, patch versions are bug fixes and documentation. Pre-1.0
the contract is "it works on RS50 and G Pro as listed here".

## 0.16.1 - 2026-07-20

Branding patch: one universal logo (steel-blue rim, legible on light
and dark surfaces) used identically as launcher icon, README logo,
window icon and in-app header mark; the GUI presents as "Logi DD"
(window title, header, desktop entry - binary and package names are
unchanged); desktop entry gains StartupWMClass; TUI header carries the
rev-light arc signature.

## 0.16.0 - 2026-07-20

The settings app grows a desktop GUI, both frontends gain LIGHTSYNC,
Setup and testing surfaces, simulated TrueForce arrives as a telemetry
daemon, and a hardware-verification campaign decoded three LIGHTSYNC
protocol facts (custom slots are effect values 5-9, effect selects need
a commit, the strip doubles as a level-driven rev display) and fixed
the driver accordingly. Packaging splits into three interdependent
packages.

### Added
- **`logi-tf-sim`, a simulated-TrueForce daemon**: synthesizes engine
  haptics from a game's own UDP telemetry (DiRT Rally 2.0 and the
  classic Codemasters format, Automobilista 2 / Project CARS 2) for
  titles without native TrueForce, and feeds the same RPM to the
  wheel's rev-light display. Per-game enable and intensity, master
  switch, tunable felt rev rate, a consent-gated test sweep, and a
  Setup panel in both frontends. Streams through `libtrueforce`
  (static-linked).
- **RPM rev-light display**: `wheel_rev_level` (0-10) drives the RS50's
  strip as a live rev display (hardware-verified), fed manually, by
  `logi-tf-sim`, or by any telemetry bridge.
- **Per-axis shaping**: throttle, brake, clutch, handbrake and steering
  each choose sensitivity or the full response curve independently.
- **Mode-coupled profiles**: onboard mode shows the wheel's five named
  slots; desktop mode manages computer-side profile presets
  (save/apply/delete under `~/.config/logi-dd/profiles`).
- **Info / Testing page**: serial, firmware, app and driver versions
  (copyable), a live input monitor (rotating wheel diagram, button
  tester with GL/GR, pedal bars) and guarded, cancelable force
  simulations.
- **Per-game Setup**: Steam/Proton game discovery with per-game shim
  install/remove, an SDK folder with live resolution, a games
  compatibility table, and helper discovery that also finds repo
  checkouts.
- **Curve editor polish**: axis legends, a hover ghost showing where a
  click adds a point, numeric per-point entry.
- **Drift watcher**: external profile/mode changes (the rim's buttons)
  refresh whatever page is open within about two seconds.
- **Desktop entry and an original logo** for `logi-dd-gui`.
- **Three-package split** in every channel: the driver package,
  `logi-dd` (TUI + `logi-ffb` + `logi-tf-sim` + shim installer) and
  `logi-dd-gui` (desktop app), with dependency chains so one install
  pulls what it needs.
- **`logi-dd-gui`, a desktop settings app** (Slint): the full `wheel_*`
  settings surface as a GUI - every category with live values, mode
  switching and refresh, a G HUB-style curve editor, an HSV color picker,
  a deadzone pair editor, onboard profile renaming, and a named-profile
  dropdown. Ships in all packaging channels alongside the TUI.
- **LIGHTSYNC redesign** in both frontends: the LED settings are composed
  into a per-slot model (colors per LED, effect, brightness, animation
  direction) with a slot editor, replacing the flat "LEDs" list.
- **Setup section** (GUI page and TUI view): per-game management of the
  TrueForce shim over Steam/Proton game discovery, with an SDK directory
  picker, plus logi-ffb helper setup.
- **Test section** (GUI page and TUI view): a live input monitor and
  guarded force-feedback simulations for checking the wheel end to end.
- **Advanced shaping toggle** in Steering and Pedals: the simple view
  shows the G HUB-equivalent sliders, Advanced reveals the full curve
  and filter set.
- **`LOGI_DD_SYSFS_DIR`** environment override for development: point the
  apps at a directory of plain `wheel_*` files and they run fully
  headless, no wheel or driver needed.
- **`install-tf-shim.sh --uninstall-prefix`** removes the shim from a
  single Wine/Proton prefix.

### Fixed
- **LIGHTSYNC direction wire encoding.** The driver wrote the sysfs 0-3
  direction enum straight into the 0x807B config, but the device expects
  1-4 on the wire; the firmware NAKed the off-by-one config (an `-EIO` on
  writing Outside-In). The driver now translates both ways.
- **logi-ffb virtual wheel identity.** The virtual wheel cloned the real
  wheel's name and IDs, so games (and the proxy itself) could bind the
  wrong device and the wrapper hid steering. It now appears as
  "logi-ffb Virtual Wheel" with its own IDs, and the proxy refuses to
  bind its own virtual device.
- **GUI/TUI fix wave**: the curve plot maps linearly with repaired point
  hit-testing and no drag stalls, editor overlays are actually modal,
  severed widget bindings re-sync from model pushes, pair edits no longer
  race, mode edits no longer desync the header, and slot renames give
  feedback and refresh the profile dropdown.
- **Custom LIGHTSYNC slots never switched on the wheel.** Decoded from
  captures and hardware-confirmed: the five custom slots ARE effect
  values 5-9 (0x05 = CUSTOM 1); the driver hardcoded 0x05, so every
  slot switch rendered slot 1, and its "activate" call was actually a
  name read. Slot selection now visibly repaints the strip.
- **Built-in LED effects never repainted.** fn3 only stages an effect;
  the strip repaints on a zero-parameter fn6 commit, which the driver
  now sends.
- **The rev-arm burst stomped the active effect** (its fn3 0x02 side
  effect); the driver snapshots and restores the user's effect and slot
  around arming.
- **Combined pedals trapped the toggle**: the driver misparsed the
  toggle's own change echo as a "profile 1" broadcast, briefly reporting
  onboard mode; the frontends then locked the row as wrong-mode.
- **GL and GR are their own buttons** (codes 0x2cc/0x2cd), not aliases
  of the shifter paddles; the input tester now maps them.
- **wheel_rev_level is not G PRO-only**: the RS50 accepts the level
  command; docs and labels corrected.

## 0.15.0 - 2026-07-18

The release adding `logi-ffb` plus the pedal-shaping restoration below
(the tag was documented in the GitHub release notes; summarized here for
completeness).

### Added
- **`logi-ffb`, a DirectInput force-feedback proxy** (`ffb-proxy` crate):
  presents a virtual force-feedback wheel to Wine/Proton sims on the
  `PROTON_ENABLE_HIDRAW=1` path and forwards effects to the real wheel's
  evdev FF interface. Run as `logi-ffb %command%` in Steam launch options.
- **Combined pedals.** `wheel_combined_pedals` (0/1, desktop only) toggles G HUB's
  legacy throttle+brake axis merge via feature `0x80D0`. Verified on an RS50: on,
  the two pedals collapse to a single centred axis (`ABS_RX` re-centres, `ABS_RY`
  goes silent); off restores separate axes. Wired into logi-dd.
- **RS Shifter & Handbrake support.** All modes work with no driver change (they
  ride the wheel's existing report): sequential shift = `BTN_TOP2` / `BTN_PINKIE`,
  digital handbrake = `BTN_THUMB2`, analog handbrake = `ABS_Z`, all hardware-mapped
  on an RS50. Added `wheel_handbrake_curve` and `wheel_handbrake_sensitivity` to
  shape the analog handbrake (base `0x80A4` axis 4), the same curve type as the
  pedals, verified live. Wired into logi-dd.
- **Hardware pedal shaping.** The pedal unit (HID++ sub-device `0x02`) applies a
  `0x80A4` response curve to each axis it reports to the PC, the same mechanism
  as the steering wheel. Verified on an RS50 for all three pedals with an
  artifact-proof test (a two-plateau throttle curve, and step curves on the
  load-cell brake and clutch, each producing a bimodal axis a linear pedal
  cannot). Each pedal `<p>` in {throttle, brake, clutch} gains three attributes,
  all writing the single curve the axis holds (last write wins):
  - `wheel_<p>_curve` - raw `in:out` points or `reset`, like `wheel_response_curve`.
  - `wheel_<p>_sensitivity` - the 0-100 G HUB slider (50 = linear).
  - `wheel_<p>_deadzone` - `"lower upper"` percent dead travel (sum <= 99).

  This corrects the v0.14.0 removal, which rested on a single untested capture;
  the pedal MCU does apply the curve, it was a measurement error.
- **logi-dd curve editor.** A G HUB-style modal point editor for the pedal and
  steering curves: edit control points (input/output percent) plus deadzones
  with a live ASCII preview, then upload. Plus registry entries for the nine new
  pedal attributes.

## 0.14.0 - 2026-07-16

An input-dropping bug fixed, onboard profiles renameable, and the pedal
attributes that never did anything removed. Also the first release carrying
`logi-dd`, the settings app. All driver changes are hardware-verified on an RS50
(native mode, kernel 7.1.3).

The sysfs surface loses the pedal shaping attributes (see **Removed**). That is
an API break, but not a behaviour change: pedals were already reported raw.

### Fixed
- **Joystick frames were parsed as HID++ and dropped.** A direct-drive wheel's
  interface 0 is a joystick whose input report declares no report ID, so its
  first byte is data - the 4-bit hat switch plus buttons 1-4. We claim that
  interface (to track the steering axis) and register no `report_table`, so
  `hidpp_raw_event` ran on those frames and switched on that byte as if it were
  a HID++ report ID. D-pad Up + button 1 is `0x10` (`REPORT_ID_HIDPP_SHORT`);
  Up-Right and Right give `0x11` and `0x12`. The 30-byte frame then failed the
  HID++ size check, logged `received hid++ report of bad size (30)` and returned
  1 - telling the HID core the report was consumed, so the frame was dropped
  before reaching the input layer. Holding such a combination discarded every
  input report: steering, pedals and buttons froze while dmesg flooded.
  Reproduced on an RS50 (170 errors in a few seconds of D-pad + button presses;
  zero afterwards). The interfaces claimed for input only are now flagged and
  skip the HID++ demux.

### Added
- **Profile rename.** `wheel_profile_names` is writable (0664): `echo "3:RACE" >
  wheel_profile_names` renames an onboard slot via feature `0x8137` fn4. The
  wheel persists the name to its own NVM on the single write; there is no
  separate save step. Slots are 1-5. An RS50 takes names of up to 9 characters
  (matching its own stock `PROFILE 3`), stores them uppercased, and accepts
  spaces; it refuses a longer name at the HID++ layer, reported as `-EIO`.
- **`logi-dd`, a settings app** (`userspace/logi-dd`): a Rust core plus a
  terminal UI over the `wheel_*` sysfs surface - typed reads/writes, per-setting
  validation, mode gating, and a profile selector that shows the slots by name.
  Onboard slots can be renamed from it (pick the slot, type a name); it caps the
  name at the wheel's 9 characters, so it cannot compose one the wheel refuses.
  First part of a G HUB replacement.

### Removed
- **Pedal shaping attributes** - `wheel_{throttle,brake,clutch}_curve`,
  `wheel_{throttle,brake,clutch}_deadzone`, `wheel_combined_pedals`, the
  Oversteer-compat `combine_pedals`, and `wheel_pedal_response_curve`.
  API-breaking for the sysfs surface, but not a behaviour change: these
  transforms never reached userspace (the rewritten report did not survive to
  the input layer), so the attributes accepted settings that did nothing.
  `wheel_pedal_response_curve` uploaded a hardware curve that a raw-HID capture
  showed the wheel stores but never applies to its PC output. Pedals are
  reported raw, exactly as before; shape them in userspace instead. The steering
  `wheel_response_curve` is unaffected. Oversteer hides its combine-pedals
  control when the attribute is absent; every other Oversteer attribute is
  unchanged.

## 0.13.0 - 2026-07-11

A correctness batch from a review of the G Hub USB captures and an audit of
places where a symptom had been masked instead of fixed, plus validation of the
real G PRO wheel against contributor captures. All driver changes are
hardware-verified on an RS50 (native mode, kernel 7.1.3).

### Fixed
- **Pedal init hang (#30).** The pedal MCU (device index 0x02) silently drops
  HID++ messages sent with software-id 0x01; it accepts 0x0a (what G Hub uses).
  Init drops from ~15-20 s of retry timeouts to ~0.4 s.
- **Damping was zeroed on any settings re-read.** The read used function index
  fn1 (which *sets* damping to 0) instead of fn0 (get). Now fn0.
- **TrueForce current-value read** used fn1 (an event slot) instead of fn2.
- **`wheel_sensitivity`** now uploads the 0x80A4 axis-response Bezier curve (the
  real desktop sensitivity control) instead of aliasing 0x8040 LED brightness.
  Sensitivity and brightness are fully independent.
- **Removed the `05 07` "FFB keepalive"**, which was a DualShock-4 lightbar
  packet, not a wheel command. FFB does not depend on it.
- **RS50 rev-lights** un-gated, with corrected ~100 Hz cadence and DMA-safe
  buffers (the previous stack buffers triggered a USB DMA warning).
- **On-wheel OLED profile edits** now trigger a settings re-read (0x8137 sw0).
- **Transport / error-handling hardening**: output_report falls back to
  SET_REPORT only on -ENOSYS; compat-lookup misses return -EOPNOTSUPP; dropped
  FFB samples are counted rather than silently lost; retry-on-timeout breaks on
  non-BUSY.
- **G PRO compat fallback (#33).** On a real G PRO, a transport-level feature
  lookup failure no longer applies RS50 fallback indices (which are shifted on a
  real G PRO and would cross-wire a setting into a bystander feature); it reports
  the feature absent and logs it.

### G PRO validation
- The real G PRO (`046d:c272`) HID++ configuration protocol is confirmed against
  contributor G Hub captures (#8): identical to the RS50 except a uniform
  feature-index shift, which the driver resolves dynamically. The FFB / TrueForce
  stream itself is not yet verified on a real G PRO (those captures were
  config-only).

### Packaging
- New distribution channels: **Debian/Ubuntu (.deb)**, **Fedora COPR (akmod)**,
  and **openSUSE OBS (DKMS)**, auto-published on each GitHub Release.

### Tooling
- `linux_game_capture.sh` gains a ring-buffer mode (`ring[:N]`) that keeps only
  the last N seconds, for capturing intermittent issues (#31).

### Documentation
- Protocol spec corrected (05-07 is a DS4 packet; sensitivity is 0x80A4; damping
  is fn0; pedal software-id is 0x0a), and the `wheel_sensitivity` sysfs reference
  rewritten to match.

## 0.12.1 - 2026-07-09

Packaging and documentation. No driver code change from v0.12.0 (the module
is byte-identical); this release adds an install path for atomic distros and
corrects the docs.

### Atomic / immutable distros (Bazzite, Silverblue, Kinoite)

DKMS cannot build on rpm-ostree systems (its build tree is read-only during
the transaction), so the module now also ships as a static **kmod RPM**
(`packaging/akmods/logitech-trueforce-kmod.spec`, kmodtool-based). You build
it once in a `toolbox`, layer it with `rpm-ostree install`, and reboot.
Verified end-to-end on Fedora Silverblue 44 (kernel 7.1.3-200.fc44): it
builds, layers, and loads, registering the `logitech-dd` driver with the
three wheel USB IDs. `docs/GETTING_STARTED.md` section 1a documents the flow,
including the post-kernel-update rebuild and the Bazzite custom-kernel
`kernel-devel` note.

### Documentation
- Corrected the RS50 LED description to match the hardware: a horizontal
  10-LED strip across the upper faceplate (rev/shift indicator), numbered
  left to right.
- Accuracy pass across the doc set, checked against the driver code.
- Trimmed verbose historical and development notes from the README.

## 0.12.0 - 2026-07-09

9 commits since the `v0.11.0` tag on 2026-07-08. The fork is now scoped
to only the direct-drive wheels (module `hid-logitech-dd`), coexisting
with the in-tree `hid-logitech-hidpp` instead of shadowing it for every
Logitech device; two LED init-stomp bugs are fixed; and the licensing +
AUR packaging groundwork is in place. All validated on RS50 hardware and
built clean on clang 7.1.3 and gcc 6.18-debug.

### Scoped to the direct-drive wheels (module renamed to hid-logitech-dd)

The driver was a full fork of the in-tree `hid-logitech-hidpp` and shipped
under that same name, so once installed it **replaced the in-tree driver
for every Logitech HID++ device** - mice, keyboards, receivers - freezing
them at the fork's snapshot (which lagged mainline by ~21 recent Bluetooth
devices plus several 7.1/7.2 hardening fixes). It only ever added value for
the direct-drive wheels.

- The module now builds as **`hid-logitech-dd`** (driver name `logitech-dd`)
  and its device table is trimmed to just the direct-drive wheel USB IDs
  (`c276` RS50 native, `c272` G PRO Xbox/PC + RS50-compat, `c268` G PRO
  PS/PC). It runs **alongside** the in-tree `hid-logitech-hidpp`, which
  keeps serving every other Logitech device at its current version. No
  symbol clash (the fork exports none) and no PID conflict (the in-tree
  driver does not claim these wheels), so **no blacklist is needed**.
- `setup.sh` now **migrates** existing installs: it removes the old
  `hid-logitech-hidpp` DKMS package and the stale
  `blacklist-hid-logitech-hidpp.conf`, restoring the in-tree driver for
  your other Logitech hardware.
- Belt-driven **G920/G923 are no longer claimed** by this fork; they use
  the in-tree driver (their standard HID++ FFB is unchanged).

### Licensing and packaging

- Added the missing license texts: `COPYING` (GPL-2.0) for the driver and
  tooling, `userspace/libtrueforce/COPYING` (LGPL-2.1) for the library,
  plus SPDX headers on every libtrueforce source. Required for AUR.
- `install-tf-shim.sh` resolves its SDK-DLL directory (`--sdk-dir` / env /
  repo `sdk/` / `~/.local/share/logitech-trueforce/sdk`) instead of
  hardcoding the repo tree, so it works when installed standalone.
- DKMS packaging skeleton for the AUR under `packaging/aur/`.

### Fixed

- **LED brightness reverting to 100% on connect** ([issue #29]): the
  LIGHTSYNC slot-apply wrote a driver-cached brightness (default 100%,
  never read back from the wheel) on every init and LED change, racing
  the wheel's profile load and stomping the user's saved RPM brightness.
  apply_slot no longer writes brightness; it stays owned by the
  `wheel_led_slot_brightness` handler. Hardware-verified on RS50.
- **LED effect reset to Custom on connect** (same class as #29): the
  load-time apply forced effect mode 5 (Custom) over any animated effect
  (modes 1-4) the wheel restored from its profile, because the effect
  mode is never read back. The init now applies the slot's colours
  without forcing the effect mode. Hardware-verified: an animated effect
  survives a reload, and Custom-mode LEDs still light on load.

[issue #29]: https://github.com/mescon/logitech-trueforce-linux-driver/issues/29

## 0.11.0 - 2026-07-08

35 commits since the `v0.10.0` tag on 2026-07-03. The TrueForce force
path was reworked from TF4ALL protocol findings and feel-verified on
RS50 hardware (clean texture route, host-alive force unaffected by
texture playback, response-curve upload/reset confirmed). SDK-driven
game TrueForce under Proton was additionally packet-confirmed in the
RS50's **native mode** (`046d:c276`, AC EVO, ~2 kHz type-0x01 stream on
ep 0x03), so native mode no longer trades away game TrueForce for its
full 2700 range.

### TrueForce stream reworked from TF4ALL cross-pollination

Protocol findings from the TF4ALL project (a Windows SimHub plugin
built on this project's documentation - issue #20; analysis in
dev/docs/tf4all-analysis.md) fed back into the driver:

- **Unified force+audio stream packets**: bytes 6-9 of a stream packet
  are the motor torque target, with the 13-slot window played
  additively on top - so the driver now sends ONE packet per 2 ms tick
  during texture playback (steering force in the preamble, four window
  slots of texture audio) instead of interleaving 500 Hz force packets
  with 250 Hz audio packets whose preamble wrongly carried the audio
  amplitude. Doubles the texture slot rate to 2 kHz and removes the
  audio-as-torque wart.
- **Texture amplitude cap** at half of full scale: above ~0.5-0.7 FS
  the wheel's DSP crosses from vibration into pulling the steering
  axis; real games stream far below the cap.
- **`wheel_rev_level` (0-10) for the real G PRO rim** - level-based
  rev lights per the TF4ALL G HUB capture decode; the RS50's per-LED
  RGB `wheel_led_*` attributes are hidden on a real G PRO (different
  rim hardware) and vice versa. Untested on real hardware; needs a
  G PRO owner.
- **G923 PIDs added to the udev rule** (c266/c26d/c26e): the G923
  speaks the same TrueForce protocol, so hidraw access lets Logitech's
  SDK DLLs reach it under Proton the way they do for RS50/G PRO.
  Untested; needs a G923 owner.
- Protocol spec corrected: the Windows game-FFB path for these wheels
  is HID++ 0x8123 fn2 (the endpoint stream is the TrueForce/SDK
  session channel and overrides it); stream rates up to ~1000 pkt/s
  observed (AC EVO).

### Overnight hardening pass (2026-07-06)

- **Fixed a regression for Unifying/Lightspeed-paired devices**: the
  device-index answer check added earlier this cycle made every HID++
  sync command on receiver-paired mice/keyboards eat the full timeout
  (the DJ transport rewrites the wire index after the driver's
  snapshot). The check is now applied only to the direct-drive wheels
  it was written for.
- Real G PRO: connect-time LIGHTSYNC initialisation no longer runs
  (wrong protocol dialect for that rim); `wheel_rev_level` hardened
  (pacing underflow, send serialisation, honest errno).
- TrueForce stream: texture window advances only when the packet
  actually queued; session wind-down sends one recentre packet, not
  one per retry.
- New **`wheel_response_curve`**: the steering axis's 64-point
  response curve (G Hub's Sensitivity slider, feature 0x80A4) -
  write `in:out` pairs or `reset`. Implemented from captures, needs
  live validation.
- libtrueforce: all -Wformat-truncation warnings fixed; sparse,
  smatch and both CI kernel builds clean across the week's changes.

### Second overnight batch (2026-07-06)

- **`wheel_pedal_response_curve`**: hardware response curves for the
  pedal unit's three axes (feature 0x80A4 on HID++ sub-device 0x02),
  sharing the steering attribute's upload core. En route, the
  sub-device send helper gained the LONG-report case it was missing
  (13-byte curve chunks would previously have been truncated to a
  SHORT report). Untested on hardware.
- **`wheel_rev_level` is now asynchronous and coalescing**: writes
  return immediately and the driver flushes only the newest level at
  the 160 ms cadence - a fast telemetry feeder no longer blocks
  ~160 ms per write or drains stale intermediate levels to the wire.
- Independent corroboration from our own captures: the 2026-01-26
  gameplay capture streams type-0x01 force packets at 999.8 Hz,
  matching the packet-paced 1 kHz model behind the unified stream.

### Per-model force strength and libtrueforce fixes (2026-07-06)

- **Per-model KF peak torque**: libtrueforce scaled every torque
  request against the RS50's 8 Nm ceiling, so on an 11 Nm G PRO a
  request for 8 Nm mapped to full scale (about 11 Nm actual, ~37% more
  than asked). Peak torque now resolves from the wheel's USB PID
  (RS50 8 Nm, G PRO 11 Nm), and the capability getters report the
  right value. G PRO figures are spec-derived, hardware confirmation
  requested in issue #28.
- **libtrueforce udev permissions gap fixed**: the rule matched only
  the RS50's USB ID (c276), silently locking G PRO owners out of the
  library without root; it now covers c276/c272/c268. File renamed to
  `99-logitech-trueforce.rules`.
- **Dead code removed**: the `gpro_sysfs_init` settings-only path was
  unreachable (the G PRO runs the direct-drive init that already
  provides the full settings surface) and was deleted, along with the
  write-only `is_gpro` marker. No behaviour change.
- Final naming sweep: the remaining `rs50` references in code comments,
  the libtrueforce sources, and the udev rule labels are generalized to
  the direct-drive family where they no longer meant the RS50
  specifically. Em-dashes and en-dashes removed from the tracked docs.

### Naming generalized to the whole direct-drive family

- **dmesg lines are now tagged with the actual wheel model** instead
  of a hardcoded `RS50:`: `RS50 (native):`, `RS50 (G PRO compatibility
  mode):`, or `G PRO:`, resolved from the bound identity at log time.
  The RS50 spoofs the G PRO product ID in compatibility mode but keeps
  its own USB product string (verified live); a real G PRO reports
  "PRO Racing Wheel" (verified from contributor captures) - so the
  compat tag doubles as a mode indicator when debugging.
- Driver symbols renamed `rs50_*` -> `hidpp_dd_*` ("dd" = direct
  drive), quirk `HIDPP_QUIRK_RS50_FFB` -> `HIDPP_QUIRK_DD_FFB`. No
  functional change; no sysfs name changes (those were already
  generic).
- User-facing artifacts renamed: udev rule
  `70-logitech-rs50.rules` -> `70-logitech-trueforce.rules`
  (`dkms-update.sh` removes the old installed filename),
  `oversteer-rs50-support.patch` -> `oversteer-logitech-trueforce.patch`,
  `docs/RS50_PROTOCOL_SPECIFICATION.md` ->
  `docs/PROTOCOL_SPECIFICATION.md` (redirect stub kept).

### Fixed

- **Every HID++ settings command stalled ~5 seconds** (introduced by
  the device-index answer check earlier in this cycle, caught the same
  day via usbmon): the answer matcher compared against a question
  snapshot taken before the transport applied the 0xff device-index
  default, so every first attempt was rejected and only an accidental
  retry-on-timeout made calls succeed. Symptoms: Oversteer appearing
  to hang, `wheel_profile_names` taking up to 50 s, deferred init
  taking minutes. Now the default is applied before the snapshot;
  range writes measure 4 ms.
- **udev permissions race**: the permissions rule fires on the hidraw
  "add" event, which is emitted before probe creates the `wheel_*` /
  compat attribute files, so a plug or driver load could leave the
  settings root-only until a manual `udevadm trigger`. The driver now
  emits a "change" uevent after creating its sysfs group so udev
  replays the rule with the files present.
- **Teardown and concurrency fixes** from adversarial review:
  HID++ answers are matched to questions by device index (a late
  sub-device reply can no longer satisfy a base-device wait and vice
  versa); the wheel sysfs group is removed at the start of teardown,
  closing a window where a store could re-arm the effect timer after
  the final delete (use-after-free); interface 0 no longer takes the
  owner teardown path via its cached FF pointer, and the owner
  invalidates that cache before freeing (use-after-free on partial
  unbind); sysfs handlers and the range-restore worker re-check the
  teardown flag between sync HID++ sends (teardown could stall for
  the full send-timeout multiple).
- **Autocenter is now independent of the game's FF_GAIN**: gain is
  applied to the summed game effects only, then the autocenter spring
  is added - a leftover low gain from a game no longer silently kills
  the user's centring force (matches hardware-autocenter semantics on
  other wheels).
- **Pre-release review hardening**: a non-finite (NaN) KF torque
  request from game force code slipped past the clamp in libtrueforce
  and reached an undefined int16 cast - an unbounded command to a
  direct-drive motor - now treated as zero force; the response-curve
  pair parser rejected trailing junk (`30000:40000x`, `5:5:5`) instead
  of silently accepting the numeric prefix; and two dead `if (ff->wq)`
  guards left by the settings-only path removal were dropped.

### Oversteer

- The bundled Oversteer patch now unlocks the full settings set
  (`gain`, `autocenter`, `spring_level`/`damper_level`/
  `friction_level`, `combine_pedals`) for both G PRO product IDs, not
  just `range` - real G PRO owners get the same Oversteer integration
  as the RS50.

## 0.10.0 - 2026-07-03

188 commits since the `v0.9-pre-simplification` tag on 2026-02-02.
Rather than enumerate all of them, this entry groups them by theme.
See `git log v0.9-pre-simplification..v0.10.0` for the full
chronology.

### The 90-degree saga closed; profile slots done right (2026-07-03)

- **Launch-time 90-degree reset: root-caused and fixed.** A usbmon
  capture of an AC EVO launch showed the game's SDK session pushing an
  operating range of 90 degrees in a TrueForce interface-2 packet
  (type 0x0e - previously misdocumented as a frequency config; its
  canonical init value 2700.0 is the wheel's max range). The new
  `wheel_range_restore` (default on) restores the pre-reset range
  automatically - detection to restore measured under 100 ms against
  a faithful replay of the game traffic - behind safety gates:
  external-and-exactly-90 only, desktop mode only, wheel stationary,
  widen-only, three strikes per session, explicit writes supersede.
  Game-side alternative documented: AC EVO's "Steering lock" setting
  pushes its configured value once touched and re-applied.
- **Profile slot select settled against the wheel's OLED**: fn2 SET is
  the plain profile index (a capture-note misparse had briefly
  suggested a [mode_class, slot] encoding; writing that switches to
  profile 2), fn1 GET returns [profile, mode], and fn3 returns each
  slot's user-assigned NAME - exposed as the new read-only
  `wheel_profile_names` attribute.
- Firmware behaviours documented from the reproduction work: type-0x0e
  is session-scoped, and an idle TF session's range change is
  reverted by the firmware itself after about a minute.

### Hardening, identity, and protocol resolutions (2026-07-02, later)

- **Ten review findings fixed** (commit `c2b3a65`) after an adversarial
  self-review of the KF/TF work: the TF session init and the range
  read-back no longer share a workqueue with the 500 Hz force stream
  (either could stall steering forces); a use-after-free window on
  unplug during TF init is closed; an effect's channel is decided at
  playback start and held (no mid-play migration); fast periodics keep
  their DC offset on the steering axis; spring damping respects the
  effect's saturation caps; TF START/STOP state only advances when the
  packet actually queued; failed TF init retries (bounded); steering
  packets get queue priority over texture; and the profile SET/GET
  wire format is per-device (the GET had been reading onboard slots
  back as "profile 2").
- **New sysfs: `wheel_serial` and `wheel_firmware`** - the real
  12-character serial (matches the USB descriptor) and the firmware
  versions of the wheel base and the motor unit, read from HID++
  DeviceInfo at init and logged in dmesg. Include `wheel_firmware`
  output in bug reports.
- **LED effects 6-9 accepted** - the wheel advertises nine effects,
  not five (live-verified supported-effect list); 6-9 are not yet
  visually labeled. External LED-effect and brightness changes (G Hub
  style tools, the wheel's own menu) now update the sysfs values via
  the wheel's broadcasts instead of going stale.
- **libtrueforce: `logitf_get_stream_feedback()`** - the stream thread
  consumes the wheel's type-0x02 responses (real-time position,
  device-side sample counter); a Linux-native API extension.
- **Protocol documentation majorly extended** - the three
  long-standing unknown features resolved (axis response curves /
  report-HID-usages / brake force), the sub-device map (display
  module, pedal base, motor unit), HID++ error packets, SW_ID and
  0x12-report semantics from Logitech's official specs, DeviceInfo
  identity decode, and corrected feature-catalog rows. See
  docs/RS50_PROTOCOL_SPECIFICATION.md sections 5 and 9.

### Project renamed (2026-07-02)

`logitech-rs50-linux-driver` is now **`logitech-trueforce-linux-driver`**:
the driver covers the whole Logitech TrueForce direct-drive family
(RS50 and G PRO today), not just the RS50, and the name should say so.
Old GitHub URLs and clone remotes redirect automatically. No change for
installed systems: the kernel module and DKMS package were always named
`hid-logitech-hidpp`.

### KF/TF separation and FFB stability (2026-07-02)

- **In-kernel TrueForce texture channel** (`wheel_texture_route`,
  default `tf`). Vibration-class effects (`FF_RUMBLE`, periodic
  effects at 20 Hz or faster) now stream on the wheel's TrueForce
  audio-haptic channel instead of being summed into the steering
  force, matching the Windows KF/TF split. Fixes the "gritty/notchy
  steering under rumble" A/B from issue #8. The TF session init
  (68-packet capture replay, twice) runs lazily on first texture
  playback; verified live on an RS50 (audible texture playback with
  the steering axis still). Texture amplitude respects `FF_GAIN` and
  `wheel_strength` (the firmware does not scale TF samples itself).
- **Spring damping** (`wheel_spring_damping`, default 25%). Emulated
  `FF_SPRING` now carries a synthetic damping term scaled by the
  spring's own coefficient. An undamped host-emulated spring rings on
  a direct-drive motor because of the position-to-force loop latency;
  observed live as AC EVO's map-load centring force oscillating the
  wheel into its over-torque failsafe.
- **Friction chatter fix.** `FF_FRICTION` now ramps through a small
  velocity stick zone (Karnopp model) instead of slamming full-scale
  force on every sign flip of the per-tick encoder delta, which
  buzzed the rim at up to 500 Hz when turning slowly.
- **Honest rotation-range reporting.** Some game launches silently
  reset the physical range to 90 degrees with no HID++ broadcast
  (AC EVO observed); the driver now re-reads the true range on its
  20 s keepalive cadence, updates `wheel_range`, logs the external
  change, and notifies sysfs pollers. Detection only - the driver
  never writes the old range back on its own (unsafe under active
  FFB on a direct-drive wheel).
- **Onboard slot select fixed in compat mode.** `wheel_profile`
  writes for slots 1-5 now encode `[0x02, slot, 0]` per the G Hub
  capture instead of `[slot, 0, 0]`, which had put the slot number
  in the mode-class byte (only desktop mode happened to work).
  On-wheel slot confirmation still pending.
- **Effect-upload debug logging** now includes the full parameters
  (condition coefficients, periodic waveform/period/magnitude, ...)
  for root-causing feel issues via dynamic debug.

### Verified game support (2026-04-26 / 2026-04-29)

End-to-end gameplay verified under Proton on Linux:

- **Assetto Corsa Competizione** (RS50 in G PRO compatibility mode)
- **Assetto Corsa EVO** (RS50 in G PRO compatibility mode)

Both produce full FFB, TrueForce haptics, and complete button /
paddle / encoder binding. The setup is documented as the
"SDK-aware sims" recipe in the README and uses Logitech's own
Authenticode-signed SDK DLLs running unmodified inside Wine via
`tools/install-tf-shim.sh`. No DLL injection, no IAT hooks, no
certificate spoofing. The same setup is expected to work for the
other Logitech-SDK-aware sims (LMU, AMS2, AC, rF2 + Logitech
plugin, iRacing) because they all link against the same SDK.

### Added

- **Full force feedback effect set** via software emulation on top
  of the RS50's constant-force endpoint (commit `d5b7cc0`). The
  driver now accepts and produces `FF_SPRING`, `FF_DAMPER`,
  `FF_FRICTION`, `FF_INERTIA`, `FF_RAMP`, `FF_PERIODIC`
  (SINE/SQUARE/TRIANGLE/SAW_UP/SAW_DOWN) and `FF_RUMBLE` (approximated
  as a low-frequency square shake on the single motor) in addition
  to `FF_CONSTANT`. Condition effects read the live wheel position,
  velocity and acceleration sampled from interface-0 input reports
  at the 500 Hz timer cadence. Motivated by ACC which uploads
  thousands of DAMPER effects and essentially no constant forces,
  revealing the previous constant-only behaviour as a feel-killer.
- **`wheel_ffb_constant_sign` sysfs attribute** (`d7dc398`). Toggles
  the FF_CONSTANT sign compensation the driver applies to line up
  Wine/Proton's DirectInput path with our wire format. Default
  `1` (invert, matching what ACC under Proton expects); set `0` for
  native-evdev apps (`fftest`, SDL FF, custom tools). Only affects
  FF_CONSTANT; condition effects, ramp, periodic, and rumble feel
  identical at either setting. See `docs/SYSFS_API.md` for the full
  rationale and the troubleshooting section in the README for the
  user-facing story.
- **FF-matrix test harness** in `tests/ff_matrix_test.c` + Makefile.
  Walks every effect-type × parameter-combination for uploads
  (16 cases including inverted envelopes, negative coefficients,
  non-zero replay.delay, all periodic waveforms) and observes
  ABS_X motion for CONSTANT direction, RAMP ramp-up, PERIODIC sine
  oscillation, CONSTANT attack envelope, and SPRING centering.
  Auto-toggles `wheel_ffb_constant_sign` off during motion checks
  so the native-convention assertions stay coherent. Found several
  of the bugs below.
- **G PRO Racing Wheel support**, both Xbox/PC (`046d:c272`) and PS/PC
  (`046d:c268`) variants. FFB via the G920-class HID++ 0x8123 path on
  interface 1, TRUEFORCE streaming via the same interface 2 endpoint 0x03
  that the RS50 uses. Every `wheel_*` sysfs attribute relevant to the
  G Pro's hardware is exposed. `gpro_sysfs_init` discovers the
  per-feature SET function numbers and any G Pro-specific sub-device
  features at init time.
- **Wheel calibration** via a new write-only sysfs attribute
  `wheel_calibrate`. Writes a 0..65535 raw encoder value that the wheel
  adopts as the new centre reference. Backed by sub-device `0x05`,
  feature page `0x812C`, function 3 (matching what G Hub does when the
  user clicks Calibrate). Originally only wired up on the G Pro;
  commit `1ed2d80` enabled the same path on RS50 once an RS50 G Hub
  capture (`2026-04-22_re_calibrate.pcapng`) confirmed the sub-device
  layout matches. Closes issue #13.
- **TRUEFORCE full-stack userspace support** in `userspace/libtrueforce/`.
  A shared library that speaks the 64-byte report ID 0x01 stream on
  interface 2 directly via hidraw. Handles the 68-packet two-pass init
  exactly as G Hub does (verified byte-for-byte against both wheels
  across multiple games). Exposes the full Logitech Steering Wheel SDK
  entry-point surface (discover / open / close, set / get torque, TF
  streaming, angle and angular velocity, operating range, damping,
  gain). Forwards range / damping / TF gain to the kernel's `wheel_*`
  sysfs knobs so the library and the driver never disagree.
- **Wine PE shim scaffolding** at `userspace/tf_wine_shim/` (later
  retired - see Removed below). Built a `trueforce_sdk.dll.so` via
  winegcc as an alternative path for Proton games that cannot load
  Logitech's real signed SDK DLL. The real-DLL approach in
  `tools/install-tf-shim.sh` superseded it before end-to-end
  verification, so the shim was moved to `dev/userspace/` (commit
  `08e1c55`).
- **Profile / rotation broadcasts** on interface 1. The wheel emits
  unsolicited notifications on profile button press and rotation-range
  changes; the driver now consumes both and updates cached sysfs state,
  including re-querying dependent settings after a mode change.
- **Onboard and desktop profile/mode support** via `wheel_mode` and
  `wheel_profile`. Switching between `desktop` and onboard profile 1-5
  applies the correct active profile to the wheel and invalidates the
  settings cache so the next sysfs read reflects reality.
- **LIGHTSYNC custom slot control** on RS50. Five user-configurable
  slots with per-LED RGB, per-slot effect/direction, brightness, and
  slot-name write. LED configuration writes are transactional (apply +
  commit) to match G Hub's behaviour.
- **Capture scripts for reverse-engineering** (originally tracked in
  `tools/`, since moved to `dev/tools/` in commit `eb726da` so the
  public repo only carries end-user-relevant tooling). Used to
  decode the G PRO compatibility-mode HID++ feature catalog and the
  desktop-mode entry sequence.
- **CI coverage for userspace**: GitHub Actions builds libtrueforce
  on every push and runs the wire-conversion unit tests
  (`make check`). The earlier Wine PE shim CI job was dropped in
  commit `c4e96b0` after the shim itself was retired (see Removed
  below). Kernel driver continues to build against 5.15 and 6.8.

### Fixed

- **FFB command queue could grow without bound** (issue #8, G920 /
  G923 / G Pro HID++ 0x8123 path): a game replaying a constant force
  re-uploads and re-plays the same effect far faster than the wheel's
  ~300 command/s HID++ drain rate, and the send queue had no coalescing
  or backpressure, so it could reach thousands of entries
  ("command queue contains N commands") and stall feedback. The queue
  now collapses a run of identical-key updates to the latest pending
  one, mirroring how G Hub only ever sends the current state of an effect
  at the device's pace. Implemented as a single drain worker over a
  coalescing FIFO; also switches the queue allocation to GFP_ATOMIC since
  the playback path runs in atomic context. Builds on 6.x and 7.x;
  verified to load and not affect the RS50 path, which uses a separate
  timer-push FFB design and was never affected. Needs confirmation on
  real G920-class hardware.
- **D-pad directions scrambled** (issue #22): the hat reported wrong
  directions in game binding screens, most visibly Left registering as
  Down. Interface 0's HID descriptor already declares a standard hat
  switch that the kernel maps correctly, but the driver also ran a
  hand-rolled byte-0 decode based on a non-standard encoding and emitted
  its own (wrong) hat frame ahead of the correct one. A binding screen
  latches the first frame, so it saw the wrong direction. The redundant
  decode was removed and the native hat mapping left to do its job.
  Verified on a live wheel: Up/Right/Down/Left all report correctly with
  no spurious frames.
- **Build break on kernel 7.x** (issue #24): `hid_report_raw_event()`
  gained a `size_t bufsize` parameter ("HID: pass the buffer size to
  hid_report_raw_event", mainline v7.1, backported into the v7.0.x
  stable series). Because the change was backported partway through a
  point-release range, two kernels with the same `x.y.0` base can carry
  different prototypes, so a `LINUX_VERSION_CODE` check is unreliable.
  Kbuild now probes the actual argument count by syntax-checking a
  six-argument call against the target kernel's own headers and passes
  the new buffer size when present. Builds on 6.x and 7.x with both gcc
  and clang.
- **rmmod regressions on live RS50**: two destroy-path crashes. The
  `ff_hdev` pointer cached on interface 1 became stale if interface 2's
  `hidpp_remove` ran first during rmmod, producing a null-ptr deref
  inside `hid_hw_close`. The thin-probe interfaces also left the
  `hidpp_device` work_structs uninitialised, tripping
  `WARN_ON_ONCE(!work->func)` in `cancel_work_sync`. Both resolved
  (995607f, simplified in 8ab5fc4).
- **FFB filter byte-0 bitfield**: earlier analysis modelled byte 0 as a
  single flag with a per-wheel offset. Cross-capture re-analysis
  decoded it as `bit 0 = user explicit, bit 2 = auto`, identical on
  RS50 and G Pro (63999d8).
- **RS50 damping and trueforce SET function numbers**: damping uses
  fn=1 and trueforce uses fn=3, not the default fn=2 both paths used
  to send. The G Pro init block already had the overrides; the RS50
  path was missing them (c2ee83e).
- **G Pro FFB filter SET**: corrected fn=3 to fn=2 and auto-flag
  encoding to `0x01 / 0x05` after capture analysis on a live G Pro
  (09e2a6c).
- **Profile broadcast handler** previously gated on the wrong nibble of
  the HID++ function byte; missed broadcasts meant the cached
  `wheel_profile` went stale on profile-button presses. Fixed to gate
  on `sw_id == 0` (46914ad).
- **G Pro interface-0 probe path** and the G Pro / RS50 hid_hw_init
  interface iteration (d1a1bd4, 8106b3a) address sixtysecondstosmash's
  "fftest shows 0 effects" report in issue #8. Retest on G Pro still
  pending user confirmation.
- **C90 compliance** on kernel 5.15 builds: three recent additions
  slipped through with C99 inline declarations that the Ubuntu-22.04
  build rejects under `-std=gnu89`. Rolled back to C90-clean
  declarations (7249eef).
- **Batch script line endings**: `tools/*.bat` scripts were LF-only,
  which broke `call :label` resolution in Windows `cmd.exe` past a
  certain file size. Forced CRLF via `.gitattributes` and `-text`
  (35d0eb4).
- **TRUEFORCE init sent twice, not once**: libtrueforce originally
  replayed the 68-packet init on session open but stopped after one
  pass. Live G Hub captures on both wheels show a duplicate pass with
  the sequence counter reset to 1; the library now matches that
  (0aebf70).
- Many smaller correctness fixes: FF_GAIN scaling in the constant-force
  path, constant_force accesses paired under `WRITE_ONCE`/`READ_ONCE`,
  timer re-arming on zero-force release, pedal deadzone overlap
  rejection, rate-limited FFB error counters, wheel_sensitivity numeric
  return in onboard mode, sysfs_emit for show handlers, LIGHTSYNC
  probe cleanup, LED stores that write the device before updating the
  cache.
- **Sensitivity cache aliasing** correctly gated on `mode_known` so a
  failed mode query no longer caches an LED-brightness value as wheel
  sensitivity (`a99847b`).
- **Out-of-tree build portability**: dropped the `usbhid/usbhid.h`
  include and inlined the one `hid_to_usb_dev` macro we used from it,
  so builds succeed on Fedora, CachyOS, Arch and similar distributions
  whose kernel-devel package does not ship that internal header
  (`f2d212c`).

### Changed

- **Phase A audit closed**. The remaining Phase A findings were all
  worked through in commits `0d8918a` (7 trivial findings closed),
  `cc3e46a` (SYS.F29: sysfs attributes moved behind a single
  `attribute_group`, -67 lines), `0cd9fc7` (SYS.F41: extract
  `hidpp_errno` helper, -21 lines across 14 call sites), `934efb7`
  (SYS.F40: document the `params[2] = 0` padding convention), and
  `25fb739` (SYS.F21: split `rs50_ff_discover_features` into settings
  and LIGHTSYNC halves). The remaining strategic items (god-struct
  split, table-drive the settings handlers) were explicitly deferred
  with rationale recorded in `dev/docs/plans/STATUS.md`.
- **Protocol spec (`docs/RS50_PROTOCOL_SPECIFICATION.md`) bumped to
  v6.1**, rescoped to cover both RS50 and G Pro, D-pad rewritten from
  4-way to 8-way, per-feature SET function numbers tabulated,
  centre-calibration section added.
- **TRUEFORCE doc** rewritten from "research only" to the current
  implementation state, including the library layout, the two-pass
  init, and the wheel-coverage table.
- **SYSFS API, README, RS50_SUPPORT** brought in sync with the code.
- **USB_CAPTURE_GUIDE** broadened from "G Pro-specific" to "any
  Logitech wheel beyond the two we already support", with references
  to the `tools/windows_*_captures.bat` scripts and updated protocol
  background.

### Removed

- **`userspace/tf_wine_shim/`** moved to `dev/userspace/`
  (gitignored) in commit `08e1c55`. It was Phase 23.1 scaffolding,
  never end-to-end-verified, and superseded by
  `tools/install-tf-shim.sh`, which copies Logitech's own
  Authenticode-signed SDK DLLs into Wine prefixes. The CI job that
  built the shim was dropped in `c4e96b0`.
- **Reverse-engineering / capture tooling** moved to `dev/`
  (commit `eb726da`): `docs/RS50_SUPPORT.md`,
  `docs/USB_CAPTURE_GUIDE.md`, `docs/WINDOWS_RE_CAPTURE_GUIDE.md`,
  `tools/windows_gpro_compat_capture.bat`,
  `tools/windows_gpro_compat_range_capture.bat`,
  `tools/windows_tf_captures.bat`,
  `tools/windows_wheel_captures.bat`. These are contributor /
  maintainer tools not needed by end users; the public repo now
  carries only end-to-end driver files plus user-facing docs.

### Documentation

- New `userspace/libtrueforce/tests/unit.c` covering the wire-format
  conversions with a 65536-sample monotonicity sweep.
- Phase B gap analysis (`dev/docs/plans/2026-04-16-windows-gap-analysis.md`)
  and the Phase A audit (`dev/docs/plans/2026-04-16-code-audit.md`)
  are archived; `dev/docs/plans/STATUS.md` maps each rank and finding
  ID to its current shipping state.

## v0.9-pre-simplification (2026-02-02)

Tagged snapshot before the simplification + audit sprint. RS50-only,
FFB constant force via the existing `rs50_ff_*` path, basic sysfs
settings, LIGHTSYNC per-slot writes. See `git log
v0.9-pre-simplification` for the full history up to that point.
