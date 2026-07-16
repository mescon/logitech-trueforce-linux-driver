# Hardware Pedal Shaping (0x80A4) Design

**Status:** draft for review
**Date:** 2026-07-16
**Supersedes:** the HID-BPF pedal-shaping approach (see "Why not HID-BPF").

## Goal

Restore G HUB-equivalent pedal shaping (throttle / brake / clutch response
curves, per-pedal sensitivity, per-pedal deadzones) plus a logi-dd advanced
curve editor, using the wheel's own hardware `0x80A4` response-curve feature on
the pedal sub-device. Bring the driver to parity with G HUB's Pedals panel.

## Why this, and why not HID-BPF

The v0.14.0 removal of the pedal-curve attributes, and the entire HID-BPF pedal
shaper, both rested on one untested capture that concluded "the pedal MCU stores
a `0x80A4` curve but does not apply it to its PC HID output."

That conclusion is **false**, proven on hardware 2026-07-16. The pedal MCU
(HID++ device index `0x02`, `0x80A4` at feature index `0x0a`) applies an
uploaded curve to the axis it reports to the PC, in desktop mode, exactly like
the steering axis on the base. Artifact-proof test: a DOUBLE-plateau curve
(physical throttle 8000-25000 -> 6000, 40000-58000 -> 50000) produced two
separate dwell spikes at exactly 6000 and 50000 with an empty middle. No
monotonic press on a linear axis can dwell at two separated values while
skipping the gap, so this is proof, not inference. The earlier July "no effect"
was a false negative from noisy manual testing; the July Windows raw-HID
"linear" capture was desktop-mode with G Hub running, where G Hub reverts the
hardware curve and shapes in software. Throttle evdev axis = `ABS_RX`; brake and
clutch are axes 1 and 2 on the same feature.

So pedal shaping needs no BPF, no userspace HID-report rewriting, and no split
of the FFB-forwarding path. It is the same mechanism the validated
`wheel_response_curve` (steering) already uses, pointed at three more axes on a
different sub-device. This design uses that mechanism.

## Global Constraints

- GPL-2.0, no PII, no AI/LLM mentions in code/commits/docs.
- Keep FFB / TrueForce untouched.
- No em-dashes or en-dashes in any output.
- The wheel stores exactly ONE curve per axis. Sensitivity and deadzone are
  curve *generators*, not independent hardware state. This must be honest in the
  sysfs surface: reads reflect the true loaded curve, not a remembered slider.
- Reuse the existing, capture-verified `0x80A4` upload machinery
  (`hidpp_dd_response_curve_upload` / `_revert`) and the steering sensitivity
  Bezier generator. No new protocol invented for curves or sensitivity.

## Architecture

Three layers, phased so each is independently shippable:

- **Phase 1 (driver, sysfs):** pedal-axis `0x80A4` curves, sensitivity, and
  deadzone attributes. Reuses existing machinery. Hardware-validated.
- **Phase 2 (logi-dd):** registry entries + the advanced curve editor TUI.
  Pure userspace, unit-tested with `FakeSysfs`.
- **Phase 3 (combined pedals):** protocol IS known from the 2026-07-14 capture
  (feature idx `0x0e` fn1 bool: `10 ff 0e 1a 01` on / `00` off, desktop-only).
  Lower priority (off for modern sims) and needs its own hardware validation
  that it transforms the PC HID output, so it is sequenced last but is not
  blocked on any new capture.

### Data flow (one pedal)

```
G HUB-simple:  sensitivity 0-100  ─┐
G HUB-advanced: point list+deadzone ┼─> 64-point 0x80A4 curve ─> dev 0x02 axis a
sysfs deadzone: "L U"              ─┘         (last write wins; the wheel
                                               holds exactly one curve/axis)
```

The wheel's axis holds one curve. Whichever attribute wrote last defines it.
`wheel_<p>_curve` always reads back the true loaded/max point count from the
device (fn1), so it is the source of truth regardless of which generator wrote.

## Phase 1: Driver sysfs surface

For each pedal `p` in {`throttle`, `brake`, `clutch`} mapping to `0x80A4` axis
`a` in {0, 1, 2} on device index `0x02`:

### `wheel_<p>_curve` (RW) — the canonical curve

Mirrors `wheel_response_curve` exactly, only `dev_idx=0x02, axis=a, idx=idx_pedal_curve`:

- Write `reset` -> `hidpp_dd_response_curve_revert(hidpp, 0x02, a, idx_pedal_curve)`.
- Write 2-64 whitespace-separated `in:out` pairs (decimal 0-65535, strictly
  increasing `in`, non-decreasing `out`, starting `0:0`, ending `65535:65535`)
  -> `hidpp_dd_response_curve_upload(...)`. Fewer than 64 pairs are resampled to
  64 by the existing uploader.
- Read -> `"<loaded>/<max> points loaded (0 = built-in curve)"` via fn1, exactly
  like the steering show.
- `-EOPNOTSUPP` if `idx_pedal_curve == HIDPP_DD_FEATURE_NOT_FOUND`.

### `wheel_<p>_sensitivity` (RW) — the simple slider

Reuses the steering Bezier generator verbatim (`wheel_sensitivity_store`'s
symmetric cubic Bezier: P1=(px,py), P2=(py,px), px=(100-s)*65535/100,
py=s*65535/100, sampled at 64 points), uploaded to `dev 0x02 axis a`:

- Write `0-100`, clamped. `50` -> fn6 revert (built-in linear), matching G HUB.
- Otherwise generate the 64-point curve and upload.
- Read -> the last written value (cached in `ff`, like steering; the wheel does
  not store the slider position, only the resulting curve).

### `wheel_<p>_deadzone` (RW) — lower/upper dead travel

A convenience generator for the common "linear with dead ends" shape:

- Write `"<lower> <upper>"`, each `0-100` percent, `lower + upper <= 99`.
- Composes a 4-point curve: `0:0`, `(lower%*65535):0`, `((100-upper)%*65535):65535`,
  `65535:65535` and uploads it via the shared uploader (which resamples to 64).
- Read -> the last written `"<lower> <upper>"` (cached in `ff`; not recoverable
  from the stored curve).
- `"0 0"` -> fn6 revert (built-in).

### Init

At probe, discover `0x80A4` on device index `0x02` via
`hidpp_root_get_feature_on_device(hidpp, 0x02, 0x80A4, &idx)` and cache it as
`ff->idx_pedal_curve` (default `HIDPP_DD_FEATURE_NOT_FOUND`). One extra root-get
at init; the pedal MCU already answers HID++ during pedal init, so no new
transport concern. Add `ff->pedal_sens[3]` and `ff->pedal_deadzone[3][2]` caches.

### Attribute count

9 new attributes (3 pedals x {curve, sensitivity, deadzone}). All in the
existing `attribute_group`. `combine_pedals` / `wheel_combined_pedals` are NOT
added here (Phase 3).

## Phase 2: logi-dd

### Core (`logi-dd-core`)

- Registry: 9 `SettingSpec`s in a new `Pedals` category. Curve attrs use the
  existing `Kind::Curve`. Sensitivity uses `Kind::Percent`. Deadzone needs a new
  `Kind::Pair { max: u8 }` that parses/formats `"L U"` into a
  `Value::Pair(u8, u8)`.
- `mode_req`: match the driver. Curve/deadzone are `Any` (no mode gating in the
  store); sensitivity mirrors whatever the driver enforces (steering sensitivity
  is desktop-only via `-EPERM`; pedal sensitivity store SHOULD match, so
  `DesktopOnly` if the driver gates it, else `Any` - to be fixed to agree with
  the Phase 1 code).

### The advanced curve editor (TUI)

A modal editor entered from a pedal curve row, chosen UX = point-list + live
ASCII preview:

- **Model:** an ordered `Vec<Point { input: u16, output: u16 }>` plus
  `lower_deadzone: u8`, `upper_deadzone: u8`. On save, compose these into a
  `Value::Curve` (deadzones become the flat end-segments; points sit between)
  and write via the existing `Device::write`.
- **Left panel:** selected point index (`< n / N >`), Input%, Output%, Lower
  deadzone%, Upper deadzone%, all editable by typing; `+`/`-` add/delete the
  selected point; arrows move between points.
- **Right panel:** a live ASCII plot (input on X, output on Y) redrawn on every
  edit, with the selected point marked.
- **Invariants enforced live:** inputs strictly increasing, outputs
  non-decreasing, endpoints pinned to `0:0` and `100:100` (the deadzones shift
  where the curve leaves 0 and reaches 100, they do not move the endpoints).
  Invalid edits are rejected in the editor, so a malformed curve never reaches
  the wheel.
- **Preview rendering** is a pure function `(points, w, h) -> Vec<String>`,
  unit-tested independently of the terminal.

### logi-dd testing

- Curve composition (points + deadzones -> `Value::Curve`) is a pure function
  with unit tests, including deadzone-only, sensitivity-shaped, and arbitrary
  curves.
- `Kind::Pair` parse/format/validate round-trip tests.
- Editor state transitions (add/delete/move/edit, invariant rejection) tested
  against the model, not the terminal.
- Registry coherence test extended for the new kinds.

## Phase 3: Combined pedals (protocol known, sequenced last)

`Kombinerade pedaler` merges gas+brake into one split axis for legacy games. It
is a separate mechanism from `0x80A4`, and its protocol IS known from the
2026-07-14 G HUB capture: **feature index `0x0e`, fn1, boolean** -
`10 ff 0e 1a 01` enables, `10 ff 0e 1a 00` disables, desktop-mode-only. It is
off for all modern sims, so it is the lowest priority. Implemented as
`wheel_combined_pedals` (RW 0/1) discovering `0x0e` at init, with the same
hardware validation the curves get (confirm it transforms the PC HID output, not
just stores state). Not blocked on any new capture. Sequenced after Phases 1-2.

## Testing and validation

- **Phase 1 hardware validation** (when the wheel returns to the Linux host):
  the throttle-curve application is already proven. Validate brake (axis 1) and
  clutch (axis 2) with the same plateau-and-control method (each is a small
  number of held presses). Validate that sensitivity and deadzone attrs produce
  the expected curve shape via the same axis recording. The cue goes in
  assistant text, never inside a recorded command; record in the background.
- **Phase 2** is fully testable without hardware (`FakeSysfs` + pure-function
  unit tests). Hardware smoke test: drive one real curve from the TUI and
  confirm the axis shapes.
- No regression to steering (`wheel_response_curve`), FFB, or TrueForce: the new
  code only adds attributes and one init-time feature lookup.

## Risks

- **Brake/clutch might gate or behave differently from throttle.** Low: same
  feature, same sub-device, adjacent axes. Validated before Phase 1 ships.
- **Last-write-wins across three generators per axis may confuse users.**
  Mitigated by `wheel_<p>_curve` always reading the true device state and by
  documenting the model in SYSFS_API.md.
- **Combined pedals capture may reveal a mechanism we cannot drive from Linux.**
  Contained: Phase 3 is isolated and does not affect Phases 1-2.
