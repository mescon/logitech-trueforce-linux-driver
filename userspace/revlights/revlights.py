#!/usr/bin/env python3
"""revlights - drive the wheel's rev/RPM LEDs from game telemetry.

On Windows, G Hub reads a sim's telemetry and lights the wheel's rev strip;
the game itself does not (verified by capture for AC EVO - it only sends
force feedback and TrueForce). This bridge does the G Hub job on Linux: it
reads engine RPM from a telemetry source and writes the wheel's
`wheel_rev_level` sysfs (0..MAX) at a steady rate.

Runs as your normal user (the sysfs attribute is group-writable via the udev
rule). No root needed.

    python3 revlights.py --source synthetic      # bench test, no game
    python3 revlights.py --source acevo          # read AC EVO shared memory

Kill switch: Ctrl-C (turns the LEDs off on exit).
"""
from __future__ import annotations

import argparse
import glob
import os
import signal
import sys
import time
from dataclasses import dataclass

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sources import SOURCES, Sample  # noqa: E402

# The driver caps the rev strip at HIDPP_DD_REV_MAX_LEVEL.
MAX_LEVEL = 10


def find_rev_sysfs() -> str | None:
    """Locate the wheel's wheel_rev_level attribute via any bound hidraw."""
    for p in glob.glob("/sys/class/hidraw/*/device/wheel_rev_level"):
        return p
    return None


@dataclass
class Config:
    source: str = "synthetic"
    redline: float = 8000.0        # fallback when the source has no maxRpm
    first_led_frac: float = 0.60   # fraction of redline where LED 1 lights
    rate_hz: float = 60.0
    verbose: bool = False


class RevMapper:
    """Maps (rpm, redline) to a 0..MAX_LEVEL rev-strip level, mirroring
    G Hub / LogiPlayLeds: nothing below the first-LED point, then linear up
    to full at the redline."""

    def __init__(self, cfg: Config):
        self.cfg = cfg

    def level(self, s: Sample) -> int:
        redline = s.max_rpm if s.max_rpm > 0 else self.cfg.redline
        if redline <= 0:
            return 0
        first = self.cfg.first_led_frac * redline
        if s.rpm <= first:
            return 0
        if s.rpm >= redline:
            return MAX_LEVEL
        span = redline - first
        frac = (s.rpm - first) / span
        return max(1, min(MAX_LEVEL, round(frac * MAX_LEVEL)))


class RevWriter:
    """Writes wheel_rev_level, skipping redundant writes so the driver's
    latest-value-wins worker is not spammed with identical levels."""

    def __init__(self, path: str):
        self.path = path
        self._last = None

    def set(self, level: int) -> None:
        if level == self._last:
            return
        try:
            with open(self.path, "w") as f:
                f.write(str(level))
            self._last = level
        except OSError as e:
            print(f"revlights: write {self.path} failed: {e}", file=sys.stderr)

    def off(self) -> None:
        self._last = None
        self.set(0)


def run(cfg: Config) -> int:
    sysfs = find_rev_sysfs()
    if not sysfs:
        print("revlights: no wheel found (no wheel_rev_level sysfs). Is the "
              "driver loaded and the wheel bound?", file=sys.stderr)
        return 1
    if cfg.source not in SOURCES:
        print(f"revlights: unknown source '{cfg.source}' "
              f"(have: {', '.join(SOURCES)})", file=sys.stderr)
        return 2

    source = SOURCES[cfg.source]()
    if cfg.source == "synthetic":
        source.redline = cfg.redline
    mapper = RevMapper(cfg)
    writer = RevWriter(sysfs)

    print(f"revlights: source={cfg.source} sysfs={sysfs} "
          f"redline={cfg.redline:.0f} rate={cfg.rate_hz:.0f}Hz "
          f"first_led={cfg.first_led_frac:.0%}")

    running = {"go": True}

    def stop(*_a):
        running["go"] = False

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    def apply_sample(s: Sample | None, state: dict) -> None:
        """Handle one telemetry read: drive the LEDs, or turn them off once
        when telemetry goes away (sim closed / between sessions)."""
        if s is None:
            if state["idle"] == 0:
                writer.off()
            state["idle"] += 1
            return
        state["idle"] = 0
        lvl = mapper.level(s)
        writer.set(lvl)
        if cfg.verbose:
            print(f"\rrpm={s.rpm:6.0f} gear={s.gear:+d} "
                  f"lvl={lvl:2d} {'#' * lvl:<{MAX_LEVEL}}", end="")

    source.open()
    period = 1.0 / cfg.rate_hz
    state = {"idle": 0}
    try:
        while running["go"]:
            t0 = time.monotonic()
            apply_sample(source.read(), state)
            dt = time.monotonic() - t0
            if dt < period:
                time.sleep(period - dt)
    finally:
        if cfg.verbose:
            print()
        writer.off()
        source.close()
        print("revlights: stopped, LEDs off.")
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Drive the wheel rev LEDs from "
                                             "game telemetry.")
    ap.add_argument("--source", default="synthetic",
                    choices=list(SOURCES),
                    help="telemetry source (default: synthetic)")
    ap.add_argument("--redline", type=float, default=8000.0,
                    help="fallback redline RPM when the source has none")
    ap.add_argument("--first-led", type=float, default=0.60,
                    help="fraction of redline where the first LED lights")
    ap.add_argument("--rate", type=float, default=60.0,
                    help="update rate in Hz")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="print rpm/level each tick")
    a = ap.parse_args(argv)
    cfg = Config(source=a.source, redline=a.redline,
                 first_led_frac=a.first_led, rate_hz=a.rate,
                 verbose=a.verbose)
    return run(cfg)


if __name__ == "__main__":
    raise SystemExit(main())
