"""Telemetry sources for the rev-light bridge.

A source yields the current engine state (rpm + redline) so the bridge can
drive the wheel's rev LEDs the way G Hub does on Windows. Sources are
interchangeable: the synthetic one needs no game and is used to test the LED
pipeline; the AC EVO one reads the sim's shared memory under Proton.
"""
from __future__ import annotations

import glob
import os
import struct
import time
from dataclasses import dataclass


@dataclass
class Sample:
    """One telemetry reading. `max_rpm` is 0 when the source cannot supply a
    redline; the bridge then falls back to its configured redline."""
    rpm: float
    max_rpm: float = 0.0
    gear: int = 0
    speed_kmh: float = 0.0


class Source:
    """Telemetry source interface. `read()` returns the latest Sample, or
    None when no fresh data is available (e.g. sim not in a session)."""

    name = "base"

    def open(self) -> None:  # noqa: A003 - deliberate verb
        pass

    def read(self) -> Sample | None:
        raise NotImplementedError

    def close(self) -> None:
        pass


class SyntheticSource(Source):
    """A game-free RPM sweep, so the LED pipeline can be verified on the
    bench. Ramps idle -> redline -> idle on a fixed period."""

    name = "synthetic"

    def __init__(self, redline: float = 8000.0, idle: float = 1000.0,
                 period_s: float = 6.0):
        self.redline = redline
        self.idle = idle
        self.period_s = period_s
        self._t0 = None

    def open(self) -> None:
        # Monotonic start captured lazily (Date/time helpers stay out of ctor).
        self._t0 = time.monotonic()

    def read(self) -> Sample | None:
        t = time.monotonic() - (self._t0 or 0.0)
        # triangle wave 0..1..0 over period_s
        phase = (t % self.period_s) / self.period_s
        tri = 1.0 - abs(2.0 * phase - 1.0)
        rpm = self.idle + (self.redline - self.idle) * tri
        gear = 1 + int(tri * 6)
        return Sample(rpm=rpm, max_rpm=self.redline, gear=gear,
                      speed_kmh=tri * 250.0)


# --- Assetto Corsa (EVO / ACC / AC) shared memory ---------------------------
#
# AC-family sims publish SPageFilePhysics via the Windows named mapping
# "Local\\acpmf_physics" and SPageFileStatic via "Local\\acpmf_static".
# Under Proton these land in /dev/shm/u<uid>-Shm_<hash> with a hashed name we
# cannot predict, so we SCAN every segment and identify physics by its layout
# plus a live-incrementing packetId. maxRpm comes from the static page.
#
# SPageFilePhysics early layout (packed LE), which is all we need:
#   int packetId; float gas,brake,fuel; int gear; int rpms; float steerAngle;
#   float speedKmh; ...
_PHYS_FMT = "<ifffiiff"   # packetId,gas,brake,fuel,gear,rpms,steer,speedKmh
_PHYS_LEN = struct.calcsize(_PHYS_FMT)

# Wine may prefix the mapping with a small header; try a few start offsets.
_TRY_OFFSETS = (0, 8, 16, 32)


def _parse_phys(buf: bytes, off: int):
    if off + _PHYS_LEN > len(buf):
        return None
    pid, gas, brake, fuel, gear, rpms, steer, kmh = struct.unpack_from(
        _PHYS_FMT, buf, off)
    return pid, gas, brake, fuel, gear, rpms, steer, kmh


def _plausible_phys(p) -> bool:
    if p is None:
        return False
    _pid, gas, brake, fuel, gear, rpms, _steer, kmh = p
    # fuel > 0: a car in a session always has some. This is the cheap
    # discriminator that rejects the all-float-zero Wine counter segments,
    # which otherwise pass every other bound and get falsely locked (their
    # incrementing counter masquerades as a packetId).
    return (0.0 <= gas <= 1.01 and 0.0 <= brake <= 1.01
            and 0.1 < fuel <= 1000.0
            and -1 <= gear <= 10 and 0 <= rpms <= 20000
            and 0.0 <= kmh <= 500.0)


class AcEvoShmSource(Source):
    """Reads rpm from AC EVO's acpmf_physics shared memory under Proton.

    Locates the segment once (by layout + a moving packetId), then reads it
    each tick. `max_rpm` is read from the static page if found, else 0 so the
    bridge uses its configured redline. Returns None while no live physics
    segment is present (sim closed or not in a session)."""

    name = "acevo"

    def __init__(self):
        self._phys_path = None
        self._phys_off = 0
        self._max_rpm = 0.0
        self._last_locate = 0.0

    def _segments(self):
        return glob.glob("/dev/shm/u%d-Shm_*" % os.getuid())

    def _read_head(self, path, n=512):
        try:
            with open(path, "rb") as f:
                return f.read(n)
        except OSError:
            return b""

    def _locate_physics(self) -> bool:
        """Find the live physics segment. Plausibility alone is far too loose
        (a zeroed segment passes), so the decisive signal is a packetId that
        STRICTLY INCREASES across successive samples - only the sim's running
        physics loop does that. This rejects static/zeroed segments and the
        occasional buffer that merely changes. Requires a car in an unpaused
        session; a paused sim freezes packetId and simply won't lock (fine -
        no rev LEDs needed while paused)."""
        seq = {}  # (path, off) -> [packetId, ...]
        for _ in range(3):
            for path in self._segments():
                buf = self._read_head(path)
                for off in _TRY_OFFSETS:
                    p = _parse_phys(buf, off)
                    if _plausible_phys(p):
                        seq.setdefault((path, off), []).append(p[0])
            time.sleep(0.04)
        for (path, off), pids in seq.items():
            if len(pids) >= 3 and pids[0] < pids[1] < pids[2]:
                self._phys_path, self._phys_off = path, off
                return True
        return False

    def open(self) -> None:
        # Per-car redline (SPageFileStatic maxRpm) auto-detection is deferred:
        # identifying the static page reliably needs the same live-signal
        # treatment as physics. For now the bridge uses its configured
        # --redline; max_rpm stays 0.
        self._locate_physics()

    def read(self) -> Sample | None:
        now = time.monotonic()
        # (Re)locate at most a few times per second if we have no lock.
        if self._phys_path is None and now - self._last_locate > 0.5:
            self._last_locate = now
            self.open()
        if self._phys_path is None:
            return None
        buf = self._read_head(self._phys_path)
        p = _parse_phys(buf, self._phys_off)
        if not _plausible_phys(p):
            # Lost the segment (sim closed / remapped): drop the lock.
            self._phys_path = None
            return None
        _pid, _gas, _brake, _fuel, gear, rpms, _steer, kmh = p
        return Sample(rpm=float(rpms), max_rpm=self._max_rpm,
                      gear=int(gear), speed_kmh=float(kmh))


SOURCES = {
    "synthetic": SyntheticSource,
    "acevo": AcEvoShmSource,
}
