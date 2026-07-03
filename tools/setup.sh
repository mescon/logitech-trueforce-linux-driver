#!/usr/bin/env bash
#
# One-command setup and diagnosis for the logitech-trueforce-linux-driver.
#
#   sudo ./tools/setup.sh            Full setup: DKMS module + udev rule +
#                                    in-tree driver blacklist + module load,
#                                    then (if the SDK DLLs are staged) the
#                                    TrueForce shim into every Steam prefix
#                                    as the invoking user.
#   ./tools/setup.sh doctor          Diagnose every layer, change nothing.
#                                    Run as your normal user.
#   ./tools/setup.sh shim            Only the TrueForce shim step (as user).
#
# The full setup is idempotent: run it again after `git pull` or a kernel
# update and it converges.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BLACKLIST_FILE="/etc/modprobe.d/blacklist-hid-logitech-hidpp.conf"
UDEV_DST="/etc/udev/rules.d/70-logitech-trueforce.rules"
WHEEL_PIDS="c276 c272 c268"
# Steam appids of the Logitech-SDK sims for launch-option checks:
#   ACC, AC EVO, AC, AMS2, Le Mans Ultimate, rFactor 2
SDK_SIM_APPIDS="805550 3058630 244210 1066890 2399420 365960"

pass=0; warn=0; fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass+1)); }
wrn()  { printf '  \033[33mWARN\033[0m %s\n' "$1"; warn=$((warn+1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail+1)); }
say()  { printf '\033[1m%s\033[0m\n' "$1"; }

find_wheel_sysfs() {
	ls -d /sys/class/hidraw/*/device/wheel_range 2>/dev/null | head -1 | xargs -r dirname
}

steam_roots() {
	local u_home
	u_home="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)"
	for d in "$u_home/.steam/steam" "$u_home/.local/share/Steam"; do
		[ -d "$d/steamapps" ] && echo "$d"
	done | sort -u
}

# ---------------------------------------------------------------- doctor --
doctor() {
	say "logitech-trueforce-linux-driver doctor"
	echo

	say "[1/7] Kernel module"
	if [ -d /sys/module/hid_logitech_hidpp ]; then
		ok "hid_logitech_hidpp is loaded"
	else
		bad "hid_logitech_hidpp is not loaded (run: sudo ./tools/setup.sh)"
	fi
	# No `grep -q` here: under `set -o pipefail`, -q exits on the first
	# match (our module sorts first in dkms output), dkms catches SIGPIPE
	# mid-print and the successful pipeline reports failure. Reading the
	# full stream avoids the race.
	if dkms status 2>/dev/null | grep '^hid-logitech-hidpp.*installed' >/dev/null; then
		ok "DKMS package installed (survives kernel updates)"
	else
		wrn "no DKMS install found - a manually insmod'ed module will not survive a reboot or kernel update (run: sudo ./tools/setup.sh)"
	fi
	if [ -f "$BLACKLIST_FILE" ]; then
		ok "in-tree driver blacklist present"
	else
		wrn "no blacklist file - the stock in-tree hid-logitech-hidpp (no RS50/G PRO FFB) may win the race at boot (run: sudo ./tools/setup.sh)"
	fi

	echo
	say "[2/7] Wheel"
	local usbline
	usbline="$(lsusb 2>/dev/null | grep -iE "046d:(c276|c272|c268)")"
	if [ -n "$usbline" ]; then
		ok "wheel on USB: ${usbline#*ID }"
	else
		wrn "no wheel detected on USB (plug it in and re-run doctor; everything below that needs the wheel is skipped)"
	fi

	local bound_generic=0 bound_ours=0
	for d in /sys/bus/hid/devices/0003:046D:C2{76,72,68}.*; do
		[ -e "$d" ] || continue
		case "$(basename "$(readlink -f "$d/driver" 2>/dev/null)")" in
			logitech-hidpp-device) bound_ours=$((bound_ours+1));;
			hid-generic) bound_generic=$((bound_generic+1));;
		esac
	done
	if [ "$bound_ours" -gt 0 ] && [ "$bound_generic" -eq 0 ]; then
		ok "all $bound_ours wheel interfaces bound to our driver"
	elif [ "$bound_generic" -gt 0 ]; then
		bad "$bound_generic wheel interface(s) stuck on hid-generic (run: sudo ./tools/rebind-wheel.sh)"
	fi

	echo
	say "[3/7] Driver health"
	local W
	W="$(find_wheel_sysfs)"
	if [ -n "$W" ]; then
		ok "wheel_* sysfs present ($W)"
		local fw
		fw="$(cat "$W/wheel_firmware" 2>/dev/null | tr '\n' ' ')"
		[ -n "$fw" ] && ok "firmware: $fw" || wrn "wheel_firmware unreadable"
		ok "range=$(cat "$W/wheel_range" 2>/dev/null) strength=$(cat "$W/wheel_strength" 2>/dev/null)% mode=$(cat "$W/wheel_mode" 2>/dev/null)"
	else
		[ -n "$usbline" ] && bad "wheel on USB but no wheel_* sysfs - driver not bound (see [2])" \
			|| wrn "skipped (no wheel)"
	fi

	echo
	say "[4/7] Permissions (udev)"
	if [ -f "$UDEV_DST" ]; then
		ok "udev rule installed"
	else
		wrn "udev rule missing - settings need sudo (run: sudo ./tools/setup.sh)"
	fi
	if [ -n "$W" ]; then
		if [ -w "$W/wheel_range" ] && [ -w "$W/range" ]; then
			ok "settings writable as $USER"
		else
			wrn "settings not writable as $USER - replug the wheel after installing the udev rule, and check 'groups' includes input"
		fi
	fi

	echo
	say "[5/7] TrueForce SDK DLLs (only needed for TrueForce in Proton sims)"
	local dll_missing=0
	for f in "sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll" \
		 "sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll" \
		 "sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll" \
		 "sdk/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll"; do
		[ -f "$REPO_ROOT/$f" ] || dll_missing=$((dll_missing+1))
	done
	if [ "$dll_missing" -eq 0 ]; then
		ok "all four SDK DLLs staged in the repo"
	else
		wrn "$dll_missing of 4 SDK DLLs not staged (see docs/GETTING_STARTED.md section 2; standard FFB works without them)"
	fi

	echo
	say "[6/7] Steam prefixes (shim)"
	local roots found_pfx=0 shimmed=0
	roots="$(steam_roots)"
	if [ -z "$roots" ]; then
		wrn "no Steam installation found for $USER"
	else
		while IFS= read -r root; do
			for pfx in "$root"/steamapps/compatdata/*/pfx; do
				[ -d "$pfx" ] || continue
				found_pfx=$((found_pfx+1))
				[ -f "$pfx/drive_c/Program Files/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll" ] && shimmed=$((shimmed+1))
			done
		done <<< "$roots"
		if [ "$found_pfx" -gt 0 ] && [ "$shimmed" -eq "$found_pfx" ]; then
			ok "TrueForce shim present in all $found_pfx Proton prefixes"
		elif [ "$shimmed" -gt 0 ]; then
			wrn "shim in $shimmed of $found_pfx Proton prefixes (run: ./tools/setup.sh shim)"
		elif [ "$found_pfx" -gt 0 ]; then
			wrn "shim not installed in any of the $found_pfx Proton prefixes (run: ./tools/setup.sh shim)"
		fi
	fi

	echo
	say "[7/7] Per-game launch options (PROTON_ENABLE_HIDRAW=1)"
	local checked=0
	local appid
	for appid in $SDK_SIM_APPIDS; do
		local installed=0 has_opt=0
		while IFS= read -r root; do
			[ -d "$root/steamapps/compatdata/$appid" ] && installed=1
			for cfg in "$root"/userdata/*/config/localconfig.vdf; do
				[ -f "$cfg" ] || continue
				if awk -v id="\"$appid\"" '$0 ~ id {inapp=1} inapp && /LaunchOptions/ {print; exit}' "$cfg" | grep -q 'PROTON_ENABLE_HIDRAW=1'; then
					has_opt=1
				fi
			done
		done <<< "$(steam_roots)"
		[ "$installed" -eq 1 ] || continue
		checked=$((checked+1))
		if [ "$has_opt" -eq 1 ]; then
			ok "appid $appid has PROTON_ENABLE_HIDRAW=1"
		else
			wrn "appid $appid: PROTON_ENABLE_HIDRAW=1 not found in launch options (needed for TrueForce; set it in Steam > Properties)"
		fi
	done
	[ "$checked" -eq 0 ] && wrn "no known SDK sims found installed (nothing to check)"

	echo
	say "Summary: $pass pass, $warn warn, $fail fail"
	[ "$fail" -eq 0 ] || return 1
	return 0
}

# ----------------------------------------------------------------- setup --
do_shim() {
	if [ "$EUID" -eq 0 ]; then
		if [ -n "${SUDO_USER:-}" ]; then
			runuser -u "$SUDO_USER" -- "$REPO_ROOT/tools/install-tf-shim.sh" --all-steam
		else
			echo "shim must run as the user owning the Steam prefixes; run: ./tools/setup.sh shim (no sudo)"
			return 1
		fi
	else
		"$REPO_ROOT/tools/install-tf-shim.sh" --all-steam
	fi
}

setup() {
	if [ "$EUID" -ne 0 ]; then
		echo "error: full setup needs root (sudo $0). For diagnosis only: $0 doctor" >&2
		exit 1
	fi

	say "[1/5] Kernel module (DKMS) + udev rule"
	"$REPO_ROOT/tools/dkms-update.sh" || exit 1

	say "[2/5] Blacklisting the in-tree drivers"
	if [ ! -f "$BLACKLIST_FILE" ]; then
		printf "blacklist hid-logitech-hidpp\nblacklist hid-logitech\n" > "$BLACKLIST_FILE"
		depmod -a
		echo "  installed $BLACKLIST_FILE"
	else
		echo "  already present"
	fi

	say "[3/5] Loading the module"
	modprobe -r hid-logitech-hidpp 2>/dev/null || true
	if modprobe hid-logitech-hidpp; then
		echo "  loaded"
	else
		echo "  modprobe failed - check dmesg" >&2
	fi
	# claim the wheel if it is currently sitting on hid-generic
	"$REPO_ROOT/tools/rebind-wheel.sh" >/dev/null 2>&1 || true

	say "[4/5] TrueForce shim (Steam prefixes)"
	if [ -f "$REPO_ROOT/sdk/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll" ]; then
		do_shim || true
	else
		echo "  SDK DLLs not staged - skipped (standard FFB works without them;"
		echo "  see docs/GETTING_STARTED.md section 2 for TrueForce)"
	fi

	say "[5/5] Doctor"
	# diagnosis runs best as the real user (permission checks)
	if [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- "$REPO_ROOT/tools/setup.sh" doctor || true
	else
		doctor || true
	fi

	echo
	say "Remaining manual steps (per game, in Steam):"
	echo "  1. Properties > Launch Options:  PROTON_ENABLE_HIDRAW=1 %command%"
	echo "  2. Properties > Controller:     Disable Steam Input"
	echo "  (both needed for TrueForce; see docs/GETTING_STARTED.md section 3)"
}

case "${1:-setup}" in
	doctor) doctor ;;
	shim)   do_shim ;;
	setup)  setup ;;
	*) echo "usage: sudo $0 [setup] | $0 doctor | $0 shim" >&2; exit 2 ;;
esac
