# Native force feedback and TrueForce under Proton: what is actually in the way

Neither force feedback nor TrueForce reaches a direct-drive wheel from a
Logitech SDK title under Proton, and the reason is not the wheel, the driver,
or the SDK. It is that Windows routes both through a component Wine has no
equivalent for: a **DirectInput OEM force-feedback driver**.

Everything here was measured on an RS50 (`046d:c276`) in Assetto Corsa EVO on
2026-08-11, with `tools/dinput8-escape-proxy.cpp` and `usbmon`.

## The short version

| Layer | State |
|---|---|
| Our shim registers the SDK | fixed; it was broken from 0.27.1 to 0.34.x by a quoting bug |
| The game loads the SDK | yes, verified by signature and loaded |
| The game resolves the SDK API | yes, 56 of 59 symbols |
| The SDK opens the wheel | yes, interface `mi_02`, the TrueForce interface |
| The SDK writes to the wheel | **no. Zero USB output transfers in a whole session** |
| Where the missing work happens on Windows | `hidpp_forcefeedback_x64.dll`, a DirectInput OEM effect driver |
| Does Wine load such drivers | **no** |

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
3. **Nothing routes DirectInput effects or `Escape` to Logitech's driver.**
   This is the one that remains.

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
