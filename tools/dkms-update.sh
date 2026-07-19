#!/usr/bin/env bash
#
# Update the DKMS-installed logitech-trueforce package (built as the
# hid-logitech-dd.ko module) from the current repo checkout. Copies
# mainline/ into /usr/src/logitech-trueforce-1.0/,
# removes any previous DKMS state for that version, and installs the
# freshly built module. Does NOT unload the running module - reload it
# manually (see the final message) once the wheel is free.
#
# Usage: sudo ./tools/dkms-update.sh
#
# Written for contributors iterating on fixes (in particular #8) who
# otherwise end up typing the full dkms-remove / rm -rf / cp / build /
# install dance every time.

set -euo pipefail

PKG="logitech-trueforce"
VER="1.0"
SRC_DIR="/usr/src/${PKG}-${VER}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO_SRC="$REPO_ROOT/mainline"
UDEV_SRC="$REPO_ROOT/udev/70-logitech-trueforce.rules"
UDEV_DST="/etc/udev/rules.d/70-logitech-trueforce.rules"
UDEV_FFB_SRC="$REPO_ROOT/udev/71-logi-ffb-uhid.rules"
UDEV_FFB_DST="/etc/udev/rules.d/71-logi-ffb-uhid.rules"

if [ "$EUID" -ne 0 ]; then
	echo "error: run as root (sudo $0)" >&2
	exit 1
fi

if [ ! -d "$REPO_SRC" ]; then
	echo "error: cannot find mainline/ at $REPO_SRC" >&2
	exit 1
fi

echo "== updating $SRC_DIR from $REPO_SRC =="
rm -rf "$SRC_DIR"
mkdir -p "$SRC_DIR"
cp -r "$REPO_SRC/." "$SRC_DIR/"

# Strip any in-tree build artefacts that snuck in via the cp above.
# These are gitignored but not auto-cleaned, and `cp` gives them the
# same mtime as the freshly copied .c, so kbuild thinks the .o is up
# to date and skips recompilation, linking the OLD object code into a
# fresh-looking .ko (issue #17).
find "$SRC_DIR" \( \
	-name '*.o' -o -name '*.ko*' -o \
	-name '*.mod' -o -name '*.mod.c' -o \
	-name '.*.cmd' -o -name '.*.o.d' -o \
	-name 'Module.symvers' -o -name 'modules.order' \
	\) -delete

# Stamp the source tree with the git hash so the loaded module can
# report which checkout it came from (Kbuild reads this). The
# `-c safe.directory=...` is needed because we run as root via sudo
# while $REPO_ROOT is owned by the invoking user; without it git's
# dubious-ownership check fails and we silently record "unknown".
GIT_HASH=$(git -c "safe.directory=$REPO_ROOT" -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)
echo "$GIT_HASH" > "$SRC_DIR/.git_hash"

# Drop previous DKMS state for this version. Ignore "not found".
dkms remove -m "$PKG" -v "$VER" --all >/dev/null 2>&1 || true

echo "== dkms install -m $PKG -v $VER =="
dkms install -m "$PKG" -v "$VER"

# Install / refresh udev rule so wheel_* sysfs attrs and hidraw nodes
# are writable by the logged-in session user (or members of "input"),
# not just root. Without this every Oversteer knob and every echo >
# wheel_* needs sudo.
if [ -f "$UDEV_SRC" ]; then
	# Pre-rename installs used this filename; drop it so the rules
	# don't run twice.
	rm -f /etc/udev/rules.d/70-logitech-rs50.rules
	if ! cmp -s "$UDEV_SRC" "$UDEV_DST" 2>/dev/null; then
		echo "== installing udev rule to $UDEV_DST =="
		install -m 0644 "$UDEV_SRC" "$UDEV_DST"
		udevadm control --reload
		udevadm trigger --subsystem-match=hidraw
	else
		echo "udev rule up to date ($UDEV_DST)"
	fi
fi

# Same for the logi-ffb rule, which opens /dev/uhid to the "input" group
# so the DirectInput FFB proxy can create its virtual wheel without sudo.
if [ -f "$UDEV_FFB_SRC" ]; then
	if ! cmp -s "$UDEV_FFB_SRC" "$UDEV_FFB_DST" 2>/dev/null; then
		echo "== installing udev rule to $UDEV_FFB_DST =="
		install -m 0644 "$UDEV_FFB_SRC" "$UDEV_FFB_DST"
		udevadm control --reload
		udevadm trigger --subsystem-match=misc
	else
		echo "udev rule up to date ($UDEV_FFB_DST)"
	fi
fi

# Install / refresh the Logitech TrueForce SDK shim so Proton games
# that use the SDK (ACC, iRacing, AMS2, ...) find it via Wine's CLSID
# lookup. The shim is installed per-prefix inside drive_c (Proton's
# pressure-vessel doesn't expose host /usr/lib to the game), so this
# step runs as the invoking user, not root. Skip silently if no Steam
# library is present.
TF_INSTALL="$REPO_ROOT/tools/install-tf-shim.sh"
if [ -x "$TF_INSTALL" ] && command -v winegcc >/dev/null 2>&1; then
	echo "== installing TrueForce SDK shim for Proton games =="
	if [ -n "${SUDO_USER:-}" ]; then
		sudo -u "$SUDO_USER" "$TF_INSTALL" --all-steam \
			|| echo "warning: TF shim install failed (continuing)"
	else
		"$TF_INSTALL" --all-steam \
			|| echo "warning: TF shim install failed (continuing)"
	fi
elif [ -x "$TF_INSTALL" ]; then
	echo "note: skipping TrueForce SDK shim (winegcc not available)."
	echo "      install wine/wine-devel and re-run tools/install-tf-shim.sh --all-steam"
fi

cat <<'EOF'

Module installed. To pick it up without a reboot:

  1) Unplug the wheel (or close anything holding the evdev / hidraw
     device open - e.g. fftest, games, browser tabs with Gamepad API)
  2) sudo modprobe -r hid-logitech-dd
  3) sudo modprobe hid-logitech-dd
  4) Plug the wheel back in

If modprobe -r reports "Module is in use", something still has the
device open. Find it with:  sudo fuser -v /dev/input/event* /dev/hidraw*

If after this the wheel still has no force feedback and no wheel_* sysfs
(hid-generic claimed it because it enumerated before the module loaded),
run:

  sudo ./tools/rebind-wheel.sh

which loads the module and rebinds the wheel to this driver.

On UEFI Secure Boot systems, DKMS should re-sign the module with your
MOK key automatically. If load fails with "Key was rejected by
service", re-enroll the MOK and reboot once.
EOF
