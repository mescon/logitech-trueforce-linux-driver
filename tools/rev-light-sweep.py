#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Try every way we know of lighting a Logitech wheel's rev strip, numbered.

Exists because "which command lights this wheel?" cannot be answered from a
feature list, a model name or a report descriptor. It has to be sent, and
somebody has to look at the rim. Every previous round of that took a
week: one guess, one reply, one more guess.

So this sends all of them, numbers each attempt, and asks for a single
number back. Nothing needs to be installed and nothing is built: it writes
HID reports to the wheel's own device nodes and reads the replies.

    sudo tools/rev-light-sweep.py

LEDs only. No force feedback is generated and the wheel will not move.

Why the attempts differ per wheel: the interface layout is not the same
across editions. A PlayStation G923 exposes a Joystick interface, a 0xFF00
HID++ interface and a 0xFFFD TrueForce one. The Xbox edition exposes only
Joystick and 0xFFFD, and carries HID++ on the Joystick interface using
vendor page 0xFF43 with report ids 0x11 (20 bytes) and 0x12 (64 bytes), and
no 0x10 short report at all. Tooling that assumes the PlayStation layout
finds no HID++ interface on an Xbox wheel and silently tests nothing, which
is how a command that was never sent got recorded as one the wheel ignores.
"""
import glob
import os
import select
import sys
import time

HOLD = 4.0                      # seconds a test stays lit
GAP = 1.5                       # seconds between tests
SWID = 0x0c                     # software id; a tag we choose, any value works
LEVEL = 10                      # full strip


def wheel_hid_dirs():
    """Every HID device directory belonging to a Logitech wheel."""
    pids = ("C266", "C267", "C26E", "C26D", "C276", "C272", "C268")
    out = []
    for d in sorted(glob.glob("/sys/bus/hid/devices/*")):
        name = os.path.basename(d).upper()
        if "046D" in name and any(f":{p}." in name for p in pids):
            out.append(d)
    return out


def node_of(hid_dir):
    """The /dev/hidrawN for a HID device directory."""
    try:
        return "/dev/" + os.listdir(os.path.join(hid_dir, "hidraw"))[0]
    except (OSError, IndexError):
        return None


def descriptor(hid_dir):
    try:
        with open(os.path.join(hid_dir, "report_descriptor"), "rb") as f:
            return f.read()
    except OSError:
        return b""


def report_ids(desc):
    """Report ids the descriptor declares, via the `85 <id>` item."""
    return {desc[i + 1] for i in range(len(desc) - 1) if desc[i] == 0x85}


def kind(desc):
    if desc[:4] == bytes([0x05, 0x01, 0x09, 0x04]):
        return "Joystick"
    if desc[:3] == bytes([0x06, 0x00, 0xFF]):
        return "vendor 0xFF00"
    if desc[:3] == bytes([0x06, 0xFD, 0xFF]):
        return "vendor 0xFFFD"
    if desc[:3] == bytes([0x06, 0x43, 0xFF]):
        return "vendor 0xFF43"
    return "unknown"


def write(fd, payload):
    """Write one report; True if the wheel's endpoint accepted it."""
    try:
        os.write(fd, bytes(payload))
        return True
    except OSError:
        return False


def ask(fd, request, timeout=0.4):
    """Send a HID++ request and return the reply, or None."""
    if not write(fd, request):
        return None
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        r, _, _ = select.select([fd], [], [], max(0.0, deadline - time.monotonic()))
        if not r:
            return None
        try:
            resp = os.read(fd, 64)
        except OSError:
            return None
        # Ignore input reports that are not our answer (the wheel streams
        # its axes continuously on the Joystick interface).
        if resp and resp[0] in (0x10, 0x11, 0x12) and len(resp) >= 5:
            return resp
    return None


def pad(payload, size):
    return list(payload) + [0] * (size - len(payload))


def hidpp_request(rid, size, feature_index, function, params):
    """One HID++ request, as the given report id and size."""
    return pad([rid, 0xFF, feature_index, (function << 4) | SWID] + list(params), size)


def find_feature(fd, rid, size, feature_id):
    """Root getFeature: the index this wheel gives a feature page."""
    resp = ask(fd, hidpp_request(rid, size, 0x00, 0x00,
                                 [feature_id >> 8, feature_id & 0xFF]))
    if resp and len(resp) > 4 and resp[4] not in (0x00, 0xFF):
        return resp[4]
    return None


class Sweep:
    def __init__(self):
        self.n = 0
        self.reached = []

    def run(self, label, node, fn):
        """One numbered attempt: light it, hold, turn it off."""
        self.n += 1
        print(f"TEST {self.n:2d}  {node}  {label}", end=" ... ", flush=True)
        try:
            fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
        except OSError as e:
            print(f"cannot open ({e.strerror})")
            return
        try:
            on, off, note = fn(fd)
            if on:
                self.reached.append(self.n)
                print(note or "sent")
                time.sleep(HOLD)
                off(fd)
            else:
                print(note or "refused")
        finally:
            os.close(fd)
        time.sleep(GAP)


def classic(level_on):
    """The lg4ff command: a plain output report, not HID++."""
    def go(fd):
        ok = write(fd, [0xF8, 0x12, 0x1F if level_on else 0x00, 0, 0, 0, 0])
        return ok, lambda f: write(f, [0xF8, 0x12, 0x00, 0, 0, 0, 0]), None
    return go


def level_dialect(rid, size, arm, index_hint=None, brightness_first=False):
    """The 0x807A level protocol, in one of its shapes.

    `rid`/`size` pick the report this wheel actually declares: a PlayStation
    G923 takes 0x10 short requests, the Xbox edition declares only 0x11 and
    0x12, so the same sequence has to go out as long reports there.
    """
    def go(fd):
        idx = find_feature(fd, rid, size, 0x807A) or index_hint
        if idx is None:
            return False, None, "this interface does not answer HID++ for 0x807A"
        if brightness_first:
            bidx = find_feature(fd, rid, size, 0x8040)
            if bidx:
                write(fd, hidpp_request(rid, size, bidx, 1, [0xFF]))
                time.sleep(0.02)
        if arm:
            for fn in (0, 1, 2, 0):
                write(fd, hidpp_request(rid, size, idx, fn, [0x00]))
                time.sleep(0.004)
        write(fd, hidpp_request(rid, size, idx, 2, [0x00]))
        # The level rides in the 6th parameter byte.
        ok = write(fd, hidpp_request(rid, size, idx, 6,
                                     [0x00, 0x01, 0x00, 0x0A, 0x00, LEVEL]))

        def off(f):
            write(f, hidpp_request(rid, size, idx, 2, [0x00]))
            write(f, hidpp_request(rid, size, idx, 6,
                                   [0x00, 0x01, 0x00, 0x0A, 0x00, 0x00]))
        return ok, off, f"sent (feature index 0x{idx:02X})"
    return go


def main():
    dirs = wheel_hid_dirs()
    if not dirs:
        sys.exit("No Logitech wheel found on the HID bus.")

    print("Each test lights the rev strip for 4 seconds, then turns it off.")
    print("LEDs only: nothing here produces force feedback and the wheel")
    print("will not move.\n")
    print("Watch the rim. Note WHICH TEST NUMBER lights it.\n")

    sweep = Sweep()
    for hid_dir in dirs:
        node = node_of(hid_dir)
        if not node:
            continue
        desc = descriptor(hid_dir)
        ids = report_ids(desc)
        what = kind(desc)
        print(f"-- {node}  [{what}]  report ids: "
              f"{', '.join(f'0x{i:02X}' for i in sorted(ids)) or 'none'}")

        sweep.run(f"[{what}] classic lg4ff", node, classic(True))

        # The level dialect, in whichever report sizes this interface
        # declares. Trying both where both exist costs one test and removes
        # the assumption that a wheel uses the short form.
        if 0x10 in ids:
            sweep.run(f"[{what}] 0x807A level, short requests", node,
                      level_dialect(0x10, 7, arm=True))
        if 0x11 in ids:
            sweep.run(f"[{what}] 0x807A level, long requests", node,
                      level_dialect(0x11, 20, arm=True))
            sweep.run(f"[{what}] 0x807A level, long, no arm burst", node,
                      level_dialect(0x11, 20, arm=False))
            sweep.run(f"[{what}] 0x807A level, long, brightness first", node,
                      level_dialect(0x11, 20, arm=True, brightness_first=True))
        if 0x12 in ids:
            sweep.run(f"[{what}] 0x807A level, very-long requests", node,
                      level_dialect(0x12, 64, arm=True))
        print()

    if sweep.reached:
        print("Reached the wheel: TEST " +
              ", ".join(str(i) for i in sweep.reached) + ".")
        print("That only means the bytes were accepted, NOT that the wheel obeyed.")
    else:
        print("Nothing could be sent. Re-run with sudo.")
    print()
    print("WHICH TEST NUMBER LIT THE STRIP? Reply with the number, or 'none'.")


if __name__ == "__main__":
    main()
