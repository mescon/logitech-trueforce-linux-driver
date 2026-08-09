#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
#
# Start a Windows helper inside a game's Proton prefix, after the game
# itself is up, as a Steam launch-options wrapper:
#
#     logi-launch %command%
#
# Exists because the sims that matter here publish telemetry into a named
# Windows shared-memory section rather than over UDP: the Assetto Corsa
# family (including EVO), iRacing, RaceRoom, rFactor 2 and Le Mans Ultimate.
# Nothing on the Linux side can read that, and nothing on another machine
# can either, so remote SimHub, a buttkicker or a phone dashboard need a
# Windows process inside the same prefix to forward it.
#
# ORDER IS THE WHOLE POINT. Proton takes the prefix exclusively when it
# launches: it runs `wineserver -w` and waits for any existing wineserver to
# exit first. Start the helper before the game and the game does not start
# at all, it sits waiting for the helper to quit. So this wrapper execs the
# game immediately and starts the helper afterwards, from a background
# subshell, once the game's own wineserver exists.
#
# The helper is run with the SAME wine build the game is using, taken from
# the prefix's own config_info. Plain `wine` from the distribution would be
# a different build against a Proton-made prefix, which triggers prefix
# initialisation (the wine-mono prompt) and risks converting it.
set -uo pipefail

# Which titles publish to shared memory, keyed by Steam appid, with the
# name logi-tf-relay knows them by. A game that is not here needs nothing
# started, so this doubles as the "should I do anything at all" test.
relay_game_for() {
	case "$1" in
	266410)  echo "iracing" ;;
	211500)  echo "raceroom" ;;
	244210)  echo "assetto" ;;
	805550)  echo "acc" ;;
	3058630) echo "ac-evo" ;;
	365960)  echo "rf2" ;;
	2399420) echo "lmu" ;;
	*)       echo "" ;;
	esac
}

# With nothing configured this runs THIS project's own relay, with the game
# worked out from the appid Steam sets. That is the case worth making
# effortless: install the packages, put `logi-launch %command%` in the
# launch options, and simulated TrueForce has its telemetry.
#
# Set LOGI_LAUNCH_EXE to run something else instead, for example a bridge
# that forwards telemetry to SimHub on another machine.
HELPER_EXE="${LOGI_LAUNCH_EXE:-}"
HELPER_ARGS="${LOGI_LAUNCH_ARGS:-}"
# How long to wait for the game's wineserver before giving up, and how long
# to let the game settle afterwards so its maps exist before the first probe.
WAIT_SECONDS="${LOGI_LAUNCH_WAIT:-120}"
SETTLE_SECONDS="${LOGI_LAUNCH_SETTLE:-15}"
LOG="${LOGI_LAUNCH_LOG:-/tmp/logi-launch.log}"

say() { printf '[logi-launch] %s\n' "$*" >>"$LOG"; }

# The prefix Steam is launching this game with. Without it there is nothing
# to attach to, so run the game and stay out of the way.
prefix_root="${STEAM_COMPAT_DATA_PATH:-}"
if [ -z "$prefix_root" ] || [ ! -d "$prefix_root/pfx" ]; then
	say "no STEAM_COMPAT_DATA_PATH; launching the game without a helper"
	exec "$@"
fi

# The Proton build this prefix belongs to. config_info's later lines carry
# paths inside the Proton tree, e.g. .../proton-cachyos-slr/files/share/...,
# so the tree root is whatever sits above files/.
wine_bin=""
if [ -r "$prefix_root/config_info" ]; then
	proton_root=$(sed -n 's#^\(/.*\)/files/.*#\1#p' "$prefix_root/config_info" | head -1)
	[ -n "$proton_root" ] && [ -x "$proton_root/files/bin/wine" ] && \
		wine_bin="$proton_root/files/bin/wine"
fi
if [ -z "$wine_bin" ]; then
	say "could not find this prefix's own wine build; launching without a helper"
	say "(refusing to fall back to the distribution's wine: it would try to"
	say " initialise a Proton-made prefix)"
	exec "$@"
fi

# Work out what to start, if the caller did not say.
if [ -z "$HELPER_EXE" ]; then
	game=$(relay_game_for "${SteamAppId:-${SteamGameId:-}}")
	if [ -z "$game" ]; then
		say "appid ${SteamAppId:-unknown} does not need an in-prefix helper; launching the game"
		exec "$@"
	fi
	if [ ! -f "$prefix_root/pfx/drive_c/logi-tf-relay.exe" ]; then
		say "this game needs logi-tf-relay in its prefix and it is not there."
		say "Install it from the app's Setup page (Install relay), then start the game again."
		exec "$@"
	fi
	HELPER_EXE='c:\logi-tf-relay.exe'
	HELPER_ARGS="--game $game"
fi

(
	# Wait for the game to take the prefix. Keying on the wineserver rather
	# than the game process on purpose: the game runs inside Steam's
	# pressure-vessel container and its process is not visible from here,
	# while the wineserver is.
	waited=0
	while [ "$waited" -lt "$WAIT_SECONDS" ]; do
		if pgrep -x wineserver >/dev/null 2>&1; then
			break
		fi
		sleep 1
		waited=$((waited + 1))
	done
	if [ "$waited" -ge "$WAIT_SECONDS" ]; then
		say "game's wineserver never appeared after ${WAIT_SECONDS}s; not attaching"
		exit 0
	fi
	# Let the game finish creating its shared-memory sections. Attaching
	# during startup is harmless but the first probes would find nothing.
	sleep "$SETTLE_SECONDS"
	say "starting $HELPER_EXE $HELPER_ARGS in $prefix_root/pfx"
	WINEPREFIX="$prefix_root/pfx" WINEDEBUG="${WINEDEBUG:--all}" \
		"$wine_bin" "$HELPER_EXE" $HELPER_ARGS >>"$LOG" 2>&1
	say "helper exited"
) &

exec "$@"
