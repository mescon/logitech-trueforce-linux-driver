#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
#
# Diagnose and fix the common "wheel stuck on the wrong driver" problem.
#
# If the wheel enumerates before hid-logitech-dd is loaded, hid-generic
# claims it (a boot race). For the G923 PIDs (c266/c267/c26e) the in-tree
# "logitech" (lg4ff family) or "logitech-hidpp-device" driver - or a
# standalone fork registered under either name - can win the bind race
# instead, since these PIDs also match their MODALIAS. Either way the
# symptom is no wheel_* sysfs and no force feedback - the wheel works as
# a plain joystick (with, on hid-generic, the dmesg "Invalid code 768"
# phantom-button spam) but none of this driver's features come up.
#
# This script loads the module, reports which driver each wheel interface
# is on, and rebinds any interface not already on this driver (whatever
# it is currently on, or nothing at all) to this driver.
# Run as root (or via sudo).
#
# udev/72-logitech-g923-rebind.rules does the same reclaim automatically
# for c266/c267/c26e on every add/bind event; this script is the manual
# fallback (and covers the other PIDs, which have no such rule).

set -euo pipefail

DRIVER="logitech-dd"
MODULE="hid-logitech-dd"
# Supported wheels (USB product IDs, upper-case to match the HID device
# directory names under /sys/bus/hid/devices). C262 (G920) is deliberately
# absent: this driver does not bind it (see mainline/hid-logitech-hidpp.c's
# id_table comment), so rebinding it here would just fail.
PIDS="C266 C267 C26E C268 C272 C276"

if [ "$(id -u)" -ne 0 ]; then
	echo "This script binds/unbinds kernel drivers and must run as root." >&2
	echo "Try: sudo $0" >&2
	exit 1
fi

# Make sure our module is present. DKMS installs it under /updates/dkms,
# which shadows the in-kernel copy, so this loads the fork. Loading the
# fork already steals a supported wheel back from hid-generic, so give
# that a moment before we inspect / rescue bindings below.
modprobe "$MODULE" 2>/dev/null || true
sleep 1

if [ ! -d "/sys/bus/hid/drivers/$DRIVER" ]; then
	echo "error: driver '$DRIVER' is not loaded - is the module built and installed?" >&2
	echo "       run: sudo ./tools/dkms-update.sh" >&2
	exit 1
fi

is_supported() {
	local name="$1" pid
	for pid in $PIDS; do
		case "$name" in *":$pid."*) return 0 ;; esac
	done
	return 1
}

found=0 rescued=0 ok=0
for dev in /sys/bus/hid/devices/*; do
	[ -e "$dev" ] || continue
	name="$(basename "$dev")"
	is_supported "$name" || continue
	found=1

	# -L, not readlink -f: on an unbound device "driver" does not exist as
	# a symlink at all, but its parent dir does, and GNU readlink -f
	# canonicalizes as far as it can and still exits 0 - so testing its
	# output for emptiness does not reliably detect "unbound".
	cur=""
	[ -L "$dev/driver" ] && cur="$(basename "$(readlink -f "$dev/driver")")"

	if [ "$cur" = "$DRIVER" ]; then
		echo "ok:      $name already on $DRIVER"
		ok=$((ok + 1))
		continue
	fi

	if [ -n "$cur" ]; then
		# Bound to something else: hid-generic (the classic boot race),
		# or a competing driver that won the bind race for this PID -
		# the in-tree "logitech" (lg4ff family) or "logitech-hidpp-device",
		# or a standalone fork registered under either name. Reclaim it
		# regardless of which; these PIDs belong to this driver.
		echo "rescue:  $name is on $cur, rebinding to $DRIVER ..."
		echo "$name" > "/sys/bus/hid/drivers/$cur/unbind" 2>/dev/null || true
		if echo "$name" > "/sys/bus/hid/drivers/$DRIVER/bind" 2>/dev/null; then
			echo "         -> now on $(basename "$(readlink -f "$dev/driver" 2>/dev/null)")"
			rescued=$((rescued + 1))
		else
			# Bind failed: put it back so the wheel still works under
			# its previous driver rather than ending up unbound.
			echo "$name" > "/sys/bus/hid/drivers/$cur/bind" 2>/dev/null || true
			echo "         -> bind to $DRIVER FAILED (is the module loaded?); left on $cur" >&2
		fi
		continue
	fi

	# Unbound entirely: some sub-interfaces of these composite devices are
	# left unbound by design (this driver's own probe declines them), so
	# a failed bind attempt here is expected and is not reported as an
	# error.
	if echo "$name" > "/sys/bus/hid/drivers/$DRIVER/bind" 2>/dev/null; then
		echo "rescue:  $name was unbound, bound to $DRIVER"
		rescued=$((rescued + 1))
	fi
done

if [ "$found" -eq 0 ]; then
	echo "No supported Logitech wheel found on the HID bus."
	echo "Plug the wheel in (and switch it to PC mode, not PlayStation mode)."
	exit 0
fi

echo
echo "Summary: $ok already bound, $rescued rescued."
if [ "$rescued" -gt 0 ]; then
	echo "Force feedback and wheel_* sysfs should be available now."
fi
