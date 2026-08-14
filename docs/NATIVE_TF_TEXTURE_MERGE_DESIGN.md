# Native TrueForce Texture Merge - Design

**Date:** 2026-08-13
**Status:** Implemented, hardware-validated 2026-08-13 (RS50, module `9C1B5855`)

## Status update: implemented

Everything below shipped as designed, with two deltas found during review and
one framing question that came up more than once and was settled explicitly.

**Delta 1: the merge state lives in a devm shim, not on `ff` directly.**
The design below describes `ff->tm`/`ff->tm_lock`. Review hardening moved
that state onto its own `struct hidpp_dd_texmerge_shim`, devm-allocated
against interface 2's device and reached through `ff->tm_shim` (which
may be `NULL` across teardown), with the shim's own spinlock serializing
every read and write against the classifier - not just against other sysfs
writers, since the classifier reads the whole struct on every packet on the
ll_driver hot path.

**Delta 2: the oscillator gained a Nyquist harmonic guard.** In the
harmonic loop, any harmonic whose frequency would reach 0.45 of the sample
rate is skipped rather than accumulated, so high RPM cannot fold a harmonic
back down into the audible band as aliasing. The phase accumulator still
advances underneath a skipped harmonic, so it re-enters cleanly in range
without a phase jump.

**Framing: the synthesis-fitted-to-capture approach was raised and approved
explicitly, not assumed.** The game supplies no texture at all - every SDK
packet from AC EVO carries `byte10=0`. On Windows the buzz is not something
sent over the wire and decoded; it is G HUB inventing it from the game's RPM
on the host and merging it into the same stream. There was accordingly no
protocol to reverse-engineer for the texture itself, only a shape to fit
(`docs/TF_TEXTURE_RECIPE.md`, from the Windows capture). That this project
should do the same synthesis, in the kernel, rather than keep hunting for a
wire format that does not exist, was confirmed before implementation started
rather than discovered as a fallback partway through.

**Delta 3: validated against the live SDK stream, two overnight fixes
(2026-08-14).** The merge was re-validated against the LIVE Logitech SDK
stream rather than a replay - 11,217 of 11,219 real stream packets spliced,
sample rms 411.2 against the 411 capture-fit target, spectrum peak exactly at
the fed firing frequency, SDK `cur` bytes byte-identical throughout. Two
fixes came out of the same pass: the type-`0x0e` range-push decode was
corrected to the true wire layout (bytes 6-9, not 8-11) after capture
analysis, and teardown was hardened with `device_lock` validation after a
pre-existing close-path oops (`hid_hw_close` on an unbound `hid_device`) was
found during rmmod testing.

**Delta 4: the splicer stamps `pkt[11] = 0x0d` (2026-08-14).** The design
below said to leave `pkt[11]` untouched. That was the one-byte gate between
"wire-perfect stream" and "texture actually felt": bytes 10 and 11 are a
demux pair (`byte10 != 0` always pairs with `byte11 = 0x0d` in Windows
captures; no-sample packets carry `0x00 0x00`), and the wheel silently
discards the whole sample window of a `byte10 != 0` + `byte11 = 0x00`
packet while still honouring its cur bytes. The splicer now stamps `0x0d`
whenever it adds samples (commit `9b15131`); the merge is felt working on
hardware. See `docs/TRUEFORCE_PROTOCOL.md`, stream layout invariants.

## Goal

Add fine engine-texture haptics (the "buzz") to native TrueForce games under
Proton by having the kernel driver splice synthesized texture samples into the
Logitech SDK's own ep3 stream, leaving the SDK's base force bit-identical. This
mirrors how Windows G HUB merges texture into the same stream.

## Background: what is already proven

Native base force feedback for the SDK path works and was hardware-verified on
2026-08-13 (see memory `native-ffb-reproduced-2026-08-13`):

- The Logitech SDK (`trueforce_sdk_x64.dll`), running in the game under Wine,
  streams the base force on USB interface 2, endpoint 0x03, as 64-byte
  type-0x01 packets. The base force lives in `cur` (bytes 6-9). It writes these
  by writing to `/dev/hidraw7` (the interface-2 hidraw node); the kernel routes
  them to the USB interrupt-OUT endpoint.
- The game (AC EVO) provides base force but **no** texture: every SDK packet
  has `byte10 = 0` (zero audio samples). The rev test confirms it - directional
  force on curbs/walls, no buzz when revving.
- On Windows the buzz is synthesized by G HUB from the game's engine RPM and
  injected into this same stream as a **separate** source. The game itself
  never calls the SDK's `SetTorqueTF*` functions on either OS.
- Injecting texture through the game's own SDK session (`SetTorqueTFfloat`) does
  **not** work: the game never opens the SDK's TF channel, so the SDK drops the
  samples (verified: `byte10` stays 0 on the wire).

Conclusion: the correct merge point is the ep3 stream itself, in the kernel,
exactly where G HUB merges on Windows.

## Feasibility (confirmed against the driver source)

All the mechanics already exist in `mainline/hid-logitech-hidpp.c`:

- **Interception template exists.** The driver already overrides
  `hid_ll_driver` on interface 0 to intercept userspace output writes
  (`hidpp_dd_pid_ll_output_report` / `hidpp_dd_pid_ll_raw_request`, installed by
  `hidpp_dd_pid_install`, gated `if (ifnum != 0) return 0;`). `.raw_event` only
  sees input; the ll_driver override is the correct hook for output. We
  replicate this pattern on interface 2's `ff->ff_hdev`.
- **The packet builder exists.** `hidpp_dd_tf_queue_stream` already writes the
  exact type-0x01 layout: `pkt[0]=0x01`, `pkt[4]=0x01` (stream cmd),
  `pkt[5]=seq`, `cur` duplicated into bytes 6-9, `pkt[10]`=new-sample count,
  `pkt[11]=0x0d`, then 13 window slots of u16 (bytes 12+, each duplicated). We
  reuse this layout knowledge for the splice.
- **Interface-2 handle is held.** `ff->ff_hdev` is cached from
  `usb_get_intfdata(iface2)` and kept open; existing `READ_ONCE`/`WRITE_ONCE`
  guards handle the remove-race.
- **RPM out of the game is solved.** The dinput8 proxy relays the game's Escape
  telemetry as a UDP datagram (`LTFR`) to `127.0.0.1:20780` at ~62 Hz into
  the userspace daemon. Since 2026-08-14 the datagram is **32 bytes**: the
  28-byte version-2 layout (`rpm`+`max_rpm` among its fields) plus an
  appended f32 LE first-shift-light rpm at offset 28, append-only so old
  consumers keep reading the first 28 bytes (see
  `docs/SHARED_MEMORY_RELAY.md`).

## Scope

**In scope:** the native-merge path only - splice RPM-synthesized texture into
the SDK's existing ep3 stream for native-TF games.

**Out of scope (untouched):** the existing `wheel_texture_route` (`kf`/`tf`)
simulated-TF path, which serves a *different* class of games (non-SDK / evdev
force-feedback titles, where the driver owns the stream and texture comes from
uploaded evdev effects). Native-merge is a separate mechanism with a different
stream owner (the SDK) and a different texture source (the RPM oscillator). The
two are mutually exclusive per game and never both active. Native-merge is NOT
a third value of `texture_route`.

## Architecture

Four units, each independently testable:

1. **Interceptor** - an `hid_ll_driver` override on `ff->ff_hdev` that inspects
   every outgoing packet and, when the merge is active, hands stream packets to
   the splicer before forwarding to the real `output_report`.
2. **Splicer** - a pure function: given an outgoing 64-byte packet and a block
   of texture samples, preserve `cur`/`seq`, set `byte10` and the window slots,
   return the modified packet.
3. **Oscillator** - a pure, fixed-point engine-note generator: given RPM and the
   tuning parameters, produce the next block of offset-binary u16 samples,
   phase-continuous across calls.
4. **RPM feed** - a sysfs input the userspace daemon writes, plus a freshness
   timestamp.

### Unit 1: Interceptor

Install the ll_driver override on `ff->ff_hdev` **once**, at the same point the
driver sets up interface 2 (where `ff->ff_hdev` becomes valid and is opened),
mirroring `hidpp_dd_pid_install` but for `ifnum == 2`. Save the real
`ll_driver`, swap in a copy whose `output_report` (and `raw_request`, for the
SET_REPORT fallback path the SDK may use) point at our wrapper. Restore the real
`ll_driver` on teardown, reusing the interface-0 pattern's ordering and guards.

The wrapper is a **pure pass-through by default**. It splices only when all of:

- `wheel_tf_merge` (sysfs, default 0) is 1,
- the packet is report-id 0x01 and `pkt[4] == 0x01` (stream) and
  `pkt[10] == 0` (never clobber a stream that already carries samples),
- RPM is fresh (written within a staleness window, ~200 ms) and above an idle
  threshold.

Otherwise it forwards the packet unchanged. With the feature off or no RPM
being fed, ep3 is byte-identical to today: zero regression risk, and the base
FFB path is untouched.

### Unit 2: Splicer

`splice(pkt[64], samples[], count)`:
- Assert/require `pkt[0]==0x01 && pkt[4]==0x01`.
- Leave bytes 0-9 (`cur` + `seq` + header) untouched.
- Set `pkt[10]` = new-sample count, stamp `pkt[11] = 0x0d` (see Delta 4: the
  wheel discards the window of a sample-carrying packet without it), and
  write the window slots (bytes 12+) using
  the same duplicated-u16 layout as `hidpp_dd_tf_queue_stream`.
- Sample count and window semantics **match the existing builder** so the
  wheel's stream engine sees a well-formed stream.

Pure and unit-testable: assert `cur` bytes are identical before/after; assert a
known sample block lands in the expected slots.

### Unit 3: Oscillator

Fixed-point, no kernel floating point (sine lookup table + integer phase
accumulators). Per call, produce the next `count` samples:

- **fundamental** `f0 = RPM/60 * (cylinders/2)` (4-stroke firing frequency);
  `cylinders` is a sysfs param (default 4 since commit `90a7489`; the design
  originally said 8, but higher counts push the firing frequency above what
  a direct-drive rim can express - excursion falls as 1/f^2 - so 4 feels
  right and the knob is a feel setting).
- **harmonic stack** h1..h5 with gains interpolated by RPM and an amplitude
  curve, seeded from `docs/TF_TEXTURE_RECIPE.md` (which fits the Windows
  capture). Values there are a starting point; the sysfs knobs make them
  tunable live.
- **phase-continuous** accumulators carried across calls so the waveform is
  seamless packet-to-packet.
- output: signed sample -> offset-binary u16 (same conversion class as
  `hidpp_dd_force_to_offset_binary`), clamped.

Tuning knobs, all sysfs, live, no rebuild: `wheel_texture_intensity` (overall
amplitude), `wheel_texture_cylinders`, and the harmonic-gain profile. These are
what make matching the Windows feel a fast feel-adjust-feel loop.

### Unit 4: RPM feed

- New sysfs `wheel_texture_rpm`: write `rpm` (and `max_rpm` for limiter feel).
  The store records the value and a timestamp; a stale value (older than the
  staleness window) disables splicing so a dead feed cannot leave a droning
  wheel.
- The userspace daemon that already receives the proxy's UDP relay is extended
  to write this sysfs at ~62 Hz. No new transport; reuses the existing chain
  (proxy Escape hook -> UDP 127.0.0.1:20780 -> daemon -> sysfs).

## Data flow

```
game engine RPM
  -> dinput8 proxy Escape hook (in-game, under Wine)
  -> UDP LTFR datagram 127.0.0.1:20780 @ ~62 Hz
  -> userspace daemon
  -> write wheel_texture_rpm (sysfs) @ ~62 Hz
  -> [kernel] oscillator generates samples from RPM + params
  -> [kernel] SDK writes ep3 packet -> ll_driver override -> splicer
       (cur preserved, byte10 + window filled, byte11 stamped 0x0d)
  -> real output_report -> USB ep3 -> wheel (adds texture to base force)
```

## Error handling and safety

- **Default off.** `wheel_tf_merge=0`; the hook is inert pass-through.
- **Stale-RPM guard.** No fresh RPM -> no splice, so a crashed daemon or closed
  game leaves the wheel smooth, never droning.
- **Never clobber.** Packets with `byte10 != 0` (a future SDK/game that streams
  its own texture) pass through untouched.
- **cur is sacred.** The splicer never writes bytes 6-9; the base force stays
  exactly what the SDK computed. This is the core invariant and a test asserts
  it.
- **Teardown.** ll_driver restore on interface-2 remove reuses the interface-0
  pattern's ordering and the existing `ff_hdev` TOCTOU guards.
- **Idle threshold.** Below an RPM floor, no texture (a stationary idling engine
  should be near-silent, matching Windows).

## Testing

- **Unit (userspace-compiled pure logic):**
  - splicer: `cur`/`seq` preserved bit-for-bit; sample block lands in the right
    slots; `byte10` set correctly; refuses non-stream packets.
  - oscillator: deterministic output for a fixed RPM+params+seed; phase
    continuity across calls; correct fundamental for a known RPM/cylinders;
    silence below idle; clamping at high intensity.
  - RPM sysfs parse + staleness logic.
  - classifier: only report-0x01 type-0x01 `byte10==0` packets are eligible.
- **Hardware validation (rev test):** with merge on and RPM fed, capture ep3;
  assert `byte10 > 0` appears and scales with RPM, assert `cur` bytes match a
  passthrough baseline, and confirm the user feels engine buzz that tracks revs.
  With merge off, assert ep3 is byte-identical to baseline.

## Rollout

1. Land the kernel units (off by default) + userspace pure-logic tests.
2. Enable via sysfs, hardware rev-test, tune the recipe against the Windows
   capture using the sysfs knobs.
3. Wire the daemon RPM write and a `logi-launch` per-game toggle so plain launch
   enables it automatically for native-TF titles.

## Open items for the plan (not design questions)

- Exact new-sample count per packet and window-slot fill: adopt whatever
  `hidpp_dd_tf_queue_stream` uses today; confirm against a Windows-capture
  packet during implementation.
- Sine LUT resolution / fixed-point width: pick during implementation to hit
  the audio band cleanly without kernel FP.
