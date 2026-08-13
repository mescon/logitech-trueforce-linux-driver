#!/usr/bin/env python3
"""Play the SDK's role for merge validation: init + START + a zero-force
type-0x01 stream on the wheel's interface-2 hidraw node, then STOP.

With the kernel texture merge enabled and an RPM value fed to
wheel_texture_rpm, the driver splices texture samples into these passing
packets; a usbmon capture then shows byte10=4 with the cur bytes untouched.
cur stays 0x8000 (zero force) in every packet, so the wheel produces no
steering force, only the 1-2 percent fullscale texture buzz.

Usage: synthetic_stream.py [seconds]   (default 10)
"""
import glob, os, subprocess, sys, time


def find_iface2():
    for h in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.join(h, "device")
        try:
            hid_id = ""
            for line in open(os.path.join(dev, "uevent")):
                if line.startswith("HID_ID="):
                    hid_id = line.strip().split("=", 1)[1]
            up = hid_id.upper()
            if "046D" not in up or not any(p in up for p in ("C276", "C272", "C268")):
                continue
            iface_dir = os.path.realpath(os.path.join(dev, ".."))
            bnum = open(os.path.join(iface_dir, "bInterfaceNumber")).read().strip()
            if int(bnum, 16) == 2:
                return "/dev/" + os.path.basename(h)
        except (OSError, ValueError):
            continue
    sys.exit("no interface-2 hidraw node found")


def pkt(cmd, seq=0, cur=0x8000):
    p = bytearray(64)
    p[0] = 0x01
    p[4] = cmd
    p[5] = seq & 0xFF
    if cmd == 0x01:  # stream packet: cur duplicated LE at 6-9, byte10=0, flag 0x0d
        p[6:8] = p[8:10] = cur.to_bytes(2, "little")
        p[10] = 0
        p[11] = 0x0D
    return bytes(p)


def main():
    secs = float(sys.argv[1]) if len(sys.argv) > 1 else 10.0
    here = os.path.dirname(os.path.abspath(__file__))
    init = os.path.join(here, "../../tools/logi-tf-init.py")
    # G HUB sends the 68-packet init twice; a single pass is unreliable on a
    # cold engine (docs/TRUEFORCE_PROTOCOL.md).
    for _ in range(2):
        subprocess.run([sys.executable, init], check=True)
    dev = find_iface2()
    fd = os.open(dev, os.O_RDWR)
    os.write(fd, pkt(0x03))  # START
    t_end = time.time() + secs
    seq = 0
    while time.time() < t_end:
        os.write(fd, pkt(0x01, seq))
        seq += 1
        time.sleep(0.0005)  # ~2 kHz
    os.write(fd, pkt(0x04))  # STOP
    os.close(fd)
    print(f"streamed {seq} zero-force packets on {dev}")


if __name__ == "__main__":
    main()
