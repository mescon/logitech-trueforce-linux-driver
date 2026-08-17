# TRUEFORCE Protocol

> **Status**: the kernel driver leaves interface 2's hidraw node open for userspace. Two userspace paths consume that node:
>
> - **Proton sims (verified working)**: `tools/install-tf-shim.sh` copies Logitech's own Authenticode-signed SDK DLLs (`trueforce_sdk_x64.dll`, `logi_steering_wheel_x64.dll`) into each Wine prefix and registers the CLSIDs. The unmodified DLLs run inside Wine and write to the wheel via Wine's HID stack, which reaches our kernel driver. No shim, no IAT hooks, no certificate spoofing. End-to-end verified against **Assetto Corsa Competizione** and **Assetto Corsa EVO** under Proton on RS50 in **both modes**: G PRO compatibility mode (`046d:c272`, 2026-04-26 / 2026-04-29) and **native mode** (`046d:c276`, AC EVO, 2026-07-08). The native run was usbmon-confirmed: Wine's HID backend opened interface 2 on the native PID and the SDK streamed ~2 kHz of type-0x01 packets (239k OUT over ~120 s) on ep 0x03. The SDK DLLs open and drive the wheel identically at either PID, so compat mode is not required for TrueForce.
> - **Native Linux apps**: `userspace/libtrueforce/` is a native C reimplementation of the same protocol described below. Useful for Linux apps that want to drive TrueForce directly (telemetry-driven haptic generators, custom test rigs, etc.).
>
> Originally reverse-engineered from [issue #5](https://github.com/mescon/logitech-trueforce-linux-driver/issues/5) captures (BeamNG.drive + G Pro, contributed by [@SandSeppel](https://github.com/SandSeppel)) and re-verified 2026-04-21 against an RS50 + ACC capture on the same host. The two wheels use byte-for-byte identical init and streaming packets.

## Overview

TRUEFORCE is a high-frequency audio-haptic stream that supplements traditional PID force feedback. Rather than low-rate constant-force updates (~50-100 Hz via HID SET_REPORT), TRUEFORCE sends a ~1000 Hz audio waveform directly to the wheel's DSP, which drives the motor with much higher fidelity.

The protocol runs entirely on **USB Interface 2** (endpoints `0x03 OUT` / `0x83 IN`), which the kernel driver delegates to hidraw. No HID++ feature activation is required; userspace opens the hidraw node for interface 2 and starts writing.

## Wheel Coverage

Verified against three wheels:

| Wheel | PID | Capture / verification | Date |
|-------|-----|------------------------|------|
| RS50 (PlayStation/PC) | `046d:c276` | ACC gameplay | 2026-04-21 |
| G Pro Racing Wheel | `046d:c272` / `046d:c268` | BeamNG.drive gameplay | 2026-04-19 |
| G923 (PlayStation/PC) | `046d:c266` | `logi-tf-sim` driven tone on hardware | 2026-07-26 |

The 68-packet init sequence is identical across the RS50 and G Pro, byte-for-byte. The streaming packet layout (type 0x01) is also identical. Treat TRUEFORCE as a single protocol across the direct-drive wheel family.

**The G923 Xbox edition (`c26e`) carries it too, confirmed 2026-08-10** from a
Windows capture of that wheel under Automobilista 2 (issue #27). Over roughly
40 seconds it received **17597** type-0x01 stream packets on its `0xFFFD`
interface, against **24** HID++ `0x8123` commands which are setup only
(`RESET_ALL`, gains, effect create/play). The stream matches the documented
layout byte for byte: byte[5] a rolling sequence, bytes[6-9] `cur` duplicated,
byte[10] the new-sample count, `00 80 00 80 00` at idle and `04` samples while
driving. So its force arrives on the stream, not over HID++, and simulated
TrueForce should be able to drive that wheel the same way it drives a `c266`.
This corrects the earlier claim that the Xbox edition is the one wheel whose
force rides HID++.

The gear-driven **G923** carries the same interface-2 transport and stream protocol (first established by the TF4ALL project's Windows captures, issue #20; hardware-confirmed on a c266 by `logi-tf-sim`'s synthetic sweep). One G923-specific caveat: while a type-0x01 stream runs, the wheel's motor follows the stream's `cur` field and stops reacting to its classic interface-0 force-feedback commands, so a G923 streamer must mirror the live FFB into `cur` for the stream's duration - `logi-tf-sim` reads the kernel driver's `ffb_output` sysfs attribute for exactly this (see `SYSFS_API.md`). Feel under real game telemetry is not yet verified on the G923.

## Traffic Characterisation

| Metric | Without TRUEFORCE | With TRUEFORCE |
|--------|-------------------|----------------|
| Interface 2 data packets | **0** | **tens of thousands** per gameplay session |
| PID constant force updates (intf 1) | ongoing | small, init only |
| Endpoint `0x03` packet rate | idle | 1000 Hz, all senders: libtrueforce, the kernel's unified stream, and games (AC EVO already used 1000; 250-500 seen in older captures). 1000 is both Logitech's stated 1 ms TRUEFORCE interval and the USB interrupt-endpoint ceiling. |
| Samples per packet (new) | N/A | 4 new + 9 history = 13-slot rolling window |
| Effective audio sample rate | N/A | ~1000 Hz (250 pkt/s * 4 new samples) |

When TRUEFORCE is active, traditional PID FFB is used only for initial setup and occasional parameter changes. The high-frequency force data moves entirely to the audio stream.

## HID Descriptor (Interface 2)

```
Usage Page: 0xFFFD (vendor-defined)
Usage:      0xFD01
Report ID:  0x01
Size:       63 bytes IN + 63 bytes OUT (64 bytes total with report ID)
```

## Packet Format (Common Header)

```
byte[0]:    0x01              Report ID
byte[1-3]:  0x00 0x00 0x00    Padding
byte[4]:    COMMAND_TYPE       See command table
byte[5]:    SEQUENCE           Rolling u8 counter (0x00-0xFF wrap), shared across all types
byte[6..]:  PAYLOAD            Type-specific
```

The sequence counter is rewritten at send time from a session-local counter. Each pass through the init sequence restarts the counter at 1; after init the stream continues from where init left off.

## Command Types (Host -> Device, endpoint 0x03)

| Type | Purpose |
|------|---------|
| `0x01` | Audio data stream (dominant during gameplay) |
| `0x03` | Start / play |
| `0x04` | Stop / clear |
| `0x05` | Parameter upload (48 floats, one per packet) |
| `0x06` | Effect slot configuration (6 slots) |
| `0x07` | Query / handshake |
| `0x09` | Runtime parameter update |
| `0x0b` | Unknown (observed from AC EVO's session init with float `1.0`) |
| `0x0e` | **Operating range, IEEE 754 LE float degrees** |

## Initialisation Sequence (sent twice)

G Hub sends a 68-packet init sequence, then sends the **same 68 packets a second time** (sequence counter reset to 1 at the start of each pass) before the main per-sample stream begins. The init must be sent twice: a single pass is unreliable on cold boot. Both the 2026-04-19 G Pro + BeamNG and 2026-04-21 RS50 + ACC captures show the duplicate pass.

The 68 packets are stored verbatim in `userspace/libtrueforce/src/tf_init_data.h`. Breakdown:

| Packets | Type | Purpose |
|---------|------|---------|
| 1-48 | `0x05` | 48 parameters (indices `0x00`-`0x1d` and `0x2b`-`0x3c`) as IEEE 754 LE floats |
| 49 | `0x01` | Neutral sample (primes the stream) |
| 50 | `0x0e` | Operating range = float `2700.0` (the wheel's max range). A **pre-START no-op**: `0x0e` only takes effect between `0x03` START and `0x04` STOP, and this packet precedes the START at packet 68 (see the type-`0x0e` session-scoping notes below) |
| 51 | `0x01` | Neutral sample |
| 52 | `0x07` | Handshake / query |
| 53 | `0x01` | Neutral sample |
| 54, 56, 58, 62, 64, 66 | `0x06` | Effect slot configurations (slots 1-6) |
| 55, 57, 59, 61, 63, 65 | `0x01` | Neutral sample between each slot config |
| 60 | `0x09` | Runtime parameter update |
| 67 | `0x04` | Stop / clear |
| 68 | `0x03` | Start / play |

Key type-0x05 parameter values, for reference. **Note:** the committed
init we replay (`userspace/libtrueforce/src/tf_init_data.h`) sends every
type-0x05 packet with a **zero** value payload (only the index byte
varies), and TrueForce still works end-to-end, so these specific values
are not required for basic operation. The table below records the
non-zero values decoded from G Hub's own init capture, kept as a guide to
what each index means:

| Index | Value | Likely meaning |
|-------|-------|----------------|
| `0x00` | 2.0 | Channel count or mode |
| `0x02` | 32768.0 | Max amplitude (0x8000) |
| `0x03` | 65535.0 | Max range (0xFFFF) |
| `0x07` | 5.4054 | Damping coefficient? |
| `0x09` | 0.3 | Gain? |
| `0x0c` | 47.1239 (15 pi) | Angular rate limit? |
| `0x0d` | 1.5708 (pi/2) | Phase offset? |
| `0x0e` | -9.4248 (-3 pi) | Filter parameter? |
| `0x0f` | 9.4248 (3 pi) | Filter parameter? |
| `0x10` | 13.0 | Samples per packet (matches the streaming window) |
| `0x12` | 4000.0 | Max frequency? |
| `0x14` | 2000.0 | Crossover frequency? |
| `0x1d` | 4.0 | New samples per packet (matches the streaming `0x04` constant) |
| `0x33` | 350.0 | Crossover frequency? |

## Audio Data Stream (Type `0x01`)

```
byte[0-3]:   01 00 00 00           Report header
byte[4]:     01                    Command type
byte[5]:     sequence              Rolling counter
byte[6-7]:   u16 LE                Most-recent sample (newest-so-far)
byte[8-9]:   u16 LE                Duplicate of bytes 6-7
byte[10]:    0x04                  Number of new samples in this packet (0x00 when none)
byte[11]:    0x0d                  Sample-window valid flag: 0x0d whenever byte[10] != 0,
                                   0x00 when byte[10] = 0 (demux pair, see invariants below)
byte[12-15]: window[0] L, window[0] R (u16 LE each, mono duplicated)
byte[16-19]: window[1]
...
byte[60-63]: window[12]
```

**Layout invariants observed across captures and replicated in `src/stream.c`:**

- The 13-slot rolling window holds the most recent samples, oldest at `window[0]`, newest at `window[12]`.
- Each packet advances the window by **as many samples as byte 10 declares**; that many fall off the front. G Hub sends 4 almost always and 5 in some captures, so the field is a count and not a constant. `libtrueforce` sends 4 in the steady state, fewer when a drain came up short, and up to 12 (`LOGITF_TF_CATCHUP_MAX`) on a packet making up a coalesced timer expiry. 13 is the ceiling the window can express.
- Every u16 sample is duplicated (L and R channels). The wheel is single-motor, the stereo duplication is ceremonial.
- Values are unsigned 16-bit little-endian, offset binary (centre `0x8000`, `0x0000` = full left, `0xFFFF` = full right).
- The preamble at bytes 6-9 ("cur") is the **motor torque target**, duplicated as
  two u16 LE. While a TrueForce session is active the wheel steers by cur and
  the window plays additively on top as audio; cur OVERRIDES the HID++ 0x8123
  force path. In G Hub/SDK captures cur usually tracks the newest window
  sample only because the games stream their FFB there. AC EVO carries its
  game force in cur; the independent audio in the window of the same packet
  is NOT the game's - AC EVO never calls the SDK's texture entry points
  (`SetTorqueTF`/`SetStreamTF`, never even probes `GetTorqueTFRateBounds`),
  so the window content in Windows captures is the driver stack's own
  synthesis, fed by the game's Escape telemetry and serialized into the
  same stream as the game's SDK force (which is why the sequence counter
  is continuous). The kernel driver's texture merge replicates that
  architecture. (cur semantics from the TF4ALL project's Windows captures;
  synthesis attribution established 2026-08-14.)
- Bytes 10 and 11 are a **demux pair, not independent constants**
  (hardware-proven 2026-08-14): every sample-carrying packet pairs
  `byte10 != 0` with `byte11 = 0x0d`, and every no-new-samples packet
  (menu keepalive, plain force) pairs `byte10 = 0x00` with
  `byte11 = 0x00`. The combination `byte10 = 0x04` + `byte11 = 0x00`
  never occurs in Windows captures, and the wheel **silently discards
  the whole sample window** of such a packet while still honouring its
  cur bytes - a wire-perfect texture stream that renders nothing. Any
  producer that adds samples to a packet must also stamp `0x0d`.

Packet cadence in libtrueforce is 1000 Hz (4 new samples * 1000 Hz = 4000 sample/s effective); the kernel driver's unified stream runs 1000 Hz (4 kHz slot rate, 4 kHz unique content) since 0.30.0, having really run at 333 Hz before it. Games vary: ACC captures show 250-500 pkt/s, AC EVO up to ~1000 pkt/s (4 kHz audio) per TF4ALL measurements - the wheel accepts the whole range. If userspace can't keep up the thread holds the window for `LOGITF_TF_STARVE_HOLD_TICKS` and then flushes it toward centre while the held force unwinds (Windows does something similar under input starvation). If userspace overruns the transport, `logitf_stream_push_s16()` drops the OLDEST queued samples to hold the backlog to `LOGITF_TF_MAX_PENDING_MS` of audio, counting and reporting them; it does not block.

## One writer at a time (measured 2026-08-17)

**The stream has exactly one owner, and the endpoint enforces it whether
the software agrees or not.** Endpoint `0x03` is an interrupt OUT with a
1 ms interval, so it carries one packet per millisecond in total, not
one per writer. Two programs each streaming at 1 kHz therefore do not
share it, they take turns on it: each gets every other frame.

What that does to the wheel is worse than halving a rate. Bytes 6-9 are
a level, not an event: the wheel holds the last value it was given. So
with two writers the torque target alternates between their two values
every millisecond, which is a 500 Hz square wave on the motor. Measured
on an RS50 with the kernel driver and a userspace producer both
streaming:

| | two writers | one writer |
|---|---|---|
| packets carrying samples | 49% | 99% |
| gap between sample packets | 2.000 ms | 1.000 ms |
| samples delivered per second | 1934 | 3860 |
| 450-500 Hz content in the torque field | dominant | none |

(The one-writer column is the finished state. Making the driver yield
took the rate from 1934/s to 3635/s on its own; the remaining gap was a
producer discarding coalesced timer expirations, and closing that
reached 3860/s against a 4000/s clock.)

Audibly this is a fixed buzz that does NOT move with the engine note, at
exactly 500.00 Hz with a strong third harmonic and almost no second: the
odd-harmonic signature of a level stepping every 2 ms rather than a
resonance (issue #59). The samples also play at half rate, an octave
low.

Consequences for anyone implementing this protocol:

- **Take the stream, do not join it.** Before streaming, establish that
  nothing else is: on Linux the kernel driver yields automatically (it
  detects a userspace writer on interface 2 and stops sending its own
  packets), but two userspace programs must arbitrate between
  themselves.
- **The torque field belongs to whoever owns the stream.** When the
  driver yields it does not go silent: if it has force of its own to
  apply it writes that force into bytes 6-9 of the owner's packet on
  the way past, so one packet carries the owner's samples and the
  driver's force. It leaves those bytes alone when it has no effect
  running, because a native SDK session puts the GAME's force there and
  overwriting it would replace force feedback with dead centre.
- **A silent endpoint is the normal idle state.** With nothing streaming
  there is no traffic at all on `0x03`; G HUB holds no idle session
  open, and neither should anything else (see the whine notes in the
  session-state section).

## Type `0x0e`: Operating Range (root cause of the "90 degrees on game launch" bug)

Decoded 2026-07-02 from a live usbmon capture of an AC EVO launch on
Linux: type-`0x0e` carries the wheel's operating range as an IEEE 754
LE float in degrees. Byte-exact layout:

```
byte[0]:     01                    Report header
byte[4]:     0e                    Command type
byte[5]:     sequence              Rolling counter
byte[6-9]:   f32 LE                Operating range, degrees
```

Evidence:

- The canonical init's packet 50 carries `2700.0` - exactly the
  wheel's maximum range, not a plausible sample rate.
- AC EVO's SDK session init appends a second `0x0e` with `90.0`
  (`01000000 0e <seq> 0000b442`), and the wheel's physical range
  flips 900 -> 90 in the same 20-second window with ZERO HID++
  traffic on interface 1 (confirmed: the only interface-1 range
  packets in the entire capture are the Linux driver's own polls,
  whose replies flip from 900 to 90).
- Two captured frames, byte-for-byte, confirm bytes 6-9 and not any
  neighbouring offset: `90.0` = `01 00 00 00 0e 46 00 00 b4 42`,
  `2700.0` = `01 00 00 00 0e 32 00 c0 28 45` (2026-08-14 usbmon, AC
  EVO session init).

A session init sends **multiple** `0x0e` pushes, not one: the
2026-08-14 capture shows `2700.0`, then `90.0`, then `2700.0` again in
the same init, which reads as the SDK negotiating its bounds around
the clamp rather than writing the final value once. Consumers of this
packet (the kernel driver's decode, any replay tooling) must not
assume a single push per session and must track the latest value seen,
not the first.

This is `logiWheelSetOperatingRange*()` on the wire, and it explains
why the launch-time range reset never produced a HID++ broadcast: it
does not go through the HID++ range feature at all. Games push their
configured steering rotation here at session start; a game whose
rotation setting is 90 (or defaulted) locks the wheel to 90 degrees.
The kernel driver's 20 s range poll detects the change and, by
default, restores the pre-reset range automatically (the
`wheel_range_restore` sysfs attribute; verified end-to-end with a
detection-to-restore latency of ~60 ms against a faithful replay of
the game traffic). Re-applying a range via HID++ sticks - the SDK
write is one-shot at session init.

Two firmware behaviours discovered while reproducing this
(2026-07-03, live wheel):

- **Type-`0x0e` is session-scoped, and the scope is the started
  stream, not the init** (sharpened 2026-08-14, live wheel + felt
  stops): a `0x0e` push is ignored on an idle interface AND after the
  init sequence alone - the canonical init's own `2700.0` pushes are
  no-ops because they precede the `0x03` START. The range write only
  takes effect between `0x03` START and `0x04` STOP. A mid-stream
  push applies immediately (900 -> 90 clamp felt at the rim; the
  wheel broadcasts the change over HID++).
- **Idle revert**: if a TF session goes quiet (no stream packets,
  roughly a minute) the firmware reverts the session's range change
  on its own and broadcasts the restored value over HID++. A running
  game keeps its session alive, which is why real launch-time resets
  persist.

### Session state and the force-mode latch

Hardware-proven across many sessions: `logiWheelSetForceMode(1)`
latches in the wheel itself, not just in the SDK, and stays latched
until a REAL power cycle. A USB reset is not enough, because the base
has its own PSU independent of the USB bus. Consequences, both
observed on real hardware:

- A second SDK session started without a power cycle comes up with a
  dead haptic thread: status `0x80000008`, no ep3 OUT stream at all.
- A hard-killed session (game terminated rather than closed cleanly)
  leaves the wheel's stream engine running on its own: the wheel keeps
  streaming ep `0x83` IN reports at 2 kHz indefinitely, with nothing on
  the host consuming them, until the base is power-cycled.

Refinement (2026-08-14): after a session dies without its teardown
reaching the wheel (hard kill, crash), the next session's
`logiWheelSetForceMode(1)` **returns success** yet there is zero ep3
traffic **in both directions** - and a healthy session also carries a
~1 kHz ep3 IN status stream, so its absence is the diagnostic, not
just missing OUT packets. Power-cycling the base before launch
recovers it. Running `logi-tf-init` between sessions is
untested/inconclusive as a recovery. An experimental pre-launch re-arm
exists: `LOGI_TF_REARM=1` makes `logi-launch` send the `0x04`+`0x03`
pair and then the 68-packet init twice from `tools/tf-init.bin` before
the game starts; it is off by default pending hardware validation.

Anyone reproducing SDK captures or writing session teardown code needs
to power-cycle the base between sessions, not just close and reopen
the handle.

**LED / HID++ traffic during a live session** (hardware, 2026-08-14):
the driver's old rev-LED form - an arm burst plus per-level fn3
activation - was fatal to a native TrueForce SDK session: sent at init
it killed the session, sent mid-session it killed wheel input. The
current form (bare SHORT fn2 GET_STATE + LONG fn6 level, nothing else)
coexists cleanly: base FFB + TrueForce texture + telemetry-driven rev
LEDs are hardware-validated running together. See
`PROTOCOL_SPECIFICATION.md` section 9 for the rev-LED wire protocol.

**Which wheels honour it** (2026-07-30):

| Wheel | Honours type-`0x0e`? |
|---|---|
| RS50 | yes, root-caused here |
| G923 Xbox (`c26e`) | **no**, see below |
| G923 PlayStation (`c266`) | **no**, tested on hardware |

The Xbox edition entry above previously read "yes", inferred from an owner
reporting a 90 degree lock in ACC. That inference was wrong, and the same
owner disproved it: in ACC's own config screen his rim turns its full 900
degrees and the input bar tracks the whole way, with the limit appearing
only on track. A real operating-range change would soft-stop the rim
everywhere, menus included. So the wheel is never reconfigured on that
edition; the game clamps its own steering because it believes the wheel has
90 degrees of travel, which it gets from the TrueForce SDK falling back to
the minimum of the legal range when it cannot reach G HUB. Nothing on the
wheel is wrong, so there is nothing for a range restore to put back.

The PlayStation edition was tested directly: a live TrueForce session was
established (full init sequence sent, silent stream running, confirmed by the
daemon's own `stream start`), a type-`0x0e` carrying `90.0` was pushed on the
`0xFFFD` transport, and the rim then turned its full travel with no early soft
stop. Repeated with the session up rather than idle, since the first attempt
made exactly the mistake the session-scoping note above describes, and a bare
push on an idle interface is a no-op on every wheel including the RS50.

That bounds the operating-range restore work to the wheels whose force rides
HID++: the direct-drive wheels, which already heal it, and the G923 Xbox
edition, which has `range_restore` (see `docs/SYSFS_API.md`). The PlayStation
edition's classic engine exposes no range readback at all, so it is fortunate
rather than incidental that it needs none.

AC EVO's init also differs from the canonical G Hub init in two more
packets: a type-`0x0b` with float `1.0` (purpose unknown) and a
type-`0x09` carrying floats `1.0` and `350.0`.

## Device Response (Type `0x02`, endpoint `0x83` -> host)

```
byte[0-3]:   01 00 00 00           Report header
byte[4]:     02                    Response type
byte[5]:     sequence              Echoes command sequence
byte[6-7]:   u16 LE                NOT current or temperature (see below)
byte[8]:     0x03                  Status byte?
byte[9-10]:  wheel_position (LE16) Matches joystick axis data
byte[11-12]: wheel_position2       Slightly delayed (~1 sample behind)
byte[13-16]: 32-bit counter        Timestamp or sample counter
byte[17]:    varying                Checksum-like
byte[18-32]: status/counters
byte[33-63]: zeros
```

**Bytes 6-7 are not a motor load reading.** They were guessed to be current
or temperature, which would have made them the only route this project has to
answering "how hard is it safe to drive this motor". Tested on an RS50
(2026-08-08) with `tests/motorlog`, which drives a known amplitude and then
pushes exact silence while still sampling, because current collapses with the
force and temperature decays over tens of seconds:

- The value slides steadily downward at the same rate whether the wheel is
  being driven hard or sitting silent, which neither hypothesis allows.
- It wraps through zero to 0xFFFF and then goes flat exactly when the status
  byte changes 3 -> 2, so it is tied to stream state rather than to load.
- Across amplitudes 0.05, 0.30 and 0.60 its range overlaps almost completely
  (min 1105 to max 62794 at the *lowest* amplitude), so per-amplitude
  statistics are dominated by the wrapping.

Counter-like, and still undecoded. What matters for anyone arriving with the
same idea: there is currently **no known telemetry from these wheels that
reports motor load, current or temperature**, so amplitude limits have to be
argued from what the motor is commanded to do, not measured from what it
reports.

Responses arrive at the same cadence as the host's packet rate, giving real-time wheel-position feedback for synchronisation. libtrueforce's stream thread consumes them while a stream is active and exposes the latest snapshot via the Linux-native `logitf_get_stream_feedback()` API (wheel position, device counter, and the still-undecoded motor/status fields); the kernel driver ignores them.

## PID FFB Commands (report `0x10`/`0x11`, for reference)

Classic PID-style FFB is addressed by HID report ID (`0x10`/`0x11`),
distinct from TRUEFORCE's report ID `0x01`; the wheel firmware
demultiplexes the two by report ID and they coexist (verified by playing
a sine on TRUEFORCE while holding a constant-torque KF effect).

Note on interfaces: on the G920-class HID++ path these PID reports are
addressed to interface 1, but the **RS50 has no FFB OUT endpoint on
interface 1**. The Linux driver actuates the RS50's constant force by
writing to **interface 2 endpoint `0x03`** (the dedicated `hidpp_dd_ff_*`
path), the same interface TRUEFORCE uses; the two are still separated by
report ID. See [`PROTOCOL_SPECIFICATION.md`](PROTOCOL_SPECIFICATION.md)
section 4 for the constant-force (KF) subset of these same ep-0x03
packets and the wider HID++ / device reference.

### Report 0x10 (7 bytes)

Format: `[10 FF <cmd> <param1> <param2> <param3>]`

| Command | Description |
|---------|-------------|
| ff 10 | Constant force update |
| ff 00 | Effect stop/reset |
| ff 01 | Effect start |
| ff 0f | Effect create/allocate |
| ff 17 | Set envelope |
| ff 08 | Set condition |
| ff 02 | Set effect type |
| ff 0d | Set periodic |
| ff 0a | Set constant force params |
| ff 09 | Set ramp |

### Report 0x11 (20 bytes)

Extended command format: `[11 FF 10 2e 01 80 00 00 00 00 XX XX ...]`

Used for constant force values with extended precision.

## Userspace Library Layout

| Path | Purpose |
|------|---------|
| `userspace/libtrueforce/src/tf_init_data.h` | 68 canonical init packets, auto-generated from capture, sent twice at session bring-up |
| `userspace/libtrueforce/src/session.c` | `logitf_session_ensure()` opens the hidraw node and runs the two-pass init |
| `userspace/libtrueforce/src/stream.c` | 250 Hz timerfd loop, 13-slot rolling window, `logitf_stream_push_s16()` / `_clear()` / `_start/stop()` |
| `userspace/libtrueforce/include/trueforce.h` | Mirrors the 62 exports of `trueforce_sdk_x64.dll` (Windows SDK) so a Linux app can call the same API surface |

## Open Items

- libtrueforce consumes type-`0x02` device responses while a stream is active and exposes them via `logitf_get_stream_feedback()` (2026-07-02). The motor field (bytes 6-7), status byte (8), and byte 17 checksum-like field are still undecoded; correlating the motor field against commanded torque on a live wheel would pin it down.
- ~~The constant flag word at byte 11 (`0x0d`) is passed through verbatim; its exact meaning is still not decoded.~~ **Resolved 2026-08-14**: byte 11 is the sample-window valid flag, one half of the byte10/byte11 demux pair (see the stream layout invariants above); the wheel discards the window of any packet that carries samples without it. Value `0x05` has been seen instead of `0x04` in byte 10 in some captures, corresponding to 5 new samples; libtrueforce sent the 4-new-samples variant exclusively until it began declaring the real count (4 in the steady state, fewer on a short drain, up to 12 when catching up a coalesced timer expiry).
- Per-title parameter variation (are the 48 init floats game-specific or universal?) is unconfirmed. So far the same data produces audible TRUEFORCE across BeamNG and ACC.


## The SDK's own IPC, and where 90 degrees comes from

The type-`0x0e` section above explains how a range reaches the wheel. This
explains why the number is so often 90, which was open for a long time and is
now settled.

`trueforce_sdk_x64.dll` contains a local IPC layer, `logi::local_connection`,
with a client, a server, and the pipe name `logi.trueforce.connect` compiled
in. On Windows the peer is G HUB. Under Proton nothing serves that pipe. From
a reporter's Wine log, repeating for as long as the game runs:

```
CreateFileW L"\\.\pipe\logi.trueforce.connect" ... creation 3
CreateFileW Unable to create file ... (status c0000034)
```

`creation 3` is OPEN_EXISTING and `c0000034` is STATUS_OBJECT_NAME_NOT_FOUND,
so the SDK is the client, nothing answers, and it retries forever, which
matches the library's own string `Client failed to connect: polling again in
%ld ms`.

With no peer, a game asking how far the wheel turns gets 90: not a value
anybody chose but the minimum of the legal 90-2700 range, which is why the
symptom is always exactly 45 degrees each way.

**Answering that pipe ourselves is not possible.** On connect the SDK calls
`GetNamedPipeServerProcessId`, resolves the server to an executable with
`K32GetModuleFileNameExA`, runs `WinVerifyTrust` on it and reads the signer
with `CertGetNameStringW`. The string `Logitech Inc` is compiled in. A probe
serving that pipe is dropped in about a millisecond, in both message and byte
mode, which is what a rejected peer looks like rather than a rejected framing.

What is possible is answering the question the SDK cannot. A game loads the
SDK through a CLSID this project's shim installer writes, so the library it
loads is ours to choose: see `tools/tf-range-proxy.c`, which forwards every
other call to Logitech's own library and answers only the rotation getters.
Signatures for those are in `docs/SDK_ABI_NOTES.md`, taken from the library's
machine code rather than from any header.

Assetto Corsa Competizione resolves 56 symbols from this SDK, including all
four rotation getters, and none at all from the older Steering Wheel SDK, so
for that title this is the right library to answer through.
