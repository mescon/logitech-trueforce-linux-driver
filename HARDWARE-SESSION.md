# 0.35.0 hardware session script (2026-08-14, ~60-90 min)

Supersedes MORNING-RUNBOOK.md's checklists (kept for background). Run with
a fresh Claude session driving; phases in order; each phase gates the next.

RECORD AS YOU GO: append each phase's results (pass/fail + the numbers) to
.superpowers/sdd/progress.md, and at session end update the memory file
texture-merge-shipped-2026-08-13.md with the verdicts (especially Phase 2's
gate hunt and Phase 3's latch result). Durable files are the only channel
between sessions; nothing in a chat survives on its own.

## Phase 0 - reboot + load (5 min, no wheel touch)

1. Reboot shu (clears the wedged module).
2. New session: `make -C mainline`, insmod as root, verify srcversion matches
   the fresh build, 3/3 interfaces bound, wheel attrs present.
3. `dmesg -w` in a spare pane for the whole session. Watch for
   "Cannot open FFB interface (error -19)" at any point (init_work gate).
4. REINSTALL the repo tools (the installed copies predate the final
   commits: no teardown sender, no signal hardening, pre-crash-guard dll):
   as root, from the repo:
     install -m755 tools/logi-launch.sh /usr/bin/logi-launch
     cc -O2 -Wall -o /tmp/lrb tools/logi-rpm-bridge.c && install -m755 /tmp/lrb /usr/local/bin/logi-rpm-bridge
     install -m644 tools/dinput8-escape.dll /usr/share/logitech-trueforce/dinput8-escape.dll
   Keep ~/.config/logi-wheel/games.conf UNTIL Phase 5 (the installed
   logi-wheel binary is still 0.34.0 and does not emit texture=merge; the
   override carries it, and override lines now inherit unstated keys).

## Phase 1 - teardown/oops safety (10 min, wheel idle, BEFORE gaming)

1. Original oops repro: unbind interface 2 via sysfs, then rmmod. MUST
   unload cleanly (this was a crash two nights ago).
2. insmod again; normal rmmod with all 3 bound; insmod again.
3. Replug the wheel (power-cycle counts); confirm rebind + attrs.

## Phase 2 - the gate hunt + feel test (20 min, THE main event)

Power-cycle the wheel, then launch AC EVO with plain `logi-launch %command%`.

1. Watch the range: it must snap back to your value (~2 s) with NO manual
   write - dmesg shows "restored range ... (attempt 1/3)".
2. In the car, BEFORE touching texture settings: find AC EVO's own
   audio-based-effects / TrueForce audio setting. Note its state. ENABLE it.
3. With the merge OFF (`echo 0 > .../wheel_tf_merge` as root, or pre-launch
   games.conf tweak): drive and rev. Read the escape log: do
   SetTorqueTF*/SetStreamTF counters move now? Does the SDK stream carry
   byte10 != 0 (game's own texture)? The new proxy logging also records
   GetTorqueTFRateBounds' answers - the gate evidence either way.
   - GAME TEXTURES ITSELF -> jackpot: native game TrueForce end-to-end.
     Feel it, capture 30 s for the archive, then decide recipe default
     (setting-on + merge-as-fallback). The merge auto-stands-down per
     packet; verify byte10 packets pass through unmodified.
   - STILL ZERO -> read the logged rate-bounds/gain answers; the gate is
     deeper; keep the merge as the texture source (it is already proven).
4. Merge ON + drive: rev test (buzz tracks RPM), feel vs your Windows
   memory; tune wheel_texture_intensity/cylinders live (now also in the
   GUI - visually confirm the new group + profile Save button while there).
5. Whine spot-checks while in menus: idle 5+ min (gate should silence the
   stream; listen), then drive again (texture resumes within a tick).

## Phase 3 - latch experiment (10 min)

1. Kill the game HARD (kill -9 the tree). Do NOT power-cycle.
2. Verify the productized cleanup already sent the teardown pair (usbmon or
   ep 0x83 silence); if testing the manual path:
   `python3 tests/texture-merge/synthetic_stream.py --stop`
3. Relaunch WITHOUT power-cycling. SDK streams (FFB present)?
   - YES: the power-cycle ritual is dead. Update docs.
   - NO: retry after `tools/logi-tf-init.py`; if still dead, latch is real,
     note it, power-cycle and continue.

## Phase 4 - remaining wire experiments (10 min)

1. 0x0e semantics: `--push-range 2700`, read range back; then
   `--push-range 90`, confirm auto-restore fires. Update
   docs/TRUEFORCE_PROTOCOL.md with the verdict.
2. Kernel KF idle-gate (new, param kf_idle_gate): with autocenter 0 and no
   game: confirm the KF keepalive goes silent after 500 ms (usbmon), and
   an effect upload restarts it seamlessly (fftest constant). With
   autocenter nonzero: stream persists (it is real force). If ANYTHING
   feels wrong: `kf_idle_gate=N` restores old behavior - note and move on.
3. Long-park resume: leave a stream in standby 2+ min, push force, confirm
   resume (or note that re-init was needed - firmware idle-revert).
4. Mid-waveform kill: start logi-tf-sim tone, kill it; confirm decay to
   centre (no DC torque, no eternal whine).

## Phase 5 - release (15 min)

1. Remove ~/.config/logi-wheel/games.conf (rebuilt logi-wheel now emits
   the plan itself).
2. Merge native-tf-texture-merge -> master; push master.
3. USER STEP: provide/refresh COPR + OBS account tokens so all four
   channels can publish (AUR + Debian need none beyond SSH, verified up).
4. Final release-notes read (drafts v2 in .superpowers/sdd/...): fill the
   two tag-time verifications noted at the bottom, adjust for Phase 2's
   gate verdict (the AC EVO texture section changes if the game textures
   itself!). Tag v0.35.0, publish channels, update wiki if desired.
5. Post the issue replies (drafts ready; #62's answer gains whatever
   Phase 2/3 learned).

## Abort rules

Any kernel warning/oops: stop, save dmesg, do not merge. Any FFB
misbehavior attributable to the new KF gate: kf_idle_gate=N and retest
before deciding. The release only happens on a green session.

## Final-review session rules (from the last gate)

- Phase 2/4 rule: do NOT set sysfs autocenter nonzero during an SDK title
  session (the KF gate could inject its pair into the SDK's stream in that
  non-default overlap; baseline behavior there was already imperfect). If
  autocenter testing during SDK play is wanted anyway, kf_idle_gate=N first.
- Phase 4 KF-gate wire check: also watch for the L1 half-pair signature
  (a lone 0x03 after keepalives without its 0x04) if a queue hiccup occurs;
  note what the wheel does with it (no capture precedent exists).
- The gate's resume is zero-latency host-side (first nonzero tick sends
  within 1 ms); the ONLY open question is firmware-side ramping after the
  re-arm - listen/feel for a soft-start on the first force after idle.
