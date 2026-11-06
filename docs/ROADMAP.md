# Roadmap

This page lists where the project goes next, in order. It gives no dates.
Owners of the hardware drive the work by what they can test, and that is
not on a schedule. [STATUS.md](STATUS.md) gives the current state of every
wheel and every game. This page is about what changes.

## Toward 1.0

1.0 means the RS50, the G PRO and both G923 editions work as this project
documents them. They must work on the distributions the project packages
for. They must need no steps the documentation does not mention. These
items still stand in the way:

1. **G PRO force feedback and TrueForce verified on the wire.** TrueForce
   is Logitech's high-rate haptic stream, which adds texture to the force.
   The configuration protocol is confirmed. The force stream is only
   inferred from RS50 captures (recordings of USB traffic). One owner's
   capture closes this item (#8).
2. **G923 Xbox edition parity with Windows.** The SDK route works end to
   end as of 0.41.0. The SDK is Logitech's library that a game loads to
   talk to the wheel. We do not know if every TrueForce effect the Windows
   stack renders reaches the wheel. A comparison capture is the instrument
   (#83).
3. **The force engine's condition effects calibrated by measurement**, not
   by feel. The reference is the firmware's own rendering. The method is
   the timed-travel method an owner established (#87).
4. **A second maintainer.** The project's bus factor is one. See
   [GOVERNANCE.md](../GOVERNANCE.md) for how that changes.

## Next

- **Test coverage measured in CI** and reported. CI is the automated check
  that runs on every change. Then the number in the documentation is a
  fact, not an estimate, and the parts that need hardware are named as
  such.
- **The three Windows binaries built by the packages rather than
  committed.** This waits until every packaging channel can build them
  offline. Until then, CI verifies them byte for byte against their source.
- **The unused wheel features** (`DUAL_CLUTCH`, `GAMING_ATTACHMENTS`,
  `AXIS_MAPPING`). Each needs a capture of Logitech's software using it.
  Only then can the driver use it safely next to live force.
- **AC Rally telemetry**, once the game publishes any.

## Not planned

- Support for wheels outside Logitech's HID++ TrueForce family. HID++ is
  Logitech's configuration protocol over USB. The in-tree drivers and
  new-lg4ff cover the older wheels.
- A graphical installer. The packages and `tools/setup.sh` are the install.
