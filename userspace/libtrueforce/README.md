# libtrueforce

Native Linux implementation of the Logitech Trueforce SDK
(`trueforce_sdk_x64.dll`, version 1.3.11). Supports both the RS50
(`046d:c276`) and the G Pro Racing Wheel (`046d:c272` / `046d:c268`):
the two wheels use byte-for-byte identical init and streaming
packets, so the same library drives both. See
`docs/TRUEFORCE_PROTOCOL.md` in the parent repo for the protocol
documentation.

The library talks to the wheel's interface-2 hidraw node directly; no
custom kernel driver is required beyond what the `hid-logitech-dd` driver already provides.

While a stream is active the library also consumes the wheel's
type-0x02 responses (real-time wheel position and a device-side
sample counter) and exposes the latest snapshot via the Linux-native
`logitf_get_stream_feedback()` call - useful for closed-loop haptic
effects and for measuring the wheel's consumption rate. This API is
an extension; it has no Windows SDK counterpart.

## Safety

These are strong direct-drive wheels - the RS50 produces up to 8 Nm
of torque and the G PRO up to 11 Nm. The wheel may self-calibrate by
rotating when it powers up, when the active profile changes, or when
the Trueforce init sequence first runs after library load. Keep hands
clear of the rim, or firmly hold the wheel, whenever you:

- load the kernel driver,
- plug or replug the wheel,
- call any `logiTrueForceSetTorqueKF` / `logiTrueForceSetTorqueTF*`
  entry point for the first time in a session (this is when the
  library sends the 68-packet init sequence),
- switch profile via sysfs or the wheel's on-base Settings menu.

All library test programs (`tests/sine`, `tests/kf`) expect the user
to already be holding the wheel; the examples below include
countdown prompts so you can brace first.

## Status

Implemented. Discovery, session bring-up (the 68-packet two-pass
init), KF (kinetic-force / constant-force) torque, TF (TrueForce
audio-haptic) streaming, gain, damping, pause/resume, and the
version / capability getters all work end-to-end against a live
RS50. Tests under `tests/` exercise each surface against the
hardware.

This library is **not required** for the in-repo Proton recipe (ACC
+ TrueForce uses Logitech's own real signed DLLs through Wine; see
the project root README). It exists for **native Linux applications**
that want to drive TrueForce directly without going through Wine,
e.g. telemetry-driven haptic generators, custom test rigs, or
non-Steam game engines.

## Build

```bash
make               # builds libtrueforce.so.1.3.11 and tests/discover
sudo make install  # installs library + header under PREFIX (default /usr/local)
sudo make udev-install   # installs the hidraw access rule
```

After the udev rule is installed and udev is reloaded, unplug and
replug the wheel so the rule applies.

```bash
sudo udevadm control --reload
sudo udevadm trigger
```

Verify discovery:

```bash
./tests/discover
```

Expected output when a single RS50 is connected:

```
libtrueforce 1.3.11
  [0] available: supported=yes, paused=no
```

## udev rule

`udev/99-logitech-trueforce.rules` grants read/write access on
`/dev/hidrawN` for interface 2 of the supported wheels (`046d:c276`
RS50, `046d:c272` / `046d:c268` G PRO) to users in the `input` group,
and also tags it with `uaccess` so logind-managed sessions get access
automatically on login.

## API

`include/trueforce.h` mirrors the 62 named exports of the Windows
SDK (`trueforce_sdk_x64.dll` 1.3.11) so a Linux app can call the
same API surface. Function signatures use the host C ABI; all
Windows-isms (HANDLE, GUID, wide strings) have been translated to
plain C equivalents.

See `docs/TRUEFORCE_PROTOCOL.md` in the parent repo for the wire-
level protocol the library implements.
