#!/usr/bin/env python3
"""Bridge rev-LED mapping: LTFR datagrams in, wheel_rev_level writes out.

Covers both mappings ("bar" default: LED 1 as soon as the engine turns,
10 at the limiter; "shift": G HUB's dash band, dark below the car's
first-shift-light rpm) plus the legacy 28-byte datagram behaviour in each.
Runs the bridge on an isolated port (LOGI_RPM_PORT) so a live bridge (or
logi-tf-sim) on the standard 20780 neither steals the test datagrams nor
makes the test's own bridge fail to bind: the port has exactly one owner.
"""
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time

PORT = 21780
HERE = os.path.dirname(os.path.abspath(__file__))
BRIDGE_SRC = os.path.join(HERE, "../../tools/logi-rpm-bridge.c")

fails = []


def check(name, got, want):
    ok = got == want
    print(f"{'PASS' if ok else 'FAIL'} {name}: got {got!r} want {want!r}")
    if not ok:
        fails.append(name)


def run_mode(tmp, bridge_bin, mode_env, cases):
    rpm_f = os.path.join(tmp, "fake_rpm")
    rev_f = os.path.join(tmp, "fake_rev")
    open(rpm_f, "w").close()
    open(rev_f, "w").close()
    env = dict(os.environ, LOGI_RPM_SYSFS=rpm_f, LOGI_REV_SYSFS=rev_f,
               LOGI_RPM_PORT=str(PORT), **mode_env)
    br = subprocess.Popen([bridge_bin], env=env)
    try:
        time.sleep(0.3)
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

        def send(rpm, mx, first_led=None):
            pkt = bytearray(28 if first_led is None else 32)
            pkt[0:4] = b"LTFR"
            pkt[4] = 2
            pkt[14:18] = struct.pack("<f", rpm)
            pkt[18:22] = struct.pack("<f", mx)
            if first_led is not None:
                pkt[28:32] = struct.pack("<f", first_led)
            sock.sendto(bytes(pkt), ("127.0.0.1", PORT))
            time.sleep(0.06)

        label = mode_env.get("LOGI_REV_MODE", "bar")
        for name, args, want in cases:
            send(*args)
            check(f"[{label}] {name}", open(rev_f).read().strip(), want)
        sock.close()
    finally:
        br.terminate()
        br.wait(timeout=3)


def main():
    with tempfile.TemporaryDirectory() as tmp:
        bridge_bin = os.path.join(tmp, "bridge")
        subprocess.run(["cc", "-O2", "-Wall", "-o", bridge_bin, BRIDGE_SRC],
                       check=True)
        run_mode(tmp, bridge_bin, {}, [
            ("engine off -> 0", (0, 14250, 11250), "0"),
            ("idle -> 2 (bar is alive at idle)", (2950, 14250, 11250), "2"),
            ("half -> 5", (7125, 14250, 11250), "5"),
            ("redline -> 10", (14250, 14250, 11250), "10"),
            ("over-rev clamps -> 10", (15500, 14250, 11250), "10"),
            ("legacy 28B still renders", (7125, 14250, None), "5"),
        ])
        run_mode(tmp, bridge_bin, {"LOGI_REV_MODE": "shift"}, [
            ("idle below band -> 0", (2950, 14250, 11250), "0"),
            ("exactly first light -> 1", (11250, 14250, 11250), "1"),
            ("redline -> 10", (14250, 14250, 11250), "10"),
            ("legacy 28B leaves LEDs alone", (13000, 14250, None), "10"),
            ("degenerate first_led leaves LEDs", (13500, 14250, 14250), "10"),
        ])
    print("FAILURES:", fails if fails else "none")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
