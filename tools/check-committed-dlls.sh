#!/usr/bin/env bash
# Freshness checks for the two committed Windows DLLs, the same promise
# build-relay.sh --check makes for logi-tf-relay.exe: a prebuilt artifact
# is only safe to ship if something notices when it falls behind its
# source. tf-range-proxy.dll and dinput8-escape.dll are committed because
# the people who need them run Linux without a Windows cross compiler
# (see tools/Makefile and tools/build-dinput8-proxy.sh).
#
# Usage:
#   tools/check-committed-dlls.sh --check
#       Fail if a committed DLL is older, in git history, than any of the
#       sources it is built from. Needs git and full history, nothing
#       else; this is what CI runs.
#
#   tools/check-committed-dlls.sh --verify-bytes
#       Rebuild both DLLs from source with the local mingw toolchain,
#       under the same output filenames (binutils varies the image base
#       with the output name), and compare them byte-for-byte against the
#       committed copies, ignoring only the three fields a rebuild
#       legitimately changes: the COFF header TimeDateStamp, the optional
#       header CheckSum, and the export directory's TimeDateStamp. Run
#       this on the machine that just rebuilt a DLL, before committing.
#
#       CI cannot run this mode. Byte reproducibility holds only on the
#       toolchain that built the committed copy, measured 2026-08-14:
#       vanilla Arch and CachyOS builds of the SAME gcc 16.1.0 differ in
#       376094 of 1294758 bytes of dinput8-escape.dll, and Ubuntu's
#       mingw 13 produces a different size entirely. On the matching
#       toolchain the delta is exactly the three fields above.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Sources per DLL: everything whose change should invalidate the binary,
# and nothing else. gen-proxy-def.sh is deliberately not a source of
# tf-range-proxy.dll: it generates tf-range-proxy.def, and the def itself
# is tracked, so a change that matters shows up there. Listing the
# generator too would mark the DLL stale for comment edits, a false
# positive with teeth (see build-relay.sh on why those are the failure
# worth avoiding: the fix CI would demand needs a toolchain most
# contributors lack).
DINPUT8_SOURCES="tools/dinput8-escape-proxy.cpp tools/build-dinput8-proxy.sh"
RANGE_SOURCES="tools/tf-range-proxy.c tools/tf-range-proxy.def tools/Makefile"

commit_time() {
	git log -1 --format=%ct -- "$@" 2>/dev/null || echo 0
}

check_age() {
	local dll="$1"; shift
	local src bin
	if [ ! -f "$dll" ]; then
		echo "$dll is missing" >&2
		return 1
	fi
	# shellcheck disable=SC2086
	src="$(commit_time $*)"
	bin="$(commit_time "$dll")"
	if [ -z "$bin" ] || [ "$bin" = "0" ]; then
		echo "$dll is not committed" >&2
		return 1
	fi
	if [ "$src" -gt "$bin" ]; then
		echo "$dll is older than its sources ($*)." >&2
		echo "The packaged DLL would ship behaviour the source no longer has." >&2
		echo "Rebuild it (see the header of each source), run" >&2
		echo "  tools/check-committed-dlls.sh --verify-bytes" >&2
		echo "and commit the refreshed DLL with the source change." >&2
		return 1
	fi
	echo "$dll is up to date with its sources."
}

# Compare two PE files, ignoring only the fields a rebuild on the same
# toolchain legitimately changes. Python rather than shell because the
# export directory's file offset needs the section table to resolve.
masked_compare() {
	python3 - "$1" "$2" <<'PYEOF'
import struct, sys

def masked(path):
    data = bytearray(open(path, "rb").read())
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    # COFF header TimeDateStamp.
    struct.pack_into("<I", data, e_lfanew + 8, 0)
    opt = e_lfanew + 24
    magic = struct.unpack_from("<H", data, opt)[0]
    if magic != 0x20B:
        sys.exit("%s: not PE32+" % path)
    # Optional header CheckSum.
    struct.pack_into("<I", data, opt + 64, 0)
    # Export directory TimeDateStamp, via the section table.
    exp_rva = struct.unpack_from("<I", data, opt + 112)[0]
    if exp_rva:
        nsec = struct.unpack_from("<H", data, e_lfanew + 6)[0]
        shdr = opt + struct.unpack_from("<H", data, e_lfanew + 20)[0]
        for i in range(nsec):
            s = shdr + 40 * i
            va = struct.unpack_from("<I", data, s + 12)[0]
            rsz = struct.unpack_from("<I", data, s + 16)[0]
            raw = struct.unpack_from("<I", data, s + 20)[0]
            if va <= exp_rva < va + rsz:
                struct.pack_into("<I", data, raw + exp_rva - va + 4, 0)
                break
    return bytes(data)

a, b = masked(sys.argv[1]), masked(sys.argv[2])
if len(a) != len(b):
    sys.exit("size differs: %d vs %d bytes" % (len(a), len(b)))
diff = sum(x != y for x, y in zip(a, b))
if diff:
    sys.exit("%d bytes differ beyond the timestamp fields" % diff)
PYEOF
}

verify_bytes() {
	local scratch
	scratch="$(mktemp -d)"
	# shellcheck disable=SC2064
	trap "rm -rf '$scratch'" EXIT
	# Build in a scratch copy so the committed DLLs are never overwritten,
	# with the builders themselves so the recipe stays single-sourced.
	cp tools/dinput8-escape-proxy.cpp tools/build-dinput8-proxy.sh \
	   tools/tf-range-proxy.c tools/tf-range-proxy.def tools/Makefile \
	   "$scratch/"
	(cd "$scratch" && ./build-dinput8-proxy.sh >/dev/null)
	(cd "$scratch" && make --quiet tf-range-proxy.dll)
	local ok=0
	for dll in dinput8-escape.dll tf-range-proxy.dll; do
		if out="$(masked_compare "tools/$dll" "$scratch/$dll" 2>&1)"; then
			echo "tools/$dll matches a fresh build of its sources."
		else
			echo "tools/$dll does NOT match a fresh build: $out" >&2
			echo "If you meant to change it, rebuild and commit it;" >&2
			echo "if not, your toolchain differs from the one that" >&2
			echo "built the committed copy (see this script's header)." >&2
			ok=1
		fi
	done
	return "$ok"
}

case "${1:-}" in
--check)
	rc=0
	check_age tools/dinput8-escape.dll $DINPUT8_SOURCES || rc=1
	check_age tools/tf-range-proxy.dll $RANGE_SOURCES || rc=1
	exit "$rc"
	;;
--verify-bytes)
	verify_bytes
	;;
*)
	echo "usage: $0 --check | --verify-bytes" >&2
	exit 2
	;;
esac
