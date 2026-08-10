#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Replay the exact rev-light commands Windows sends a G923 Xbox edition.

    sudo tools/g923-xbox-led-replay.py

LEDs only. No force feedback is generated and the wheel will not move.

Every previous round of issue #27 sent a command we believed was right and
asked whether the strip lit. This sends the bytes a Windows machine was
recorded sending to this exact model while its lights worked, so a negative
result is finally evidence about our understanding rather than about our
guesswork.

Taken from simonr2k4's Automobilista 2 capture, 2026-08-10. Three things in
it differ from what this project has been sending:

1. **Every command is a LONG (0x11) report.** We sent the `fn2` that applies
   a level as a SHORT (0x10) one. That wheel's HID++ interface declares
   `0x11` and `0x12` and no `0x10` at all, so those writes were a report id
   it never agreed to receive. hidraw accepts them and returns success
   regardless, which is why the probe kept printing "sent".

2. **A one-time setup at feature index 0x0b**, before any rev-light command:
   `fn1`, `fn8`, `fn5`, `fn6` with `03 6f`. We have never sent anything to
   that feature. What it is, is unknown; it is replayed verbatim rather than
   understood, which is the point of a replay.

3. **The level command repeats at about 11 Hz** for as long as the lights
   are meant to be on, rather than being sent once and left.

TEST 1 sends only (1) and (3), TEST 2 adds (2). Which of them lights the
strip says which of these mattered, and a single run answers it.
"""
import glob
import os
import select
import sys
import time

HOLD = 5.0      # seconds a test holds the strip lit
REFRESH = 0.09  # ~11 Hz, the rate the capture repeats the pair at
LEVEL = 5       # full strip on a five-LED wheel

# Feature indices as this wheel reports them. Both are read from the
# capture, and both are properties of its firmware rather than constants of
# the protocol, so this script is for a c26e and nothing else.
IDX_RPM = 0x12      # 0x807A, confirmed by the driver's own dmesg line
IDX_SETUP = 0x0b    # unidentified; replayed, not understood


def long_report(idx, function, params=()):
    """One 20-byte HID++ LONG request, framed as the capture frames them.

    Software id 0x0a in the low nibble, which is what G HUB used here. This
    project's rev-light code uses 0x0d, and an RS50 obeys that, so the id is
    not believed to matter; it is matched anyway so this stays a replay
    rather than an adaptation.
    """
    r = [0x11, 0xFF, idx, (function << 4) | 0x0A]
    r += list(params)
    return bytes(r + [0] * (20 - len(r)))


# The setup block, in capture order, verbatim.
SETUP = [
    long_report(IDX_SETUP, 1),
    long_report(IDX_SETUP, 8, [0xFF, 0xFF]),
    long_report(IDX_SETUP, 8, [0x00, 0x00]),
    long_report(IDX_SETUP, 1),
    long_report(IDX_SETUP, 8, [0xFF, 0xFF]),
    long_report(IDX_RPM, 0),
    long_report(IDX_SETUP, 5),
    long_report(IDX_SETUP, 6, [0x03, 0x6F]),
    long_report(IDX_SETUP, 6, [0x03, 0x6F]),
    long_report(IDX_SETUP, 5),
    long_report(IDX_SETUP, 5),
    long_report(IDX_SETUP, 6, [0x03, 0x6F]),
    long_report(IDX_RPM, 1),
]


def level_pair(level):
    """The apply/level pair the capture repeats: fn2, then fn6 carrying it."""
    return [
        long_report(IDX_RPM, 2),
        long_report(IDX_RPM, 6, [0x00, 0x01, 0x00, 0x05, 0x00, level]),
    ]


# The Xbox G923, and deliberately only it. Feature INDICES are per firmware:
# 0x0b is whatever that wheel's feature table puts there, and on another
# model it is some unrelated feature being sent `fn8 ff ff`. A replay is
# only meaningful on the hardware it was captured from, and on anything else
# it is an unknown write to an unknown feature.
PID_G923_XBOX = "C26E"


def candidate_nodes():
    """Every hidraw node of an attached c26e that declares report id 0x11.

    Not filtered to one interface on purpose: on this wheel HID++ rides the
    Joystick interface rather than a separate vendor one, which is the
    layout assumption that made earlier tooling test nothing at all.
    """
    out = []
    for d in sorted(glob.glob("/sys/bus/hid/devices/*")):
        name = os.path.basename(d).upper()
        if "046D" not in name or PID_G923_XBOX not in name:
            continue
        try:
            with open(os.path.join(d, "report_descriptor"), "rb") as f:
                desc = f.read()
            node = "/dev/" + os.listdir(os.path.join(d, "hidraw"))[0]
        except (OSError, IndexError):
            continue
        ids = {desc[i + 1] for i in range(len(desc) - 1) if desc[i] == 0x85}
        if 0x11 in ids:
            out.append((node, name.split(".")[0][-4:], sorted(ids)))
    return out


def drain(fd):
    """Read anything the wheel has replied, so its queue does not back up."""
    while True:
        r, _, _ = select.select([fd], [], [], 0)
        if not r:
            return
        try:
            os.read(fd, 64)
        except OSError:
            return


def hold(fd, reports, seconds, level_on):
    """Send `reports`, then repeat the level pair for `seconds`."""
    for r in reports:
        os.write(fd, r)
        drain(fd)
        time.sleep(0.01)
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        for r in level_pair(level_on):
            os.write(fd, r)
            drain(fd)
        time.sleep(REFRESH)
    # Leave it dark rather than stuck lit from the previous test.
    for r in level_pair(0):
        os.write(fd, r)
        drain(fd)


def main():
    nodes = candidate_nodes()
    if not nodes:
        sys.exit(
            "No G923 Xbox edition (046d:c26e) found on the HID bus.\n"
            "This replay is for that model only: the feature indices in it\n"
            "come from its firmware and mean something else on any other\n"
            "wheel. Use `logi-wheel --led-probe` for the general case."
        )

    print(__doc__.split("\n\n")[0])
    print("\nLEDs only: nothing here produces force feedback and the wheel")
    print("will not move. Each test holds the strip for %.0f seconds.\n" % HOLD)

    n = 0
    for node, pid, ids in nodes:
        print("-- %s  [pid %s]  report ids: %s"
              % (node, pid, ", ".join("0x%02X" % i for i in ids)))
        for label, reports in (
            ("long reports, repeated, NO setup block", [long_report(IDX_RPM, 1)]),
            ("long reports, repeated, WITH the setup block", SETUP),
        ):
            n += 1
            print("TEST %d  %s ... " % (n, label), end="", flush=True)
            try:
                fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
            except OSError as e:
                print("cannot open (%s)" % e.strerror)
                continue
            try:
                hold(fd, reports, HOLD, LEVEL)
                print("sent")
            except OSError as e:
                print("refused (%s)" % e.strerror)
            finally:
                os.close(fd)
            time.sleep(1.5)
        print()

    print("WHICH TEST NUMBER LIT THE STRIP? Reply with the number, or 'none',")
    print("and paste this whole output with it.")


if __name__ == "__main__":
    main()
