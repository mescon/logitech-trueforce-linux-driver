#!/usr/bin/env bash
# Fail if a shipped binary demands a newer glibc than our floor.
#
# glibc versions its symbols and a binary asks for whatever version the
# build host offered. Built on a rolling distribution, that can be a
# version no stable or frozen distribution has yet, and the binary then
# refuses to start there with:
#
#   version `GLIBC_2.43' not found (required by ...)
#
# which is what SteamOS, a frozen Arch snapshot, did with our window
# (issue #68): the Steam Deck could not run it at all. The floor below is
# what a Deck has, so anything at or under it runs there.
#
# The window pins two symbols back to their oldest versions in
# crates/logi-wheel-gui/src/glibc_compat.rs. This check exists because a
# dependency can reintroduce the problem with a different symbol, and the
# only visible sign would be a bug report from someone we cannot test on.
#
# Usage: tools/check-glibc-floor.sh <binary> [<binary>...]
set -euo pipefail

FLOOR="2.39"

fail=0
for bin in "$@"; do
	[ -f "$bin" ] || { echo "check-glibc-floor: $bin is missing" >&2; exit 1; }
	max="$(objdump -T "$bin" 2>/dev/null |
		grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sed 's/GLIBC_//' |
		sort -uV | tail -1)"
	[ -n "$max" ] || { echo "  $(basename "$bin"): no glibc references (static?)"; continue; }
	if [ "$(printf '%s\n%s\n' "$FLOOR" "$max" | sort -V | tail -1)" != "$FLOOR" ]; then
		echo "  $(basename "$bin"): needs glibc $max, floor is $FLOOR" >&2
		objdump -T "$bin" | grep -E "GLIBC_$max\$" | awk '{print "      " $NF, $(NF-1)}' | sort -u >&2
		fail=1
	else
		echo "  $(basename "$bin"): needs glibc $max (floor $FLOOR)"
	fi
done

if [ "$fail" -ne 0 ]; then
	cat >&2 <<'MSG'

Those symbols would stop the binary starting on a distribution whose glibc
is older than the build host's, SteamOS among them. Either avoid the call,
or pin the symbol to an older version the way
crates/logi-wheel-gui/src/glibc_compat.rs does.
MSG
	exit 1
fi
