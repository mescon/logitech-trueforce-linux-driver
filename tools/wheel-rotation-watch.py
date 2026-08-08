#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Sample a wheel's steering position while something drives it.

Exists because "did the wheel move?" kept being answered by feel, and feel
cannot distinguish a 500 degree excursion from a 500 degree excursion that
reverses direction seven times. The axis can. It also means nobody has to
hold a direct-drive wheel that may slam to its stop.

    tools/wheel-rotation-watch.py --sweep 40
    tools/wheel-rotation-watch.py --wheel g923 --sweep 40
    tools/wheel-rotation-watch.py --cmd userspace/libtrueforce/tests/sine 50 2 0.3

Reports the SHAPE of the motion, not a single number: peak excursion alone
is unsigned and saturates at the range limit, so it renders a runaway and a
seven-reversal oscillation identically. Total travel, reversal count and
time spent against each stop are what actually distinguish them.

Two limits worth knowing before trusting a run:

  - The axis clamps at +/- half the configured range. A wheel that spins
    past its soft stop reports no further change, so rotation beyond the
    range is INVISIBLE here and total travel undercounts it.
  - The wheel driven must match the axis watched. Driving one wheel while
    watching another reads as a clean zero, which looks like a pass.
"""
import argparse
import shutil
import os
import subprocess
import sys
import threading
import time

try:
    import evdev
    from evdev import ecodes
except ImportError:
    sys.exit("needs python-evdev (pacman -S python-evdev, apt install python3-evdev)")

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SIM = os.path.join(REPO, "userspace/logi-wheel/target/release/logi-tf-sim")

# Steering-wheel product ids: RS50, G PRO, G923 (PS), G923 (PS/alt), G923 (Xbox).
WHEEL_IDS = {"rs50": ["c276"], "gpro": ["c272"], "g923": ["c266", "c267", "c26e"]}
SAMPLE_HZ = 50


def find_wheel(want):
    """Return (InputDevice, product_id) for the first matching wheel axis."""
    wanted = WHEEL_IDS.get(want, sum(WHEEL_IDS.values(), [])) if want != "auto" \
        else sum(WHEEL_IDS.values(), [])
    for path in evdev.list_devices():
        dev = evdev.InputDevice(path)
        caps = dev.capabilities().get(ecodes.EV_ABS, [])
        if not any(code == ecodes.ABS_X for code, _ in caps):
            continue
        pid = f"{dev.info.product:04x}"
        if pid in wanted:
            return dev, pid
        dev.close()
    return None, None


def read_range(pid):
    """The wheel's configured range in degrees, from the driver's sysfs."""
    base = "/sys/bus/hid/devices"
    for entry in sorted(os.listdir(base)):
        if pid.upper() not in entry:
            continue
        for attr in ("wheel_range", "range"):
            try:
                with open(os.path.join(base, entry, attr)) as fh:
                    return float(fh.read().strip())
            except OSError:
                continue
    return 1080.0


def sweep_pitch(text):
    """A pitch percentage, and nothing else.

    This value becomes an argv entry for the simulated-TrueForce daemon, so
    it is validated here rather than forwarded verbatim: the daemon accepts
    10-200 and anything outside that is a mistake worth catching before it
    reaches another program's command line.
    """
    try:
        value = int(text, 10)
    except ValueError:
        raise argparse.ArgumentTypeError(f"pitch must be a whole number, got {text!r}")
    if not 10 <= value <= 200:
        raise argparse.ArgumentTypeError(f"pitch must be 10-200, got {value}")
    return str(value)


def resolved_command(argv):
    """Resolve `argv` to a concrete executable path plus its arguments.

    --cmd exists to run an arbitrary test binary, so the program itself is
    the caller's choice by design. What is checked is that it resolves to
    something that exists and is executable, so a typo or a stray argument
    fails here with a clear message instead of becoming an exec of whatever
    that text happened to name. Nothing is ever passed to a shell: the
    command is executed as an argument vector.
    """
    if not argv:
        raise SystemExit("--cmd needs a program to run")
    program = shutil.which(argv[0]) or (
        os.path.abspath(argv[0]) if os.path.isfile(argv[0]) else None
    )
    if program is None or not os.access(program, os.X_OK):
        raise SystemExit(f"--cmd: {argv[0]!r} is not an executable on PATH or a runnable file")
    return [program, *argv[1:]]


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--wheel", default="auto", choices=["auto", "rs50", "gpro", "g923"])
    ap.add_argument("--sweep", metavar="PITCH", type=sweep_pitch,
                    help="run logi-tf-sim --sweep PITCH (10-200)")
    ap.add_argument("--tail", type=float, default=6.0,
                    help="seconds to keep sampling after the command exits "
                         "(motion often continues well past it)")
    ap.add_argument("--cmd", nargs=argparse.REMAINDER,
                    help="run this instead: PROGRAM [ARGS...]")
    args = ap.parse_args()

    if not args.sweep and not args.cmd:
        ap.error("give --sweep PITCH or --cmd ...")

    dev, pid = find_wheel(args.wheel)
    if not dev:
        sys.exit(f"no {args.wheel} wheel with a steering axis found")
    info = dev.absinfo(ecodes.ABS_X)
    span = info.max - info.min
    range_deg = read_range(pid)

    def degrees(raw):
        return (raw - info.min) / span * range_deg - range_deg / 2.0

    samples = []
    stop = threading.Event()

    def sampler():
        while not stop.is_set():
            samples.append((time.monotonic(), dev.absinfo(ecodes.ABS_X).value))
            time.sleep(1.0 / SAMPLE_HZ)

    thread = threading.Thread(target=sampler, daemon=True)
    thread.start()
    time.sleep(0.5)                       # baseline before anything is sent

    env = dict(os.environ)
    if args.sweep:
        # Force the DD path for a DD wheel: the daemon prefers a G923
        # whenever one is attached, so on a two-wheel rig the sweep would
        # otherwise drive a wheel this script is not watching.
        env.setdefault("LOGI_TF_SIM_WHEEL", "dd" if pid != "c266" else "auto")
        if not os.access(SIM, os.X_OK):
            raise SystemExit(f"{SIM} is not built; run: cargo build --release")
        cmd = [SIM, "--sweep", args.sweep]
    else:
        cmd = resolved_command(args.cmd)

    print(f"watching {dev.name} ({pid}), range {range_deg:.0f} deg")
    print(f"running  {' '.join(cmd)}")
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True)
    time.sleep(args.tail)
    stop.set()
    thread.join()

    if len(samples) < 2:
        sys.exit("no samples captured")

    t0 = samples[0][0]
    degs = [degrees(v) for _, v in samples]
    start, lo, hi = degs[0], min(degs), max(degs)

    print(f"\n  t(s)   deg   {-range_deg / 2:>6.0f}{'0':^29}{range_deg / 2:<+6.0f}")
    buckets = {}
    for (ts, _), d in zip(samples, degs):
        buckets.setdefault(int((ts - t0) * 5), []).append(d)
    for key in sorted(buckets):
        d = sum(buckets[key]) / len(buckets[key])
        col = max(0, min(40, int((d + range_deg / 2) / range_deg * 40)))
        row = ["."] * 41
        row[20] = "|"
        row[col] = "#"
        print(f"  {key / 5:4.1f} {d:7.1f}   {''.join(row)}")

    stop_band = range_deg / 2 * 0.97
    travel = sum(abs(b - a) for a, b in zip(degs, degs[1:]))
    at_left = sum(1 for d in degs if d <= -stop_band) / SAMPLE_HZ
    at_right = sum(1 for d in degs if d >= stop_band) / SAMPLE_HZ
    reversals, last = 0, 0
    for a, b in zip(degs, degs[1:]):
        if abs(b - a) < 3.0:              # ignore jitter
            continue
        direction = 1 if b > a else -1
        if last and direction != last:
            reversals += 1
        last = direction

    print(f"\n  start {start:+.0f} deg     visited {lo:+.0f} .. {hi:+.0f}")
    print(f"  total travel        {travel:.0f} deg")
    print(f"  direction reversals {reversals}")
    print(f"  time at LEFT stop   {at_left:.1f} s")
    print(f"  time at RIGHT stop  {at_right:.1f} s")
    print(f"  NOTE: the axis clamps at +/-{range_deg / 2:.0f}; rotation past "
          "the stop is not visible here.")
    if proc.returncode != 0:
        print(f"\n  exited {proc.returncode}: {proc.stderr.strip()[:300]}")


if __name__ == "__main__":
    main()
