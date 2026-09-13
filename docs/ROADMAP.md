# Roadmap

Where the project is going, in the order it intends to get there. Dates are
deliberately absent: the work is driven by what owners of the hardware can
test, and that is not on a schedule. The current state of every wheel and
every game is in [STATUS.md](STATUS.md); this page is about what changes.

## Toward 1.0

1.0 means the RS50, the G PRO and both G923 editions work as this project
documents them, on the distributions it packages for, without steps the
documentation does not mention. What still stands between here and there:

1. **G PRO force feedback and TrueForce verified on the wire.** The
   configuration protocol is confirmed; the force stream has only been
   inferred from RS50 captures. One owner's capture closes this (#8).
2. **G923 Xbox edition parity with Windows.** The SDK route works end to
   end as of 0.41.0; whether every TrueForce effect the Windows stack
   renders reaches the wheel is open, and the comparison capture is the
   instrument (#83).
3. **The force engine's condition effects calibrated by measurement**, not
   by feel, against the firmware's own rendering, using the timed-travel
   method an owner established (#87).
4. **A second maintainer.** The project's bus factor is one. See
   [GOVERNANCE.md](../GOVERNANCE.md) for how that changes.

## Next

- **Test coverage measured in CI** and reported, so the number in the
  documentation is a fact rather than an estimate, and the parts that need
  hardware are named as such.
- **The three Windows binaries built by the packages rather than
  committed**, once every packaging channel can build them offline; until
  then they are byte-verified against their source in CI.
- **The unused wheel features** (`DUAL_CLUTCH`, `GAMING_ATTACHMENTS`,
  `AXIS_MAPPING`), each of which needs a capture of Logitech's software
  using it before it can be driven safely alongside live force.
- **AC Rally telemetry**, once the game publishes any.

## Not planned

- Support for wheels outside Logitech's HID++ TrueForce family; the
  in-tree drivers and new-lg4ff cover the older ones.
- A graphical installer; the packages and `tools/setup.sh` are the install.
