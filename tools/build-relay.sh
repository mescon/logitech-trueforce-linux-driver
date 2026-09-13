#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Rebuild the prebuilt logi-tf-relay.exe that packaging installs.
#
# The relay runs inside a game's Proton prefix, so it is a Windows binary.
# No distro builder can produce one: Debian, Fedora and openSUSE ship no Rust
# Windows target, so a package cannot build it even though it builds every
# other part of this project. The same problem already applies to
# tf-range-proxy.dll, and it is solved the same way, by committing the built
# artifact and installing it from `tools/`.
#
# Two things keep that honest. This script is the only supported way to
# refresh the binary, so it is always built the same way. And
# `--check` compares the committed copy's age against the sources it is built
# from, so a source change without a refresh fails CI rather than shipping a
# stale decoder to everyone.
#
# Usage:
#   tools/build-relay.sh           rebuild tools/logi-tf-relay.exe
#   tools/build-relay.sh --check   fail if the committed copy is out of date
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="$REPO_ROOT/userspace/logi-wheel"
TARGET="x86_64-pc-windows-gnu"
OUT="$REPO_ROOT/tools/logi-tf-relay.exe"

# Every source whose change should invalidate the binary: the relay itself,
# the parts of logi-wheel-core it actually links, and Cargo.lock, since a
# dependency bump changes the artifact without touching any source.
#
# Named modules rather than the whole core crate. The relay imports exactly
# one thing from it, `relay`, which pulls in `telemetry`; listing the crate
# wholesale marked the binary stale for edits to hidpp.rs, device.rs and
# fftest.rs, none of which it links. That is a false positive with teeth,
# because CI fails on it and the fix it demands is rebuilding a Windows
# binary, which needs a toolchain most contributors and every distro builder
# lack. A release should not be blocked by a file the artifact cannot
# contain.
#
# If the relay ever imports more of core, add the module here. Listing too
# little is the failure worth avoiding, so err toward adding.
sources() {
	printf '%s\n' \
		"$WORKSPACE/crates/logi-tf-relay" \
		"$WORKSPACE/crates/logi-wheel-core/src/relay.rs" \
		"$WORKSPACE/crates/logi-wheel-core/src/telemetry.rs" \
		"$WORKSPACE/crates/logi-wheel-core/Cargo.toml" \
		"$WORKSPACE/Cargo.toml" \
		"$WORKSPACE/Cargo.lock"
}

# The most recent commit touching any of them.
newest_source_commit() {
	# shellcheck disable=SC2046
	git -C "$REPO_ROOT" log -1 --format=%ct -- $(sources) 2>/dev/null || echo 0
}

binary_commit() {
	git -C "$REPO_ROOT" log -1 --format=%ct -- "$OUT" 2>/dev/null || echo 0
}

if [ "${1:-}" = "--check" ]; then
	if [ ! -f "$OUT" ]; then
		echo "tools/logi-tf-relay.exe is missing; run tools/build-relay.sh" >&2
		exit 1
	fi
	src="$(newest_source_commit)"
	bin="$(binary_commit)"
	if [ -z "$bin" ] || [ "$bin" = "0" ]; then
		echo "tools/logi-tf-relay.exe is not committed; run tools/build-relay.sh" >&2
		exit 1
	fi
	if [ "$src" -gt "$bin" ]; then
		echo "tools/logi-tf-relay.exe is older than the relay sources." >&2
		echo "The packaged relay would ship behaviour the source no longer has." >&2
		echo "Refresh it:  tools/build-relay.sh" >&2
		exit 1
	fi
	echo "tools/logi-tf-relay.exe is up to date with its sources."
	exit 0
fi

if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
	echo "The $TARGET target is not installed. Add it with:" >&2
	echo "  rustup target add $TARGET" >&2
	echo "and make sure a MinGW linker is present (gcc-mingw-w64-x86-64)." >&2
	exit 1
fi

cd "$WORKSPACE"
cargo build --profile relay-dist --locked -p logi-tf-relay --target "$TARGET"
install -m 0644 "target/$TARGET/relay-dist/logi-tf-relay.exe" "$OUT"
printf 'wrote %s (%s KB)\n' "$OUT" "$(( $(stat -c%s "$OUT") / 1024 ))"
echo "Commit it together with whatever source change prompted the rebuild."
