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

# A packaged install registers under its real version (logitech-trueforce/
# 0.30.0), this script under 1.0. Both build hid-logitech-dd.ko to the same
# path, so with both registered a kernel upgrade rebuilds each and whichever
# finishes last is the module that ends up loaded. That is not a failure
# anyone would connect to this script weeks later, so say it now.
warn_about_packaged_install() {
	local other
	other=$(dkms status 2>/dev/null \
		| sed -n 's|^logitech-trueforce/\([^,]*\),.*|\1|p' \
		| grep -v '^1\.0$' | sort -u | head -1) || true
	[ -n "${other:-}" ] || return 0
	cat >&2 <<EOF

WARNING: a packaged build is already registered with DKMS:

    logitech-trueforce/$other

Installing this development build alongside it leaves two DKMS packages
producing the same module. On the next kernel upgrade both rebuild and the
one that finishes last is the module you get, which is not a coin toss worth
debugging later.

Remove the packaged one first if you mean to work from source:

    sudo dkms remove -m logitech-trueforce -v $other --all
    # and uninstall the distribution package, or it will come back

Continuing in 5 seconds; Ctrl-C to stop.
EOF
	sleep 5
}

PKG="logitech-trueforce"
# A fixed development slot, deliberately not the release version: this
# script exists to be run repeatedly from a working tree, and a version that
# moved would leave a trail of stale DKMS entries.
VER="1.0"
SRC_DIR="/usr/src/${PKG}-${VER}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO_SRC="$REPO_ROOT/mainline"
UDEV_SRC="$REPO_ROOT/udev/70-logitech-trueforce.rules"
UDEV_DST="/etc/udev/rules.d/70-logitech-trueforce.rules"
UDEV_FFB_SRC="$REPO_ROOT/udev/71-logi-ffb-uhid.rules"
UDEV_FFB_DST="/etc/udev/rules.d/71-logi-ffb-uhid.rules"
UDEV_G923_SRC="$REPO_ROOT/udev/72-logitech-g923-rebind.rules"
UDEV_G923_DST="/etc/udev/rules.d/72-logitech-g923-rebind.rules"
UDEV_G923_XBOX_SRC="$REPO_ROOT/udev/73-logitech-g923-xbox-modeswitch.rules"
UDEV_G923_XBOX_DST="/etc/udev/rules.d/73-logitech-g923-xbox-modeswitch.rules"
MODESWITCH_SRC="$REPO_ROOT/tools/g923-xbox-modeswitch.sh"
MODESWITCH_DST="/usr/bin/logi-g923-modeswitch"
REBIND_SRC="$REPO_ROOT/tools/rebind-wheel.sh"
REBIND_DST="/usr/bin/logi-rebind-wheel"
MODPROBE_SRC="$REPO_ROOT/packaging/modprobe.d/hid-logitech-dd.conf"
MODPROBE_DST="/etc/modprobe.d/hid-logitech-dd.conf"

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
# Tag-derived version (v0.16.0 at a tag, v0.16.0-3-gabc1234 between tags)
# so the DKMS-built module reports the release, not a bare hash.
GIT_HASH=$(git -c "safe.directory=$REPO_ROOT" -C "$REPO_ROOT" describe --tags --always --dirty 2>/dev/null || echo unknown)
echo "$GIT_HASH" > "$SRC_DIR/.git_hash"

# Drop previous DKMS state for this version. Ignore "not found".
warn_about_packaged_install
dkms remove -m "$PKG" -v "$VER" --all >/dev/null 2>&1 || true

echo "== dkms install -m $PKG -v $VER =="
dkms install -m "$PKG" -v "$VER"

# Install / refresh udev rule so wheel_* sysfs attrs and hidraw nodes
# are writable by the logged-in session user (or members of "input"),
# not just root. Without this every Oversteer knob and every echo >
# A rule in /etc/udev/rules.d takes precedence over the same filename in
# /usr/lib/udev/rules.d, which is where the distribution packages put
# theirs. Installing from a checkout on top of a packaged install therefore
# SHADOWS the package: identical today, but the next package upgrade
# updates /usr/lib while the stale copy in /etc keeps winning, and the
# upgrade silently does nothing.
#
# So: skip when the packaged copy is already identical, and say plainly
# what is happening when it is not.
#
#   needs_rule <source> <dest>   -> 0 to install, 1 to skip
needs_rule() {
	local src="$1" dst="$2"
	local packaged="/usr/lib/udev/rules.d/$(basename "$dst")"

	if cmp -s "$src" "$dst" 2>/dev/null; then
		echo "udev rule up to date ($dst)"
		return 1
	fi
	if [ -f "$packaged" ]; then
		if cmp -s "$src" "$packaged" 2>/dev/null; then
			echo "udev rule already provided by the package ($packaged); not shadowing it"
			# An older checkout may have left one behind; take it away
			# so the package's copy is the one that applies.
			if [ -f "$dst" ]; then
				echo "== removing the checkout copy that shadowed it: $dst =="
				rm -f "$dst"
				udevadm control --reload
			fi
			return 1
		fi
		echo "NOTE: $packaged exists and differs from this checkout."
		echo "      Installing to $dst, which OVERRIDES the packaged rule."
		echo "      Remove $dst to go back to the packaged one."
	fi
	return 0
}

# wheel_* needs sudo.
if [ -f "$UDEV_SRC" ]; then
	# Pre-rename installs used this filename; drop it so the rules
	# don't run twice.
	rm -f /etc/udev/rules.d/70-logitech-rs50.rules
	if needs_rule "$UDEV_SRC" "$UDEV_DST"; then
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
	if needs_rule "$UDEV_FFB_SRC" "$UDEV_FFB_DST"; then
		echo "== installing udev rule to $UDEV_FFB_DST =="
		install -m 0644 "$UDEV_FFB_SRC" "$UDEV_FFB_DST"
		udevadm control --reload
		udevadm trigger --subsystem-match=misc
	else
		echo "udev rule up to date ($UDEV_FFB_DST)"
	fi
fi

# Same for the G923 (c266/c267/c26e) bind-race rebind rule: it fires on
# SUBSYSTEM=="hid" add/bind, not hidraw, so it needs its own trigger match.
if [ -f "$UDEV_G923_SRC" ]; then
	if needs_rule "$UDEV_G923_SRC" "$UDEV_G923_DST"; then
		echo "== installing udev rule to $UDEV_G923_DST =="
		install -m 0644 "$UDEV_G923_SRC" "$UDEV_G923_DST"
		udevadm control --reload
		udevadm trigger --subsystem-match=hid
	else
		echo "udev rule up to date ($UDEV_G923_DST)"
	fi
fi

# The helper the c26d rule runs. It has to be installed BEFORE the rule
# that invokes it: the rule dispatches through systemd-run with its output
# discarded, so a missing helper fails invisibly and the wheel simply never
# leaves Xbox console mode. That is indistinguishable from a dead wheel,
# which is exactly how it was reported (issue #27). Every distro package
# already installed it; only this from-source path did not.
if [ -f "$MODESWITCH_SRC" ]; then
	if ! cmp -s "$MODESWITCH_SRC" "$MODESWITCH_DST" 2>/dev/null; then
		echo "== installing $MODESWITCH_DST =="
		install -Dm 0755 "$MODESWITCH_SRC" "$MODESWITCH_DST"
	else
		echo "mode-switch helper up to date ($MODESWITCH_DST)"
	fi
fi

# The rebind helper, which the settings apps' diagnostics offer by name
# when another driver has claimed the wheel. Offering a command that is
# not installed is worse than offering none, so it ships everywhere the
# apps do: every distro package installs it, and so does this path.
if [ -f "$REBIND_SRC" ]; then
	if ! cmp -s "$REBIND_SRC" "$REBIND_DST" 2>/dev/null; then
		echo "== installing $REBIND_DST =="
		install -Dm 0755 "$REBIND_SRC" "$REBIND_DST"
	else
		echo "rebind helper up to date ($REBIND_DST)"
	fi
fi

# Same for the G923 Xbox edition (c26d) boot-mode switch: it fires on
# SUBSYSTEM=="usb" add/change, on the raw USB device, not the HID
# interfaces the other two rules watch.
if [ -f "$UDEV_G923_XBOX_SRC" ]; then
	if needs_rule "$UDEV_G923_XBOX_SRC" "$UDEV_G923_XBOX_DST"; then
		echo "== installing udev rule to $UDEV_G923_XBOX_DST =="
		install -m 0644 "$UDEV_G923_XBOX_SRC" "$UDEV_G923_XBOX_DST"
		udevadm control --reload
		udevadm trigger --subsystem-match=usb
	else
		echo "udev rule up to date ($UDEV_G923_XBOX_DST)"
	fi
fi

# modprobe.d: softdep ordering hint for the G923 PIDs, plus a narrow
# blacklist of the standalone new-lg4ff fork (see the file for why that
# one is safe to blacklist and hid-logitech-hidpp is not).
if [ -f "$MODPROBE_SRC" ]; then
	if ! cmp -s "$MODPROBE_SRC" "$MODPROBE_DST" 2>/dev/null; then
		echo "== installing $MODPROBE_DST =="
		install -Dm 0644 "$MODPROBE_SRC" "$MODPROBE_DST"
	else
		echo "modprobe.d config up to date ($MODPROBE_DST)"
	fi
fi

# Install / refresh the Logitech TrueForce SDK shim so Proton games
# that use the SDK (ACC, iRacing, AMS2, ...) find it via Wine's CLSID
# lookup. The shim is installed per-prefix inside drive_c (Proton's
# pressure-vessel doesn't expose host /usr/lib to the game), so this
# step runs as the invoking user, not root. Skip silently if no Steam
# library is present.
# No winegcc check: this used to gate on it and tell the user to install
# wine-devel, which was true when the shim was compiled and has not been
# since it became a copy of Logitech's own DLLs plus a system.reg edit. A
# Proton-only machine, which is most of them, was told the shim was skipped
# for a tool it had no reason to have. setup.sh's own shim step never had
# the gate, so the two entry points disagreed about the same install.
TF_INSTALL="$REPO_ROOT/tools/install-tf-shim.sh"
if [ -x "$TF_INSTALL" ]; then
	echo "== installing TrueForce SDK shim for Proton games =="
	if [ -n "${SUDO_USER:-}" ]; then
		sudo -u "$SUDO_USER" "$TF_INSTALL" --all-steam \
			|| echo "warning: TF shim install failed (continuing)"
	else
		"$TF_INSTALL" --all-steam \
			|| echo "warning: TF shim install failed (continuing)"
	fi
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
