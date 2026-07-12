# revlights - rev/RPM LEDs from game telemetry

Drives the wheel's rev strip (`wheel_rev_level`) from a sim's engine RPM, the
way G Hub does on Windows.

## Why this exists

The rev LEDs are **not driven by the game**. A usbmon capture of ~6 minutes of
Assetto Corsa EVO showed the game sends only force feedback / TrueForce (on the
FFB endpoint) and a rotation-range command at track load - and **zero LED
commands**. On Windows, G Hub reads the sim's telemetry and lights the strip;
on Linux nothing does, so the LEDs stay dark or static. This bridge fills that
gap: it reads RPM from a telemetry source and writes the driver's
`wheel_rev_level` (0..10) at a steady rate.

The wheel's LED hardware and the driver's control of it already work - sweeping
`wheel_rev_level` animates the strip. The only missing link was a source of
live RPM.

## Usage

Runs as your normal user (the sysfs attribute is group-writable via the udev
rule). No root.

```bash
# Bench test with a synthetic RPM sweep - no game needed. Watch the rim LEDs
# climb and fall:
python3 revlights.py --source synthetic

# Read Assetto Corsa EVO's shared memory (run it while driving):
python3 revlights.py --source acevo --redline 8000
```

Options:

| flag | default | meaning |
|------|---------|---------|
| `--source` | `synthetic` | `synthetic` or `acevo` |
| `--redline` | `8000` | fallback redline RPM (per-car auto-detect is TODO) |
| `--first-led` | `0.60` | fraction of redline where LED 1 lights |
| `--rate` | `60` | update rate (Hz) |
| `-v` | off | print rpm/level each tick |

Ctrl-C stops it and turns the LEDs off.

**Desktop mode:** drive the wheel in desktop mode (`echo desktop >
.../wheel_mode`); onboard mode shows a stored LED profile and ignores live LED
writes.

## Sources

- **synthetic** - a triangle-wave RPM sweep, idle → redline → idle. No game.
  Used to verify the LED pipeline; verified live on an RS50.
- **acevo** - reads `acpmf_physics` from AC EVO's Proton shared memory. The
  Windows mapping lands in `/dev/shm/u<uid>-Shm_<hash>` under a hashed name, so
  the source scans segments and locks onto the one whose `packetId` strictly
  increases (the live physics loop), then reads `rpms`. **Status: written,
  pending live verification against the running game.**

## Design notes

- The telemetry source is a small interface (`sources.py`): `open()` /
  `read() -> Sample | None` / `close()`. Adding a game = adding a source.
- `Sample.max_rpm` is `0` when the source has no redline; the bridge then uses
  `--redline`. Per-car redline (AC's `SPageFileStatic.maxRpm`) is a future
  addition - it needs the same live-signal identification the physics page uses.
- Redundant `wheel_rev_level` writes are suppressed so the driver's
  latest-value-wins worker isn't spammed.

This is the first telemetry piece of the planned Linux wheel-control app (the
"G Hub replacement"); it is deliberately standalone so it is useful on its own.
