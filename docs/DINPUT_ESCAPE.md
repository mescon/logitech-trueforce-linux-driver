# Native force feedback and TrueForce under Proton: what is actually in the way

Neither force feedback nor TrueForce reaches a direct-drive wheel from a
Logitech SDK title under Proton. The wheel, the driver and the SDK are all
healthy; something in the handshake between the game and the SDK is not.

> **This has worked before, and that constrains every theory here.**
> `docs/TRUEFORCE_PROTOCOL.md` records AC EVO driving an RS50 in **native**
> mode (`c276`) on 2026-07-08, usbmon-confirmed: Wine's HID backend opened
> interface 2 and the SDK streamed roughly 2 kHz of type-0x01 packets, 239k
> OUT transfers over two minutes, on endpoint 0x03.
>
> So the SDK can write to this wheel, at this product id, under Wine, with no
> OEM force-feedback driver in the picture. Any explanation that says the
> native path is structurally impossible is wrong. What changed since is the
> question, and AC EVO updated on 2026-07-23.

Everything here was measured on an RS50 (`046d:c276`) in Assetto Corsa EVO on
2026-08-11, with `tools/dinput8-escape-proxy.cpp` and `usbmon`.

## The short version

| Layer | State |
|---|---|
| Our shim registers the SDK | fixed; it was broken from 0.27.1 to 0.34.x by a quoting bug |
| The game loads the SDK | yes, verified by signature and loaded |
| The game resolves the SDK API | yes, 56 of 59 symbols |
| The SDK opens the wheel | yes, interface `mi_02`, the TrueForce interface |
| The SDK writes to the wheel | **yes**, on endpoint 3, once the range questions are answered locally (see 2026-08-12 below) |
| The game streams torque | **yes**, `logiTrueForceSetTorqueKF` at ~190/sec |
| What still fails | the wheel oscillates: the force loop is not held open (#57) |

> The first capture in this document showed zero output transfers and was
> taken with a parked car and nobody touching the wheel, which is exactly
> when a correct stream carries zeros. Later captures while driving show the
> stream running. Do not read the early rows as evidence of a dead link.

## Three faults, stacked

Each of these hid the one below it, which is why this took so long to see.

1. **Our registered SDK path was unopenable.** A quoting change in 0.27.1
   halved every backslash in the CLSID's value, so the path named a file that
   did not exist and the SDK never loaded. Fixed; the installer now derives
   the path from one source and verifies it resolves to a real file.
2. **An unsigned SDK can never load.** The game reads the DLL path from the
   CLSID's default value and calls `WinVerifyTrust` before loading it. Our
   `--proxy` build is refused before its first instruction. This is a hard
   ceiling on replacing that DLL and explains several dead ends recorded
   against issue #27 as protocol problems.
3. **Logitech's own operating-range getters break the wheel's HID++ channel.**
   Answering them locally is what lets the TrueForce stream start at all;
   measured below. With that done the stream runs and the remaining problem
   is the wheel running away for want of a force session (#57).

## What the game sends, and what it is not

The game calls `IDirectInputDevice8::Escape` at 187 calls per second with a
20-byte payload:

| Offset | Size | Meaning |
|---|---|---|
| 0 | 4 | struct size, always 20 |
| 4 | 4 | type, always 1 so far |
| 8 | 4 | **float: engine RPM, live** |
| 12 | 4 | float: car constant, believed shift point |
| 16 | 4 | float: car constant, believed limiter |

RPM was established, not guessed: with the car **stationary**, four rev cycles
and a held fifth pull reproduced exactly that shape in the live field, and
nothing else in a stationary car varies that way.

**This is the game's engine state, not the SDK's haptic output.** The game
sends it even when the SDK has never loaded (3012 calls in a run where the
CLSID path was still broken), so it is an input to the Logitech stack rather
than a product of it. Do not mistake it for TrueForce: what the SDK generates
has never been observed leaving it.

Wine returns `DI_OK` for every one of these while doing nothing, so the sender
has no way to know they went nowhere.

## The component that is missing

G HUB installs `Logitech\Direct Input Force Feedback\<ver>\`:

- `hidpp_forcefeedback_x64.dll` for the HID++ wheels
- `jerry_forcefeedback_x64.dll` for the classic wheels

`hidpp_forcefeedback_x64.dll` is a COM in-process server implementing
**`IDirectInputEffectDriver`** (its RTTI carries `.?AUIDirectInputEffectDriver@@`).
On Windows, DirectInput finds it through

    System\CurrentControlSet\Control\MediaProperties\PrivateProperties\Joystick\OEM\VID_046D&PID_xxxx\OEMForceFeedback

and routes **force-feedback effects and `Escape`** to it. It opens the wheel
itself via `SETUPAPI`/`CFGMGR32` and `CreateFile`, importing no `HID.DLL` and
referencing no G HUB agent, so it needs no background service.

`DllRegisterServer` registers two CLSIDs:

| CLSID | Name |
|---|---|
| `{62B43F0E-E7DB-4329-8C13-A966D84A289F}` | Logitech HID++ Force Feedback Device |
| `{88D042C8-EAC5-4F86-85D1-F4446AAFE1D4}` | Logitech HID++ Force Feedback API |

The per-device `OEMForceFeedback` entry is written by G HUB's installer, not
by the DLL.

### It does not know the RS50's native mode

The device ids in `hidpp_forcefeedback_x64.dll` are

    VID_046D&PID_C262, C268, C26E, C272

**`C276` is absent**, and appears nowhere in that package. `C272` is the
RS50's *compatibility* mode. So even on Windows, this driver has nothing to
say about an RS50 in native mode.

### It runs under Wine, up to a point

Registered into a prefix with `regsvr32` and probed directly (Wine 11.14):

    CoCreateInstance(driver) -> 0x00000000   instantiated
    GetVersions              -> 0x00000000   firmware/hardware/driver = 0x010003e7
    DeviceID(external=0)     -> crash: movups (%rbx),%xmm0 with rbx = 0

So the driver loads and answers under Wine. `DeviceID` dereferences null,
which is consistent with its "no supported device found" path being unhandled:
the only wheel attached was a `c276`, which it does not recognise. Adding the
`OEMForceFeedback` registry entries for `C276` and `C272` did not change it,
which suggests the device filter is an internal PID list rather than the
registry.

## Where the OEM driver does and does not fit

Given the 2026-07-08 evidence above, this driver is **not** required for
TrueForce: the SDK reached the wheel directly then, and can again. Its
relevance is narrower and still real:

- It is how **force feedback** works on Windows for these wheels, which is
  the half that currently dies under `PROTON_ENABLE_HIDRAW`. If it can be
  driven under Wine, force feedback stops depending on Proton's evdev
  backend and the hidraw trade-off disappears.
- It is the only thing that consumes the `Escape` engine-state stream, so it
  is also where rev lights would come from.

It is not the explanation for the SDK going quiet, and treating it as one
would be chasing the wrong thing.

## What follows

Two things have to be true at once, and only one of them is ours:

1. **The wheel must present an id the driver knows.** For an RS50 that means
   compatibility mode (`c272`) rather than native (`c276`). Untested.
2. **Something must route effects and `Escape` to the driver**, because Wine
   does not. That can be done from `tools/dinput8-escape-proxy.cpp`, which is
   already in the call path, is not signature-checked, and already wraps every
   `IDirectInputDevice8`.

If both hold, the result is native force feedback and native TrueForce from
Logitech's own driver, which is the outcome worth having.

### Why this would also fix force feedback, not just TrueForce

Force feedback currently depends on which backend Proton picked:

| | TrueForce | Force feedback |
|---|---|---|
| `PROTON_ENABLE_HIDRAW` set | dead (nothing routes `Escape`) | **gone**: the raw descriptor has no PID collection |
| not set | dead | works, via Proton's synthesised evdev device |

With the OEM driver in the path, force feedback comes from Logitech's driver
over HID++ rather than from a PID collection, so it no longer needs the evdev
backend, and raw HID stops being a trade-off.

## Things that look like causes and are not

- **The missing viscosity API.** The game resolves
  `logiTrueForceGetViscosity`, `GetViscosityMax` and `SetViscosity` and all
  three fail. This is not the blocker: `1_3_11` and `1_3_12` have identical
  76-symbol export sets and neither has them, so no shipped G HUB provides
  them and Windows users are equally without. The game probes optionally.
- **A G HUB agent.** The SDK and the effect driver both enumerate HID
  themselves; neither references `LGHUB`, a pipe, or a socket to one.
- **Device permissions.** The wheel's `/dev/hidraw*` nodes carry a `uaccess`
  ACL and open read/write as the desktop user; `winebus` holds all three open
  while a game runs.
- **The SDK not finding the wheel.** It opens `mi_02`, the correct interface.

## What the game actually does, measured 2026-08-12

Instrumented with `tools/dinput8-escape-proxy.cpp` (a `GetProcAddress` hook in
the game's own import table, which needs no signature), against Assetto Corsa
EVO on an RS50 with `PROTON_ENABLE_HIDRAW` scoped to the wheel.

The game is not refusing to drive the wheel. In order, it:

1. calls `dllOpen`, once
2. calls `logiWheelOpenByDirectInputW`, once, successfully (the wheel visibly
   takes a position when the track loads)
3. asks `logiTrueForceSupportedByDirectInputW` per device and is told **yes**
   for the wheel, **no** for a gamepad
4. reads the operating range
5. calls `logiWheelSetForceMode(handle, 1)`, accepted
6. streams torque with **`logiTrueForceSetTorqueKF`**, 33632 calls in one
   session, roughly 190/sec

It never calls `dllClose` or `logiWheelClose`, so it does not give up.

**`SetTorqueKF`, not `SetTorqueTF`.** Watching only the `SetTorqueTF*` family
showed zeros for hours and produced the wrong conclusion that the game sends
nothing. The KF family is a full channel with its own gain, maximum torque,
reconstruction filter and clear.

### Answering the operating-range questions is load-bearing

Separate launches, toggled with `LOGI_RANGE_FIX`. Honestly stated: this is
a strong correlation, not a controlled single-variable experiment. The wheel
carries state across game restarts (a disturbed session left it misbehaving
into the next run at least once), so session hysteresis is a confound.
Fix off was bad in two of two launches; fix on was good in two of three,
with the bad one immediately following a deliberately torn-down stream:

| | answered by us | answered by the SDK |
|---|---|---|
| OUT packets per 6 s | 11745 (~2 kHz) | 56 (~9 Hz) |
| packet type | `0x01`, the TrueForce stream | `0x0c` only, no stream |
| payload | varying, real content | no stream at all |
| wheel input reports on ep1 | 9918 | **0** |
| the game | normal | stutters, car will not move |

With Logitech's own implementations in the path the TrueForce stream never
starts and the wheel stops reporting input altogether. There are **no** control
transfers in either case, so this is not slow HID++ queries: those functions
take the wheel's HID++ channel out of service.

This is the same function family `tools/tf-range-proxy.c` was written against
for issue #27's 90 degree clamp, which now looks like the same root cause seen
from a different angle.

### Reading the captures: two traps

Two measurement artefacts ran through this investigation and should be
remembered before trusting any single capture:

- **The wheel only sends input reports on change.** A parked rim produces
  zero ep1 traffic, so "no input reports" alone never proves a wedged wheel
  (this is already recorded in the project's test notes, and was still
  misread twice today).
- The physical rim was watched over a webcam during the runaway and really
  was rotating violently, so that event was real force, not a graphic
  artefact. The caveat that survives is narrower: in-game wheel motion and
  physical rim motion are separate observables, and a frozen or off-centre
  input axis can make the in-game wheel look wild while the rim sits still,
  so captures should say which one they mean.

## 2026-08-12 evening: the transport works; the SDK is the only thing not using it

A power cycle reset the wheel, and every "no effects" symptom that followed
is one finding: **Logitech's SDK does not emit the TrueForce stream under
Proton on a clean wheel.** Measured across a full A/B:

| module | ep1 input | ep3 TF stream | KF into SDK |
|---|---|---|---|
| current, this morning (keepalive running) | 9918 | 11745 | 33k |
| current, tonight | 0 | 0 | 193 |
| v0.24.0 baseline, tonight | 11980 | 0 | 22k |

The kernel module is not the variable: v0.24.0 restored input reporting but
still produced no ep3 stream. The game's handshake is byte-identical to the
working morning run - dllOpen, SetForceMode(1) accepted, KF streaming at
~190/sec - yet the SDK writes nothing to endpoint 3.

**This morning's 11745-packet stream was almost certainly ours**, from the
FF_CONSTANT keepalive session, which triggers the driver's own 68-packet TF
init and streaming. Not the SDK's. The SDK has not been observed emitting
the stream in any clean test.

**Our TF transport, by contrast, works on the fresh wheel.** `logi-tf-sim
--sweep` with no game running produced a correct session on ep3: init, START,
2879 type-0x01 audio-sample packets at ~2 kHz with real varying content,
STOP. Confirmed by feel - the owner felt the sweep.

So the fix does not route through Logitech's SDK at all. The game's KF torque
(190/sec, cleanly intercepted by the dinput8 proxy) is the SDK's *input*, and
our TF channel is the transport the SDK would have used. Path: game -> KF
torque -> proxy -> driver TF channel -> wheel. No Logitech binary in the force
path, so no dependency on one that will not run correctly here. The forces are
the game's own; only the interpolation from ~190 Hz to 2 kHz is ours (the SDK
uses GetReconstructionFilterKF for the same step), which is the one thing that
is close rather than provably bit-identical.

### Superseded: the runaway theory
The runaway below was real but is now understood as the range-clamp war plus
an uncontrolled force session, not the core blocker. Kept for the record.

### It is not usable yet: the wheel runs away

With the range questions answered, the 2 kHz stream carries real content and
the rim moves through roughly full travel. That is **not** a working force
feedback reading. `logi-tf-sim`'s `ffb_keepalive` documents this exact failure
for issue #57: streaming to a direct-drive wheel with no force session open
drives it into its stops and oscillates there. Confirmed by feel report: the
wheel swings violently.

Holding a zero-level `FF_CONSTANT` open does stabilise it, and starts the 2 kHz
stream on its own, but it also stops the `0x0c` traffic that carries the game's
own values: a zero constant appears to cancel the game's torque.

So the open question is how to keep a direct-drive wheel's force loop alive
without zeroing what the game is sending. Doing it in the driver when it sees a
TrueForce stream on the vendor interface would cover a game as well as our own
daemon, and is a better depth for it than a userspace effect.

## 2026-08-12 late: NATIVE FFB WORKS, and the texture is decoded from Windows captures

**Native base force feedback through Logitech's real SDK works under Proton.**
Verified by feel and by wire: after a wheel power cycle, with the dinput8
proxy answering the four operating-range getters, the SDK streams type-0x01
packets at 2 kHz with the game's live force in `cur` (11883 packets/6 s,
sane values, felt correct). The steering-ratio problem was the SDK's
session-init clamp to 90 degrees: the quiesce patch had removed the
auto-heal, so it stuck. The quiesce is reverted (a single heal holds; the
SDK clamps once and does not fight), and the range poll now runs every 3 s
so the heal lands within seconds of track load.

## 2026-08-13: shipped - the texture is a kernel splice, and the OEM path is closed

Two things this document was open on are now settled.

**The base FFB recipe above is productized, not a manual step anymore.**
`logi-launch %command%` stages the range-answering proxy itself for Assetto
Corsa EVO on a direct-drive wheel: it copies `dinput8-escape.dll` into the
game's install directory and sets `WINEDLLOVERRIDES=dinput8=n,b`, tearing
both down again on exit. The power cycle this document found necessary for a
clean SDK session is still the user's to do; nothing here changes that.

**The engine-note texture is a kernel splice into the SDK's own stream, and
it is hardware-validated.** Rather than chase the SDK into sending texture it
was never observed sending on any OS, the driver now inserts synthesised
samples into the type-0x01 packets it is already relaying, the same point
G HUB merges at on Windows. Measured on the RS50 (module `9C1B5855`,
2026-08-13):

- merge off is byte-identical passthrough: 7847 packets, every one
  `byte10=00`, `cur=0x8000`;
- merge on with no RPM fed is untouched, proving the stale/zero gate on real
  hardware, not just in the unit tests;
- merge on with 6000 rpm fed splices every packet at the 4 kHz sample budget,
  `cur` bit-identical in every packet, sample rms 523.2 counts against the
  524-count capture-fit target (0.2% off), and the dominant frequency exactly
  400 Hz, which is a 6000 rpm V8's firing frequency;
- the SDK's operating-range push still lands (90.0 degrees), and the driver's
  auto-heal restores 900 in under 2 s with no manual write, logged as
  `restored range to 900 after SDK push (attempt 1/3)`;
- killing the RPM feed lets the texture die out inside the 200 ms staleness
  window, rather than droning on stale data.

`logi-launch %command%` arms this automatically for Assetto Corsa EVO on a
direct-drive wheel: proxy staged, `logi-rpm-bridge` started, the merge turned
on, all torn down again when the game exits.

**The OEM driver / compat-mode path is a dead end, not an unfinished lead.**
`hidpp_forcefeedback_x64.dll`'s device id list never contains `C276`, the
RS50's native mode, on Windows either, so there was never a version of this
where that driver had something to say about this wheel's native identity.
More to the point, the texture it would have carried is not a wire format to
reverse: on Windows it is G HUB synthesising it from the game's RPM on the
host and merging it into the SDK stream, the same operation this feature now
does in the kernel. There was no captured protocol to complete; the destination
was always synthesis, and the driver now does that synthesis itself.
