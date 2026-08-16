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
# LOGI_LAUNCH_HELPERS runs things AS WELL AS the relay, rather than instead
# of it, as a semicolon-separated list of `exe args`:
#
#   LOGI_LAUNCH_HELPERS='c:\sim-teleport.exe source'
#
# Exists because these two wants are not alternatives. Someone running
# SimHub on a second machine needs its bridge inside the prefix, and still
# wants the rev lights and simulated TrueForce driven here; LOGI_LAUNCH_EXE
# made that an either/or and quietly cost them the second one.
#
# Several readers of the same telemetry is not a conflict: they only read,
# and a Windows file mapping takes any number of readers.
#
# The exe is whatever precedes the first space, so it cannot itself contain
# one. Helpers belong in the prefix's drive_c anyway, which is where the
# documentation puts them and where no path has spaces.
EXTRA_HELPERS="${LOGI_LAUNCH_HELPERS:-}"
# How long to wait for the game's wineserver before giving up, and how long
# to let the game settle afterwards so its maps exist before the first probe.
WAIT_SECONDS="${LOGI_LAUNCH_WAIT:-120}"
SETTLE_SECONDS="${LOGI_LAUNCH_SETTLE:-15}"
LOG="${LOGI_LAUNCH_LOG:-/tmp/logi-launch.log}"

say() { printf '[logi-launch] %s\n' "$*" >>"$LOG"; }

# `logi-launch --game <name> %command%` names the title explicitly, for when
# the appid cannot identify it: a non-Steam shortcut (whose id Steam
# generates locally), a copy bought elsewhere, or a delisted game
# reinstalled from a backup. `logi-wheel --launch-plan --list` prints the
# names.
# --game names the title when the appid cannot identify it. --wheel names
# which wheel to set up for, which matters only when more than one kind is
# plugged in: the game chooses the wheel it uses in its own settings and
# never tells us, so with a direct-drive wheel and a G923 both attached we
# decline to guess rather than risk setting PROTON_ENABLE_HIDRAW on the
# G923 and costing it force feedback.
named_game=""
named_wheel=""
while :; do
	case "${1:-}" in
	--game)  named_game="${2:-}";  shift 2 ;;
	--wheel) named_wheel="${2:-}"; shift 2 ;;
	--list|--help) exec logi-wheel --launch-plan --list ;;
	*) break ;;
	esac
done

# The prefix Steam is launching this game with. Without it there is nothing
# to attach to, so run the game and stay out of the way.
# No prefix means no in-prefix helper, but everything else still applies:
# a native Linux game, or one whose prefix does not exist yet, still wants
# the right HIDRAW setting, the proxy for DirectInput, and the daemon. An
# earlier version returned here and silently dropped all three.
prefix_root="${STEAM_COMPAT_DATA_PATH:-}"
if [ -n "$prefix_root" ] && [ ! -d "$prefix_root/pfx" ]; then
	prefix_root=""
fi

# The Proton build this prefix belongs to. config_info's later lines carry
# paths inside the Proton tree, e.g. .../proton-cachyos-slr/files/share/...,
# so the tree root is whatever sits above files/.
wine_bin=""
if [ -n "$prefix_root" ] && [ -r "$prefix_root/config_info" ]; then
	proton_root=$(sed -n 's#^\(/.*\)/files/.*#\1#p' "$prefix_root/config_info" | head -1)
	[ -n "$proton_root" ] && [ -x "$proton_root/files/bin/wine" ] && \
		wine_bin="$proton_root/files/bin/wine"
fi
# Without the prefix's own wine there is no safe way to run anything inside
# it. Refusing to fall back to the distribution's wine is deliberate: that
# is a different build against a Proton-made prefix, and it prompts to
# install wine-mono and can convert the prefix.
if [ -z "$wine_bin" ]; then
	[ -n "$prefix_root" ] && say "no usable wine for this prefix; skipping in-prefix helpers"
	HELPER_EXE=""
	EXTRA_HELPERS=""
fi

# Ask the app what this game needs on the wheel that is attached. The
# registry behind it is tested and already drives the Setup page; deciding
# any of it again in shell would be a second copy to drift, and the
# per-wheel half is exactly what went wrong when the Setup page described
# the wrong wheel.
plan=""
if command -v logi-wheel >/dev/null 2>&1; then
	set -- "$@"
	wheel_args=""
	[ -n "$named_wheel" ] && wheel_args="--wheel $named_wheel"
	if [ -n "$named_game" ]; then
		# shellcheck disable=SC2086
		plan=$(logi-wheel --launch-plan --game "$named_game" $wheel_args 2>/dev/null)
	else
		# shellcheck disable=SC2086
		plan=$(logi-wheel --launch-plan "${SteamAppId:-${SteamGameId:-0}}" $wheel_args 2>/dev/null)
	fi
fi

# A game we do not know yet is not a dead end. Anyone can describe one in
# ~/.config/logi-wheel/games.conf, a line per appid:
#
#   3058630  hidraw=1 relay=ac-evo tfsim=1
#   1234567  ffb=proxy tfsim=0
#
# A line wins for the keys it STATES; a key it does not state keeps the
# built-in plan's value. Per key rather than wholesale, because these lines
# outlive releases: an old `3058630 hidraw=1`, written before the kernel
# texture merge existed, must not silently turn `texture=merge` off for
# every release after it. To force a key off, state it (`texture=none`,
# `tfsim=0`). Getting a working line into that file is also exactly the
# report needed to add the game properly.
user_conf="${XDG_CONFIG_HOME:-$HOME/.config}/logi-wheel/games.conf"
this_app="${SteamAppId:-${SteamGameId:-0}}"
if [ -r "$user_conf" ]; then
	user_line=$(sed -n "s/^[[:space:]]*$this_app[[:space:]]\+//p" "$user_conf" | head -1)
	if [ -n "$user_line" ]; then
		say "using your games.conf entry for appid $this_app"
		# plan_get below takes the FIRST match for a key, so the user's
		# tokens go in front of the computed plan: a stated key shadows
		# the built-in value, an unstated one falls through to it.
		# shellcheck disable=SC2086
		plan=$(printf '%s\n' $user_line; printf '%s\n' "$plan")
	fi
fi
plan_get() { printf '%s\n' "$plan" | sed -n "s/^$1=//p" | head -1; }

want_hidraw=$(plan_get hidraw)
want_ffb=$(plan_get ffb)
want_relay=$(plan_get relay)
want_tfsim=$(plan_get tfsim)
want_texture=$(plan_get texture)
say "plan: wheel=$(plan_get wheel) game=$(plan_get game) hidraw=${want_hidraw:-unset} ffb=${want_ffb:-native} relay=${want_relay:-none} tfsim=${want_tfsim:-0} texture=${want_texture:-none}"

# TrueForce in an SDK title needs the game to reach the wheel's raw HID
# interface. Set here so nobody has to remember it, and NEVER guessed: on a
# wheel that cannot take it this costs the owner force feedback, so it is
# set only when the plan says this wheel wants it for this game.
# The value is normally `0xVID/0xPID` naming the wheel, because Proton
# matches this variable as a substring against each device's own
# `0xVID/0xPID` (dlls/winebus.sys/main.c). The bare `1` short-circuits that
# test and hands EVERY HID device on the machine to the game: keyboards,
# headsets, other controllers. It is still accepted here, as the fallback
# when no wheel could be named and for anyone who set it by hand.
# Turning this on REMOVES a working force-feedback path, and only Logitech's
# own TrueForce files put one back.
#
# Without it, Proton hands the game an evdev-backed device and force feedback
# works with nothing installed. With it, the game gets the raw HID device
# instead, and this wheel's descriptor has no PID collection, so the older
# Windows force-feedback protocol has nowhere to land. What remains is
# Logitech's SDK, which is what those files are.
#
# So on a prefix without them, setting it costs the owner their force
# feedback and gives nothing back. That is issue #60, where it read as
# "logi-launch gives me no FFB". Checked here rather than in the plan
# because only this wrapper knows which prefix the game is launching with.
shim_dir="$prefix_root/pfx/drive_c/Program Files/Logi/Trueforce"
have_tf_files=0
if [ -n "$prefix_root" ]; then
	# Any version directory holding the SDK dll counts; the version numbers
	# are whatever that person's G HUB shipped.
	for f in "$shim_dir"/*/trueforce_sdk_x64.dll; do
		[ -f "$f" ] && have_tf_files=1 && break
	done
fi

# Nonzero when the plan granted the game raw HID access (an SDK title):
# those sessions can leave the wheel's TrueForce engine started, so they
# get the teardown pair on exit (see send_teardown_pair below).
hidraw_granted=""
case "$want_hidraw" in
"") ;;
0)
	export PROTON_ENABLE_HIDRAW=0
	say "set PROTON_ENABLE_HIDRAW=0"
	;;
*)
	if [ "$have_tf_files" = "1" ] || [ -z "$prefix_root" ]; then
		export PROTON_ENABLE_HIDRAW="$want_hidraw"
		hidraw_granted=1
		say "set PROTON_ENABLE_HIDRAW=$want_hidraw"
	else
		say "NOT setting PROTON_ENABLE_HIDRAW: this game wants it, but"
		say "Logitech's TrueForce files are not in this prefix, and turning"
		say "it on without them would take away the force feedback you have"
		say "and give nothing back. Install them from the app's Setup page"
		say "(TrueForce files), then start the game again."
		say "Force feedback still works; the game's own TrueForce does not."
		# Fall back to simulated TrueForce, which is exactly the recipe this
		# title gets on a wheel that cannot receive the native kind. Asking
		# for that answer rather than inventing one keeps the fallback in the
		# registry with everything else.
		if command -v logi-wheel >/dev/null 2>&1; then
			# Ask the same way the first query did. Using the appid
			# here throws away --game, and a title named that way
			# has no usable appid by definition, so the fallback
			# came back as "unknown": simulated TrueForce with no
			# relay, and therefore no telemetry to drive it.
			if [ -n "$named_game" ]; then
				fallback=$(logi-wheel --launch-plan --game "$named_game" --wheel classic 2>/dev/null)
			else
				fallback=$(logi-wheel --launch-plan "$this_app" --wheel classic 2>/dev/null)
			fi
			want_tfsim=$(printf '%s\n' "$fallback" | sed -n 's/^tfsim=//p' | head -1)
			want_relay=$(printf '%s\n' "$fallback" | sed -n 's/^relay=//p' | head -1)
			[ "${want_tfsim:-0}" = "1" ] && \
				say "using simulated TrueForce instead (relay=${want_relay:-none})"
		fi
	fi
	;;
esac

# The kernel texture merge: the driver mixes an engine-note texture into
# the game's own TrueForce stream, on the wheel itself. Three pieces, all
# undone when the game exits:
#   - the dinput8 escape proxy staged into the game's directory. It answers
#     the SDK's range getters (without which the SDK's stream never comes
#     up under Proton) and relays the game's RPM telemetry over the Escape
#     channel as localhost UDP.
#   - logi-rpm-bridge, which turns that UDP into wheel_texture_rpm writes.
#   - wheel_tf_merge=1, which tells the driver to render the texture.
# Gated on the TrueForce files exactly like hidraw above: without the SDK
# there is no native stream to merge into. The no-prefix case passes for
# the same reason it does there: the prefix may simply not exist yet.
rpm_bridge_pid=""
merge_enabled=""
if [ "$want_texture" = "merge" ] && \
   { [ "$have_tf_files" = "1" ] || [ -z "$prefix_root" ]; }; then
	# The game's own directory, taken from the .exe in the command Steam
	# hands us. A native Linux game or a bare test command has none, and
	# then there is nowhere to stage the proxy: say so and move on.
	game_exe=$(printf '%s\n' "$@" | grep -m1 -e '\.exe$' || true)
	game_dir=""
	[ -n "$game_exe" ] && game_dir=$(dirname "$game_exe")
	proxy_src="/usr/share/logitech-trueforce/dinput8-escape.dll"
	[ -r "$proxy_src" ] || proxy_src="$(dirname "$0")/dinput8-escape.dll"
	if [ -n "$game_dir" ] && [ -d "$game_dir" ] && [ -r "$proxy_src" ]; then
		# cmp, not a timestamp: Steam validation rewrites files and a
		# stale proxy looks exactly like a missing one.
		if ! cmp -s "$proxy_src" "$game_dir/dinput8.dll" 2>/dev/null; then
			if cp -f "$proxy_src" "$game_dir/dinput8.dll" 2>/dev/null; then
				say "staged dinput8 proxy into $game_dir"
			else
				say "could not copy the dinput8 proxy into $game_dir"
			fi
		fi
		# Merge with whatever the user already set; never clobber it.
		case "${WINEDLLOVERRIDES:-}" in
		*dinput8*) ;;
		*)
			export WINEDLLOVERRIDES="dinput8=n,b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
			say "set WINEDLLOVERRIDES=$WINEDLLOVERRIDES"
			;;
		esac
	else
		say "not staging the dinput8 proxy (no game dir or dll found);"
		say "the texture merge will idle without its RPM feed"
	fi
	bridge_bin=""
	if command -v logi-rpm-bridge >/dev/null 2>&1; then
		bridge_bin="logi-rpm-bridge"
	elif [ -x "$(dirname "$0")/logi-rpm-bridge" ]; then
		bridge_bin="$(dirname "$0")/logi-rpm-bridge"
	fi
	if [ -n "$bridge_bin" ]; then
		"$bridge_bin" >>"$LOG" 2>&1 &
		rpm_bridge_pid=$!
		say "started logi-rpm-bridge (pid $rpm_bridge_pid)"
	else
		say "logi-rpm-bridge is not installed; the texture merge has no RPM feed"
	fi
	for d in /sys/bus/hid/devices/*046D:C2*/wheel_tf_merge; do
		[ -w "$d" ] && echo 1 > "$d" && merge_enabled=1 && \
			say "texture merge enabled ($d)"
	done
fi

# Work out what to start in the prefix, if the caller did not say.
if [ -z "$HELPER_EXE" ] && [ -n "$prefix_root" ] && [ -n "$wine_bin" ]; then
	game="$want_relay"
	[ "$game" = "none" ] && game=""
	if [ -z "$game" ]; then
		# Deliberately "relay", not "helper": LOGI_LAUNCH_HELPERS may
		# still start something, and a line claiming nothing was needed
		# would be contradicted moments later.
		say "no in-prefix relay needed for this game"
	else
		# Stage or refresh the relay from the packaged master copy. The
		# prefix copy is a snapshot from whenever it was installed, and a
		# stale one fails in ways that look like telemetry problems: an
		# old build exits instead of waiting for the game, or does not
		# know the game id at all (#59). cmp, not a timestamp, same
		# reason as the dinput8 proxy above.
		relay_src="/usr/share/logitech-trueforce/logi-tf-relay.exe"
		[ -r "$relay_src" ] || relay_src="$(dirname "$0")/logi-tf-relay.exe"
		relay_dst="$prefix_root/pfx/drive_c/logi-tf-relay.exe"
		if [ -r "$relay_src" ] && \
		   ! cmp -s "$relay_src" "$relay_dst" 2>/dev/null; then
			if cp -f "$relay_src" "$relay_dst" 2>/dev/null; then
				say "staged logi-tf-relay into the prefix"
			else
				say "could not copy logi-tf-relay into the prefix"
			fi
		fi
		if [ ! -f "$relay_dst" ]; then
			say "this game needs logi-tf-relay in its prefix and it is not there."
			say "Install it from the app's Setup page (Install relay), then start the game again."
			game=""
		else
			HELPER_EXE='c:\logi-tf-relay.exe'
			HELPER_ARGS="--game $game"
		fi
	fi
fi

# The relay only carries telemetry OUT of the prefix. Something has to
# read it and drive the wheel, and that is logi-tf-sim. Leaving it to
# the user is the remaining manual step in an otherwise automatic
# chain, and forgetting it looks exactly like the relay not working:
# the game runs, the wheel behaves normally, and the rev lights stay
# dark. Started only if it is not already up, and left running, since
# it idles when nothing is streaming.
if [ "${LOGI_LAUNCH_TF_SIM:-1}" = "1" ] && [ "${want_tfsim:-1}" = "1" ]; then
	if pgrep -x logi-tf-sim >/dev/null 2>&1; then
		say "logi-tf-sim is already running"
		# It was started for some other session, possibly aimed at the
		# other wheel. Say so rather than let a named wheel look like it
		# was honoured when the running daemon never saw it.
		if [ -n "$named_wheel" ]; then
			say "note: it was already running, so --wheel $named_wheel did not reach it."
			say "note: stop it and start the game again to aim it at that wheel."
		fi
	elif command -v logi-tf-sim >/dev/null 2>&1; then
		# The daemon drives ONE wheel and has its own picker, which
		# defaults to preferring a G923. Naming a wheel here and leaving
		# the daemon to its own default would be two answers to one
		# question, and on a two-wheel rig they would disagree: the game
		# on the direct-drive wheel, the haptics on the G923.
		if [ -n "$named_wheel" ]; then
			say "starting logi-tf-sim, aimed at $named_wheel"
			setsid env LOGI_TF_SIM_WHEEL="$named_wheel" logi-tf-sim >>"$LOG" 2>&1 </dev/null &
		else
			say "starting logi-tf-sim"
			setsid logi-tf-sim >>"$LOG" 2>&1 </dev/null &
		fi
	else
		say "logi-tf-sim is not installed; the rev lights and simulated"
		say "TrueForce need it. Install the logi-wheel package."
	fi
fi

# Everything to run inside the prefix, as parallel exe/args arrays. Kept
# apart rather than joined into one string so an exe path containing spaces
# still works, which is what quoting "$HELPER_EXE" bought before this
# supported more than one.
helper_exes=()
helper_argv=()
if [ -n "$HELPER_EXE" ]; then
	helper_exes+=("$HELPER_EXE")
	helper_argv+=("$HELPER_ARGS")
fi
if [ -n "$EXTRA_HELPERS" ]; then
	# `;` between helpers, first space inside one separating exe from args.
	saved_ifs="$IFS"
	IFS=';'
	for entry in $EXTRA_HELPERS; do
		# Leading and trailing blanks, so a list can be written spaced out.
		entry="${entry#"${entry%%[![:space:]]*}"}"
		entry="${entry%"${entry##*[![:space:]]}"}"
		[ -z "$entry" ] && continue
		exe="${entry%% *}"
		if [ "$entry" = "$exe" ]; then
			args=""
		else
			args="${entry#* }"
		fi
		helper_exes+=("$exe")
		helper_argv+=("$args")
	done
	IFS="$saved_ifs"
fi

if [ ${#helper_exes[@]} -gt 0 ] && [ -n "$prefix_root" ] && [ -n "$wine_bin" ]; then
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

	# One wine process each, started together. Waiting for the first to
	# exit before starting the second would mean the second never runs:
	# these are long-lived bridges that stay up for the whole session.
	#
	# The sync flags must match the wineserver Proton started for the
	# game, or wine refuses to join it and the helper dies before its
	# first instruction ("Server is running with WINEFSYNC but this
	# process is not", #59). Proton enables fsync and esync unless the
	# user opted out, so mirror exactly that.
	helper_fsync=1; helper_esync=1
	[ "${PROTON_NO_FSYNC:-0}" = "1" ] && helper_fsync=0
	[ "${PROTON_NO_ESYNC:-0}" = "1" ] && helper_esync=0
	i=0
	while [ "$i" -lt ${#helper_exes[@]} ]; do
		exe="${helper_exes[$i]}"
		args="${helper_argv[$i]}"
		(
			say "starting $exe${args:+ $args} in $prefix_root/pfx"
			WINEPREFIX="$prefix_root/pfx" WINEDEBUG="${WINEDEBUG:--all}" \
				WINEFSYNC="$helper_fsync" WINEESYNC="$helper_esync" \
				"$wine_bin" "$exe" $args >>"$LOG" 2>&1
			# Named, because "helper exited" says nothing about which one
			# when two are running.
			say "$exe exited"
		) &
		i=$((i + 1))
	done
	wait
) &
fi

# A DirectInput title drives force feedback through the older Windows path,
# which needs the logi-ffb proxy in front of the game. Chained rather than
# asked of the user, so one prepend really is enough.
if [ "$want_ffb" = "proxy" ] && command -v logi-ffb >/dev/null 2>&1; then
	say "launching through logi-ffb for DirectInput force feedback"
	set -- logi-ffb "$@"
fi

# The captured 0x04+0x03 teardown pair, sent to the wheel's interface-2
# hidraw node once the game is gone. An SDK title that exits (or is
# killed) mid-stream leaves the wheel's TrueForce engine started and fed
# by nobody - the abort-capture state that whines until power cycle -
# while every clean Windows session ends with exactly this pair, then
# silence. python3 rather than shell printf, deliberately: the packets
# are raw 64-byte binaries full of NUL bytes with a 2 ms gap between
# them, and the interface-2 lookup is a sysfs walk (the same one
# logi-tf-init.py's find_tf_hidraw does, keyed on bInterfaceNumber so it
# survives hidraw renumbering). All of that is exact and readable in
# python; as printf escapes it needs a NUL-safe printf, a fractional
# sleep, and a realpath chain that differ across shells. logi-tf-init.py
# already makes python3 part of this tool set.
send_teardown_pair() {
	if ! command -v python3 >/dev/null 2>&1; then
		say "python3 not found; cannot send the wheel teardown pair"
		return 0
	fi
	python3 - >>"$LOG" 2>&1 <<'PYEOF'
import glob, os, time

def find_tf_hidraw():
    for h in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.join(h, "device")
        try:
            hid_id = ""
            for line in open(os.path.join(dev, "uevent")):
                if line.startswith("HID_ID="):
                    hid_id = line.strip().split("=", 1)[1]
            up = hid_id.upper()
            if "046D" not in up or not any(p in up for p in ("C276", "C272", "C268")):
                continue
            iface = os.path.realpath(os.path.join(dev, ".."))
            bnum = open(os.path.join(iface, "bInterfaceNumber")).read().strip()
            if int(bnum, 16) == 2:
                return "/dev/" + os.path.basename(h)
        except (OSError, ValueError):
            continue
    return None

node = find_tf_hidraw()
if not node:
    print("[logi-launch] no direct-drive interface-2 hidraw; teardown pair skipped")
    raise SystemExit(0)
try:
    fd = os.open(node, os.O_WRONLY)
except OSError as e:
    print("[logi-launch] cannot open %s (%s); teardown pair skipped" % (node, e))
    raise SystemExit(0)
# 0x04 stop/clear, then 0x03 arm, ~2 ms apart like the captures. The
# sequence byte (byte 5) is left 0: control packets are accepted with
# any sequence, and the SDK's own counter is unknowable from out here.
for cmd in (0x04, 0x03):
    pkt = bytearray(64)
    pkt[0] = 0x01
    pkt[4] = cmd
    os.write(fd, bytes(pkt))
    time.sleep(0.002)
os.close(fd)
print("[logi-launch] sent TrueForce teardown pair to %s" % node)
PYEOF
}

# Experimental (LOGI_TF_REARM=1): reset-and-rearm the wheel's TrueForce
# engine before the game starts. A session that died without its teardown
# reaching the wheel (hard-killed game, crash) leaves the next SDK session
# opening successfully but never streaming; today only a power cycle
# recovers it. This replays what a clean boot gives the wheel: the
# captured 0x04+0x03 teardown pair, then G HUB's 68-packet init twice
# (tools/tf-init.bin, generated from libtrueforce's tf_init_data.h).
# Off by default until a hardware A/B proves it replaces the power cycle;
# harmless bytes either way - a healthy wheel gets the same init dupes a
# real session start sends.
send_tf_rearm() {
	rearm_blob=""
	for c in /usr/share/logitech-trueforce/tf-init.bin \
		 "$(dirname "$0")/tf-init.bin"; do
		[ -r "$c" ] && rearm_blob="$c" && break
	done
	if [ -z "$rearm_blob" ] || ! command -v python3 >/dev/null 2>&1; then
		say "TF re-arm requested but tf-init.bin or python3 missing; skipped"
		return 0
	fi
	REARM_BLOB="$rearm_blob" python3 - >>"$LOG" 2>&1 <<'PYEOF'
import glob, os, time

def find_tf_hidraw():
    for h in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        dev = os.path.join(h, "device")
        try:
            hid_id = ""
            for line in open(os.path.join(dev, "uevent")):
                if line.startswith("HID_ID="):
                    hid_id = line.strip().split("=", 1)[1]
            up = hid_id.upper()
            if "046D" not in up or not any(p in up for p in ("C276", "C272", "C268")):
                continue
            iface = os.path.realpath(os.path.join(dev, ".."))
            if int(open(os.path.join(iface, "bInterfaceNumber")).read().strip(), 16) == 2:
                return "/dev/" + os.path.basename(h)
        except (OSError, ValueError):
            continue
    return None

node = find_tf_hidraw()
if not node:
    print("[logi-launch] TF re-arm: no interface-2 hidraw node; skipped")
    raise SystemExit(0)
blob = open(os.environ["REARM_BLOB"], "rb").read()
pkts = [blob[i:i+64] for i in range(0, len(blob), 64)]
fd = os.open(node, os.O_WRONLY)
stop = bytearray(64); stop[0] = 0x01; stop[4] = 0x04
arm = bytearray(64); arm[0] = 0x01; arm[4] = 0x03; arm[5] = 0x01
os.write(fd, bytes(stop)); time.sleep(0.002); os.write(fd, bytes(arm))
time.sleep(0.002)
for _ in range(2):
    for p in pkts:
        os.write(fd, p)
os.close(fd)
print("[logi-launch] TF re-arm: teardown pair + %dx2 init packets to %s" % (len(pkts), node))
PYEOF
}
if [ "${LOGI_TF_REARM:-0}" = "1" ] && [ -n "$hidraw_granted" ]; then
	send_tf_rearm
fi

# With the texture merge armed or raw HID granted there is teardown to do
# after the game exits, so the game runs as a child rather than by exec:
# the bridge dies with the session, the merge switches off so a later
# non-SDK session does not inherit a texture with no RPM behind it, and
# the wheel gets the teardown pair an SDK title never sends under Proton.
# Everything else keeps the historical exec, which leaves no wrapper
# process behind.
if [ -n "$rpm_bridge_pid" ] || [ -n "$merge_enabled" ] || [ -n "$hidraw_granted" ]; then
	session_cleanup() {
		[ -n "$rpm_bridge_pid" ] && kill "$rpm_bridge_pid" 2>/dev/null
		if [ -n "$rpm_bridge_pid" ] || [ -n "$merge_enabled" ]; then
			for d in /sys/bus/hid/devices/*046D:C2*/wheel_tf_merge; do
				[ -w "$d" ] && echo 0 > "$d"
			done
			say "texture merge disabled"
		fi
		send_teardown_pair
	}
	trap session_cleanup EXIT
	# Signal hardening: a bare "$@" would make SIGTERM/SIGINT hit only
	# this wrapper while the game keeps running (bash defers signals
	# until a foreground child exits). Run the game in the background,
	# forward TERM/INT to it, and wait. The second wait matters: a
	# trapped signal interrupts the first wait before the game has
	# actually exited, and bash then reports the game's real status
	# from the re-wait (statuses of reaped background jobs are
	# remembered). The EXIT trap above still runs at exit, after
	# cleanup here, composing with this.
	"$@" &
	game_pid=$!
	trap 'kill -TERM "$game_pid" 2>/dev/null' TERM INT
	wait "$game_pid"
	wait "$game_pid"
	exit $?
fi

exec "$@"
