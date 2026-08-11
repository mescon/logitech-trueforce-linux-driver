#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Replay the exact rev-light commands Windows sends a G923 Xbox edition.

    sudo tools/g923-xbox-led-replay.py

This sends rev-light commands only, and does not command any force.

**An earlier version of this script also replayed a block of commands at
feature index `0x0b`, and that block engaged self-centring on the wheel,
which stayed on until it was unplugged.** It was replayed because it appeared
in the capture before the rev-light commands, and was described here as
"replayed verbatim rather than understood". That was true, and it was not a
good enough reason to send it: the wheel's replies (`fn8` with `ff ff`, `fn5`
and `fn6` with `03 6f`) read as a force feature, and it has now been shown to
make no difference to the lights. It is gone. The bytes are kept at the
bottom of this file for reference, not sent.

The lesson, recorded because it is more useful than the bytes: replaying an
unidentified command is not a safe operation, and calling a script "LEDs
only" while it does so is a claim about something not actually known.

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

2. **The level command repeats at about 11 Hz** for as long as the lights
   are meant to be on, rather than being sent once and left.

## What running this established (2026-08-11)

Both differences turned out not to be the answer. With the bytes matching
Windows exactly, the wheel still refuses the level:

    ok:    feature 0x12 fn0 -> 03 05 02      (identical to Windows)
    ok:    feature 0x12 fn1 -> 00 02         (identical to Windows)
    ok:    feature 0x12 fn2 -> 00            (Windows gets 02)
    ERROR: feature 0x12 fn6: LogitechInternal (5)

Windows gets `fn6` accepted 437 times in the same capture, and never sees an
error. So the wheel is not rejecting the command's contents: it is in a
different state, and `fn2` answering `00` where Windows sees `02` is where
that difference becomes visible.

Whatever establishes that state happens **before** the capture begins, which
started with G HUB already running. A capture from a cold start, wheel
unplugged and G HUB not yet launched, is what would contain it.
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
# 0x0b is NOT sent. See REFERENCE_ONLY_0x0B at the bottom.


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


# The rev-light preamble: the two 0x807A queries Windows makes, then the
# call that actually switches the display on.
#
# fn3 with effect 2 is the answer to this whole issue. fn2 reports a display
# state; a wheel in state 0 refuses every level with LogitechInternal(5), and
# fn3(2) moves it to state 2, after which the identical level is accepted.
# Confirmed on a PlayStation G923 on 2026-08-11, machine-readable and then
# visually: 0x807A had never lit that strip before, and with fn3(2) first it
# does. The same state 2 is what this wheel reports in the Windows capture.
#
# The feature-0x0b block that sat among these in the capture is NOT here; see
# the note at the top of this file, and REFERENCE_ONLY_0x0B at the bottom.
PREAMBLE = [
    long_report(IDX_RPM, 0),
    long_report(IDX_RPM, 1),
    long_report(IDX_RPM, 3, [0x02]),
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


# HID++ 2.0 error codes, as they come back in an error reply.
HIDPP_ERRORS = {
    0: "NoError",
    1: "Unknown",
    2: "InvalidArgument",
    3: "OutOfRange",
    4: "HWError",
    5: "LogitechInternal",
    6: "INVALID_FEATURE_INDEX",
    7: "INVALID_FUNCTION_ID",
    8: "Busy",
    9: "Unsupported",
}


def describe_reply(b):
    """One line for a reply, saying whether it is an error and to what.

    The wheel answers every one of these commands, and until now this script
    read those answers and threw them away. That is the same mistake the
    rest of this investigation kept making: a test that reports only what it
    SENT cannot tell "the wheel refused this" from "the wheel did it and the
    lights are off for another reason". An error reply names the feature and
    function that failed and why, which is the difference between guessing
    and knowing.
    """
    if len(b) < 5:
        return "short reply: %s" % b.hex(" ")
    # An error reply carries 0xFF where a normal one echoes the feature
    # index, then the feature and function it is complaining about.
    if b[2] == 0xFF:
        code = b[5] if len(b) > 5 else 0
        return "ERROR on feature 0x%02X fn%d: %s (%d)" % (
            b[3], b[4] >> 4, HIDPP_ERRORS.get(code, "code %d" % code), code)
    return "ok: feature 0x%02X fn%d -> %s" % (b[2], b[3] >> 4, b[4:10].hex(" "))


def drain(fd, seen):
    """Collect the wheel's replies into `seen` instead of discarding them."""
    while True:
        r, _, _ = select.select([fd], [], [], 0.002)
        if not r:
            return
        try:
            b = os.read(fd, 64)
        except OSError:
            return
        # Ignore the axis reports the joystick interface streams constantly;
        # only HID++ replies start with these report ids.
        if b and b[0] in (0x10, 0x11, 0x12):
            seen.append(describe_reply(b))


def hold(fd, reports, seconds, level_on):
    """Send `reports`, then repeat the level pair for `seconds`.

    Returns the distinct replies the wheel gave, in the order first seen.
    Distinct rather than all of them: the level pair goes out about eleven
    times a second for five seconds, so the raw list is a hundred copies of
    the same two lines.
    """
    seen = []
    for r in reports:
        os.write(fd, r)
        drain(fd, seen)
        time.sleep(0.01)
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        for r in level_pair(level_on):
            os.write(fd, r)
            drain(fd, seen)
        time.sleep(REFRESH)
    # Leave it dark rather than stuck lit from the previous test.
    for r in level_pair(0):
        os.write(fd, r)
        drain(fd, seen)

    distinct = []
    for line in seen:
        if line not in distinct:
            distinct.append(line)
    return distinct


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
            ("0x807A level, display switched on with fn3(2) first", PREAMBLE),
        ):
            n += 1
            print("TEST %d  %s ... " % (n, label), end="", flush=True)
            try:
                fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
            except OSError as e:
                print("cannot open (%s)" % e.strerror)
                continue
            try:
                replies = hold(fd, reports, HOLD, LEVEL)
                print("sent")
                if replies:
                    for line in replies:
                        print("          %s" % line)
                else:
                    # Nothing came back at all, which is itself a result: the
                    # wheel answers HID++ on this interface, so silence means
                    # the requests are not reaching it as HID++ requests.
                    print("          (no HID++ reply of any kind)")
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


# The feature-0x0b block from the capture, kept as evidence and deliberately
# never sent. Replaying it engaged self-centring on a real wheel, which
# persisted until the wheel was unplugged, so whatever this feature is, it
# governs force and not lights. Recorded here so the next person does not
# have to re-derive it from the capture, and does not have to find out what
# it does the way we did.
#
#   11 ff 0b 1a 00 00        fn1
#   11 ff 0b 8a ff ff        fn8, params ff ff
#   11 ff 0b 8a 00 00        fn8, params 00 00
#   11 ff 0b 5a 00 00        fn5   -> replies 03 6f
#   11 ff 0b 6a 03 6f        fn6, params 03 6f
#
# It is also now known to make no difference to the rev lights: the level
# command fails with the same error whether or not this block precedes it.
REFERENCE_ONLY_0x0B = None
