#!/usr/bin/env bash
#
# One-command setup and diagnosis for the logitech-trueforce-linux-driver.
#
#   sudo ./tools/setup.sh            Full setup: DKMS module + udev rule +
#                                    module load (migrating off any old
#                                    full-fork install),
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
# Left behind by the old full-fork install; this scoped build must NOT
# blacklist hid-logitech-hidpp (that would strip the in-tree driver from
# the user's Logitech mice/keyboards). Removed during migration below.
OLD_BLACKLIST_FILE="/etc/modprobe.d/blacklist-hid-logitech-hidpp.conf"
UDEV_DST="/etc/udev/rules.d/70-logitech-trueforce.rules"
UDEV_FFB_DST="/etc/udev/rules.d/71-logi-ffb-uhid.rules"
UDEV_G923_DST="/etc/udev/rules.d/72-logitech-g923-rebind.rules"
UDEV_G923_XBOX_DST="/etc/udev/rules.d/73-logitech-g923-xbox-modeswitch.rules"
MODPROBE_DST="/etc/modprobe.d/hid-logitech-dd.conf"
MODESWITCH_DST="/usr/bin/logi-g923-modeswitch"
# Direct-drive wheels, then the G923 editions. doctor was written before the
# G923 was supported and checked only the first three, so every G923 owner was
# told "no wheel detected" with the wheel plugged in and working, and the
# driver-health section was skipped as a consequence (issue #27).
WHEEL_PIDS_DD="c276 c272 c268"
WHEEL_PIDS_G923="c266 c267 c26e"
WHEEL_PID_G923_CONSOLE="c26d"
WHEEL_PIDS="$WHEEL_PIDS_DD $WHEEL_PIDS_G923"
# G HUB revises the SDK and the version is a directory name, so never assume
# one: a current install ships 1_3_12 and 9_1_1, and hardcoding the older
# pair made those invisible with no explanation (issue #54).
# Where the SDK DLLs live inside a wine prefix, above the version directory.
# Deliberately NOT carrying the trailing "/*": held as one string with the
# glob in it, this could only be used unquoted, and unquoted it word-split on
# the space in "Program Files" into two arguments that matched nothing. The
# check that depends on it had therefore never once fired. Callers append
# their own glob to the quoted path.
TF_PFX_DIR_REL="drive_c/Program Files/Logi/Trueforce"

# Steam appids grouped by what the game actually needs, mirroring the app's
# compatibility registry (logi-wheel-core/src/games.rs).
#
# These used to be one undifferentiated list of six, and every game in it was
# told to set PROTON_ENABLE_HIDRAW=1. Only the first two want that. For the
# DirectInput pair it is actively harmful: the variable sends the game to raw
# HID reports it cannot drive force feedback through, so doctor was telling
# those owners to break the force feedback they had. The other two needed
# nothing and were pure noise (#54).
#
# Kept in step with the registry by
# logi-wheel-core/tests/shell_appid_sync.rs, which fails when these lists and
# `games::launch_option_appid_groups()` disagree. The lists have drifted twice
# already, both times leaving an owner unwarned about the setting that stopped
# their force feedback, so the copy is guarded rather than trusted.
#
# Sims that load Logitech's TrueForce SDK: ACC, AC EVO.
SDK_SIM_APPIDS="805550 3058630"
# Sims driven through DirectInput, i.e. logi-ffb: rFactor 2, Le Mans Ultimate,
# iRacing, RaceRoom. PROTON_ENABLE_HIDRAW must NOT be set for these.
DINPUT_SIM_APPIDS="365960 2399420 266410 211500"

pass=0; warn=0; fail=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass+1)); }
wrn()  { printf '  \033[33mWARN\033[0m %s\n' "$1"; warn=$((warn+1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail+1)); }
say()  { printf '\033[1m%s\033[0m\n' "$1"; }

# Where the shim installer lives. In a checkout it sits beside this script;
# a distro package installs it as logi-shim on PATH and ships no checkout at
# all, which is why nothing here may assume $REPO_ROOT exists.
find_shim_installer() {
	if [ -x "$REPO_ROOT/tools/install-tf-shim.sh" ]; then
		echo "$REPO_ROOT/tools/install-tf-shim.sh"
	elif command -v logi-shim >/dev/null 2>&1; then
		command -v logi-shim
	fi
}

# The SDK directory the installer would actually read, asked of the installer
# rather than guessed. Prints nothing when it cannot be found.
#
# Resolved as the invoking user, never as root. The installer's fallback is
# under $HOME, so asking it as root answers /root/.local/share/... while the
# install itself runs under the user via runuser and uses theirs. The setup
# flow gated the install on this, so a user who had staged the DLLs exactly
# where the README says was told "SDK DLLs not staged - skipped" and then,
# seconds later, told by doctor that all four were staged. Same run, opposite
# answers, no shim. runuser without -m sets HOME for us, which is the whole
# point of routing through it.
resolved_sdk_dir() {
	local inst
	inst="$(find_shim_installer)"
	[ -n "$inst" ] || return 0
	if [ "$EUID" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- "$inst" --print-sdk-dir 2>/dev/null \
			| sed -n 's/^sdk_dir=//p'
	else
		"$inst" --print-sdk-dir 2>/dev/null | sed -n 's/^sdk_dir=//p'
	fi
}

# The direct-drive wheels expose wheel_*; the G923 exposes the classic set
# (range, gain, autocenter) and no wheel_* at all. Look for either.
find_wheel_sysfs() {
	ls -d /sys/class/hidraw/*/device/wheel_range 2>/dev/null | head -1 | xargs -r dirname
}

# Whether the attached wheel answers Logitech's TrueForce SDK. The
# direct-drive family does; the G923 does not, and telling a G923 owner to
# set PROTON_ENABLE_HIDRAW=1 costs them their force feedback. Mirrors
# `games::WheelCaps` in the app. With no wheel plugged in this reports the
# general case, the wheels this driver was written for.
wheel_has_sdk_trueforce() {
	local re
	# sysfs first, lsusb second. The two attribute namespaces never
	# overlap, so a device carrying wheel_range is direct-drive and one
	# carrying the classic set is not, which is a fact about the machine
	# rather than about which tools are installed. Keying on lsusb alone
	# meant that on a box without usbutils - undeclared by every packaging
	# channel until now - the check fell through to "yes", and a G923
	# owner was told to set the variable that removes their force
	# feedback. Wrong answers here cost people their force feedback, so it
	# should not hinge on an optional binary being present.
	[ -n "$(find_wheel_sysfs)" ] && return 0
	[ -n "$(find_g923_sysfs)" ] && return 1
	re="$(echo "$WHEEL_PIDS_G923 $WHEEL_PID_G923_CONSOLE" | tr ' ' '|')"
	if lsusb 2>/dev/null | grep -qiE "046d:($re)"; then
		re="$(echo "$WHEEL_PIDS_DD" | tr ' ' '|')"
		lsusb 2>/dev/null | grep -qiE "046d:($re)"
		return
	fi
	return 0
}

# Whether a G923 of any edition is on USB, including the Xbox one still in
# console mode. Keyed on USB rather than on our sysfs, because the case the
# rebind rule exists for is precisely the one where the in-tree driver won
# and the wheel has none of our attributes to find.
g923_on_usb() {
	local re
	re="$(echo "$WHEEL_PIDS_G923 $WHEEL_PID_G923_CONSOLE" | tr ' ' '|')"
	lsusb 2>/dev/null | grep -qiE "046d:($re)"
}

find_g923_sysfs() {
	local d
	for d in /sys/class/hidraw/*/device/range; do
		[ -e "$d" ] || continue
		# wheel_range means a direct-drive wheel, which the caller above
		# already handles; this is only for the classic-only wheels.
		[ -e "$(dirname "$d")/wheel_range" ] && continue
		dirname "$d"
		return
	done
}

# Every Steam library root, deduped by canonical path.
#
# Two bugs lived here. `sort -u` deduped the strings, but ~/.steam/steam is
# normally a symlink to ~/.local/share/Steam, so the same library came back
# twice and every count in sections 6 and 7 was doubled: "4 installed SDK
# sims" for a machine with two. And libraries the user added on another
# drive were skipped entirely, so a report from someone whose games live on
# a second disk showed nothing installed while the shim installer, which
# does read libraryfolders.vdf, was staging into them quite happily
# (reported by @sugituber). Both are what install-tf-shim.sh and the app
# already did; this is doctor catching up.
steam_roots() {
	local u_home base vdf real
	u_home="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)"
	{
		# Flatpak Steam last: it is a different install root entirely,
		# and the akmods spec targets Bazzite/Silverblue where it is the
		# normal way to have Steam. None of the three components looked
		# for it, so those users got "no Steam installation found".
		for base in "$u_home/.steam/steam" "$u_home/.local/share/Steam" \
			    "$u_home/.steam/debian-installation" \
			    "$u_home/.var/app/com.valvesoftware.Steam/.local/share/Steam"; do
			[ -d "$base" ] || continue
			printf '%s\n' "$base"
			vdf="$base/steamapps/libraryfolders.vdf"
			[ -f "$vdf" ] || continue
			sed -nE 's/^[[:space:]]*"path"[[:space:]]+"(.*)"[[:space:]]*$/\1/p' "$vdf"
		done
	} | while IFS= read -r d; do
		[ -d "$d/steamapps" ] || continue
		# Resolve before deduping: the string forms differ, the directory
		# does not.
		real="$(readlink -f "$d" 2>/dev/null || echo "$d")"
		printf '%s\n' "$real"
	done | awk '!seen[$0]++'
}

# ---------------------------------------------------------------- doctor --
doctor() {
	say "logitech-trueforce-linux-driver doctor"
	echo

	say "[1/7] Kernel module"
	if [ -d /sys/module/hid_logitech_dd ]; then
		local loaded_ver repo_ver
		loaded_ver="$(cat /sys/module/hid_logitech_dd/version 2>/dev/null || echo unknown)"
		ok "hid_logitech_dd is loaded (version: $loaded_ver)"
		# Running module vs the source it came from. Pulling without
		# rebuilding leaves an old driver in memory and every symptom
		# belongs to code nobody is reading any more, which is a
		# spectacularly good way to waste an afternoon.
		if [ -d "$REPO_ROOT/.git" ]; then
			# --dirty, because that is what dkms-update.sh stamps into
			# the module. Without it a contributor with uncommitted
			# changes was told to rebuild, and rebuilding never cleared
			# it: permanently wrong for the only people it addresses.
			repo_ver="$(git -C "$REPO_ROOT" describe --tags --always --dirty 2>/dev/null)"
			if [ -n "$repo_ver" ] && [ "$loaded_ver" != "$repo_ver" ]; then
				wrn "the loaded module is $loaded_ver but this checkout is $repo_ver - rebuild so you are testing the code you have (run: sudo ./tools/setup.sh)"
			elif [ -n "$repo_ver" ]; then
				ok "module matches this checkout ($repo_ver)"
			fi
		fi
	else
		bad "hid_logitech_dd is not loaded (run: sudo ./tools/setup.sh)"
	fi
	# App versions, and their absence. Reporting only the ones that are
	# present used to hide the case that actually bites: a from-source
	# install put the driver on but never built the apps, so a months-old
	# binary sat next to a current driver and doctor said nothing at all.
	# The version of this checkout, when there is one, is what they should
	# match.
	local want=""
	if [ -f "$REPO_ROOT/userspace/logi-wheel/Cargo.toml" ]; then
		want=$(sed -n 's/^version = "\(.*\)"/\1/p' \
			"$REPO_ROOT/userspace/logi-wheel/Cargo.toml" | head -1)
	fi
	local tool have
	for tool in logi-wheel logi-ffb logi-tf-sim logi-wheel-gui; do
		if command -v "$tool" >/dev/null 2>&1; then
			have=$("$tool" --version 2>/dev/null || echo "")
			ok "$tool on PATH (${have:-version flag unsupported})"
			if [ -n "$want" ] && [ -n "$have" ] && [ "${have##* }" != "$want" ]; then
				wrn "$tool is ${have##* } but this checkout is $want (run: sudo $0)"
			fi
		elif [ "$tool" = "logi-wheel-gui" ]; then
			wrn "$tool is not installed (optional: the window; the terminal app does the same job)"
		elif [ -n "$want" ]; then
			# A checkout: setup.sh builds and installs these, so missing
			# means the install did not finish.
			bad "$tool is not installed (run: sudo $0)"
		else
			# No checkout, so this is a packaged system and the driver
			# package alone is a legitimate way to run. Telling such a
			# user to run a script they do not have would be worse than
			# saying nothing.
			wrn "$tool is not installed (install the logi-wheel package for your distribution)"
		fi
	done
	# No `grep -q` here: under `set -o pipefail`, -q exits on the first
	# match (our module sorts first in dkms output), dkms catches SIGPIPE
	# mid-print and the successful pipeline reports failure. Reading the
	# full stream avoids the race.
	if dkms status 2>/dev/null | grep '^logitech-trueforce.*installed' >/dev/null; then
		ok "DKMS package installed (survives kernel updates)"
	else
		wrn "no DKMS install found - a manually insmod'ed module will not survive a reboot or kernel update (run: sudo ./tools/setup.sh)"
	fi
	if [ -f "$OLD_BLACKLIST_FILE" ]; then
		wrn "stale blacklist from the old full-fork install present ($OLD_BLACKLIST_FILE) - it strips the in-tree driver from your other Logitech devices; remove it (run: sudo ./tools/setup.sh)"
	fi
	if dkms status 2>/dev/null | grep '^hid-logitech-hidpp.*installed' >/dev/null; then
		wrn "old full-fork DKMS package 'hid-logitech-hidpp' still installed - it shadowed the in-tree driver for all Logitech devices; remove it (run: sudo ./tools/setup.sh)"
	fi

	echo
	say "[2/7] Wheel"
	local usbline
	local pid_re console_line
	pid_re="$(echo "$WHEEL_PIDS" | tr ' ' '|')"
	usbline="$(lsusb 2>/dev/null | grep -iE "046d:($pid_re)")"
	console_line="$(lsusb 2>/dev/null | grep -iE "046d:$WHEEL_PID_G923_CONSOLE")"
	if [ -n "$usbline" ]; then
		# One line per wheel. More than one is normal here (a G923 and a
		# direct-drive base together), and printing the list as a single
		# value left every wheel after the first unlabelled.
		while IFS= read -r l; do
			[ -n "$l" ] && ok "wheel on USB: ${l#*ID }"
		done <<< "$usbline"
	elif [ -n "$console_line" ]; then
		# Not "no wheel": a G923 Xbox that never left console mode. Saying
		# nothing was found sends the owner looking for the wrong fault.
		bad "G923 Xbox edition is in console mode ($WHEEL_PID_G923_CONSOLE) and unusable until it switches to $WHEEL_PIDS_G923; install usb_modeswitch and replug (see [4] for the rule and helper)"
	else
		wrn "no wheel detected on USB (plug it in and re-run doctor; everything below that needs the wheel is skipped)"
	fi

	local bound_generic=0 bound_ours=0 bound_foreign=0 foreign_name=""
	local pid_up
	for pid_up in $(echo "$WHEEL_PIDS" | tr 'a-z ' 'A-Z\n'); do
	for d in /sys/bus/hid/devices/0003:046D:${pid_up}.*; do
		[ -e "$d" ] || continue
		# The third case is the one that mattered and was missing. The
		# rebind rule and rebind-wheel.sh both exist because an in-tree
		# Logitech driver can win the race, and those bind as `logitech`
		# or `logitech-hidpp-device` - neither of which matched an arm
		# here, so both counters stayed zero and this section printed
		# nothing at all. Section 3 then said "driver not bound (see
		# [2])", pointing at a section that had been silent.
		case "$(basename "$(readlink -f "$d/driver" 2>/dev/null)")" in
			logitech-dd) bound_ours=$((bound_ours+1));;
			hid-generic) bound_generic=$((bound_generic+1));;
			"") ;;
			*) bound_foreign=$((bound_foreign+1))
			   foreign_name="$(basename "$(readlink -f "$d/driver" 2>/dev/null)")";;
		esac
	done
	done
	if [ "$bound_foreign" -gt 0 ]; then
		bad "$bound_foreign wheel interface(s) claimed by the $foreign_name driver instead of ours (run: sudo ./tools/rebind-wheel.sh)"
	elif [ "$bound_ours" -gt 0 ] && [ "$bound_generic" -eq 0 ]; then
		ok "all $bound_ours wheel interfaces bound to our driver"
	elif [ "$bound_generic" -gt 0 ]; then
		bad "$bound_generic wheel interface(s) stuck on hid-generic (run: sudo ./tools/rebind-wheel.sh)"
	fi

	echo
	say "[3/7] Driver health"
	local W G
	W="$(find_wheel_sysfs)"
	G="$(find_g923_sysfs)"
	if [ -n "$W" ]; then
		ok "wheel_* sysfs present ($W)"
		local fw
		fw="$(cat "$W/wheel_firmware" 2>/dev/null | tr '\n' ' ')"
		[ -n "$fw" ] && ok "firmware: $fw" || wrn "wheel_firmware unreadable"
		ok "range=$(cat "$W/wheel_range" 2>/dev/null) strength=$(cat "$W/wheel_strength" 2>/dev/null)% mode=$(cat "$W/wheel_mode" 2>/dev/null)"
		# The G923's equivalent was reported here and this one was not,
		# which left direct-drive owners with no way to see whether the
		# 90-degree healing was on.
		case "$(cat "$W/wheel_range_restore" 2>/dev/null)" in
			1) ok "wheel_range_restore on (puts the range back if a game moves it)";;
			0) wrn "wheel_range_restore off - a game that collapses your rotation to 90 degrees will stay that way (echo 1 > $W/wheel_range_restore)";;
		esac
	fi
	if [ -n "$G" ]; then
		# A G923 has no wheel_* files at all. Reporting their absence as a
		# fault told owners their driver was not bound when it was. Checked
		# independently of the block above so a rig with both wheels gets
		# both reported rather than whichever was found first.
		ok "G923 classic sysfs present ($G)"
		local g_range g_restore
		g_range="$(cat "$G/range" 2>/dev/null)"
		[ -n "$g_range" ] && ok "range=$g_range" || wrn "range unreadable"
		g_restore="$(cat "$G/range_restore" 2>/dev/null)"
		case "$g_restore" in
			1) ok "range_restore on (puts the range back if a game moves it)";;
			0) wrn "range_restore off (echo 1 > $G/range_restore to re-enable)";;
		esac
	fi
	if [ -z "$W" ] && [ -z "$G" ]; then
		[ -n "$usbline" ] && bad "wheel on USB but no sysfs attributes - driver not bound (see [2])" \
			|| wrn "skipped (no wheel)"
	fi

	echo
	say "[4/7] Permissions (udev)"
	# Distro packages install rules under /usr/lib/udev/rules.d; setup.sh
	# uses /etc/udev/rules.d. Either location counts as installed.
	if [ -f "$UDEV_DST" ] || [ -f "/usr/lib/udev/rules.d/70-logitech-trueforce.rules" ]; then
		ok "udev rule installed"
	else
		wrn "udev rule missing - settings need sudo (run: sudo ./tools/setup.sh)"
	fi
	if [ -f "$UDEV_FFB_DST" ] || [ -f "/usr/lib/udev/rules.d/71-logi-ffb-uhid.rules" ]; then
		ok "logi-ffb uhid udev rule installed"
	else
		wrn "logi-ffb uhid udev rule missing - logi-ffb needs sudo for /dev/uhid (run: sudo ./tools/setup.sh)"
	fi
	# Only reported on a machine that has a G923. Printed unconditionally
	# these read as claims about the reader's hardware: an RS50 owner saw
	# "G923 (c266/c267/c26e) rebind rule installed" in a report about his
	# own wheel and reasonably concluded doctor had misidentified it (#54).
	if g923_on_usb; then
		if [ -f "$UDEV_G923_DST" ] || [ -f "/usr/lib/udev/rules.d/72-logitech-g923-rebind.rules" ]; then
			ok "G923 (c266/c267/c26e) rebind rule installed"
		else
			wrn "G923 rebind rule missing - the in-tree driver may keep winning the bind race on c266/c267/c26e (run: sudo ./tools/setup.sh)"
		fi
		if [ -f "$UDEV_G923_XBOX_DST" ] || [ -f "/usr/lib/udev/rules.d/73-logitech-g923-xbox-modeswitch.rules" ]; then
			ok "G923 Xbox edition (c26d) mode-switch rule installed"
		else
			wrn "G923 Xbox mode-switch rule missing - the Xbox edition will not switch out of console mode (run: sudo ./tools/setup.sh)"
		fi
		# Checked separately from the rule above: the rule dispatches this
		# through systemd-run with the output discarded, so a missing helper
		# leaves no trace anywhere and simply looks like a wheel that never
		# enumerates (issue #27). Inside this guard because its warning
		# refers to "the rule above", which is only printed here.
		if [ -x "$MODESWITCH_DST" ]; then
			ok "G923 Xbox mode-switch helper installed"
		else
			wrn "G923 Xbox mode-switch helper missing ($MODESWITCH_DST) - the rule above cannot do anything without it, and the Xbox edition will look like a dead wheel (run: sudo ./tools/setup.sh)"
		fi
	fi
	if [ -f "$MODPROBE_DST" ]; then
		ok "hid-logitech-dd modprobe.d config installed"
	else
		wrn "hid-logitech-dd modprobe.d config missing (run: sudo ./tools/setup.sh)"
	fi
	if [ -n "$W" ]; then
		if [ -w "$W/wheel_range" ] && [ -w "$W/range" ]; then
			ok "settings writable as $USER"
		else
			wrn "settings not writable as $USER - replug the wheel so the udev rule reapplies (it makes the wheel settings writable with no group setup)"
		fi
	fi

	echo
	say "[5/7] TrueForce SDK DLLs (only needed for TrueForce in Proton sims)"
	# Checked in the directory the installer resolves to, not in the repo.
	# This used to look only inside $REPO_ROOT/sdk, so every user who
	# installed from a package (no checkout, and the SDK lives under
	# ~/.local/share) was told the DLLs were missing however correctly they
	# had staged them, and the report gave no hint of where it had looked
	# (#54).
	local sdk_root dll_missing=0
	sdk_root="$(resolved_sdk_dir)"
	if [ -z "$sdk_root" ]; then
		# Two different failures used to print the same line. Saying
		# "cannot locate the shim installer" when it was located and
		# then failed is the sort of claim this whole review was about.
		if [ -n "$(find_shim_installer)" ]; then
			wrn "the shim installer ($(find_shim_installer)) could not report its SDK directory; run it directly with --print-sdk-dir to see why"
		else
			wrn "cannot locate the shim installer, so the SDK cannot be checked (expected tools/install-tf-shim.sh or logi-shim on PATH)"
		fi
	else
		for f in "Logi/Trueforce/*/trueforce_sdk_x64.dll" \
			 "Logi/Trueforce/*/trueforce_sdk_x86.dll" \
			 "Logi/wheel_sdk/*/logi_steering_wheel_x64.dll" \
			 "Logi/wheel_sdk/*/logi_steering_wheel_x86.dll"; do
			ls "$sdk_root"/$f >/dev/null 2>&1 || dll_missing=$((dll_missing+1))
		done
		if [ "$dll_missing" -eq 0 ]; then
			local tf_ver
			tf_ver="$(ls -1 "$sdk_root/Logi/Trueforce" 2>/dev/null | grep -E '^[0-9_]+$' | sort -V | tail -1)"
			ok "all four SDK DLLs staged${tf_ver:+ (Trueforce $tf_ver)} in $sdk_root"
		else
			wrn "$dll_missing of 4 SDK DLLs not staged in $sdk_root - copy G HUB's Logi folder there so it becomes $sdk_root/Logi/Trueforce/<version>/ (standard FFB works without them)"
		fi
	fi

	echo
	say "[6/7] Steam prefixes (shim)"
	local roots found_pfx=0 shimmed=0
	roots="$(steam_roots)"
	if [ -z "$roots" ]; then
		wrn "no Steam installation found for $USER"
	else
		# Only the sims that actually load Logitech's SDK need the shim.
		# Counting every Proton prefix meant a warning that scaled with a
		# person's library rather than with anything wrong: one real report
		# read "shim in 50 of 52 prefixes", which reads like a fault and is
		# in fact 50 shims more than that person needed. A missing shim in
		# some unrelated game's prefix is not a problem to solve.
		while IFS= read -r root; do
			for appid in $SDK_SIM_APPIDS; do
				pfx="$root/steamapps/compatdata/$appid/pfx"
				[ -d "$pfx" ] || continue
				found_pfx=$((found_pfx+1))
				ls "$pfx"/drive_c/Program\ Files/Logi/Trueforce/*/trueforce_sdk_x64.dll >/dev/null 2>&1 && shimmed=$((shimmed+1))
			done
		done <<< "$roots"
		if [ "$found_pfx" -gt 0 ] && [ "$shimmed" -eq "$found_pfx" ]; then
			ok "TrueForce shim present in all $found_pfx installed SDK sim(s)"
		elif [ "$shimmed" -gt 0 ]; then
			wrn "shim in $shimmed of $found_pfx installed SDK sim(s) (run: ./tools/setup.sh shim)"
		elif [ "$found_pfx" -gt 0 ]; then
			wrn "shim not installed in any of the $found_pfx installed SDK sim(s) (run: ./tools/setup.sh shim)"
		else
			# Say so rather than printing a bare heading. With no SDK
			# sim installed every branch above was skipped and the
			# section rendered as a title with nothing under it, which
			# reads like a check that failed to run (#54).
			ok "no SDK sims installed, so there is nothing to shim"
		fi

		# If the rotation shim is installed, Logitech's library has to be
		# beside it under the name its forwards resolve through. Without
		# that, the four calls the shim answers itself keep working and
		# the fifty-four it forwards do not, which is a wheel that steers
		# to full lock and produces no force at all (issue #27).
		local proxied=0 orphaned=0
		while IFS= read -r root; do
			for appid in $SDK_SIM_APPIDS; do
				local d; d=$(ls -d "$root/steamapps/compatdata/$appid/pfx/$TF_PFX_DIR_REL/"*/ 2>/dev/null | tail -1)
				[ -n "$d" ] && [ -f "$d/trueforce_sdk_x64.dll" ] || continue
				# -a: it is a binary, and without it grep declines to match.
				# The string only appears in our proxy (it is the forward
				# target); Logitech's own library has no mention of it.
				grep -aq "trueforce_real" "$d/trueforce_sdk_x64.dll" 2>/dev/null || continue
				proxied=$((proxied+1))
				[ -f "$d/trueforce_real.dll" ] || orphaned=$((orphaned+1))
			done
		done <<< "$roots"
		if [ "$orphaned" -gt 0 ]; then
			bad "rotation shim installed in $orphaned prefix(es) without Logitech's library beside it - those games get no force feedback (re-run: ./tools/install-tf-shim.sh --all-steam --range-proxy)"
		elif [ "$proxied" -gt 0 ]; then
			ok "rotation shim installed in $proxied SDK sim(s), with Logitech's library beside it"
		fi
	fi

	echo
	say "[7/7] Per-game launch options"
	local checked=0
	local appid
	# Each group gets the advice it actually needs. The SDK sims want
	# PROTON_ENABLE_HIDRAW=1, and only on a wheel that answers the SDK; the
	# DirectInput sims want it absent, because setting it there removes
	# force feedback. Titles that need neither are not listed at all.
	for appid in $SDK_SIM_APPIDS $DINPUT_SIM_APPIDS; do
		local installed=0 has_opt=0 has_wrapper=0 wants_hidraw=0 opts_line=""
		case " $SDK_SIM_APPIDS " in *" $appid "*) wants_hidraw=1;; esac
		while IFS= read -r root; do
			[ -d "$root/steamapps/compatdata/$appid" ] && installed=1
			for cfg in "$root"/userdata/*/config/localconfig.vdf; do
				[ -f "$cfg" ] || continue
				# Read LaunchOptions from the app's OWN block. Anchoring
				# on the first line mentioning the id anywhere was wrong
				# twice over: an appid appears several times in a
				# localconfig (six, in one real file), and if the block
				# it lands on has no LaunchOptions the scan runs on and
				# reports the NEXT app's. Measured against a real config
				# that got two of three wrong, both false negatives, so
				# it told owners to set a variable they had already set.
				opts_line=$(awk -v id="\"$appid\"" '
					$0 ~ "^[ \t]*" id "[ \t]*$" { cand = 1; depth = 0; seen = 0; next }
					cand {
						o = gsub(/\{/, "{"); c = gsub(/\}/, "}")
						if (o) seen = 1
						depth += o - c
						if (/"LaunchOptions"/) { print; exit }
						if (seen && depth <= 0) cand = 0
					}' "$cfg")
				case "$opts_line" in *PROTON_ENABLE_HIDRAW=1*) has_opt=1;; esac
				# The wrapper resolves the whole recipe per game and
				# wheel, so its presence satisfies this check outright
				# for every group (it sets the scoped hidraw form, which
				# the =1 grep above deliberately does not match).
				case "$opts_line" in *logi-launch*) has_wrapper=1;; esac
			done
		done <<< "$(steam_roots)"
		[ "$installed" -eq 1 ] || continue
		checked=$((checked+1))
		if [ "$has_wrapper" -eq 1 ]; then
			if [ "$has_opt" -eq 1 ] && [ "$wants_hidraw" -eq 0 ]; then
				bad "appid $appid: logi-launch is set but so is PROTON_ENABLE_HIDRAW=1, which overrides it and stops force feedback on a DirectInput sim - remove the variable, keep the wrapper"
			else
				ok "appid $appid launches through logi-launch (the wrapper works the recipe out per game and wheel)"
			fi
		elif [ "$wants_hidraw" -eq 1 ]; then
			if ! wheel_has_sdk_trueforce; then
				# The shim cannot reach this wheel, so the variable buys
				# nothing and costs the force feedback it already has.
				if [ "$has_opt" -eq 1 ]; then
					bad "appid $appid: PROTON_ENABLE_HIDRAW=1 is set, but this wheel has no SDK TrueForce - remove it, it is what is stopping force feedback"
				else
					ok "appid $appid correctly has no PROTON_ENABLE_HIDRAW (this wheel has no SDK TrueForce)"
				fi
			elif [ "$has_opt" -eq 1 ]; then
				ok "appid $appid has PROTON_ENABLE_HIDRAW=1 (by hand; 'logi-launch %command%' does this and the rest for you)"
			else
				wrn "appid $appid: launch options do not run logi-launch (set 'logi-launch %command%' in Steam > Properties; it enables TrueForce here)"
			fi
		elif [ "$has_opt" -eq 1 ]; then
			bad "appid $appid: PROTON_ENABLE_HIDRAW=1 is set on a DirectInput sim - it stops force feedback reaching the game; remove it and set 'logi-launch %command%' instead"
		else
			wrn "appid $appid: launch options do not run logi-launch (set 'logi-launch %command%' in Steam > Properties; this sim needs the logi-ffb proxy it starts)"
		fi
	done
	[ "$checked" -eq 0 ] && wrn "no known SDK or DirectInput sims found installed (nothing to check)"

	echo
	say "Summary: $pass pass, $warn warn, $fail fail"
	[ "$fail" -eq 0 ] || return 1
	return 0
}

# ----------------------------------------------------------------- setup --
# --- telemetry helpers ------------------------------------------------------
#
# Two files games load rather than the user running: the relay (a Windows
# executable, one copy per Proton prefix) and the truck-sim plugin (loaded
# from inside the game's own installation). Packages stage master copies in
# /usr/share/logitech-trueforce; a checkout has them in tools/ and the build
# output. Placing them is what stops a user hunting for anything beyond
# Logitech's own DLLs.

RELAY_BIN="logi-tf-relay.exe"
SCS_PLUGIN="liblogi_tf_scs.so"
# Keep in step with logi-wheel-core's telemetry_helpers module and
# docs/SCS_PLUGIN.md.
SCS_PLUGIN_DIR="bin/linux_x64/plugins"

# Print the master copy of $1, or nothing.
helper_source() {
	local name="$1" c
	for c in "/usr/share/logitech-trueforce/$name" \
		 "/usr/local/share/logitech-trueforce/$name" \
		 "$REPO_ROOT/tools/$name" \
		 "$REPO_ROOT/userspace/logi-wheel/target/release/$name"; do
		[ -f "$c" ] && { printf '%s\n' "$c"; return 0; }
	done
	return 1
}

# Copy $1 to $2 as the invoking user, reporting what happened.
place_helper() {
	local src="$1" dst="$2" what="$3" verb="installed"
	[ -f "$dst" ] && verb="updated"
	if [ "$EUID" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- mkdir -p "$(dirname "$dst")" || return 1
		runuser -u "$SUDO_USER" -- cp -f "$src" "$dst" || return 1
	else
		mkdir -p "$(dirname "$dst")" || return 1
		cp -f "$src" "$dst" || return 1
	fi
	echo "  $verb: $what"
}

# Put the relay in every Proton prefix and the plugin in every truck sim.
do_helpers() {
	local relay scs root pfx appid game manifest installdir dir n=0

	relay="$(helper_source "$RELAY_BIN" || true)"
	scs="$(helper_source "$SCS_PLUGIN" || true)"
	if [ -z "$relay" ] && [ -z "$scs" ]; then
		echo "  no telemetry helpers found to install (not packaged, not built)"
		return 0
	fi

	if [ -n "$relay" ]; then
		for root in $(steam_roots); do
			[ -d "$root/steamapps/compatdata" ] || continue
			for pfx in "$root"/steamapps/compatdata/*/pfx; do
				[ -d "$pfx" ] || continue
				appid="$(basename "$(dirname "$pfx")")"
				place_helper "$relay" "$pfx/drive_c/$RELAY_BIN" \
					"relay -> prefix $appid" && n=$((n+1))
			done
		done
	fi

	if [ -n "$scs" ]; then
		# Native Linux titles, so they have no Proton prefix and the loop
		# above cannot see them; they are looked up by appid instead.
		for root in $(steam_roots); do
			for appid in 227300 270880; do
				manifest="$root/steamapps/appmanifest_$appid.acf"
				[ -f "$manifest" ] || continue
				installdir="$(sed -nE 's/^[[:space:]]*"installdir"[[:space:]]+"(.*)"[[:space:]]*$/\1/p' "$manifest" | head -1)"
				[ -n "$installdir" ] || continue
				dir="$root/steamapps/common/$installdir"
				[ -d "$dir" ] || continue
				place_helper "$scs" "$dir/$SCS_PLUGIN_DIR/$SCS_PLUGIN" \
					"truck-sim plugin -> $installdir" && n=$((n+1))
			done
		done
	fi

	[ "$n" -eq 0 ] && echo "  nothing to install into (no Steam prefixes or truck sims found)"
	return 0
}

# Install the launch-time tool chain from this checkout: the logi-launch
# wrapper, the RPM bridge it starts, and the prebuilt Windows artifacts it
# stages into game folders and prefixes. Every distro package ships these;
# the from-source path did not, so a checkout install had a current driver
# and a launcher with nothing to stage (#60).
do_tools() {
	install -Dm 0755 "$REPO_ROOT/tools/logi-launch.sh" /usr/bin/logi-launch
	echo "  installed /usr/bin/logi-launch"
	if command -v cc >/dev/null 2>&1; then
		if cc -O2 -Wall -o /tmp/logi-rpm-bridge.$$ \
		   "$REPO_ROOT/tools/logi-rpm-bridge.c" 2>/dev/null; then
			install -Dm 0755 /tmp/logi-rpm-bridge.$$ /usr/bin/logi-rpm-bridge
			rm -f /tmp/logi-rpm-bridge.$$
			echo "  installed /usr/bin/logi-rpm-bridge"
		else
			echo "  logi-rpm-bridge build failed; the texture merge will have no RPM feed" >&2
		fi
	else
		echo "  no C compiler; skipped logi-rpm-bridge (the texture merge needs it)" >&2
	fi
	local f
	for f in dinput8-escape.dll tf-range-proxy.dll logi-tf-relay.exe tf-init.bin; do
		if [ -f "$REPO_ROOT/tools/$f" ]; then
			install -Dm 0644 "$REPO_ROOT/tools/$f" \
				"/usr/share/logitech-trueforce/$f"
			echo "  installed /usr/share/logitech-trueforce/$f"
		fi
	done
}

# Build and install the settings apps from this checkout.
#
# The from-source path installed the driver, the udev rules and the shell
# helpers, and then stopped. The apps were left to the reader, so a
# checkout install ended up with a current driver and whatever binaries the
# user had built by hand months earlier. `doctor` even reported their
# versions, which made the mismatch look deliberate. Every distro package
# has always installed them; only this path did not.
#
# Cargo runs as the invoking user: building as root leaves a root-owned
# target/ that the user's next plain `cargo build` cannot write, and roots
# ~/.cargo too.
do_apps() {
	local ws="$REPO_ROOT/userspace/logi-wheel"
	[ -d "$ws" ] || { echo "  no userspace workspace here; skipping"; return 0; }

	local cargo_bin=""
	if [ -n "${SUDO_USER:-}" ]; then
		cargo_bin=$(runuser -u "$SUDO_USER" -- sh -lc 'command -v cargo' 2>/dev/null || true)
	else
		cargo_bin=$(command -v cargo 2>/dev/null || true)
	fi
	if [ -z "$cargo_bin" ]; then
		echo "  cargo not found, so the apps cannot be built here."
		echo "  Install Rust (https://rustup.rs, or your distro's rust package) and re-run,"
		echo "  or install the packaged apps for your distribution instead."
		return 0
	fi

	# The terminal app, the FFB proxy and the simulated-TrueForce daemon.
	# Built as one invocation so cargo shares the work.
	echo "  building logi-wheel, logi-ffb, logi-tf-sim (this takes a few minutes)"
	# -p takes PACKAGE names, and the terminal app's package is
	# logi-wheel-tui; logi-wheel is the binary it produces.
	local build='cd "$1" && cargo build --release -p logi-wheel-tui -p logi-ffb -p logi-tf-sim'
	if [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- sh -lc "$build" _ "$ws" || {
			echo "  build failed; leaving any existing apps alone" >&2
			return 0
		}
	else
		sh -lc "$build" _ "$ws" || { echo "  build failed" >&2; return 0; }
	fi
	local bin
	for bin in logi-wheel logi-ffb logi-tf-sim; do
		if [ -x "$ws/target/release/$bin" ]; then
			install -Dm 0755 "$ws/target/release/$bin" "/usr/bin/$bin"
			echo "  installed /usr/bin/$bin"
		fi
	done

	# The window is optional: it needs fontconfig headers and a working
	# graphics stack, and a headless rig has no use for it. A failure here
	# must not fail the install, because the terminal app is the one every
	# other step assumes.
	echo "  building logi-wheel-gui (optional; needs fontconfig headers)"
	local gbuild='cd "$1" && cargo build --release -p logi-wheel-gui'
	local built_gui=1
	if [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- sh -lc "$gbuild" _ "$ws" || built_gui=0
	else
		sh -lc "$gbuild" _ "$ws" || built_gui=0
	fi
	if [ "$built_gui" -eq 1 ] && [ -x "$ws/target/release/logi-wheel-gui" ]; then
		install -Dm 0755 "$ws/target/release/logi-wheel-gui" /usr/bin/logi-wheel-gui
		echo "  installed /usr/bin/logi-wheel-gui"
	else
		echo "  skipped the window (install fontconfig's headers to get it:"
		echo "  libfontconfig-dev on Debian/Ubuntu, fontconfig-devel on Fedora, fontconfig on Arch)"
	fi
}

do_shim() {
	if [ "$EUID" -eq 0 ]; then
		if [ -n "${SUDO_USER:-}" ]; then
			runuser -u "$SUDO_USER" -- "$(find_shim_installer)" --all-steam
		else
			echo "shim must run as the user owning the Steam prefixes; run: ./tools/setup.sh shim (no sudo)"
			return 1
		fi
	else
		"$(find_shim_installer)" --all-steam
	fi
}

setup() {
	if [ "$EUID" -ne 0 ]; then
		echo "error: full setup needs root (sudo $0). For diagnosis only: $0 doctor" >&2
		exit 1
	fi
	# The install path builds from source and needs the sibling scripts a
	# checkout has. A distro package ships this file without them, and its
	# driver is already installed, so say that rather than failing on a
	# missing path several steps in.
	if [ ! -x "$REPO_ROOT/tools/dkms-update.sh" ]; then
		echo "error: full setup runs from a git checkout; this looks like a packaged install." >&2
		echo "       The driver is already installed by your package manager. Use: $0 doctor" >&2
		exit 1
	fi

	say "[1/8] Kernel module (DKMS) + udev rule"
	"$REPO_ROOT/tools/dkms-update.sh" || exit 1

	say "[2/8] Migrating off any old full-fork install"
	# The old build shipped its module as hid-logitech-hidpp - the SAME
	# name as the in-tree driver - so DKMS DISPLACED the genuine in-tree
	# module (backing it up under .../original_module/) and the installer
	# blacklisted it. This scoped build ships as hid-logitech-dd and claims
	# only the wheels, so fully undo the old state: drop the blacklist,
	# remove the old DKMS package, RESTORE the displaced in-tree module, and
	# delete the fork's leftover .ko. Skipping the restore would leave the
	# stale fork as the only hid-logitech-hidpp on disk, so mice/keyboards
	# would keep loading it instead of the maintained in-tree driver.
	local migrated=0 dkms_base=/var/lib/dkms/hid-logitech-hidpp
	if [ -f "$OLD_BLACKLIST_FILE" ]; then
		rm -f "$OLD_BLACKLIST_FILE"
		echo "  removed stale blacklist $OLD_BLACKLIST_FILE"
		migrated=1
	fi
	if dkms status 2>/dev/null | grep -q '^hid-logitech-hidpp' \
	   || [ -d "$dkms_base" ] \
	   || ls /usr/lib/modules/*/updates/dkms/hid-logitech-hidpp.ko* >/dev/null 2>&1; then
		# Best-effort clean removal (restores the original when the source
		# is still intact); tolerate an already-broken state.
		dkms remove -m hid-logitech-hidpp -v 1.0 --all >/dev/null 2>&1 || true
		# Restore any displaced in-tree module from DKMS's own backup.
		if [ -d "$dkms_base/original_module" ]; then
			local kdir k om dst
			for kdir in "$dkms_base"/original_module/*/; do
				[ -d "$kdir" ] || continue
				k=$(basename "$kdir")
				om=$(ls "$kdir"*/hid-logitech-hidpp.ko* 2>/dev/null | head -1)
				dst=/usr/lib/modules/$k/kernel/drivers/hid
				if [ -n "$om" ] && [ -d "$dst" ]; then
					cp -f "$om" "$dst/"
					echo "  restored in-tree hid-logitech-hidpp for $k"
				fi
			done
		fi
		# Drop the fork's installed module and DKMS state for good.
		rm -f /usr/lib/modules/*/updates/dkms/hid-logitech-hidpp.ko* 2>/dev/null || true
		rm -rf "$dkms_base" /usr/src/hid-logitech-hidpp-*
		echo "  removed old full-fork DKMS package hid-logitech-hidpp"
		migrated=1
	fi
	modprobe -r hid-logitech-hidpp 2>/dev/null || true
	if [ "$migrated" -eq 1 ]; then
		depmod -a
		if modprobe -n hid-logitech-hidpp >/dev/null 2>&1; then
			echo "  in-tree hid-logitech-hidpp restored for your other Logitech devices"
		else
			wrn "in-tree hid-logitech-hidpp missing after migration - reinstall your kernel package (e.g. sudo pacman -S linux) to restore it for non-wheel Logitech devices"
		fi
	else
		echo "  nothing to migrate (clean install)"
	fi

	say "[3/8] Loading the module"
	modprobe -r hid-logitech-dd 2>/dev/null || true
	if modprobe hid-logitech-dd; then
		echo "  loaded"
	else
		echo "  modprobe failed - check dmesg" >&2
	fi
	# claim the wheel if it is currently sitting on hid-generic
	"$REPO_ROOT/tools/rebind-wheel.sh" >/dev/null 2>&1 || true

	say "[4/8] Launch tools (logi-launch + helpers)"
	do_tools || true

	say "[5/8] Settings apps"
	do_apps || true

	say "[6/8] TrueForce shim (Steam prefixes)"
	if ls "$(resolved_sdk_dir)"/Logi/Trueforce/*/trueforce_sdk_x64.dll >/dev/null 2>&1; then
		do_shim || true
	else
		echo "  SDK DLLs not staged - skipped (standard FFB works without them;"
		echo "  see the wiki's Force-feedback-in-games page for TrueForce)"
	fi

	say "[7/8] Telemetry helpers (relay + truck-sim plugin)"
	do_helpers || true

	say "[8/8] Doctor"
	# diagnosis runs best as the real user (permission checks)
	if [ -n "${SUDO_USER:-}" ]; then
		runuser -u "$SUDO_USER" -- "$(readlink -f "$0")" doctor || true
	else
		doctor || true
	fi

	echo
	say "Remaining manual steps (per game, in Steam):"
	# One launch option for every game and wheel: the wrapper resolves the
	# per-game plan itself (raw HID only where this wheel wants it, the
	# logi-ffb proxy for DirectInput sims, telemetry helpers where needed).
	# The old advice printed hand-set variables here, and told to the wrong
	# owner they remove force feedback (#54); the wrapper cannot make that
	# mistake because it asks the wheel.
	echo "  1. Properties > Launch Options:  logi-launch %command%"
	echo "     (same line for every game; it works the recipe out per game"
	echo "     and per wheel. Hand-tuning instead: docs/GAME_SETUP.md.)"
	echo "  2. Properties > Controller:     Disable Steam Input"
	echo "  (per-game recipes: docs/GAME_SETUP.md, or the app's Setup page)"
}

case "${1:-setup}" in
	doctor) doctor ;;
	shim)   do_shim ;;
	setup)  setup ;;
	*) echo "usage: sudo $0 [setup] | $0 doctor | $0 shim" >&2; exit 2 ;;
esac
