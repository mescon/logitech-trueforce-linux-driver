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
#   ./tools/install-tf-shim.sh --uninstall-prefix <path>  Remove from one prefix
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

# The Windows path the registry advertises is derived from the directory
# above rather than written out a second time. Keeping one copy is not
# tidiness: the two were separate strings until 0.34.1, in bash quoting that
# halves every backslash, and the registered path silently named a file that
# did not exist. Escaping for the .reg format happens once, in the writer.

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
SDK_MARKER='Logi/Trueforce/*/trueforce_sdk_x64.dll'

# G HUB revises these SDKs, and the version is a directory name. Hardcoding
# 1_3_11 and 9_1_0 meant a current G HUB install (1_3_12, 9_1_1) was simply
# not seen, with no hint as to why (issue #54). Discover whatever is there
# and prefer the newest.
newest_sdk_version() {
	# $1 = parent directory holding version-named subdirectories.
	#
	# Succeeds with empty output when there is nothing to report. "No
	# versions installed" is a legitimate answer to "which is newest", not
	# an error, and under `set -e` returning non-zero here killed the whole
	# script at the assignment below before its own fallback could run: a
	# user who had not staged the SDK yet got no install and no message.
	#
	# That first fix covered only the missing-directory case. A directory
	# that exists but holds no version-named entry made grep exit 1,
	# pipefail carried it out of the pipeline, and set -e killed the script
	# at the assignment before the caller's own default could apply. The
	# way to reach that state is to copy G HUB's Logi folder but drop the
	# DLLs straight into Logi/Trueforce/ instead of a version directory,
	# which is a mistake the missing-files message exists to correct and
	# which instead produced no output at all.
	#
	# So: no answer is an answer. Every path here succeeds, and an empty
	# result means "nothing installed", never "something went wrong".
	[ -d "$1" ] || return 0
	ls -1 "$1" 2>/dev/null | grep -E '^[0-9]+(_[0-9]+)*$' | sort -V | tail -1 || true
}

resolve_sdk_dir() {
	if [ -n "$SDK_DIR_OVERRIDE" ]; then
		SDK_DIR="$SDK_DIR_OVERRIDE"
	elif [ -n "${LOGITECH_TRUEFORCE_SDK_DIR:-}" ]; then
		SDK_DIR="$LOGITECH_TRUEFORCE_SDK_DIR"
	# Globbed with ls, not `[ -e ]`: the marker carries a `*` for the
	# version directory and `[ -e ]` compares it literally, so this branch
	# never matched and a populated checkout was silently passed over.
	elif ls "$REPO_ROOT/sdk/"$SDK_MARKER >/dev/null 2>&1; then
		SDK_DIR="$REPO_ROOT/sdk"
	else
		SDK_DIR="$(default_sdk_dir)"
		# Create the drop directory so the "place the DLLs here" message
		# below always points at a path that exists.
		mkdir -p "$SDK_DIR" 2>/dev/null || true
	fi
	TF_VER="$(newest_sdk_version "$SDK_DIR/Logi/Trueforce")"
	WHEEL_VER="$(newest_sdk_version "$SDK_DIR/Logi/wheel_sdk")"
	: "${TF_VER:=1_3_11}"
	: "${WHEEL_VER:=9_1_0}"
	SRC_TF_X64="$SDK_DIR/Logi/Trueforce/$TF_VER/trueforce_sdk_x64.dll"
	SRC_TF_X86="$SDK_DIR/Logi/Trueforce/$TF_VER/trueforce_sdk_x86.dll"
	SRC_WHEEL_X64="$SDK_DIR/Logi/wheel_sdk/$WHEEL_VER/logi_steering_wheel_x64.dll"
	SRC_WHEEL_X86="$SDK_DIR/Logi/wheel_sdk/$WHEEL_VER/logi_steering_wheel_x86.dll"
	# The prefix keeps the same version directory the files came from, so a
	# game reading the registered path finds the version it was given.
	TF_PFX_DIR="drive_c/Program Files/Logi/Trueforce/$TF_VER"
	WHEEL_PFX_DIR="drive_c/Program Files/Logi/wheel_sdk/$WHEEL_VER"
	# No Windows paths here: the writer derives them from the two directories
	# above, so a version change cannot leave the registry pointing at the
	# directory we no longer install into.

	# G HUB's DirectInput force-feedback driver, which lives beside the SDK
	# in a "Logitech" tree rather than the "Logi" one. This is the piece
	# Windows routes force-feedback effects and Escape through, and the
	# piece Wine has no equivalent for; see docs/DINPUT_ESCAPE.md.
	OEM_FFB_DIR="$SDK_DIR/Logitech/Direct Input Force Feedback"
	OEM_VER="$(newest_sdk_version "$OEM_FFB_DIR")"
	: "${OEM_VER:=1_1_13}"
	SRC_OEM_X64="$OEM_FFB_DIR/$OEM_VER/hidpp_forcefeedback_x64.dll"
	OEM_PFX_DIR="drive_c/Program Files/Logitech/Direct Input Force Feedback/$OEM_VER"
}

RANGE_PROXY=${RANGE_PROXY:-0}

# Install G HUB's DirectInput force-feedback driver too. Off by default: it
# is unproven on hardware and puts a third-party driver in the path of every
# effect, so it is asked for rather than assumed.
OEM_FFB=${OEM_FFB:-0}

# The CLSIDs hidpp_forcefeedback_x64.dll registers for itself, read back from
# its own DllRegisterServer under Wine rather than guessed.
OEM_FF_DEVICE_CLSID='{62B43F0E-E7DB-4329-8C13-A966D84A289F}'
OEM_FF_API_CLSID='{88D042C8-EAC5-4F86-85D1-F4446AAFE1D4}'

# The wheels that driver claims. An RS50 in native mode (c276) is not among
# them and calling into the driver for one crashes it, so the proxy checks
# this same list before binding.
OEM_FF_PIDS='C262 C268 C26E C272'

# Stage and register the OEM force-feedback driver into one prefix.
install_oem_ffb() {
	local prefix="$1"
	local sys_reg="$prefix/system.reg"

	if [ ! -f "$SRC_OEM_X64" ]; then
		echo "  no DirectInput force-feedback driver found at:" >&2
		echo "    $SRC_OEM_X64" >&2
		echo "  Copy the whole 'Logitech' folder from a Windows G HUB install" >&2
		echo "  next to the 'Logi' one, keeping its layout." >&2
		return 1
	fi

	install -d "$prefix/$OEM_PFX_DIR"
	install -m 0644 "$SRC_OEM_X64" "$prefix/$OEM_PFX_DIR/hidpp_forcefeedback_x64.dll"

	python3 - "$sys_reg" "$OEM_PFX_DIR" "$OEM_FF_DEVICE_CLSID" "$OEM_FF_API_CLSID" \
		"$OEM_FF_PIDS" <<'PY'
import os, sys, time

reg_path, pfx_dir, dev_clsid, api_clsid, pids = sys.argv[1:6]


def wine_path(pfx, dll):
    if not pfx.startswith("drive_c/"):
        sys.exit(f"install path is not under drive_c: {pfx}")
    return "C:\\" + pfx[len("drive_c/"):].replace("/", "\\") + "\\" + dll


def reg_value(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


dll = wine_path(pfx_dir, "hidpp_forcefeedback_x64.dll")

# Wine keeps HKLM\System\CurrentControlSet as ControlSet001 on disk, and a
# literal "CurrentControlSet" key written here would not alias to it.
oem_base = "System\\\\ControlSet001\\\\Control\\\\MediaProperties\\\\" \
           "PrivateProperties\\\\Joystick\\\\OEM"

keys = []
for clsid, name in ((dev_clsid, "Logitech HID++ Force Feedback Device"),
                    (api_clsid, "Logitech HID++ Force Feedback API")):
    keys.append((f"[Software\\\\Classes\\\\CLSID\\\\{clsid}]", [f'@="{name}"']))
    keys.append((f"[Software\\\\Classes\\\\CLSID\\\\{clsid}\\\\InProcServer32]",
                 [f'@="{reg_value(dll)}"', '"ThreadingModel"="Both"']))

# The per-device hookup. On Windows this is written by G HUB's installer,
# not by DllRegisterServer, which is why registering the DLL alone is not
# enough.
for pid in pids.split():
    dev = f"VID_046D&PID_{pid}"
    keys.append((f"[{oem_base}\\\\{dev}]", ['"OEMName"="Logitech Wheel"']))
    keys.append((f"[{oem_base}\\\\{dev}\\\\OEMForceFeedback]",
                 [f'@="{dev_clsid}"', f'"CLSID"="{dev_clsid}"']))

with open(reg_path) as f:
    lines = f.readlines()

# Drop any previous copy of each block, then append a fresh one.
starts = {k for k, _ in keys}
out, skip = [], False
for line in lines:
    if any(line.startswith(k) for k in starts):
        skip = True
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
for key, values in keys:
    out.append(f"{key} {ts}\n")
    out.extend(v + "\n" for v in values)
    out.append("\n")

tmp = reg_path + ".new"
with open(tmp, "w") as f:
    f.writelines(out)
os.replace(tmp, reg_path)
PY

	verify_registered_dll "$sys_reg" "$OEM_FF_DEVICE_CLSID" "$prefix" InProcServer32 || return 1
	echo "  OEM force-feedback driver installed ($OEM_VER)"
}

usage() {
	cat <<EOF
Usage:
  $0 --print-sdk-dir           Print the resolved SDK directory and versions, then exit
  $0 --all-steam               Install into every Steam wine prefix, in every Steam
                               library (including libraries on other drives)
  $0 --prefix <path>           Install into a single wine prefix (the .../pfx directory)
  $0 --uninstall               Remove from all Steam prefixes
  $0 --uninstall-prefix <path> Remove from a single wine prefix (the .../pfx directory)

Options:
  --sdk-dir <path>             Directory holding your Logitech SDK DLLs
                               (default: \$LOGITECH_TRUEFORCE_SDK_DIR, the repo
                               sdk/ tree, or $(default_sdk_dir))
  --proxy                      Also install this project's SDK proxy. It does
                               two things: answers the rotation question so a
                               game stops clamping the wheel to 90 degrees
                               (#27), and carries the game's own TrueForce to
                               a wheel Logitech's SDK will not drive, which is
                               how a G923 gets TrueForce in ACC and AC EVO.
                               Spelled --range-proxy before it did the second
                               job; both names still work.
  --oem-ffb                    Also install G HUB's DirectInput force-feedback
                               driver (hidpp_forcefeedback), from a "Logitech"
                               folder beside the "Logi" one. Windows routes
                               force-feedback effects and TrueForce's Escape
                               calls through it; Wine loads no such driver, so
                               this only helps together with the dinput8 proxy
                               that does the routing. Unproven on hardware.
                               See docs/DINPUT_ESCAPE.md.
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
	# A prefix that already carries the rotation proxy keeps it. Without
	# this, the install below overwrites trueforce_sdk_x64.dll, which on a
	# proxied prefix IS the proxy, with Logitech's stock library: a plain
	# re-run silently undid a fix the user had deliberately applied. The
	# way people met that is `sudo ./tools/setup.sh` after a kernel update,
	# from a script whose own header calls itself idempotent, and the only
	# symptom is the 90-degree steering clamp quietly coming back.
	#
	# Detected by the forward target sitting beside it, which is the same
	# evidence `setup.sh doctor` uses.
	# Scoped to this prefix, deliberately: mutating the global would make
	# one proxied prefix turn the proxy on for every prefix processed after
	# it, which is a worse bug than the one being fixed.
	local want_proxy="$RANGE_PROXY"
	if [ -f "$tf_dir/trueforce_real.dll" ] && [ "$want_proxy" != "1" ]; then
		want_proxy=1
		echo "  keeping the rotation shim already installed in $prefix"
	fi
	install -m 0644 "$SRC_TF_X64" "$tf_dir/trueforce_sdk_x64.dll"
	if [ "$want_proxy" = "1" ]; then
		# Logitech's library keeps working and keeps every call that
		# matters; it just moves aside so ours can answer the one
		# question it cannot answer here. Ours forwards the rest to it
		# by name, which is why the name it moves to is fixed.
		# Checked-out tree first, then the packaged location. Resolving
		# it only under $REPO_ROOT meant /usr/tools/ for a packaged
		# logi-shim, so the documented fix for the 90-degree clamp told
		# every package user to run `make -C tools` in a directory they
		# do not have, with a cross-compiler they have no reason to own.
		local proxy=""
		local cand
		for cand in "$REPO_ROOT/tools/tf-range-proxy.dll" \
			    "/usr/share/logitech-trueforce/tf-range-proxy.dll" \
			    "/usr/local/share/logitech-trueforce/tf-range-proxy.dll"; do
			[ -f "$cand" ] && { proxy="$cand"; break; }
		done
		if [ -f "$proxy" ]; then
			mv -f "$tf_dir/trueforce_sdk_x64.dll" "$tf_dir/trueforce_real.dll"
			install -m 0644 "$proxy" "$tf_dir/trueforce_sdk_x64.dll"
			echo "  rotation shim installed in $prefix"
		else
			echo "  WARNING: --range-proxy asked for but tf-range-proxy.dll was not found" >&2
			echo "           in the checkout or /usr/share/logitech-trueforce/." >&2
			echo "           From a git checkout: make -C tools tf-range-proxy.dll" >&2
		fi
	fi
	install -m 0644 "$SRC_TF_X86" "$tf_dir/trueforce_sdk_x86.dll"
	install -m 0644 "$SRC_WHEEL_X64" "$wheel_dir/logi_steering_wheel_x64.dll"
	install -m 0644 "$SRC_WHEEL_X86" "$wheel_dir/logi_steering_wheel_x86.dll"

	# 2) Register both CLSIDs. Wine's system.reg is a plain text file; we
	#    edit it directly rather than launching the prefix's wine binary
	#    (which may be Proton's and inconvenient to invoke from here).
	python3 - "$sys_reg" "$TF_CLSID" "$TF_PFX_DIR" "$WHEEL_CLSID" "$WHEEL_PFX_DIR" <<'PY'
import os, sys, time

reg_path, tf_clsid, tf_dir, wheel_clsid, wheel_dir = sys.argv[1:6]


def wine_path(pfx_dir, dll):
    """"drive_c/Program Files/Logi/..." -> "C:\\Program Files\\Logi\\...\\dll"."""
    if not pfx_dir.startswith("drive_c/"):
        sys.exit(f"install path is not under drive_c: {pfx_dir}")
    return "C:\\" + pfx_dir[len("drive_c/"):].replace("/", "\\") + "\\" + dll


def reg_value(s):
    """Escape a string for a .reg value, where a lone backslash is an escape.

    Without this the parser eats the separators and the path names nothing.
    """
    return s.replace("\\", "\\\\").replace('"', '\\"')


tf_path = wine_path(tf_dir, "trueforce_sdk_x64.dll")
wheel_path = wine_path(wheel_dir, "logi_steering_wheel_x64.dll")

# TF SDK registration: default value of the CLSID key holds the DLL path,
# and an InProcServer32 sub-key holds it the ordinary COM way as well.
#
# The default value is the one that matters, confirmed by watching the
# lookup: Assetto Corsa EVO opens the key and calls RegQueryValueExW with a
# null value name, then verifies the file's signature and loads it. The
# InProcServer32 sub-key is written as well because it is what an ordinary
# COM consumer reads, and nothing about this registration is ours to
# reinvent. Neither is any use if the path is unopenable, which is what the
# escaping bug above made it.
tf_key = f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}]"
tf_inproc_key = f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}\\\\InProcServer32]"

# Wheel SDK registration: CLSID key default holds a friendly name, and a
# \\ServerBinary sub-key default holds the DLL path. Matches the layout
# DllRegisterServer creates inside the real wheel SDK (extracted from
# logi_steering_wheel_x64.dll @ DllRegisterServer).
wheel_key = f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}]"
wheel_sb_key = f"[Software\\\\Classes\\\\CLSID\\\\{wheel_clsid}\\\\ServerBinary]"

blocks_to_replace = {tf_key, tf_inproc_key, wheel_key, wheel_sb_key}

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
out.append(f'@="{reg_value(tf_path)}"\n')
out.append("\n")
out.append(f"{tf_inproc_key} {ts}\n")
out.append(f'@="{reg_value(tf_path)}"\n')
# Both, so the loader is free to use the calling thread's apartment. The
# SDK runs its own haptic thread either way.
out.append('"ThreadingModel"="Both"\n')
out.append("\n")

# Wheel SDK - friendly name at top, path under ServerBinary
out.append(f"{wheel_key} {ts}\n")
out.append('@="Logitech GHUB Legacy Steering Wheel SDK"\n')
out.append("\n")
out.append(f"{wheel_sb_key} {ts}\n")
out.append(f'@="{reg_value(wheel_path)}"\n')
out.append("\n")

tmp = reg_path + ".new"
with open(tmp, "w") as f:
    f.writelines(out)
os.replace(tmp, reg_path)
PY

	# Read the registration back and check it names a file that is really
	# there. A CLSID pointing at a path the game cannot open fails silently:
	# the game asks for TrueForce, gets nothing, and plays on without it.
	# That is exactly how a backslash-escaping bug went unnoticed through
	# five releases, so the postcondition is checked rather than assumed.
	verify_registered_dll "$sys_reg" "$TF_CLSID" "$prefix" || return 1

	# Independent of the SDK: a prefix can have one without the other, and
	# a failure here should not undo an otherwise good SDK install.
	if [ "$OEM_FFB" = "1" ]; then
		install_oem_ffb "$prefix" || echo "  OEM force-feedback driver NOT installed" >&2
	fi

	echo "  installed $prefix"
}

# Resolve the path a CLSID advertises back to a real file under the prefix.
verify_registered_dll() {
	local reg=$1 clsid=$2 pfx=$3 subkey=${4:-} win unix
	win=$(python3 - "$reg" "$clsid" "$subkey" <<'PY'
import re, sys
reg_path, clsid, subkey = sys.argv[1:4]
# Some registrations keep the path in the key's default value and some in a
# sub-key, so the caller says which.
tail = f"\\\\{subkey}" if subkey else ""
key = f"[Software\\\\Classes\\\\CLSID\\\\{clsid}{tail}]"
want = False
for line in open(reg_path, encoding="utf-8", errors="replace"):
    if line.startswith(key):
        want = True
        continue
    if want and line.startswith("@="):
        # Undo the .reg escaping to get the path Wine will hand the game.
        print(re.sub(r'\\(.)', r'\1', line.strip()[3:-1]))
        break
    if want and line.startswith("["):
        break
PY
	)
	if [ -z "$win" ]; then
		echo "  error: no DLL path registered for $clsid" >&2
		return 1
	fi
	# "C:\Program Files\..." -> "<pfx>/drive_c/Program Files/..."
	unix="$pfx/drive_c/${win#?:\\}"
	unix=${unix//\\//}
	if [ ! -f "$unix" ]; then
		echo "  error: the registry points at a file that is not there:" >&2
		echo "    registered: $win" >&2
		echo "    resolves to: $unix" >&2
		return 1
	fi
}

uninstall_in_prefix() {
	local prefix="$1"
	local sys_reg="$prefix/system.reg"
	# Remove our DLL drops from EVERY version directory in the prefix, not
	# just the one the SDK directory happens to hold right now.
	#
	# $TF_PFX_DIR is derived from whatever version is newest in the SDK
	# tree, which has nothing to do with what was staged here: upgrade the
	# staged SDK, or run the front-ends' Remove button (which passes no
	# --sdk-dir and so resolves somewhere else entirely), and this removed
	# a version that was never installed while leaving the one that was.
	# It then stripped the registry keys and reported success, leaving a
	# prefix with DLLs present and no registration - a state the app still
	# reads as "TrueForce on", because it looks for any version.
	#
	# trueforce_real.dll goes too. It is Logitech's own library, moved
	# aside by --range-proxy, and leaving it behind made the README's
	# "--uninstall puts the original back" false.
	#
	# The parent directories stay: a user may have populated them with
	# real G HUB files outside this installer.
	local d
	for d in "$prefix/drive_c/Program Files/Logi/Trueforce/"*/; do
		[ -d "$d" ] || continue
		rm -f "$d/trueforce_sdk_x64.dll" "$d/trueforce_sdk_x86.dll" \
		      "$d/trueforce_real.dll"
	done
	for d in "$prefix/drive_c/Program Files/Logi/wheel_sdk/"*/; do
		[ -d "$d" ] || continue
		rm -f "$d/logi_steering_wheel_x64.dll" "$d/logi_steering_wheel_x86.dll"
	done
	# Also clean our old shim path (older versions installed there)
	[ -d "$prefix/drive_c/logi-tf-shim" ] && rm -rf "$prefix/drive_c/logi-tf-shim"

	[ -f "$sys_reg" ] || return 0
	python3 - "$sys_reg" "$TF_CLSID" "$WHEEL_CLSID" <<'PY'
import os, sys
reg_path, tf_clsid, wheel_clsid = sys.argv[1:4]

keys = [
    f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}]",
    f"[Software\\\\Classes\\\\CLSID\\\\{tf_clsid}\\\\InProcServer32]",
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

# Every Steam library root: the install itself, plus any library folders the
# user added on other drives. Steam records those in libraryfolders.vdf; without
# reading it we silently skip every game outside the default library, which
# looks like the shim "not working" for that game (issue #27, found and
# diagnosed by @sugituber, whose games live on a second drive).
steam_library_roots() {
	local base vdf
	# Standard Steam install (Arch, Fedora, most distros), then Debian's
	# steam-installer package, which keeps its tree here instead
	# (issue #18, reported by @matthiasvegh).
	# ~/.steam/steam was missing, which is the only root on a machine whose
	# Steam was moved and symlinked, and Flatpak Steam was missing from
	# every component. The three copies of this list disagreed about both.
	for base in "$HOME/.local/share/Steam" "$HOME/.steam/steam" \
		    "$HOME/.steam/debian-installation" \
		    "$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam"; do
		[ -d "$base" ] || continue
		printf '%s\n' "$base"
		vdf="$base/steamapps/libraryfolders.vdf"
		[ -f "$vdf" ] || continue
		# Entries look like:  "path"    "/run/media/you/Games/SteamLibrary"
		sed -nE 's/^[[:space:]]*"path"[[:space:]]+"(.*)"[[:space:]]*$/\1/p' "$vdf"
	done
}

# One prefix per line, so library paths containing spaces survive.
steam_prefixes() {
	local root pfx
	# Deduped by resolved path, not by string. ~/.steam/steam is normally a
	# symlink to ~/.local/share/Steam, so with both in the root list a
	# plain `sort -u` leaves the same library twice and every prefix gets
	# installed into twice. Harmless for a copy, but it is the same
	# counting bug that made doctor report four SDK sims on a machine with
	# two, and it would make --range-proxy move an already-moved DLL.
	steam_library_roots | while IFS= read -r root; do
		[ -d "$root/steamapps" ] || continue
		readlink -f "$root" 2>/dev/null || printf '%s\n' "$root"
	done | awk '!seen[$0]++' | while IFS= read -r root; do
		for pfx in "$root"/steamapps/compatdata/*/pfx; do
			[ -d "$pfx" ] && printf '%s\n' "$pfx"
		done
	done | awk '!seen[$0]++'
}

# Parse flags in any order: a mode (--all-steam / --prefix / --uninstall)
# plus the optional --sdk-dir override.
MODE=""
PREFIX_ARG=""
while [ $# -gt 0 ]; do
	case "$1" in
	--all-steam|--uninstall|--print-sdk-dir)
		MODE="$1"
		;;
	--prefix|--uninstall-prefix)
		MODE="$1"
		PREFIX_ARG="${2:-}"
		[ -n "$PREFIX_ARG" ] || usage
		shift
		;;
	--proxy|--range-proxy)
		# --range-proxy is the original name, from when the DLL only
		# answered the SDK's rotation question. It now also carries a
		# game's own TrueForce to a G923, so the name undersells it and
		# reads as irrelevant to anyone chasing haptics. Both spellings
		# work; --proxy is the one the docs use.
		RANGE_PROXY=1
		;;
	--oem-ffb)
		# G HUB's DirectInput force-feedback driver. Windows routes
		# effects and Escape through it; Wine does not load it at all,
		# so installing it only helps together with the dinput8 proxy
		# that does the routing. See docs/DINPUT_ESCAPE.md.
		OEM_FFB=1
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
	while IFS= read -r pfx; do
		[ -n "$pfx" ] || continue
		install_in_prefix "$pfx"
		count=$((count+1))
	done < <(steam_prefixes)
	echo "installed in $count Steam prefix(es)"
	;;
--prefix)
	require_sources
	install_in_prefix "$PREFIX_ARG"
	;;
--uninstall)
	while IFS= read -r pfx; do
		[ -n "$pfx" ] && uninstall_in_prefix "$pfx"
	done < <(steam_prefixes)
	;;
--uninstall-prefix)
	uninstall_in_prefix "$PREFIX_ARG"
	;;
--print-sdk-dir)
	# Report where the SDK resolved to, and to which versions, without
	# touching anything. This exists so `setup.sh doctor` can check the
	# directory this script would actually read instead of keeping its own
	# idea of where the files live: doctor used to look only inside the
	# repo checkout, so for anyone who installed from a package it reported
	# the SDK missing no matter where they put it (#54).
	echo "sdk_dir=$SDK_DIR"
	echo "tf_version=$TF_VER"
	echo "wheel_version=$WHEEL_VER"
	# Reported separately because it is a different tree ("Logitech", not
	# "Logi") and is commonly absent: someone who copied only the SDK has
	# no force-feedback driver, and that is worth being able to see.
	if [ -f "$SRC_OEM_X64" ]; then
		echo "oem_ffb_version=$OEM_VER"
	else
		echo "oem_ffb_version="
	fi
	;;
*) usage ;;
esac
