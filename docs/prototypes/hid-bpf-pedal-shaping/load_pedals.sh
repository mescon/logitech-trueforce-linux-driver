#!/bin/bash
# SPDX-License-Identifier: GPL-2.0-only
# Phase-1 loader: resolve the wheel's interface-0 hid_id and attach the
# map-driven pedal shaper, pinning shaping_map at
# /sys/fs/bpf/hid-logitech-dd/shaping_map (root:input 0660). Run as root; holds
# the attachment until Ctrl-C. logi-dd writes the pinned map to shape live.
#
#   sudo ./load_pedals.sh            # auto-resolve the wheel, build if needed
#   sudo ./load_pedals.sh <hid_id>   # force a specific hid device id
set -euo pipefail
cd "$(dirname "$0")"

[ -x ./load_pedals ] && [ -f pedals.bpf.o ] || make >/dev/null

hid_id=${1:-}
if [ -z "$hid_id" ]; then
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

exec ./load_pedals "$hid_id"
