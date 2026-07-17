#!/usr/bin/env bash
# Build the logitech-trueforce-dkms .deb from a clean export of the repo.
# Run from anywhere; it locates the repo via git. Produces the .deb (and
# source package) in the directory above the build tree.
#
#   packaging/debian/build-deb.sh [output-dir]
#
# Needs: dpkg-dev, debhelper, dkms, dh-dkms (or dkms providing dh_dkms), cargo.
# Also builds and packages logi-ffb and logi-dd-tui from userspace/logi-dd.
set -euo pipefail

repo="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
outdir="${1:-$repo/build/debian}"

ver="$(sed -n '1s/.*(\([0-9][^-)]*\).*/\1/p' "$repo/packaging/debian/changelog")"
[ -n "$ver" ] || { echo "could not parse version from debian/changelog" >&2; exit 1; }
name="logitech-trueforce-dkms"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tree="$work/${name}-${ver}"

# Clean source export (respects .gitignore; no working-tree cruft).
mkdir -p "$tree"
git -C "$repo" archive HEAD | tar -x -C "$tree"

# orig tarball (source without debian/), then drop the packaging in.
tar -czf "$work/${name}_${ver}.orig.tar.gz" -C "$work" "${name}-${ver}"
cp -a "$repo/packaging/debian" "$tree/debian"

( cd "$tree" && dpkg-buildpackage -us -uc -b )

mkdir -p "$outdir"
cp -v "$work/"*.deb "$outdir/"
echo "built: $outdir/${name}_${ver}_all.deb"
