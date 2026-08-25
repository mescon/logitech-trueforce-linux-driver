#!/usr/bin/env bash
# Fail if anything looks for our staged files by a fixed path alone.
#
# The files we stage into games (the dinput8 escape proxy, the telemetry
# relay, the range proxy, the recorded init burst) live in a directory
# whose location depends on how the package was installed. Most
# distributions put it at /usr/share/logitech-trueforce; NixOS, and any
# other prefix-style install, keeps it beside the binaries under the
# package's own root instead.
#
# A consumer that names only the absolute path finds nothing there, and
# the failure is quiet: the launcher stages nothing, so a game gets no
# engine texture, no telemetry, and on a title where raw HID is turned on
# for the SDK, no force feedback either. That was issue #70, and it had
# been true for two other files besides the one it was reported for.
#
# So every lookup must offer a prefix-relative candidate as well. This
# checks that mechanically: any file mentioning the shared directory must
# also derive it from its own location.
set -euo pipefail

# The PACKAGED directory specifically. `~/.local/share/logitech-trueforce`
# is the user's own SDK copy and must stay relative to their home, so it
# is deliberately not matched here.
SHARED='/usr/share/logitech-trueforce|/usr/local/share/logitech-trueforce'
fail=0

# Installers are exempt: writing to /usr is their job, not a lookup.
EXEMPT='tools/setup.sh|tools/dkms-update.sh|tools/check-share-lookups.sh|packaging/|\.md$'

for f in $(grep -rlE "$SHARED" tools userspace/logi-wheel/crates --include='*.sh' --include='*.rs' 2>/dev/null |
	   grep -vE "$EXEMPT" | sort -u); do
	# A prefix-relative candidate looks like one of these: the shell
	# form derived from $0, or the Rust form derived from the exe.
	if grep -qE '\.\./share/logitech-trueforce|SHARED_SUBDIR|share/logitech-trueforce"\)' "$f"; then
		echo "  ok    $f"
	else
		echo "  FAIL  $f names the shared directory but never derives it from its own location" >&2
		fail=1
	fi
done

if [ "$fail" -ne 0 ]; then
	cat >&2 <<'MSG'

Add a candidate derived from the program's own path, before the absolute
ones. In shell: "$(dirname "$0")/../share/logitech-trueforce/<file>". In
Rust: the step telemetry_helpers::resolve makes from the executable.
Otherwise this file works everywhere except the distributions that need
it most, and fails silently when it does not.
MSG
	exit 1
fi
