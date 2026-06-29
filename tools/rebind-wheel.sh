#!/usr/bin/env bash
#
# Diagnose and fix the common "wheel stuck on hid-generic" problem.
#
# If the wheel enumerates before hid-logitech-hidpp is loaded (a boot
# race, or the in-kernel module loaded instead of this fork), hid-generic
# claims it. The symptom is no wheel_* sysfs, no force feedback, and the
# dmesg "Invalid code 768" phantom-button spam - the wheel works as a
# plain joystick but none of the driver features come up.
#
# This script loads the module, reports which driver each wheel interface
# is on, and rebinds any interface left on hid-generic to this driver.
# Run as root (or via sudo).

set -euo pipefail

DRIVER="logitech-hidpp-device"
MODULE="hid-logitech-hidpp"
# Supported wheels (USB product IDs, upper-case to match the HID device
# directory names under /sys/bus/hid/devices).
PIDS="C262 C26E C268 C272 C276"

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

	cur="$(basename "$(readlink -f "$dev/driver" 2>/dev/null)" 2>/dev/null || true)"
	if [ "$cur" = "$DRIVER" ]; then
		echo "ok:      $name already on $DRIVER"
		ok=$((ok + 1))
		continue
	fi

	if [ "$cur" = "hid-generic" ]; then
		echo "rescue:  $name is on hid-generic, rebinding to $DRIVER ..."
		echo "$name" > "/sys/bus/hid/drivers/hid-generic/unbind" 2>/dev/null || true
		if echo "$name" > "/sys/bus/hid/drivers/$DRIVER/bind" 2>/dev/null; then
			echo "         -> now on $(basename "$(readlink -f "$dev/driver" 2>/dev/null)")"
			rescued=$((rescued + 1))
		else
			# Bind failed: put it back so the wheel still works as a
			# plain joystick rather than ending up unbound.
			echo "$name" > "/sys/bus/hid/drivers/hid-generic/bind" 2>/dev/null || true
			echo "         -> bind to $DRIVER FAILED (is the in-kernel module loaded instead of the fork?); left on hid-generic" >&2
		fi
		continue
	fi

	echo "info:    $name is on '${cur:-none}' (not hid-generic); leaving it alone"
done

if [ "$found" -eq 0 ]; then
	echo "No supported Logitech wheel found on the HID bus."
	echo "Plug the wheel in (and switch it to PC mode, not PlayStation mode)."
	exit 0
fi

echo
echo "Summary: $ok already bound, $rescued rescued from hid-generic."
if [ "$rescued" -gt 0 ]; then
	echo "Force feedback and wheel_* sysfs should be available now."
fi
