# TRUEFORCE Protocol

> **Status**: the kernel driver leaves interface 2's hidraw node open for userspace. Two userspace paths consume that node:
>
> - **Proton sims (verified working)**: `tools/install-tf-shim.sh` copies Logitech's own Authenticode-signed SDK DLLs (`trueforce_sdk_x64.dll`, `logi_steering_wheel_x64.dll`) into each Wine prefix and registers the CLSIDs. The unmodified DLLs run inside Wine and write to the wheel via Wine's HID stack, which reaches our kernel driver. No shim, no IAT hooks, no certificate spoofing. End-to-end verified against **Assetto Corsa Competizione** and **Assetto Corsa EVO** on RS50 in G PRO compatibility mode (both 2026-04-26 / 2026-04-29).
> - **Native Linux apps**: `userspace/libtrueforce/` is a native C reimplementation of the same protocol described below. Useful for Linux apps that want to drive TrueForce directly (telemetry-driven haptic generators, custom test rigs, etc.).
>
> Originally reverse-engineered from [issue #5](https://github.com/mescon/logitech-trueforce-linux-driver/issues/5) captures (BeamNG.drive + G Pro, contributed by [@SandSeppel](https://github.com/SandSeppel)) and re-verified 2026-04-21 against an RS50 + ACC capture on the same host. The two wheels use byte-for-byte identical init and streaming packets.

## Overview

TRUEFORCE is a high-frequency audio-haptic stream that supplements traditional PID force feedback. Rather than low-rate constant-force updates (~50-100 Hz via HID SET_REPORT), TRUEFORCE sends a ~1000 Hz audio waveform directly to the wheel's DSP, which drives the motor with much higher fidelity.

The protocol runs entirely on **USB Interface 2** (endpoints `0x03 OUT` / `0x83 IN`), which the kernel driver delegates to hidraw. No HID++ feature activation is required; userspace opens the hidraw node for interface 2 and starts writing.

## Wheel Coverage

Verified against two wheels this session:

| Wheel | PID | Capture | Date |
|-------|-----|---------|------|
| RS50 (PlayStation/PC) | `046d:c276` | ACC gameplay | 2026-04-21 |
| G Pro Racing Wheel | `046d:c272` / `046d:c268` | BeamNG.drive gameplay | 2026-04-19 |

The 68-packet init sequence is identical across the two, byte-for-byte. The streaming packet layout (type 0x01) is also identical. Treat TRUEFORCE as a single protocol across the direct-drive wheel family.

## Traffic Characterisation

| Metric | Without TRUEFORCE | With TRUEFORCE |
|--------|-------------------|----------------|
| Interface 2 data packets | **0** | **tens of thousands** per gameplay session |
| PID constant force updates (intf 1) | ongoing | small, init only |
| Endpoint `0x03` packet rate | idle | ~250 Hz (libtrueforce cadence; captures saw 250-500 Hz depending on game) |
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
| `0x0e` | **Operating range, IEEE 754 LE float degrees** (previously misread as a frequency config) |

## Initialisation Sequence (sent twice)

G Hub sends a 68-packet init sequence, then sends the **same 68 packets a second time** (sequence counter reset to 1 at the start of each pass) before the main per-sample stream begins. We replicate this two-pass behaviour exactly. Single-pass init did produce audible TRUEFORCE on the bench but was less reliable on cold boot; both the 2026-04-19 G Pro + BeamNG and 2026-04-21 RS50 + ACC captures show the duplicate pass. See commit `0aebf70`.

The 68 packets are stored verbatim in `userspace/libtrueforce/src/tf_init_data.h`. Breakdown:

| Packets | Type | Purpose |
|---------|------|---------|
| 1-48 | `0x05` | 48 parameters (indices `0x00`-`0x1d` and `0x2b`-`0x3c`) as IEEE 754 LE floats |
| 49 | `0x01` | Neutral sample (primes the stream) |
| 50 | `0x0e` | Operating range = float `2700.0` (the wheel's max range; previously misread as a frequency) |
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
byte[10]:    0x04                  Number of new samples in this packet
byte[11]:    0x0d                  Constant (observed byte, rarely varies)
byte[12-15]: window[0] L, window[0] R (u16 LE each, mono duplicated)
byte[16-19]: window[1]
...
byte[60-63]: window[12]
```

**Layout invariants observed across captures and replicated in `src/stream.c`:**

- The 13-slot rolling window holds the most recent samples, oldest at `window[0]`, newest at `window[12]`.
- Each packet advances the window by **4 new samples**; the oldest 4 fall off the front.
- Every u16 sample is duplicated (L and R channels). The wheel is single-motor, the stereo duplication is ceremonial.
- Values are unsigned 16-bit little-endian, offset binary (centre `0x8000`, `0x0000` = full left, `0xFFFF` = full right).
- The preamble at bytes 6-9 is the newest sample, i.e. `window[12]`, also duplicated.
- Bytes 10 (`0x04`) and 11 (`0x0d`) are constants per capture.

Packet cadence in libtrueforce is 250 Hz (4 new samples * 250 Hz = 1000 sample/s effective). If userspace can't keep up the thread repeats the previous window (Windows does the same under input starvation) and the wheel gradually unwinds. If userspace overruns the ring, `logitf_stream_push_s16()` blocks on `ring_space`.

## Type `0x0e`: Operating Range (root cause of the "90 degrees on game launch" bug)

Decoded 2026-07-02 from a live usbmon capture of an AC EVO launch on
Linux: type-`0x0e` carries the wheel's operating range as an IEEE 754
LE float in degrees at payload bytes 6-9. Evidence:

- The canonical init's packet 50 carries `2700.0` - exactly the
  wheel's maximum range, not a plausible sample rate.
- AC EVO's SDK session init appends a second `0x0e` with `90.0`
  (`01000000 0e <seq> 0000b442`), and the wheel's physical range
  flips 900 -> 90 in the same 20-second window with ZERO HID++
  traffic on interface 1 (confirmed: the only interface-1 range
  packets in the entire capture are the Linux driver's own polls,
  whose replies flip from 900 to 90).

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

- **Type-`0x0e` is session-scoped**: a bare `0x0e` packet on an
  otherwise idle interface is ignored; the range write only takes
  effect inside an initialised TF session (init sequence sent).
- **Idle revert**: if a TF session goes quiet (no stream packets,
  roughly a minute) the firmware reverts the session's range change
  on its own and broadcasts the restored value over HID++. A running
  game keeps its session alive, which is why real launch-time resets
  persist.

AC EVO's init also differs from the canonical G Hub init in two more
packets: a type-`0x0b` with float `1.0` (purpose unknown) and a
type-`0x09` carrying floats `1.0` and `350.0`.

## Device Response (Type `0x02`, endpoint `0x83` -> host)

```
byte[0-3]:   01 00 00 00           Report header
byte[4]:     02                    Response type
byte[5]:     sequence              Echoes command sequence
byte[6-7]:   u16 LE                Motor current or temperature?
byte[8]:     0x03                  Status byte?
byte[9-10]:  wheel_position (LE16) Matches joystick axis data
byte[11-12]: wheel_position2       Slightly delayed (~1 sample behind)
byte[13-16]: 32-bit counter        Timestamp or sample counter
byte[17]:    varying                Checksum-like
byte[18-32]: status/counters
byte[33-63]: zeros
```

Responses arrive at the same cadence as the host's packet rate, giving real-time wheel-position feedback for synchronisation. libtrueforce's stream thread consumes them while a stream is active (since 2026-07-02) and exposes the latest snapshot via the Linux-native `logitf_get_stream_feedback()` API (wheel position, device counter, and the still-undecoded motor/status fields); the kernel driver ignores them.

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
report ID. See the RS50 FFB section of `PROTOCOL_SPECIFICATION.md`.

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
- The constant flag word at byte 11 (`0x0d`) is passed through verbatim; its exact meaning is still not decoded. Value `0x05` has been seen instead of `0x04` in byte 10 in some captures, corresponding to 5 new samples; libtrueforce uses the 4-new-samples variant exclusively.
- Per-title parameter variation (are the 48 init floats game-specific or universal?) is unconfirmed. So far the same data produces audible TRUEFORCE across BeamNG and ACC.
