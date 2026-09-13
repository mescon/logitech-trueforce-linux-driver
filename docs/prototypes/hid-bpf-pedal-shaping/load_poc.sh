#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only
# Phase-0 convenience wrapper: resolve the RS50 interface-0 hid device id and
# attach the hardcoded-deadzone PoC to it. Run as root (BPF struct_ops attach
# needs privilege). Holds the attachment until Ctrl-C.
#
#   sudo ./load_poc.sh            # auto-resolve the wheel, build if needed
#   sudo ./load_poc.sh <hid_id>   # force a specific hid device id
set -euo pipefail
cd "$(dirname "$0")"

[ -x ./load_poc ] && [ -f pedals_poc.bpf.o ] || make >/dev/null

hid_id=${1:-}
if [ -z "$hid_id" ]; then
	# The joystick (interface 0) is the hid device that owns an eventN node.
	# hdev->id is the trailing hex field of the sysfs device name.
	for p in /sys/bus/hid/devices/*C276* /sys/bus/hid/devices/*C272* \
		 /sys/bus/hid/devices/*C268*; do
		[ -e "$p" ] || continue
		if find "$p" -name 'event*' -print -quit 2>/dev/null | grep -q .; then
			name=$(basename "$p")
			hid_id=$((16#${name##*.}))
			echo "wheel iface-0 hid dev: $name -> hid_id $hid_id"
			break
		fi
	done
fi
[ -n "$hid_id" ] || { echo "could not resolve a wheel hid_id; pass one explicitly" >&2; exit 1; }

exec ./load_poc "$hid_id"
