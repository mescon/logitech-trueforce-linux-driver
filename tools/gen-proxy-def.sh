#!/bin/bash
# Regenerate tf-range-proxy.def from the real SDK's export table.
#
# Written because doing it by hand went wrong in a way that was invisible:
# a "logi[A-Za-z]+" pattern truncates every export ending in digits, so
# logiTrueForceSetTorqueTFint16/int32/int8 collapsed into one name that does
# not exist. The proxy then forwarded a symbol nobody exports and omitted
# three that games may well call. Nothing complains at build time.
#
# The .def it writes is linked into tf-range-proxy.dll by tools/Makefile.
# Run it only when the SDK's export table changes (a new SDK version), then
# rebuild the DLL (make -C tools tf-range-proxy.dll) and commit both.
set -e
set -o pipefail
# Default matches the layout every other consumer uses. It pointed at
# sdk/trueforce_1_3_11/, a path nothing in the tree stages, so running this
# with no argument after staging the SDK the documented way failed on a
# missing file - and, with no pipefail, truncated the .def to its header
# while reporting success, yielding a proxy that forwards nothing.
DLL="${1:-$(ls -1 sdk/Logi/Trueforce/*/trueforce_sdk_x64.dll 2>/dev/null | sort -V | tail -1)}"
if [ -z "$DLL" ] || [ ! -f "$DLL" ]; then
	echo "error: no trueforce_sdk_x64.dll found; stage the SDK or pass the path" >&2
	exit 1
fi
OUT="${2:-tools/tf-range-proxy.def}"
# Named in the .def without a forward, so the linker resolves them to the
# implementations in tf-range-proxy.c: the four rotation getters the proxy
# answers itself (the 90-degree fix, #27).
OVERRIDE="logiWheelGetOperatingRangeDegrees logiWheelGetOperatingRangeRadians
          logiWheelGetOperatingRangeBoundsDegrees logiWheelGetOperatingRangeBoundsRadians"
# Left out of the .def entirely: the TrueForce entry points the proxy
# implements so it can copy the game's own TrueForce to a G923. They are
# exported by __declspec(dllexport) in tf-range-proxy.c; a forward line
# here would send calls to Logitech's library instead and silence the
# copy. This list must match the source's dllexport'd TF functions.
LOCAL="logiTrueForceSetTorqueTFint8 logiTrueForceSetTorqueTFint16
       logiTrueForceSetTorqueTFint32 logiTrueForceSetTorqueTFdouble
       logiTrueForceSetTorqueTFfloat logiTrueForceSetStreamTF"
{
	echo "EXPORTS"
	echo "; Generated from the real DLL's Ordinal/Name Pointer table."
	echo "; All but the rotation getters and the TrueForce setters, which"
	echo "; this file implements so it can copy the game's own TrueForce"
	echo "; to a G923; those still call through to Logitech's library."
	echo "; Regenerate with: tools/gen-proxy-def.sh"
	# The WHOLE export table, not just the logi* names. Filtering to those
	# dropped eighteen exports including dllOpen and dllClose, which is how
	# a game brings the SDK up: a proxy without them loads and does nothing,
	# leaving no TrueForce, no force feedback, and no rotation push either.
	# Two rounds of testing were spent on the symptoms of that omission.
	x86_64-w64-mingw32-objdump -p "$DLL" \
		| grep -E "^[[:space:]]+\[[[:space:]]*[0-9]+\] \+base\[" \
		| awk '{print $NF}' \
		| grep -vx "RVA" \
		| sort -u \
		| while read -r fn; do
			if echo "$LOCAL" | grep -qw "$fn"; then continue
			elif echo "$OVERRIDE" | grep -qw "$fn"; then echo "$fn"
			else echo "$fn=trueforce_real.$fn"; fi
		done
} > "$OUT"
echo "wrote $OUT ($(grep -c '=' "$OUT") forwarded, $(grep -vc '[=;]' "$OUT") implemented locally)"
