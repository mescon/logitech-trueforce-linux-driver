#!/usr/bin/env bash
#
# Install Logitech's real, Authenticode-signed SDK DLLs into Proton wine
# prefixes so sims that use TrueForce / the Wheel SDK find them via CLSID
# lookup. Running the real Logitech DLLs unmodified means no DLL injection,
# no cert bypass, no IAT hooks - anti-cheat has nothing to flag. The DLLs
# talk to the wheel via Wine's HID stack which reaches our kernel driver.
#
# What this does, per target prefix:
#   1. Install the Logitech DLLs under the exact Windows paths they use
#        <prefix>/drive_c/Program Files/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
#        <prefix>/drive_c/Program Files/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
#      plus 32-bit variants.
#   2. Register the two known CLSIDs by editing system.reg directly:
#        HKLM\SOFTWARE\Classes\CLSID\{e8dfb59f-...}   -> default = TF DLL path
#        HKLM\SOFTWARE\Classes\CLSID\{63bd165d-...}   -> ServerBinary subkey
#                                                        points at Wheel SDK DLL
#   3. Games load the DLLs, pass all cert checks natively (Logitech-signed),
#      call into the real SDK, which uses standard Windows HID APIs that
#      Wine translates to /dev/hidrawN on our kernel driver.
#
# Usage:
#   ./tools/install-tf-shim.sh --all-steam              Install in every Steam prefix
#   ./tools/install-tf-shim.sh --prefix <path>          Install in one prefix
#   ./tools/install-tf-shim.sh --uninstall              Remove from all Steam prefixes
#
# Run as the user that owns the wine prefix (do NOT sudo). Idempotent.

set -euo pipefail

# Both known Logitech SDK CLSIDs, extracted from the DLLs' DllRegisterServer.
TF_CLSID='{e8dfb59f-141f-40e4-8dd4-5526ead25a4c}'
WHEEL_CLSID='{63bd165d-1584-4e75-ab56-08330350545f}'

# Where in drive_c we install the DLLs. Mirrors Logitech's Windows layout
# byte-for-byte because some sims key off the path string; keep it stable.
TF_PFX_DIR='drive_c/Program Files/Logi/Trueforce/1_3_11'
WHEEL_PFX_DIR='drive_c/Program Files/Logi/wheel_sdk/9_1_0'

TF_WINE_PATH='C:\\Program Files\\Logi\\Trueforce\\1_3_11\\trueforce_sdk_x64.dll'
WHEEL_WINE_PATH='C:\\Program Files\\Logi\\wheel_sdk\\9_1_0\\logi_steering_wheel_x64.dll'

# Directory holding your own copies of Logitech's signed SDK DLLs, laid
# out the same way Logitech ships them on Windows (a "Logi/..." subtree).
# We never redistribute these; you supply them once. The directory is
# resolved (highest precedence first) by resolve_sdk_dir():
#   1. --sdk-dir <path>                      (explicit, this run)
#   2. $LOGITECH_TRUEFORCE_SDK_DIR           (environment)
#   3. repo sdk/ next to this script         (in-tree checkout)
#   4. $XDG_DATA_HOME/logitech-trueforce/sdk (default; ~/.local/share/...)
# so the same script works from a git checkout and from an AUR/system
# install where there is no repo tree.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK_DIR_OVERRIDE=""
SDK_DIR=""

default_sdk_dir() {
	echo "${XDG_DATA_HOME:-$HOME/.local/share}/logitech-trueforce/sdk"
}

# Relative path of the marker DLL used to detect a populated SDK tree.
SDK_MARKER='Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll'

resolve_sdk_dir() {
	if [ -n "$SDK_DIR_OVERRIDE" ]; then
		SDK_DIR="$SDK_DIR_OVERRIDE"
	elif [ -n "${LOGITECH_TRUEFORCE_SDK_DIR:-}" ]; then
		SDK_DIR="$LOGITECH_TRUEFORCE_SDK_DIR"
	elif [ -e "$REPO_ROOT/sdk/$SDK_MARKER" ]; then
		SDK_DIR="$REPO_ROOT/sdk"
	else
		SDK_DIR="$(default_sdk_dir)"
	fi
	SRC_TF_X64="$SDK_DIR/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll"
	SRC_TF_X86="$SDK_DIR/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll"
	SRC_WHEEL_X64="$SDK_DIR/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll"
	SRC_WHEEL_X86="$SDK_DIR/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll"
}

usage() {
	cat <<EOF
Usage:
  $0 --all-steam               Install into every Steam wine prefix under ~/.local/share/Steam
  $0 --prefix <path>           Install into a single wine prefix (the .../pfx directory)
  $0 --uninstall               Remove from all Steam prefixes

Options:
  --sdk-dir <path>             Directory holding your Logitech SDK DLLs
                               (default: \$LOGITECH_TRUEFORCE_SDK_DIR, the repo
                               sdk/ tree, or $(default_sdk_dir))
EOF
	exit 1
}

require_sources() {
	local missing=0
	for f in "$SRC_TF_X64" "$SRC_TF_X86" "$SRC_WHEEL_X64" "$SRC_WHEEL_X86"; do
		if [ ! -f "$f" ]; then
			echo "error: missing $f" >&2
			missing=1
		fi
	done
	if [ $missing -ne 0 ]; then
		cat >&2 <<EOF

The Logitech SDK DLLs were not found under:
  $SDK_DIR

They ship with Logitech G HUB on Windows and we do not redistribute them;
you supply them once. Place these four files (Logitech's own Windows
layout) under that directory:

  \$SDK/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll
  \$SDK/Logi/Trueforce/1_3_11/trueforce_sdk_x86.dll
  \$SDK/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x64.dll
  \$SDK/Logi/wheel_sdk/9_1_0/logi_steering_wheel_x86.dll

To get them: on a Windows machine with G HUB, copy C:\Program Files\Logi\
Trueforce\1_3_11\ and C:\Program Files\Logi\wheel_sdk\9_1_0\ into the tree
above; or install G HUB in a throwaway wine prefix and copy from there.

Point elsewhere with --sdk-dir <path> or \$LOGITECH_TRUEFORCE_SDK_DIR.
EOF
		exit 2
	fi
}

install_in_prefix() {
	local prefix="$1"
	local sys_reg="$prefix/system.reg"

	if [ ! -f "$sys_reg" ]; then
		echo "  skip $prefix (no system.reg)" >&2
		return 0
	fi

	# 1) Drop the real DLLs under drive_c, preserving Logitech's Windows layout.
	local tf_dir="$prefix/$TF_PFX_DIR"
	local wheel_dir="$prefix/$WHEEL_PFX_DIR"
	mkdir -p "$tf_dir" "$wheel_dir"
	install -m 0644 "$SRC_TF_X64" "$tf_dir/trueforce_sdk_x64.dll"
	install -m 0644 "$SRC_TF_X86" "$tf_dir/trueforce_sdk_x86.dll"
	install -m 0644 "$SRC_WHEEL_X64" "$wheel_dir/logi_steering_wheel_x64.dll"
	install -m 0644 "$SRC_WHEEL_X86" "$wheel_dir/logi_steering_wheel_x86.dll"

	# 2) Register both CLSIDs. Wine's system.reg is a plain text file; we
	#    edit it directly rather than launching the prefix's wine binary
	#    (which may be Proton's and inconvenient to invoke from here).
	python3 - "$sys_reg" "$TF_CLSID" "$TF_WINE_PATH" "$WHEEL_CLSID" "$WHEEL_WINE_PATH" <<'PY'
import os, sys, time

reg_path, tf_clsid, tf_path, wheel_clsid, wheel_path = sys.argv[1:6]

# TF SDK registration: default value of the CLSID key holds the DLL path.
tf_key = f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}]"

# Wheel SDK registration: CLSID key default holds a friendly name, and a
# \\ServerBinary sub-key default holds the DLL path. Matches the layout
# DllRegisterServer creates inside the real wheel SDK (extracted from
# logi_steering_wheel_x64.dll @ DllRegisterServer).
wheel_key = f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}]"
wheel_sb_key = f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}\\\\ServerBinary]"

blocks_to_replace = {tf_key, wheel_key, wheel_sb_key}

with open(reg_path) as f:
    lines = f.readlines()

out = []
skip = False
for line in lines:
    matched = False
    for k in blocks_to_replace:
        if line.startswith(k):
            skip = True
            matched = True
            break
    if matched:
        continue
    if skip:
        if line.strip() == "":
            skip = False
        continue
    out.append(line)

if out and not out[-1].endswith("\n"):
    out[-1] += "\n"
if out and out[-1].strip() != "":
    out.append("\n")

ts = int(time.time())

# TF SDK
out.append(f"{tf_key} {ts}\n")
out.append(f'@="{tf_path}"\n')
out.append("\n")

# Wheel SDK - friendly name at top, path under ServerBinary
out.append(f"{wheel_key} {ts}\n")
out.append('@="Logitech GHUB Legacy Steering Wheel SDK"\n')
out.append("\n")
out.append(f"{wheel_sb_key} {ts}\n")
out.append(f'@="{wheel_path}"\n')
out.append("\n")

tmp = reg_path + ".new"
with open(tmp, "w") as f:
    f.writelines(out)
os.replace(tmp, reg_path)
PY
	echo "  installed $prefix"
}

uninstall_in_prefix() {
	local prefix="$1"
	local sys_reg="$prefix/system.reg"
	# Remove our DLL drops (careful: respect the Logitech dir layout users
	# may have populated with real G HUB files outside of our installer).
	# We only remove our installed files, not the parent dirs if empty.
	for f in \
		"$prefix/$TF_PFX_DIR/trueforce_sdk_x64.dll" \
		"$prefix/$TF_PFX_DIR/trueforce_sdk_x86.dll" \
		"$prefix/$WHEEL_PFX_DIR/logi_steering_wheel_x64.dll" \
		"$prefix/$WHEEL_PFX_DIR/logi_steering_wheel_x86.dll"; do
		rm -f "$f"
	done
	# Also clean our old shim path (older versions installed there)
	[ -d "$prefix/drive_c/logi-tf-shim" ] && rm -rf "$prefix/drive_c/logi-tf-shim"

	[ -f "$sys_reg" ] || return 0
	python3 - "$sys_reg" "$TF_CLSID" "$WHEEL_CLSID" <<'PY'
import os, sys
reg_path, tf_clsid, wheel_clsid = sys.argv[1:4]

keys = [
    f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}]",
    f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}]",
    f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}\\\\ServerBinary]",
]

with open(reg_path) as f: lines = f.readlines()
out = []; skip = False
for line in lines:
    if any(line.startswith(k) for k in keys):
        skip = True; continue
    if skip:
        if line.strip() == "":
            skip = False
        continue
    out.append(line)
tmp = reg_path + ".new"
with open(tmp, "w") as f: f.writelines(out)
os.replace(tmp, reg_path)
PY
	echo "  uninstalled $prefix"
}

steam_prefixes() {
	# Standard Steam install (Arch, Fedora, most distros).
	echo "$HOME"/.local/share/Steam/steamapps/compatdata/*/pfx
	# Debian's steam-installer package keeps prefixes here instead
	# (issue #18, reported by @matthiasvegh).
	echo "$HOME"/.steam/debian-installation/steamapps/compatdata/*/pfx
}

# Parse flags in any order: a mode (--all-steam / --prefix / --uninstall)
# plus the optional --sdk-dir override.
MODE=""
PREFIX_ARG=""
while [ $# -gt 0 ]; do
	case "$1" in
	--all-steam|--uninstall)
		MODE="$1"
		;;
	--prefix)
		MODE="--prefix"
		PREFIX_ARG="${2:-}"
		[ -n "$PREFIX_ARG" ] || usage
		shift
		;;
	--sdk-dir)
		SDK_DIR_OVERRIDE="${2:-}"
		[ -n "$SDK_DIR_OVERRIDE" ] || usage
		shift
		;;
	-h|--help)
		usage
		;;
	*)
		echo "unknown argument: $1" >&2
		usage
		;;
	esac
	shift
done

resolve_sdk_dir

case "$MODE" in
--all-steam)
	require_sources
	count=0
	for pfx in $(steam_prefixes); do
		[ -d "$pfx" ] || continue
		install_in_prefix "$pfx"
		count=$((count+1))
	done
	echo "installed in $count Steam prefix(es)"
	;;
--prefix)
	require_sources
	install_in_prefix "$PREFIX_ARG"
	;;
--uninstall)
	for pfx in $(steam_prefixes); do
		[ -d "$pfx" ] && uninstall_in_prefix "$pfx"
	done
	;;
*) usage ;;
esac
