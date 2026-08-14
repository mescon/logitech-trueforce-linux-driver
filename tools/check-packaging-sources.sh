#!/usr/bin/env bash
# The DKMS packaging recipes for Debian, the AUR and OBS stage the module
# source with explicit file lists, and a header added to mainline/ without
# a matching manifest update ships a source tree that cannot compile on the
# user's machine, which CI never sees because the DKMS build happens at
# install time (issue #64: 0.35.0 shipped without hidpp_dd_texture_merge.h
# and every deb install failed). This asserts that every locally included
# header in the module sources appears in each explicit manifest.
# The akmods spec copies mainline/ wholesale and needs no check.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

manifests=(
	packaging/debian/rules
	packaging/aur/logitech-trueforce-dkms/PKGBUILD
	packaging/obs/logitech-trueforce-dkms.spec
)

headers=$(grep -h '#include "' mainline/*.c | sed 's/.*"\(.*\)".*/\1/' | sort -u)

rc=0
for m in "${manifests[@]}"; do
	for h in $headers; do
		if ! grep -q "$h" "$m"; then
			echo "MISSING: $m does not stage mainline/$h"
			rc=1
		fi
	done
done

if [ "$rc" -eq 0 ]; then
	echo "All packaging manifests stage every included header."
fi
exit "$rc"
