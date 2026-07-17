# Logitech SDK Inventory

This directory documents the Logitech Windows SDK artifacts used to reason about
the driver's Linux-side API shape and the Wine bridge design. The SDK files
themselves are Logitech's and are **not** redistributed here (see the licensing
note below); what is tracked is our own derived research, chiefly the export
listings. Where an SDK file is referenced, obtain it as noted.

## What's here (tracked) and what you supply

| Path | Contents | Tracked? |
|---|---|---|
| `trueforce_1_3_11/exports_x64.txt` | Trueforce SDK 1.3.11 export listing (75 symbols), from `winedump -j export` | yes (our research) |
| `wheel_9_1_0/exports_x64.txt` | Wheel SDK 9.1.0 export listing (58 symbols) | yes (our research) |
| `Include/LogitechSteeringWheelLib.h`, `Include/LogitechGSDK.cs` | Public Steering Wheel SDK (2015) C header + C# binding | no - see "Obtaining the SDK files" |
| `trueforce_1_3_11/*.dll`, `wheel_9_1_0/*.dll`, `Logi/` | Trueforce SDK 1.3.11 and Wheel SDK 9.1.0 DLLs | no - see "Obtaining the SDK files" |

## Obtaining the SDK files

- **Public Steering Wheel SDK (2015)** - the `LogitechSteeringWheelLib.h` header
  and `LogitechGSDK.cs` binding ship in Logitech's public Gaming SDK, from the
  [Logitech G developer lab](https://www.logitechg.com/en-us/innovation/developer-lab.html).
  They are only needed for reference; the driver does not compile against them.
- **Trueforce / Wheel SDK DLLs** - not public. Copy them from a Logitech G HUB
  install (Windows, or unpacked into a throwaway wine prefix); see "DLL layout"
  below for the exact paths.

Drop either into this `sdk/` tree if you want them locally; the DLLs and the
public headers are listed in `sdk/.gitignore`, so they stay out of the repo.

## Legacy public SDK vs newer SDKs - what changed

### Public Steering Wheel SDK (2015)

Models supported top out at G920 / G29. The header enumerates constants like `LOGI_FORCE_SPRING`, `LOGI_FORCE_DIRT_ROAD`, and up to `LOGI_NUMBER_DEVICE_TYPES=4`. 39 functions. No Trueforce concept.

### Wheel SDK 9.1.0 (newer, post-2015)

58 exports. Adds versus the legacy SDK:

- `LogiGetDiState`, `LogiGetDiStateENGINES` - direct DirectInput state passthrough (bypasses internal bookkeeping)
- `LogiFreeStateENGINES` - explicit state struct cleanup (paired with GetStateENGINES)
- `LogiGetLedCaps`, `LogiGetLedCapsDInput` - runtime LED capability discovery
- `LogiSetRpmLedsDirect`, `LogiSetRpmLedsDirectDInput` - direct RPM LED control (vs `LogiPlayLeds` which takes RPM/first/redline)

Everything else is the same API. No Trueforce.

### Trueforce SDK 1.3.11

This is the SDK that Trueforce-aware games (BeamNG, some AC titles, iRacing) link against. 75 total exports; 62 have readable names, 13 are obfuscated C++ symbols that we don't need to shim.

**API groups (readable exports):**

Device lifecycle:
- `logiWheelOpenByDirectInputA/W`, `logiWheelClose`, `logiWheelSdkHasControl`
- `logiTrueForceAvailable`, `logiTrueForceSupported`, `logiTrueForceSupportedByDirectInputA/W`
- `logiWheelGetVersion`, `logiWheelGetCoreLibraryVersion`

Wheel state:
- `logiTrueForceGetAngleDegrees`, `logiTrueForceGetAngleRadians`
- `logiTrueForceGetAngularVelocityDegrees`, `logiTrueForceGetAngularVelocityRadians`

Force mode and range (mirrors Wheel SDK 9.1.0):
- `logiWheelGetForceMode`, `logiWheelSetForceMode`
- `logiWheelGetOperatingRangeDegrees/Radians`, `logiWheelSetOperatingRangeDegrees/Radians`
- `logiWheelGetOperatingRangeBoundsDegrees/Radians`
- `logiWheelGetRpmLedCaps`, `logiWheelPlayLeds`

Kinetic Force (KF - the classic constant-force channel):
- `logiTrueForceSetTorqueKF`, `logiTrueForceSetTorqueKFPiecewise`, `logiTrueForceGetTorqueKF`, `logiTrueForceClearKF`
- `logiTrueForceGetGainKF`, `logiTrueForceSetGainKF`
- `logiTrueForceGetMaxContinuousTorqueKF`, `logiTrueForceGetMaxPeakTorqueKF`
- `logiTrueForceGetReconstructionFilterKF`, `logiTrueForceSetReconstructionFilterKF`

Trueforce audio stream (TF):
- `logiTrueForceSetStreamTF` - set a stream of samples (the ~1kHz bulk API)
- `logiTrueForceSetTorqueTFfloat`, `logiTrueForceSetTorqueTFdouble`, `logiTrueForceSetTorqueTFint8/16/32` - per-sample or small-buffer setters, numeric type variants
- `logiTrueForceGetTorqueTF`, `logiTrueForceClearTF`
- `logiTrueForceGetGainTF`, `logiTrueForceSetGainTF`
- `logiTrueForceGetHapticRate`, `logiTrueForceGetHapticThreadStatus`
- `logiTrueForcePause`, `logiTrueForceResume`, `logiTrueForceIsPaused`
- `logiTrueForceSync` - stream synchronization
- `logiTrueForceGetTorqueTFRateBounds`

Damping / viscosity (shared between KF and TF):
- `logiTrueForceGetDamping`, `logiTrueForceSetDamping`, `logiTrueForceGetDampingMax`
- (viscosity appears in string table; not currently an export, likely deprecated/unused from this version)

Advanced / diagnostics:
- `logiAdvancedGetThreadHandles` - exposes SDK's internal thread handles to the host (for affinity / priority control)

DllRegisterServer, DllUnregisterServer, dllOpen, dllClose - standard DLL boilerplate.

**Architecture revealed by strings:**

The SDK does NOT talk to USB directly. String table includes:

- `local_connection::Connection`, `local_connection::CodecConnection` - the SDK uses a "local connection" abstraction
- `logi.trueforce.connect` - almost certainly the IPC endpoint name (named pipe, local socket, or similar)
- `Packet::Header`, `Packet::Gains`, `Packet::Aperture`, `Packet::HeloContainerId`, `Packet::HeloProtocolVersion` - packet types serialized over the IPC
- `"TrueForce message pump"` - SDK runs a background thread processing incoming packets
- `trueforce_features.cfg`, `trueforce_data.bin` - device-specific config files (likely under `C:\Program Files\LGHUB\` or similar)

**Implication:** on Windows the flow is:

```
Game ── links ──▶ trueforce_sdk_x64.dll ── IPC(logi.trueforce.connect) ──▶ G HUB Agent ──▶ USB ep 0x03 ──▶ RS50
```

The USB wire protocol we see in captures is generated by the G HUB Agent, not by the SDK. The SDK only serializes high-level "packets" and hands them off.

### "KF" vs "TF" inside the SDK

- **KF - Kinetic Force** - classic constant-force style torque. Single value per call, or piecewise curve. Maps to the existing PID FFB path on the wheel (feature 0x8123 via HID++, or on the RS50 the dedicated endpoint 0x03 with a DC force value).
- **TF - Trueforce** - the audio haptic stream. ~1000 samples/sec (per captures). Multiple numeric types accepted (int8/int16/int32/float/double) - the SDK does the conversion before serializing.

Both channels coexist at runtime: the SDK sets both KF (slow, steering feel) and TF (fast, vibration/texture) simultaneously.

### How we use this in practice

- The Proton recipe in the project README installs the unmodified, Logitech-signed Trueforce DLL into each Wine prefix and registers its CLSID. The DLL talks to Wine's HID stack rather than the (nonexistent on Linux) G HUB Agent named pipe; Wine's HID stack reaches our kernel driver. End-to-end verified against ACC and AC EVO, on RS50 in both G PRO compatibility mode and native mode (`046d:c276`, AC EVO 2026-07-08, usbmon-confirmed). See `tools/install-tf-shim.sh`.
- For native Linux apps that want to drive Trueforce directly (no Wine in the loop), `userspace/libtrueforce/` is a from-scratch C reimplementation of the same protocol; `include/trueforce.h` mirrors the 62 named exports of the Windows DLL.
- `logiTrueForceSetStreamTF` suggested a bulk-upload path in addition to per-sample. libtrueforce supports streaming writes via `write(2)` on the wheel's interface-2 hidraw node rather than a single ioctl per sample, matching the wire-level format.

## How to re-dump the exports

```bash
# From the repo root:
winedump -j export sdk/trueforce_1_3_11/trueforce_sdk_x64.dll > sdk/trueforce_1_3_11/exports_x64.txt
winedump -j export sdk/wheel_9_1_0/logi_steering_wheel_x64.dll > sdk/wheel_9_1_0/exports_x64.txt
```

On Arch/Fedora, `winedump` ships with `wine-core`. On Debian/Ubuntu, install `wine`.

## DLL layout consumed by `tools/install-tf-shim.sh`

The shim installer needs the four real Logitech-signed SDK DLLs, in
Logitech's own `Logi/...` directory tree. These files are gitignored and
**must be supplied by the user**. The installer resolves the directory
that holds the `Logi/` tree in this order: `--sdk-dir <path>`, then
`$LOGITECH_TRUEFORCE_SDK_DIR`, then the repo's `sdk/` subdirectory if
present, otherwise `~/.local/share/logitech-trueforce/sdk` (the default
when installed from the AUR, where there is no repo tree).

Required files, relative to that directory:

```
Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll
```

How to obtain them: install Logitech G HUB on Windows (or in a throwaway
wine prefix on Linux) and copy the contents of
`C:\Program Files\Logi\Trueforce\1_3_11\` and
`C:\Program Files\Logi\wheel_sdk\9_1_0\` into the matching paths above.
File names and directory casing must match.

`tools/install-tf-shim.sh` runs `require_sources` first; if any of the
four files are missing it prints the resolved directory and the expected
paths and exits with status 2 without touching any wine prefix, so you
can re-run it safely after populating the tree.

### Newer SDK releases are drop-in compatible

Logitech ships point-release updates to these DLLs through G HUB (for
example a build a patch version above 1.3.11, with a slightly larger
file). These are safe to use as-is.

The `1_3_11` / `9_1_0` folder names are a fixed label the games expect,
not the DLL's own version. The installer keeps the DLLs at those exact
paths and registers their COM CLSIDs to point there; games find the SDK
through the CLSID, and some also key off the install path string, which
is why the path is held stable. A newer DLL dropped into the same path
therefore satisfies both. It also keeps the same exported interface:
diffing the export tables of a later TrueForce / wheel release against
the 1.3.11 / 9.1.0 ones gives identical symbol names and counts. So if
G HUB gives you a higher point release, place those files at the paths
above and the shim works unchanged. A version bump on its own is not a
cause of missing force feedback.

## Licensing note

The Logitech SDK files - the DLLs in `trueforce_1_3_11/`, `wheel_9_1_0/` and
`Logi/`, and the public SDK headers in `Include/` - are Logitech's copyrighted
material. They are kept locally for reference and interoperability only and are
not redistributed here; every one of those paths is listed in `sdk/.gitignore`.
Obtain them from Logitech as described in "Obtaining the SDK files". Only the
export listings we generate ourselves are derived research data and tracked.
