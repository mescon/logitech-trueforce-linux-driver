#!/usr/bin/env bash
#
# Linux USB capture for the Logitech RS50 / G PRO wheel during a game
# session. Writes a single .zip into dev/captures/ (gitignored scratch
# dir) that an analyst can inspect end-to-end without needing access
# to the user's machine.
#
# Bundle contents:
#   <stem>.pcap       - usbmon traffic for the wheel's USB bus only
#   <stem>_pre.txt    - wheel sysfs state + recent dmesg, before launch
#   <stem>_post.txt   - same, immediately after the test
#   <stem>_meta.txt   - kernel, driver git hash, distro, lsusb, dmidecode
#
# Why this exists: the SDK / Wine path issues a mix of HID++ commands,
# raw HID writes, and USB control transfers. Filtering only on usbhid.data
# in tshark misses control-stage traffic; usbmon-of-the-bus catches
# everything. The captures we already had ("trueforce_ace.pcapng" etc.)
# only show HID++ writes and miss the control transfers that may explain
# the wheel reverting to factory-default range on game launch.
#
# Usage:
#   sudo ./dev/tools/linux_game_capture.sh [label] [duration_seconds]
#
#   label    : short tag, e.g. "ace_launch" or "txr_no_ffb" (default: session)
#   duration : how long to capture (default: 60). 0 means wait for Ctrl-C.
#
# Typical session for the "wheel resets to 90 degrees on game launch"
# investigation:
#   1. Set wheel to a non-default range via the OLED (e.g. 1080).
#   2. Verify: cat /sys/class/hidraw/*/device/wheel_range
#   3. sudo ./dev/tools/linux_game_capture.sh ace_range_reset 60
#   4. Wait for the START marker, then immediately launch ACE in Steam.
#   5. Drive for ~10 seconds, exit ACE.
#   6. Capture stops; bundle is printed at the end. Attach to the issue.

set -euo pipefail

LABEL="${1:-session}"
DURATION="${2:-60}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# dev/captures is the scratch convention this repo uses for all
# Wireshark / USBPcap pcaps (see dev/tools/windows_*.bat). The whole
# dev/ tree is gitignored so captures never accidentally land in a
# commit.
CAPTURE_DIR="$REPO_ROOT/dev/captures"
mkdir -p "$CAPTURE_DIR"

if [ "$EUID" -ne 0 ]; then
	echo "error: must run as root (sudo $0)" >&2
	exit 1
fi

if ! command -v tshark >/dev/null 2>&1; then
	echo "error: tshark not installed (apt install tshark / pacman -S wireshark-cli)" >&2
	exit 1
fi

# Discover the wheel: walk lsusb for any of the IDs we ship support
# for. If the user has more than one matching device connected we bail
# rather than guess.
WHEEL_LINE=$(lsusb | grep -iE '046d:(c272|c268|c276|c269)' || true)
if [ -z "$WHEEL_LINE" ]; then
	echo "error: no Logitech RS50 / G PRO wheel found via lsusb" >&2
	exit 1
fi
if [ "$(echo "$WHEEL_LINE" | wc -l)" -gt 1 ]; then
	echo "error: more than one wheel detected, please disconnect all but one:" >&2
	echo "$WHEEL_LINE" >&2
	exit 1
fi

# lsusb prints "Bus 001 Device 020: ID 046d:c272 ..." - strip the
# trailing colon and any leading zeros separately so we don't end up
# with `usb.device == 20:` in the capture filter (which fails silently
# and forces the whole-bus fallback).
WHEEL_BUS=$(echo "$WHEEL_LINE" | awk '{ print $2 }' | sed 's/^0*//;s/^$/0/')
WHEEL_DEV=$(echo "$WHEEL_LINE" | awk '{ print $4 }' | tr -d ':' | sed 's/^0*//;s/^$/0/')
WHEEL_PID=$(echo "$WHEEL_LINE" | awk '{ print $6 }')

DATE=$(date +%Y-%m-%d)
STEM="${DATE}_${LABEL}_linux"
TMPDIR=$(mktemp -d -t rs50-cap.XXXXXX)
trap 'rm -rf "$TMPDIR"' EXIT

PCAP="$TMPDIR/${STEM}.pcap"
PRE="$TMPDIR/${STEM}_pre.txt"
POST="$TMPDIR/${STEM}_post.txt"
META="$TMPDIR/${STEM}_meta.txt"

# Make sure the usbmon kernel module is available; tshark on bus N
# requires either CONFIG_USB_MON=y or the usbmon module loaded.
if ! grep -q '^usbmon ' /proc/modules 2>/dev/null && [ ! -d /sys/kernel/debug/usb/usbmon ]; then
	if ! modprobe usbmon 2>/dev/null; then
		echo "error: failed to load usbmon module" >&2
		exit 1
	fi
fi

# Capture sysfs / dmesg snapshot the analyst can correlate with the
# capture timeline. dmesg gets cleared between pre/post so the post
# file shows only what happened during the session.
snapshot() {
	local out="$1"
	{
		echo "=== date ==="
		date -u +"%Y-%m-%dT%H:%M:%SZ"
		echo
		echo "=== wheel sysfs (relevant attributes) ==="
		for attr in wheel_profile wheel_mode wheel_range wheel_strength \
			    wheel_damping wheel_ffb_filter wheel_ffb_constant_sign \
			    wheel_trueforce wheel_led_brightness; do
			for f in /sys/class/hidraw/*/device/"$attr"; do
				[ -e "$f" ] || continue
				printf '%-30s = %s\n' "$attr" "$(cat "$f" 2>/dev/null || echo '<error>')"
				break
			done
		done
		echo
		echo "=== lsusb -t ==="
		lsusb -t || true
		echo
		echo "=== current modules ==="
		lsmod | grep -E '^(hid_logitech_hidpp|usbhid|usbmon)' || true
		echo
		echo "=== modinfo version ==="
		modinfo -F version hid-logitech-hidpp 2>/dev/null || echo "(unknown)"
		echo
		echo "=== recent wheel dmesg ==="
		dmesg --since '60 seconds ago' 2>/dev/null | grep -E 'RS50|G PRO|hidpp|046d' | tail -30 || true
	} > "$out"
}

# Static metadata we never need to re-collect.
{
	echo "=== uname ==="
	uname -a
	echo
	echo "=== distro ==="
	cat /etc/os-release 2>/dev/null || true
	echo
	echo "=== driver git hash ==="
	modinfo -F version hid-logitech-hidpp 2>/dev/null || echo "(unknown)"
	echo
	echo "=== wheel device ==="
	echo "lsusb line: $WHEEL_LINE"
	echo "bus=$WHEEL_BUS dev=$WHEEL_DEV pid=$WHEEL_PID"
} > "$META"

snapshot "$PRE"

cat <<EOF

----------------------------------------------------------------
 Wheel detected: $WHEEL_LINE
 Bus $WHEEL_BUS device $WHEEL_DEV
 Capture target : $PCAP
 Duration       : ${DURATION}s ($([ "$DURATION" = 0 ] && echo "wait for Ctrl-C" || echo "auto-stops"))
 Bundle output  : $CAPTURE_DIR/${STEM}.zip
----------------------------------------------------------------

EOF

read -r -p "Press Enter when you are READY to start capturing... " _

# tshark on usbmon$BUS gets every packet on that USB bus. We
# post-filter on the wheel's USB IDs in the bundle phase if we want a
# smaller artefact, but we record everything first so a missed event on
# a control endpoint does not invalidate the session.
if [ "$DURATION" = 0 ]; then
	DUR_ARG=""
else
	DUR_ARG="-a duration:$DURATION"
fi

echo
echo "====== START START START - launch the game NOW ======"
echo

# Capture the whole USB bus the wheel sits on. We do not pass a BPF
# capture filter (`-f`) because usbmon's link-layer headers do not
# expose Wireshark's `usb.device` field to BPF - that's a dissector-
# decoded field, only usable as a display filter. Filtering by device
# in BPF on usbmon would require raw byte offsets into the usbmon
# header, which is fragile across kernel versions. Whole-bus capture
# at 60 s is small (low-100s of kB even with FFB streaming) so the
# extra packets are not a problem.
set +e
tshark -i "usbmon${WHEEL_BUS}" \
	-w "$PCAP" \
	$DUR_ARG \
	-q 2>&1 | tail -2
set -e

echo
echo "!!!!!!! ENDING ENDING ENDING - capture stopped !!!!!!!!"
echo

snapshot "$POST"

# Bundle. zip is in basically every distro; tar.gz is the fallback.
ZIPOUT="$CAPTURE_DIR/${STEM}.zip"
if command -v zip >/dev/null 2>&1; then
	(cd "$TMPDIR" && zip -q "$ZIPOUT" "${STEM}.pcap" "${STEM}_pre.txt" \
		"${STEM}_post.txt" "${STEM}_meta.txt")
else
	ZIPOUT="$CAPTURE_DIR/${STEM}.tar.gz"
	(cd "$TMPDIR" && tar czf "$ZIPOUT" "${STEM}.pcap" "${STEM}_pre.txt" \
		"${STEM}_post.txt" "${STEM}_meta.txt")
fi

# Drop root ownership so the user can attach the file without further
# sudo gymnastics.
if [ -n "${SUDO_USER:-}" ]; then
	chown "$SUDO_USER:" "$ZIPOUT" || true
fi

PACKETS=$(tshark -r "$PCAP" 2>/dev/null | wc -l)
SIZE=$(du -h "$ZIPOUT" | cut -f1)

cat <<EOF

----------------------------------------------------------------
 Capture finished.
 Packets recorded : $PACKETS
 Bundle           : $ZIPOUT ($SIZE)
 Attach the bundle to your GitHub issue.
----------------------------------------------------------------
EOF
