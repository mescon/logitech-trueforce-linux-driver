#!/usr/bin/env python3
"""Send the TrueForce stream-engine init to the wheel and leave it ready.

G HUB performs this 68-packet init on interface 2 at boot and leaves the
wheel's TrueForce engine initialised; the game's SDK then streams into it.
Under Proton nothing performs it, so the SDK's stream/haptic thread has no
initialised engine to attach to. logi-tf-sim --sweep does the init too but
tears it down on exit; this sends the init and stops, leaving the engine up.

Finds interface 2 by bInterfaceNumber, so it survives hidraw renumbering.
"""
import glob, os, re, sys

def find_tf_hidraw():
    for h in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.join(h, "device")
        uevent = os.path.join(dev, "uevent")
        try:
            hid_id = ""
            for line in open(uevent):
                if line.startswith("HID_ID="):
                    hid_id = line.strip().split("=", 1)[1]
            # HID_ID is 0003:0000046D:0000C276 (zero-padded), so match VID+PID loosely.
            up = hid_id.upper()
            if "046D" not in up or not any(pid in up for pid in ("C276", "C272", "C268")):
                continue
            # the USB interface dir is device/../ ; read bInterfaceNumber
            iface_dir = os.path.realpath(os.path.join(dev, ".."))
            bnum = open(os.path.join(iface_dir, "bInterfaceNumber")).read().strip()
            if int(bnum, 16) == 2:
                return "/dev/" + os.path.basename(h)
        except (OSError, ValueError):
            continue
    return None

def load_init_packets(header):
    pkts = []
    for line in open(header):
        m = re.search(r"\{\s*((?:0x[0-9a-fA-F]{2},\s*){63}0x[0-9a-fA-F]{2})\s*\}", line)
        if m:
            pkts.append(bytes(int(x, 16) for x in m.group(1).replace(" ", "").split(",")))
    return pkts

def main():
    header = os.path.join(os.path.dirname(__file__),
                          "../userspace/libtrueforce/src/tf_init_data.h")
    pkts = load_init_packets(header)
    if len(pkts) != 68:
        print(f"expected 68 init packets, parsed {len(pkts)}", file=sys.stderr)
        return 1
    node = find_tf_hidraw()
    if not node:
        print("could not find interface 2 (TrueForce) hidraw node", file=sys.stderr)
        return 1
    try:
        fd = os.open(node, os.O_WRONLY)
    except OSError as e:
        print(f"cannot open {node}: {e}", file=sys.stderr)
        return 1
    sent = 0
    for p in pkts:
        try:
            os.write(fd, p)
            sent += 1
        except OSError as e:
            print(f"write {sent} failed on {node}: {e}", file=sys.stderr)
            break
    os.close(fd)
    # NOTE: deliberately no stop/clear/teardown - leave the engine initialised.
    print(f"TrueForce init: sent {sent}/68 packets to {node}, engine left ready")
    return 0 if sent == 68 else 1

if __name__ == "__main__":
    sys.exit(main())
