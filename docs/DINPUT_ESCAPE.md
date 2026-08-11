# How TrueForce actually reaches the wheel: DirectInput Escape

TrueForce does not travel over the wheel's HID interfaces from the game's
side. It travels through `IDirectInputDevice8::Escape`, the DirectInput
vendor passthrough, which on Windows lands in Logitech's own driver. Wine
does not implement that method, so under Proton every packet is discarded
and the game is told it succeeded.

Everything below was measured on an RS50 (`046d:c276`) in Assetto Corsa EVO
on 2026-08-11, using `tools/dinput8-escape-proxy.cpp`.

## Why it looked like several different bugs

Three faults sat on top of each other, and each hid the one below it.

1. **The registered SDK path was unopenable.** A quoting bug introduced in
   0.27.1 halved every backslash, so the CLSID named a file that did not
   exist and Logitech's library never loaded. Fixed; see
   `tools/install-tf-shim.sh`.
2. **The game verifies the SDK's Authenticode signature.** It reads the DLL
   path from the CLSID's default value and calls `WinVerifyTrust` before
   loading. An unsigned rebuild or proxy is refused before its first
   instruction, which is a hard ceiling on replacing this DLL and explains
   several dead ends recorded against issue #27.
3. **Wine's `Escape` is a stub.** Even a correctly registered, genuine,
   successfully loaded SDK reaches nothing.

The third is the one that remains, and it is not ours to fix in the driver.

## What the game does

`AssettoCorsaEVO.exe` resolves the `logiTrueForce*` entry points from the
SDK and carries the string

    logiWheelInit failed. The TrueForce SDK dll has not been registered.

It opens `SOFTWARE\Classes\CLSID\{e8dfb59f-141f-40e4-8dd4-5526ead25a4c}`,
reads the **default value** for the path (not `InProcServer32`), verifies
the signature, and loads it. About 5.5 s later the SDK starts its own
haptic thread, which is where the traffic comes from.

## The stream

The SDK's haptic thread calls `Escape` at a steady **187 calls per second**
with `dwCommand == 0` and a 20-byte input buffer:

| Offset | Size | Meaning |
|---|---|---|
| 0 | 4 | struct size, always 20 |
| 4 | 4 | type, always 1 in everything seen so far |
| 8 | 4 | **float: engine RPM, live** |
| 12 | 4 | float: car constant, believed shift point |
| 16 | 4 | float: car constant, believed limiter |

Wine returns `DI_OK` for all of them, so the SDK has no way to know the
data went nowhere.

### How the RPM reading was established

With the car **stationary** in the garage, revving the engine four times and
then holding one long pull produced this in the last field, sampled every
20th call:

    idle ~2950, four rises to ~15000 and back, then a slow climb to ~14900

Stationary is the point: road speed, wheel angle and force feedback output
are all constant there, so nothing else in the car could produce that trace.
Across the same run the two constants stayed at 11250.0 and 14250.0 and
changed only with the car, and RPM briefly exceeded the second constant
(15053 against 14250), which is why it reads as a limiter rather than a
ceiling.

Other contexts in the same log had the pair frozen: menus at
`b=1000 c=15000` with RPM 0, and another at `b=900 c=6000` with RPM pinned
at 1000.

### Other commands seen

Sent a handful of times at init, not yet decoded:

| Command | In | Out | First bytes |
|---|---|---|---|
| 3 | 8 | 2 | `08 00 00 00 01 00 00 00` |
| 5 | 20 | 8 | `14 00 00 00 01 00 00 00 01 ...` |

Command 5's payload is not three floats, so it is a different shape from
the stream and is probably a getter, given it asks for 8 bytes back.

## What this means for us

The payload carries **parameters, not a waveform**. The synthesis happens
below `Escape`, inside Logitech's Windows driver, so implementing this is
not "forward these bytes to the wheel". It is "generate TrueForce from an
RPM stream", which `logi-tf-sim` already does for games we have telemetry
parsers for.

Two consequences worth stating plainly:

- Feeding `logi-tf-sim` from here makes the haptics follow the game's own
  authoritative numbers instead of a per-title shared-memory parser, and it
  works for **any** title that uses the SDK, with no per-game work.
- It also supplies the RPM feed the rev lights have never had in this game,
  along with the two reference points a rev display needs.

### Force feedback is a separate question

Force feedback does not come through `Escape`. It comes through ordinary
DirectInput effects, and those work only when Proton exposes the wheel via
its evdev backend, because the wheel's raw HID descriptor has no PID
collection. So:

| | TrueForce | Force feedback |
|---|---|---|
| `PROTON_ENABLE_HIDRAW` set | still dead (Escape) | **gone**: no PID collection |
| not set | still dead (Escape) | works |

Since TrueForce rides DirectInput rather than raw HID, implementing
`Escape` removes the reason to set that variable at all for these titles,
which is what would let both work at once.
