#!/bin/sh
# Switch a Logitech wheel out of Xbox console mode.
#
# Two wheels ship an Xbox edition that boots speaking Xbox GIP instead of
# HID++, under a product id of its own. In that mode there is no HID++
# interface at all, so nothing here can bind it and the wheel has to be told
# to re-enumerate first:
#
#   046d:c26d  G923 Xbox edition   -> 046d:c26e  (reported in issue #52)
#   046d:c275  RS50 Xbox edition   -> 046d:c276  (reported in issue #65)
#
# Both take the same vendor mode-switch message, which is the sequence the
# Windows driver sends. The RS50 pair was confirmed on hardware by the
# reporter of #65 before this handled it; the G923 pair has been in use
# since 0.20.0.
#
# Two things this does that a bare usb_modeswitch call in a udev rule did not,
# both from a detailed report on a Legion Go running SteamOS (issue #52):
#
#   1. It releases whatever already holds the interfaces. In console mode
#      xbox-gip binds the wheel (xone does the same on other setups), and
#      usb_modeswitch cannot drive an interface another driver owns.
#
#   2. It is meant to be dispatched asynchronously, NOT from a udev RUN+=.
#      RUN+= runs inside the udev worker and holds the device lock for the
#      whole USB control transfer. On a machine whose built-in controllers are
#      internal USB devices, that wedged the USB stack hard enough to take the
#      desktop down and stop the machine booting while the wheel was attached.
#      73-logitech-xbox-modeswitch.rules now dispatches this through
#      systemd-run --no-block so the worker returns immediately.
#
# Safe to run by hand, which is also the answer on a system without systemd:
#   sudo logi-wheel-modeswitch
#
# With no argument it switches whichever of the wheels above is attached, so
# the same command works on either. A product id may be given to name one:
#   sudo logi-wheel-modeswitch c275
set -eu

SELF=$(basename "$0")
VID=046d
# Console-mode product ids, and what each becomes. Adding a wheel here is
# the whole change: the rules file matches the same list.
CONSOLE_PIDS="c26d c275"
# Vendor mode-switch message, from the Windows driver's own sequence.
MSG=0f00010142

want=${1:-}
case "$want" in
"") ;;
c26d|c275) CONSOLE_PIDS=$want ;;
*)
	echo "$SELF: $want is not a console-mode product id ($CONSOLE_PIDS)" >&2
	exit 1
	;;
esac

if ! command -v usb_modeswitch >/dev/null 2>&1; then
	echo "$SELF: usb_modeswitch not installed" >&2
	exit 1
fi

# Which of them is actually here. Asking the bus rather than assuming keeps
# the no-wheel case quiet: this runs from a udev rule, and a wheel that
# already switched (or was unplugged in between) must not look like a
# failure.
attached=""
for pid in $CONSOLE_PIDS; do
	for dev in /sys/bus/usb/devices/*; do
		[ -e "$dev/idProduct" ] || continue
		[ "$(cat "$dev/idVendor" 2>/dev/null)" = "$VID" ] || continue
		[ "$(cat "$dev/idProduct" 2>/dev/null)" = "$pid" ] || continue
		attached="$attached $pid"
		break
	done
done
if [ -z "$attached" ]; then
	echo "$SELF: no wheel in console mode ($CONSOLE_PIDS) attached" >&2
	exit 0
fi

for pid in $attached; do
	# Release any driver bound to this wheel's interfaces. Failure is not
	# fatal: an interface with no driver is already in the state we want.
	for iface in /sys/bus/usb/devices/*:*.*; do
		[ -e "$iface/../idVendor" ] || continue
		v=$(cat "$iface/../idVendor" 2>/dev/null) || continue
		p=$(cat "$iface/../idProduct" 2>/dev/null) || continue
		[ "$v" = "$VID" ] || continue
		[ "$p" = "$pid" ] || continue
		[ -L "$iface/driver" ] || continue

		drv=$(basename "$(readlink -f "$iface/driver")")
		name=$(basename "$iface")
		echo "$SELF: releasing $name from $drv" >&2
		echo "$name" > "/sys/bus/usb/drivers/$drv/unbind" 2>/dev/null || true
	done

	echo "$SELF: switching $VID:$pid to PC mode" >&2
	usb_modeswitch -v "$VID" -p "$pid" -M "$MSG" -C 0x03 -m 01 -r 81
done
