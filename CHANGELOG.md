# Changelog

This project follows a loose semver: major versions mark API-breaking
changes to the sysfs surface, minor versions add supported wheels or
new attributes, patch versions are bug fixes and documentation. Pre-1.0
the contract is "it works on RS50 and G Pro as listed here".

## Unreleased

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
