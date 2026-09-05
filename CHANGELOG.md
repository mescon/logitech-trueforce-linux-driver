# Changelog

This project follows a loose semver: major versions mark API-breaking
changes to the sysfs surface, minor versions add supported wheels or
new attributes, patch versions are bug fixes and documentation. Pre-1.0
the contract is "it works on RS50 and G Pro as listed here".

## Unreleased

**A screen editor in both apps, presets first.** On the lights page the
window's "Choose what to show..." opens a preview of the base's screen
and a list of ready-made screens: gear and speed, a big speed, a rev bar,
a rev bar with gear and speed, a four-row race board, a throttle and
brake readout, throttle and brake gauges, two static messages and blank.
Picking one fills the preview, with sample values where a field is fed
from the game. "Show now" puts it on the screen, "Use during games"
makes it the simulated-TrueForce dashboard and switches that on, and
"Give the wheel its menu back" is what it says. The design behind any
preset is open for changes: a layout picker by plain name, one input per
field with what fits beside it, and the game placeholders spelled out.
The terminal app opens the same editor with Enter on the Screen row: Up
and Down pick a preset, Enter shows it, `g` uses it during games, `x`
hands the screen back and Tab opens the design. Both compose the frame
through the shared `logi_wheel_core::oled` module, so a field that will
not fit is refused with a reason before anything is sent. The window
also drew the pedal Sensitivity/Curve toggle on the screen row, a
widget-kind collision; fixed, and a test now keeps the kinds distinct.

**Rev lights and the screen in ACC on a direct-drive wheel.** The
launcher used to keep `logi-tf-sim` off for a title whose own TrueForce
reaches the wheel (ACC, AC EVO on an RS50 or G PRO), so nothing fed the
rev lights or the screen in those games. Now, where the relay can read
the game's shared memory (ACC today), the plan stages the relay and runs
the helper for the lights and the screen only. The helper enforces the
haptics-off half of that itself, from the same registry rule the launcher
uses, so it holds even when it was started by hand or for another game:
a native-TrueForce title on a direct-drive wheel never gets a synthesised
engine note, whatever the strength says. AC EVO with the texture merge
keeps its rev lights from the bridge as before.

**The screen is a dashboard during a game.** With `screen=1` in
`tf-sim.conf`, `logi-tf-sim` writes the base's OLED from the same
telemetry that drives the rev lights: by default layout G with the gear
and the speed, and `screen.template` takes any `wheel_oled` frame with
`{gear}`, `{speed}`, `{speed_mph}`, `{rpm}` and, for the gauge layouts,
`{rpm_pct}`, `{throttle_pct}` and `{brake_pct}` filled in. Written only
when the text changes, and handed back to the wheel's menu when the
session ends. Works in the lights-only mode too, so a strength of zero
gives rev lights and a dashboard with no haptics at all.

**The base's OLED can be written to.** `wheel_oled` puts a frame on the
RS50's (and G PRO's) Dynamic OLED, `wheel_oled_layouts` lists the ten
layouts and what each takes, and `off` hands the screen back at once.
Gear and speed, a gauge with a label, or four rows of text, from a shell
or from anything that reads telemetry. The protocol came from three
people over a month and was watched on an RS50 here before this was
written (#75); the driver resends a frame every 50 ms because the panel
returns to its own menu after under two seconds of silence.

**A wheel that stops answering is reported instead of pretended.** A G923
Xbox edition can wedge after a stalled motor so that HID++ commands fail
while force feedback still reports as loaded and steering still works
(#72). The driver now notices a run of transport failures, timeouts and
fast submit errors alike, and says so once in dmesg, with the recovery
its reporter measured: reloading the driver or replugging the wheel,
both of which re-run force-feedback init. Recovery is declared only after
a run of answers, since a wedged wheel produces a mix of fast failures
and silence and one stray reply is not recovery, and the rotation-range
readback poll is not counted, since some wheels never answer it and on
those it was the only traffic between games and warned on its own.

**The steering position comes from the descriptor.** The direct-drive
engine's condition effects (spring, damper, friction, inertia) read the
wheel position from the interface-0 report at a fixed offset behind a
fixed-length gate, both properties of the RS50's report; the G923 Xbox
edition's report is a different length with X one byte later, so the
gate rejected every report and its conditions saw a wheel permanently
centred and still. The axis is now located by usage in the parsed
descriptor, cached per report layout, which works on both and on
whatever the next wheel does. The lookup and its verification against
evdev are the #72 reporter's.

**The rev display runs even when the wheel refuses a stream.** A G923
Xbox edition on the firmware force path cannot take a synthesised
stream without losing force feedback, and the helper refused the session
and retried into the same refusal with the lights dark (#76). It now
drives the rev display for such a wheel and says why the haptics stay
off; `g923_xbox_dd_engine=1` remains the way to have both.

**The doctor notices a module that was updated but never reloaded, and
there is a `report` mode.** DKMS installs a new driver build and leaves
the old one running; every version number on disk then says one thing
while the kernel executes another, and nothing reported it. `setup.sh
doctor` now compares the module in memory with the one installed for the
running kernel, and lists driver source trees left behind by old
packages. `setup.sh report` prints one paste-ready block with everything
a bug report keeps being asked for: what is really running, the wheel's
firmware, what the launcher decided and set, and what DKMS believes.

## 0.39.2 - 2026-09-01

**A profile no longer undoes itself on apply.** Setting TrueForce intensity
to 0, saving and applying the profile brought it back to whatever the
wheel's onboard slot had stored, and the same happened silently to every
other setting (#73). The snapshot recorded the mode and slot selectors
last, so applying it wrote every setting and then, as its final act, told
the wheel to reload the slot over all of them. Those two are selectors
rather than settings and are no longer saved or replayed; files written
before this still apply, with their selector lines skipped. Verified on
an RS50 against the wheel's slot reload.

**The escape proxy stops writing to disk inside the game ten times a
second.** It logs what a title's TrueForce SDK is doing, and every line
was flushed before the game was allowed to continue. At the Escape stream
rate that is a synchronous write on the game's own thread for as long as
anyone is driving, paid by every user of a title that uses the merge. The
flush now happens at most twice a second, except during startup, where
individual lines still reach the disk because that is where a log has to
survive the process dying.

## 0.39.1 - 2026-08-30

**Assetto Corsa EVO keeps its frame rate with the launcher.** Launching
with `logi-launch %command%` made the game stutter badly, while blank
launch options ran perfectly with force feedback intact, which is the
signature of a cost paid per call rather than per force (#74). The escape
proxy staged into the game hooks symbol resolution so it can answer the
range getters the SDK faults on, and it decided whether each symbol
belonged to Logitech by asking the loader for the owning module's full
path and scanning it, on every symbol the game resolved, including while
drawing. That answer never changes for a module, so it is now worked out
once per module. Which symbols get wrapped is unchanged.

## 0.39.0 - 2026-08-30

**Strength zero now means the rev lights alone, not a quiet engine.**
Setting simulated TrueForce to 0% left the daemon opening a TrueForce
session anyway and streaming silence into it, which arms the wheel's
engine and takes the one-writer lease for a wheel somebody had just asked
to be quiet (#59). Zero now drives the rev display and leaves the stream
closed, which is the combination that was asked for: telemetry lights, no
haptics. Any non-zero strength behaves exactly as before.

**The light strip comes back on the setting a profile saved.** Choosing
one of the four built-in sweeps, saving a profile and loading it landed on
CUSTOM 1 every time (#73). The value was never lost: the snapshot records
it and apply writes it without error, but three attributes written
afterwards are custom-slot content, and writing any of them moves the
display onto that slot, which is what makes an edited colour appear at
all. The selection is now replayed last. Verified on an RS50 across both
built-in sweeps and custom slots.

**Two of the light-strip sweeps had their names the wrong way round.**
"Right to left" filled the strip from the left. Reported in #73 and then
watched here on an RS50, so both sweep names and the direction setting's
own labels now say what the strip does, seen from the driver's seat. A
test ties the two lists together, since naming the same sweep differently
in each place is how this survived.

**A way to give the G923 Xbox edition force feedback and TrueForce at
once.** 0.38.5 stopped the daemon silencing that wheel, but the honest
position it left was force feedback or synthesized haptics, not both,
because the wheel's own firmware sums the effects and the driver cannot
carry a force it cannot see (#72). `g923_xbox_dd_engine=1` moves that
wheel onto the engine the direct-drive wheels use, which sums the effects
here and puts them in the stream alongside the texture. The daemon
recognises a wheel driven that way and streams to it without an override.

Off by default, load-time only, and untested: it swaps a working force
path for one nobody here can try, since no such wheel is available. What
it does on that hardware is the open question, and the parameter exists
so its owners can answer it. Condition effects stay silent under it until
that edition's steering reports reach the engine, which is the next piece
of work.

A wheel this engine drives now reads as centred and still until a
steering report actually arrives, rather than as fully deflected to one
side, which is where a fresh allocation leaves it. The autocenter spring
already refused to run before the first report; a spring uploaded by a
game had nothing stopping it, and would have answered that reading with
everything the wheel has, in one direction, before anyone touched it.

## 0.38.5 - 2026-08-26

**The launcher finds its own files on a distribution that has no
`/usr/share`.** It staged the dinput8 escape proxy, the telemetry relay
and the recorded init burst from two fixed places, neither of which
exists on NixOS, where a package keeps everything together under its own
prefix. The apps reported "proxy master copy missing" on a machine whose
package had just installed it, and worse, the launcher silently staged
nothing: no engine texture, no relay telemetry, and on a title where it
also turns raw HID on, no force feedback either, because the game then
has neither the SDK path nor the DirectInput one (#70). All three
lookups, and the apps' matching one, now work out the prefix from where
they are installed. Nothing changes on a normal install, where that
resolves to the same `/usr/share` as before.

Auditing the rest turned up a fourth: the shim installer's range proxy,
the fix for the 90 degree steering clamp, was unreachable the same way.
A check now fails the build if anything looks for these files by fixed
path alone, because each of the four was written separately and each had
to remember the same thing. `LOGI_SHARE_DIR` overrides the search for a
layout nobody here has thought of.

**The G923 Xbox edition keeps its force feedback.** Starting
`logi-tf-sim` on that wheel silently zeroed its steering force while the
rev lights and engine texture carried on, which is a hard fault to
attribute (#72). The cause is structural rather than a slip: once a
sample stream runs, this wheel's motor follows the stream's force field
and stops obeying the classic force commands, so the stream carries the
live force alongside the texture. The PlayStation edition publishes that
force for us to carry; the Xbox edition's is summed inside the wheel's
own firmware, so nothing in the kernel knows it, and we were carrying a
constant zero. The daemon now refuses that wheel and says why, because on
it the honest position is force feedback or synthesized haptics, not
both. `g923.stream_without_ffb_mirror=1` streams anyway for anyone who
wants the haptics and not the force.

**Fedora: a kernel update no longer needs a Rust toolchain.** The akmod
rebuilds the driver on your machine when a new kernel arrives, and it was
asking for `cargo`, `rust` and fontconfig's headers to do it, because one
spec builds both the kernel module and the apps. Without them the rebuild
failed outright with four unmet dependencies, leaving no driver for the
new kernel (#71). None of it was ever needed: the module is C, built by
the kernel's own build system. The userspace half is now built only when
the release packages are built, so an on-machine rebuild wants a C
compiler and nothing else.

**A game's force-feedback slider now governs the synthesized haptics
too.** In a title with no TrueForce of its own, everything the wheel does
comes from `logi-tf-sim`, and it obeyed only its own strength setting: a
driver who turned force feedback down in the game still felt a
full-strength engine note, which reads as the setting being ignored
(#59). The driver publishes what the game asked for as
`wheel_ffb_game_gain`, and the daemon scales itself by it, so the slider
that quietens the forces quietens the engine note with them and zero is
silence. `follow_game_gain=0` in `tf-sim.conf` restores the old
behaviour.

Worth knowing which slider is which. A game's **TrueForce** setting
reaches its own TrueForce, which a title like Assetto Corsa (Original)
does not have; its **force feedback** setting is the one that reaches
this.

## 0.38.4 - 2026-08-21

**The G923 Xbox edition's pedals are turned the right way up too.** The
correction shipped in 0.38.1 for the PlayStation edition only, because
that was where the evidence was. Its owner has now confirmed the Xbox
edition does the same thing, 255 released and 0 fully pressed, so it gets
the same treatment (#68). Nothing else changes: the two editions order
their pedal axes differently, which matters to whichever label goes on
each bar, but all three axes take the same correction either way.

## 0.38.3 - 2026-08-21

**The Xbox edition of the G923 gets its own button names.** Its owner saw
PlayStation labels, Square and Circle and Triangle, on an Xbox wheel,
because the G923 button table was captured from a PlayStation one and
applied to both. They are different wheels: the face buttons and the
whole middle cluster differ, and so do the codes for the plus, minus and
dial, because the H-shifter's buttons sit between the wheel's own and
that cluster, so one fewer face button shifts everything after it. The
Xbox layout is now its own table, captured button by button from the
hardware by [pokesl0w](https://github.com/pokesl0w) in #68, including
the shifter's gears.

**The apps name a G923's pedals correctly.** The Test view read the
pedals off fixed axes, which are the direct-drive wheels' axes, and a
G923 does not use them: its brake appeared as the accessory handbrake and
its throttle did not appear at all (#68). Each wheel now has its own
layout. The two G923 editions are not the same either, which is why this
took a second wheel to see: the PlayStation edition sends throttle,
brake and clutch as `ABS_Z`, `ABS_RZ`, `ABS_Y`, the Xbox edition as
`ABS_Y`, `ABS_Z`, `ABS_RZ`. Both orders are read from those wheels' own
report descriptors, and the PlayStation one is confirmed on hardware
through `combine_pedals`, which merges the first two pedal bytes and so
says which they are.

## 0.38.2 - 2026-08-21

**The window runs on SteamOS.** A Steam Deck could install the driver and
run the terminal app, but `logi-wheel-gui` refused to start with
`libm.so.6: version 'GLIBC_2.43' not found` (#68). Nothing was wrong with
the Deck. glibc versions its symbols, a binary asks for whatever version
its build host offered, and ours are built on a rolling distribution, so
two drawing-stack calls (`acosf` and `atan2f`) were bound to a glibc newer
than any frozen distribution has. Those two calls now ask for the version
of each symbol that has existed since 2001, which every glibc still
provides, and the window's requirement drops to the same level as the
other programs here. Verified by running the binary on glibc 2.39, where
the previous build reproduces the Deck's exact error and this one starts.

A check now fails the build if any shipped binary needs a newer glibc than
that floor, because a dependency can reintroduce this with a different
symbol and the only sign would be a bug report from a machine we cannot
test on. It covers the truck-sim plugin too: that one is loaded by the
game rather than run by you, so a version it could not satisfy would make
the sim skip it in silence.

The README now sends Steam Deck owners to the signed repository rather
than the AUR, since building on SteamOS cannot work: it ships libraries
without the files needed to compile against them.

## 0.38.1 - 2026-08-19

**The G923's pedals read the right way round.** The wheel sends all three
inverted, 255 with the pedal released and 0 with it flat to the floor, on
both of its classic product ids. Nothing downstream corrected that,
because HID has no way to mark an axis as inverted: the wheel's own
report descriptor declares three ordinary absolute axes and every layer
faithfully passes on what it is given. The visible result was a game
seeing every pedal fully applied at rest, which is how it was reported
(#67: a replay rewinding until the clutch was pressed). The driver now
turns them the right way up, so a released pedal reads 0 and a pressed
one reads full scale, matching the direct-drive wheels and every other
Linux input device.

Two details worth knowing. The correction is applied at the input layer
only, leaving the raw HID reports exactly as the wheel sent them, because
a Wine or Proton game reading the wheel over raw HID expects the Windows
convention and correcting it underneath would break the games that work
today. And if you have already inverted these three axes inside your
games, this will invert them a second time: clear that setting in the
game, or set the `g923_pedal_invert` module parameter to `N`, which is
documented with the others in [docs/SYSFS_API.md](docs/SYSFS_API.md).
Verified on hardware, both directions, on a `c266`. `combine_pedals` is
left alone: that mode merges throttle and brake into one bidirectional
axis, so turning it round would swap the two rather than correct
anything.

## 0.38.0 - 2026-08-18

**The RS50 Xbox edition works now.** It boots as `046d:c275` speaking the
console's own protocol, with no HID++ interface for anything here to bind,
so it looked like a wheel that simply did not enumerate. One vendor
message switches it to `046d:c276`, after which it is an ordinary RS50 to
every part of this project. That switch now happens by itself on every
plug-in, the same way the G923 Xbox edition's has since 0.20.0, and it
needs `usb_modeswitch` installed. The product id and the switch were
confirmed on hardware by [kangaro0](https://github.com/kangaro0) in issue
#65, who ran the message by hand and watched the wheel come back as "RS50
Base for PC".

One helper now covers both wheels rather than one command per wheel, so
**`logi-g923-modeswitch` is now `logi-wheel-modeswitch`**. The old name is
gone rather than kept as a link: it was a copy of the whole script, and
leaving it behind would leave an older mode switch on the system for
anyone who remembers the name. Installing over an existing checkout
removes it, and the distro packages replace it on upgrade. Both apps'
diagnostics name the wheel they found in console mode rather than always
saying G923, and the doctor does the same.

**Every install now carries the whole project.** An audit of the six
install paths (Debian, Arch, openSUSE, Fedora, Nix and from source)
against everything a working setup actually loads found each of them
short of something, and none of the gaps could be seen while reading one
recipe on its own:

- Installing from a checkout never put the shim installer on the path, so
  the apps' "Install TrueForce" action could only work while a git
  checkout happened to be lying around. It also never built or staged the
  truck sims' telemetry plugin, and never installed the window's menu
  entry or icon, so the only way to start it was by typing its name.
- Arch, openSUSE, Fedora and Nix did not ship `tf-init.bin`, the recorded
  init burst `logi-launch` replays when `LOGI_TF_REARM` is set, so that
  recovery path could not work on four of the six.
- NixOS got neither the module load-order hint nor the narrow blacklist
  that keeps another out-of-tree fork from racing this driver for the
  G923, because those live in a file NixOS does not take. They are set
  declaratively now.

A test now checks the whole matrix, every piece against every channel, and
names the channel and the missing piece when one drifts. Logitech's own
SDK files are deliberately not part of it: those are yours to install from
G HUB, and this project never redistributes them.

## 0.37.1 - 2026-08-18

**Both readers of the game's telemetry get it now.** `logi-rpm-bridge`
(the rev lights and the kernel's engine-texture merge) and `logi-tf-sim`
(the synthesized engine note) read the same relay port, and the kernel
gives a datagram to exactly one socket no matter what the two of them
ask for. 0.37.0 made that failure legible: whichever lost said so and
named the other. Explaining a collision is not the same as removing it,
so it is removed. Whichever program has the port now forwards every
datagram, verbatim and before parsing it, to the fan-out ports behind it
(20782 to 20784), and whichever arrives second reads one of those. A
follower also keeps trying to take the relay port, so when the first one
exits the survivor is promoted within a couple of seconds and telemetry
aimed where every producer sends it keeps arriving. Nothing has to be
stopped and no port has to be moved to run both, `LOGI_RPM_PORT` and the
daemon's `port.relay` still move the whole block for a producer that
needs a different one, and forwarding only ever goes upward, from the
relay port to the fan-out ports, so no arrangement of these programs can
make a datagram circulate.

**Copying no longer waits for the desktop.** Both front-ends offer to copy
a launch line to the clipboard, and the helper behind that waited for the
clipboard tool to exit. On Wayland and X11 alike the tool is supposed to
keep running: a selection has no storage of its own, so the program that
owns it serves it until something else takes over. With no clipboard
manager running, nothing ever does, so the terminal app froze on the copy
key and the window left a thread stuck behind its copy button. The tool is
now given a moment to fail, and left alone if it is still alive, which is
what doing its job looks like.

## 0.37.0 - 2026-08-18

**The harsh buzz is gone, and the engine note plays at its proper
pitch.** Two programs streaming to the wheel's TrueForce endpoint do not
share it. That endpoint carries one packet per millisecond in total, so
two writers at 1 kHz take turns on it, one frame each, and because the
torque field is a level the wheel holds rather than an event, the motor
then alternates between their two values every millisecond: a 500 Hz
square wave, heard as a fixed harsh buzz that does not move with the
engine note, with the samples underneath it playing at half rate, an
octave low (#59). The kernel driver was the second writer in the cases
people hit. It now notices a userspace writer on the stream and stops
sending packets of its own, while still applying its own force by
writing it into the owner's packet in passing, so nothing goes silent.
Measured on an RS50, before and after: packets carrying samples 49% ->
99%, the gap between them 2.000 ms -> 1.000 ms, samples delivered
1934/s -> 3860/s, and the 500 Hz component gone. The wire evidence and
the rules that follow from it are in
[docs/TRUEFORCE_PROTOCOL.md](docs/TRUEFORCE_PROTOCOL.md); the
`stream_yield` module parameter turns the new behaviour off again and is
documented in [docs/SYSFS_API.md](docs/SYSFS_API.md).

Two of this project's own programs could still collide the same way (the
daemon, the app's test sweep and the range proxy all stream), so
whichever one streams to a wheel now takes a lease on it first: a lock
file per wheel under the runtime directory, with the holder's name
written inside, released by the kernel if a holder dies. The test sweep
asks for it before its countdown rather than after, and a refusal names
who has the wheel instead of the two of them fighting over it.

**Engine note latency, and the samples that used to go missing.** The
streaming thread treated one timer wakeup as one packet, so whenever the
system delivered two expiries at once the extra samples were dropped for
good. It now makes them up in the next packet, which the wheel honours
(verified by ear and on the wire against the usual four-sample form).
Pushing samples no longer blocks either: the backlog is bounded by time,
128 ms, oldest dropped and counted, where before a 1.02 second buffer
filled and stayed full, putting about a second between what the car did
and what the rim did. Measured after fifteen seconds of continuous
streaming, a change in engine speed now reaches the wire in 15 ms and
has fully taken over the note by 90 ms. The G923 path had a bound
smaller than the largest batch its producer sends, so part of every
batch was quietly discarded; both bounds now come from one constant.

**Force feedback no longer disappears for the next game.** `logi-ffb`
left the wheel's force gain at zero when it exited. That gain is a
device-wide setting nothing else resets, so the next game, DirectInput
or not, had no force at all until the wheel was replugged. It is handed
back at full gain now, which is the state a freshly powered wheel is in.

**Nothing lands on the wrong wheel any more.** With two wheels attached,
several pieces picked one by position and then held it: the first sysfs
match, the first event node that looked like a wheel, an index into a
list that gets rebuilt, a raw-HID path cached across a replug (node
numbers are recycled). So lights and settings could land on the wheel
you were not playing, and after a replug a helper could quietly stop
working. Each piece now carries the wheel's own identity instead:
`logi-launch` resolves one wheel up front and hands it to everything it
starts, the telemetry bridge writes that wheel's attributes and
`logi-ffb` drives that wheel's force node, the daemon lights the strip
of the wheel it is streaming to, and the window follows the wheel it was
managing when the list changes underneath it. The bridge also re-resolves
when the device moves, so a wheel that comes back is picked up without a
restart.

`logi-launch` also arms the engine-texture merge on that one wheel and
undoes exactly what it armed. Before, it switched the merge on for every
direct-drive wheel attached and cleared them all again on exit, so
quitting one game reached into another session. The end-of-session
TrueForce teardown goes to the same wheel, and is skipped on a G923,
which has no such engine to tear down.

**One feeder for the rev strip, one consumer for the telemetry port.**
Three things could drive the rev lights at once (the telemetry bridge,
the simulated-TrueForce daemon, and an editable row in the apps): the
daemon now stands aside while the bridge owns the strip, and the apps
show that row as the live state it is rather than a control that loses a
race it never announced. Separately, `logi-rpm-bridge` and `logi-tf-sim`
both want the telemetry port (udp/20780), and measured on this kernel no
socket option delivers the same datagrams to both. Whichever loses now
says so plainly, names the other, says what is lost and how to move one
of them, instead of one of them dying without explanation.


## 0.36.0 - 2026-08-17

**The apps now teach one recipe.** Their sections were written across
several eras of this project, and it showed: the window's force-feedback
page still taught the bare `logi-ffb %command%` line one panel below the
game rows that said the wrapper runs it, eight per-game sentences told
you to start helpers `logi-launch` starts itself, and the plan text
quoted a raw-HID setting different from the one the wrapper actually
uses. One pass over both front-ends: every surface leads with
`logi-launch %command%`, by-hand lines are labelled as the fallback they
are, Setup opens with whether `logi-launch` is installed at all (every
line it hands out depends on that), and `doctor` stops warning about
games that are already set up correctly through the wrapper.

**The rev lights get a style, and it is a setting.** Full bar by default
(first LED as the engine turns, all ten at the limiter) or the car's own
dashboard band (dark until its first shift light), chosen on the game's
card in the window or with `b` in the terminal, stored in
`~/.config/logi-wheel/launch.conf` and passed to the telemetry bridge by
`logi-launch`. The thresholds come live from the game, per car.

**Fewer silent surprises.** The simulated-TrueForce config file used to
ignore lines it could not parse without saying so, which quietly cost a
tester their first working setup; the daemon and both apps now say how
many lines were skipped and show the first one. Game cards that get the
engine-note merge say whether the SDK forwarder is staged yet, and
Assetto Corsa Competizione's card explains why it has no merge (it
produces its own TrueForce audio).

**Window fixes.**

- The rotation range's dial and big readout follow the slider while you
  drag it, instead of jumping when you let go. The readout has always
  been editable; type a number and press Enter.
- The rotation dial's arc no longer draws the long way round between
  1370 and 1882 degrees (an arc flag that assumed a full circle on a
  gauge that sweeps 260).
- Curve editor: control points now sit on the line they belong to and a
  new point lands where the preview promised. With a deadzone set, the
  handles were drawn in the curve's own space while the line was drawn
  in the deadzone-compressed one, so they disagreed.
- The rev-light picker no longer stretches to fill its card.

**Terminal app.** Everything above is in both, and the key overlay now
lists the relay-install and copy-launch keys the cards advertise; Enter
on a read-only row says who does write it; the rev-light key is offered
only where it applies; and the escape-proxy line stays put during a
rescan.


## 0.35.3 - 2026-08-16

**In-prefix helpers no longer die on the wineserver's sync flags.**
`logi-launch` started its telemetry relay with wine's default sync
settings, but Proton runs the game's wineserver with fsync enabled, and
a wine process whose flags do not match the server refuses to start:
the relay exited before its first instruction, silently under the
wrapper's quiet wine logging, which presented as "no telemetry" (#59).
Helpers now inherit exactly Proton's logic, fsync and esync on unless
`PROTON_NO_FSYNC` / `PROTON_NO_ESYNC` opt out.


## 0.35.2 - 2026-08-15

**The from-source install now ships the whole launch chain, and stale
in-prefix relays refresh themselves.** Two field reports within a day of
0.35.1 (#59, #60) traced to the same class: runtime pieces the packages
ship that other paths did not.

- `tools/setup.sh` now installs `logi-launch`, builds and installs
  `logi-rpm-bridge`, and stages the prebuilt Windows artifacts
  (`dinput8-escape.dll`, `tf-range-proxy.dll`, `logi-tf-relay.exe`,
  `tf-init.bin`) exactly as every distro package does; a checkout install
  previously ended up with a current driver and a launcher with nothing
  to stage (#60).
- The Debian package now ships `logi-launch` and `tf-init.bin`; it was
  the one channel that referenced the launcher without installing it.
- `logi-launch` now stages and refreshes `logi-tf-relay.exe` in the
  game's prefix from the packaged master copy, the same cmp-based
  refresh the dinput8 proxy already gets. A prefix's relay was a
  snapshot from whenever it was installed, and a stale one fails in
  ways that look like telemetry problems (#59).
- Its helper log line also no longer prints the helper arguments twice.
- Setup's closing advice is one line for every game and wheel:
  `logi-launch %command%`.


## 0.35.1 - 2026-08-14

**0.35.0's DKMS packages could not build (#64).** The Debian, AUR and OBS
recipes stage the module source with explicit file lists, and none of them
listed `hidpp_dd_texture_merge.h`, so every DKMS build failed at install
time on the missing header while CI stayed green (the DKMS compile happens
on the user's machine). All three manifests now stage it, and a new CI
check asserts that every header the module sources include appears in
every explicit manifest, so this class of breakage cannot ship again.
The Fedora akmod was unaffected (it copies the whole source directory).


## 0.35.0 - 2026-08-14

**Native TrueForce works, end to end, and Assetto Corsa EVO gets the
engine texture.** Two faults of ours had made the native path impossible:
since 0.27.1 the installer wrote the SDK's registry path with broken
separators, so games never loaded Logitech's DLL at all (reinstall the
TrueForce files once from the Setup page or `install-tf-shim.sh` to get
the corrected registration, which is now read back and verified before
the installer reports success); and even a loaded SDK asks the wheel's
operating range through calls only G HUB answers on Windows, so its
streaming thread never started. `logi-launch` now stages a small dinput8
forwarder per game that answers those calls with the wheel's real values
and passes everything else through.

On top of that, AC EVO's engine buzz: the game itself sends no texture
on any platform, its packets carry force with an empty sample field
while its live RPM streams out over a telemetry channel, and on Windows
it is G HUB that synthesises the buzz from that RPM and merges it into
the same stream. The driver now performs that same merge, an RPM-driven
engine note fitted to a capture of the real Windows output, spliced only
into packets whose sample field is empty, with the game's force bytes
left untouched. Along the way the session uncovered the render gate: the
wheel only plays a packet's samples when the sample count is paired with
the `0x0d` marker at byte 11, a pairing every Windows capture honours,
and one byte short of which the texture is silently discarded. Seat
verdict on an RS50: kerbs, engine and force all present together. The
texture pitch defaults to 4 cylinders after back-to-back comparison
(`wheel_texture_cylinders` tunes it live, it is a feel knob more than an
engine spec).

**The rev lights come on.** The telemetry the game was already streaming
turns out to carry Logitech's full LED triple: live rpm, the car's own
first-shift-light rpm, and its redline. `logi-rpm-bridge` now maps that
onto the wheel's rev strip: by default a full rev bar (first LED as soon
as the engine turns, all ten at the limiter), or `LOGI_REV_MODE=shift`
for the same band the car's dashboard shows. Per-car thresholds arrive
live from the game; nothing is configured. Making this safe took a
protocol correction: the driver's old first-write arming sequence, sent
mid-session, reliably killed a native TrueForce session, and G HUB's own
captures show no arming at all, just a bare two-command level update.
The driver now speaks exactly that, and force feedback, TrueForce
texture and telemetry rev lights are validated running together.

**The 90-degree lock heals itself.** The SDK writes range values straight
to the wheel at session start; the driver's automatic restore now decodes
those pushes from the wire's real bytes (the old decode read the wrong
offsets and silently ignored every live push) and puts your range back
within a strike-capped budget. Also established on hardware: those pushes
only take effect while a stream session is started, so only a live game
can move your range in the first place.

**Session lifecycle, learned from Windows captures.** Idle keepalives
carry the exact bytes Windows sends, every teardown sends the same
stop-and-arm pair Windows sends, a producer that dies mid-waveform decays
to centre instead of holding torque, and the simulated-TrueForce daemon
goes quiet in menus instead of streaming zeros. One firmware behaviour
survives: a session killed before its teardown reaches the wheel leaves
the next session opening successfully but never streaming, and only a
power cycle of the wheel recovers it today. `LOGI_TF_REARM=1` enables an
experimental pre-launch reset-and-rearm that may replace the power
cycle; it is off by default until validated on hardware.

**Crash and regression fixes.**

- 0.34.0 defaulted `inject_pid=2`, which broke steering and pedals; the
  default is reverted (#59, #63).
- A long-standing crash when unloading the module with the wheel attached
  is fixed, and interface teardown is hardened against unbind/rebind races.
- If a game's launch options still carry `LOGI_ESCAPE_RELAY=0` or a
  manually set `PROTON_ENABLE_HIDRAW` from an older recipe, remove them:
  `logi-launch %command%` is the entire recipe now, and stale variables
  starve the texture merge and the rev lights of the telemetry they need.

**App.**

- Every profile card has a Save button that updates the profile in place
  (#61).
- New TrueForce texture group on the wheel settings page (intensity,
  cylinders, live RPM-feed status).
- The onboard-slot editor's slot picker can be backed out of.
- Recipes: BeamNG.drive recorded as the native Linux title it is; iRacing
  marked TrueForce-capable per capture evidence.


## 0.34.1 - 2026-08-11

**Fixes a regression in 0.34.0 that broke steering and pedals.** 0.34.0 made
PID injection the default, which adds the part of the HID protocol that
older-style force feedback needs. It also stops the wheel's input working:
these wheels send their input reports with no report id, the injected part
declares thirteen, and a descriptor that uses report ids means every report
carries one, so the kernel then misreads every input report the wheel sends.

Measured on an RS50: no input events at all with it on, about ten thousand in
five seconds with it off. It is off again, and documented as something not to
turn on until the driver can rewrite incoming reports as well.

The force-feedback problem it was meant to solve is therefore still open. Set
`hidraw=0` for that game in `games.conf` to keep force feedback, at the cost
of the game's own TrueForce.

Everything else in 0.34.0 is unaffected and stays.

## 0.34.0 - 2026-08-11

**Force feedback no longer disappears when the raw HID interface is on.**
Setting `PROTON_ENABLE_HIDRAW` makes Proton hand the game the raw HID
device, and Wine's DirectInput drives force by writing HID PID reports to
it. These wheels have no PID collection, so a game using DirectInput for
force had nowhere to write: force feedback was there a moment earlier and
then silent.

The driver has been able to add that collection and route those writes into
its real force-feedback path since April, as `inject_pid`, switched off
because "only Proton + HIDRAW=1 users need it". That stopped being true when
this project's own tooling began setting that variable for every
direct-drive wheel playing Assetto Corsa Competizione or EVO. The situation
it exists for became the normal one while the answer to it stayed off. It is
now on by default, and `inject_pid=0` restores the old behaviour.

Confirmed on an RS50: a constant force written as PID reports drove the
wheel and released cleanly.

**Rev lights on the G923 Xbox edition**, which have never worked on Linux.
Two separate faults kept that strip dark, and either alone was enough.

The display starts switched off. `0x807A` fn2 reports which effect the strip
is showing and this wheel answers 0 for "nothing", in which state it refuses
every level with an internal error. So a completely correct command looked
ignored, which is what a month of issue #27 was spent chasing. Sending fn3
with effect 2 starts the display, after which the same level is accepted.
The call goes only to a wheel reporting 0: fn2 is not a boolean but the live
effect, and switching a wheel that IS displaying would discard the colours
its owner chose. That is why the call had been removed from this sequence
once before.

And nothing registered an LED device for that edition at all. It reaches
neither the direct-drive path nor the classic one, so a correct protocol had
nothing to write to. It now exposes the same five `::RPM1`..`RPM5` entries the
PlayStation edition does, so `logi-tf-sim`, Oversteer and any LED-aware tool
work unchanged.

The strip has been made to light on a real Xbox wheel, by its owner, using
the commands this release now sends. Following RPM in a game is new code that
has not yet run on that hardware, so treat it as expected rather than
verified.

**The rev-light command states the strip's real length.** It always claimed
ten LEDs. The level is a fraction of the stated length, so a five-LED G923
was told it had ten and showed half of everything. The count now comes from
the wheel.

**`PROTON_ENABLE_HIDRAW` names your wheel** (`0x046D/0xC276`) instead of the
bare `1`. Proton matches that variable as a substring against each device's
own id, and `1` short-circuits the test and hands **every HID device on the
machine** to the game: keyboards, headsets, other controllers. The id is read
from the attached wheel, so nothing needs configuring.

**`logi-launch` no longer trades away working force feedback.** Turning the
raw HID interface on replaces the path force feedback normally arrives by,
and this wheel's raw descriptor carries no force-feedback protocol of its
own; Logitech's SDK is what fills that gap. On a prefix without those files
the wrapper was taking force feedback away and giving nothing back. It now
checks, declines, says why, and falls back to simulated TrueForce. With the
files present nothing changes, and that remains the configuration to want:
the game's own TrueForce instead of a synthesised engine note.

**`logi-tf-sim` says when it cannot write the rev display.** A refused write
was indistinguishable from a level that had not changed, so the daemon
reported it was driving the display and then failed silently at 60 Hz. It now
names the file, the reason and the fix, once.

**Your own helpers can run inside a game's prefix**, alongside our relay
rather than instead of it:

```
LOGI_LAUNCH_HELPERS='c:\sim-teleport.exe source' logi-launch %command%
```

Semicolons separate several. Previously the only way to run something in the
prefix was `LOGI_LAUNCH_EXE`, which REPLACES the relay, so anyone bridging
telemetry to SimHub on another machine lost simulated TrueForce and their rev
lights to get it. Now they run side by side: several readers of the same
shared-memory section is not a conflict.

**The Setup page says what the launch option will do for each game**, on the
wheel being managed, with the manual steps below it as the alternative rather
than as a second conflicting recipe.

**Two development tools are gone**, replaced by the app itself:
`tools/rev-light-sweep.py` by `logi-wheel --led-probe` and
`tools/hidpp-feature-probe.py` by `logi-wheel --hidpp-features`. Neither was
installed by any package, and the app versions are the ones the
documentation points at.

**Documentation now starts with getting started**, covers all four wheels
equally, and states something never written down: for Assetto Corsa
Competizione and EVO on a direct-drive wheel, Logitech's files carry force
feedback and not only TrueForce.

## 0.33.0 - 2026-08-11

**One launch option now sets a game up, whatever wheel you have.** Put
`logi-launch %command%` in a sim's Steam launch options and it works out what
that game needs on the wheel plugged in, then does it: sets
`PROTON_ENABLE_HIDRAW` where the wheel wants it and never where it would cost
force feedback, routes DirectInput sims through `logi-ffb`, starts the
telemetry daemon, and attaches the shared-memory relay inside the game's
Proton prefix once the game is up. The same line is correct for every game and
every wheel, which the old per-game recipes were not: the same title needs
opposite settings on an RS50 and a G923, and a launch line copied from
someone else's post is the most common way a G923 owner loses force feedback.

It knows 28 titles by Steam appid. A game it does not know still gets the
daemon, `--game <name>` names one whose appid cannot identify it (a non-Steam
shortcut, a delisted title), and `~/.config/logi-wheel/games.conf` lets anyone
add or override a recipe without waiting for a release.
`logi-wheel --launch-plan --list` prints every title and what each resolves to
on the wheel you have.

**With two wheels attached it declines to guess.** The game chooses which
wheel it uses, in its own settings, and never tells us. Taking the first one
found is a coin toss whose losing side sets `PROTON_ENABLE_HIDRAW` on a G923.
`--wheel dd` or `--wheel g923` says which, and the named wheel aims the
telemetry daemon too.

**Simulated TrueForce is no longer layered on games that have their own.** The
daemon treats an unlisted game as enabled, so on a direct-drive wheel running
Assetto Corsa Competizione or EVO it was synthesising an engine note on top of
the real haptics the game was already sending.

**The Setup page says what that launch option will do for each game.** Since
the line is now identical everywhere, the page showed no sign that the recipe
behind it differs per wheel, and it sat under an instruction to set
`PROTON_ENABLE_HIDRAW=1` by hand, which read as two conflicting recipes. Each
row now describes what will happen on the wheel being managed, with the manual
steps below it as the alternative.

**Rev lights on a wheel that declares no short HID++ report.** The G923 Xbox
edition's interface declares report ids `0x11` and `0x12` and no `0x10`, but
the level sequence sent its arm burst and the command that APPLIES a level as
`0x10` anyway. hidraw accepts a write of an undeclared report id and returns
success, so all of them went out, none arrived, and `--led-probe` reported
"sent" in front of a correct command. This is why 0.32.3's five-LED fix
changed nothing on that wheel (#27).

**`logi-tf-sim` says when it cannot write the rev display.** A refused write
was indistinguishable from a level that had not changed, so the daemon printed
that it was driving the display and then failed silently at 60 Hz. Two
separate investigations ended in that blind spot, because "the lights do not
move" is also exactly what no telemetry at all looks like. It now reports the
file, the reason and the fix, once (#59).

**Extra helpers can run inside a game's prefix.** `LOGI_LAUNCH_HELPERS` starts
your own Windows programs alongside the relay, so feeding SimHub on another
machine and driving the wheel on this one are no longer alternatives.
`LOGI_LAUNCH_EXE` still replaces the relay, as before.

**Documentation now starts with getting started.** The README opened with the
project's positioning and put installation 190 lines down, and `logi-launch`
appeared once, near the bottom. The README and the wiki now both lead with the
same four steps, and both cover all four wheels equally rather than treating
the G923 as a footnote. Two G923 claims that were wrong are corrected: it does
need `logi-ffb` for DirectInput sims, and "no launch options at all" was only
ever true for the sims that are not DirectInput.

**`tools/g923-xbox-led-replay.py`**, which replays a Windows capture of a
G923 Xbox edition lighting its strip, verified byte-for-byte against that
capture. It separates the ways our sequence differed from Windows', so one run
says which mattered. It refuses to run on any other model, because feature
indices are per firmware.

## 0.32.3 - 2026-08-10

**The rev-light command now states the wheel's real strip length.** A
Windows capture of a G923 Xbox edition driving its lights under
Automobilista 2 shows the level command carrying `05` where this project
has always sent `0a`. That parameter is the strip length, and the level is
a fraction of it: measured on an RS50, "level 5 of 10" lights five LEDs and
"level 5 of 5" lights all ten. A G923 has five LEDs where the direct-drive
wheels have ten, so every rev-light command this project ever sent that
wheel described a strip twice the size of the one it has. `--led-probe`
tries both lengths now.

**`--led-probe` stopped spoiling its own results.** The classic command was
being sent to HID++ interfaces, where its report id means nothing: the write
is refused and the refusal stalls the endpoint for several seconds, so the
next test on that interface failed too and read as a result about that test.
It is skipped there now. A test number can also be given, to re-run one test
without watching the rim through all of them, and the closing prompt says
that numbering is local to a run, since it depends on how many interfaces a
wheel has and the same number means different things on different wheels.

**`tools/windows-usb-capture.bat`**, for anyone helping decode a wheel from
a Windows capture. It checks it is elevated, finds Wireshark's `dumpcap`,
records every USB interface so there is no interface to choose, and writes a
capture to the Desktop with instructions for what to do during it.

**The G923 is gear-driven, not belt-driven**, corrected everywhere including
the README's opening paragraph, and the Xbox edition's force is now
documented as arriving on its `0xFFFD` stream rather than over HID++, from
the same capture.

## 0.32.2 - 2026-08-09

**The apps now ask a wheel what it has, instead of what we thought to ask
about.** `logi-wheel --hidpp-features` walked a fixed list of the features
this project already documents, which can confirm a wheel has what you
expect and can never show what you did not. A wheel carrying an
undocumented capability looked exactly like one carrying nothing. It now
enumerates the wheel's own feature set and names the ones we have never
written down.

The gap is not small. An RS50 reports **37** features against the 14
documented here; a G923 reports **21** against 3, including `0x80A3`,
`0x8122` and `0x8124`, which sit next to features we do decode and are not
decoded at all. The driver's own log had been listing four of these all
along. This matters most on the G923 Xbox edition, where no rev-light
dialect we know lights the strip: what that wheel implements and we have
never tried is now visible rather than assumed.

**Four fixes to the diagnostic report**, all found by reading a real one.
The kernel log was never actually read: the section was hardcoded to say it
needed root, including when run as root. Packaged udev rules were listed
twice, because `/lib` is a symlink to `/usr/lib` on most distributions, and
that reads as a duplicate install. A G923 reported one attribute, dropping
gain, autocenter and combined pedals. And with two wheels attached neither
was named, so both appeared as raw HID ids. The report also says when the
loaded driver and the apps disagree about version.

**The G923 is gear-driven, not belt-driven.** Reported by a reader, and
wrong in eighteen places including the README's opening paragraph, the
driver's own comments and every distribution package description.

## 0.32.1 - 2026-08-09

**Direct-drive wheels had lost every HID++ feature.** On an RS50 or G PRO,
the firmware query, `logi-wheel --hidpp-features` and the level-dialect test
in `--led-probe` all reported "no HID++ interface could be opened" on a
wheel that plainly has one, and suggested trying sudo or checking the udev
rules, neither of which was the problem. The wheel's HID device directory
was not being carried through discovery, and every HID++ lookup starts from
it. An RS50 enumerates fourteen features again.

**The diagnostics cover every wheel, and bug reports carry the feature
map.** `--hidpp-features` and `--led-probe` described whichever wheel sysfs
yielded first, so on a two-wheel rig the second was silently untested while
the output looked complete. Both now walk every attached wheel. `logi-wheel
--report` also collects each wheel's HID++ feature map now, because that is
what a protocol question actually turns on, and a wheel that answers nothing
looks identical to a wheel nobody addressed correctly in every other kind of
report.

**The LIGHTSYNC preview button says what it does.** "Preview animation",
next to a picture of the LED strip, was read as "play this on the wheel",
which it never was: the strip is a rev-light display and holds a static
pattern until a game or telemetry bridge feeds it RPM. The button now says
"Preview on screen".

## 0.32.0 - 2026-08-09

**The Setup page was giving direct-drive owners the G923's advice.** With
more than one wheel plugged in it worked out which wheel it was describing
by rediscovering the hardware, which returns whichever wheel enumerated
first rather than the one you have selected. So an RS50 owner with a G923
also attached was told to leave `PROTON_ENABLE_HIDRAW` unset and use
simulated TrueForce for ACC and Assetto Corsa EVO, when their wheel gets the
real thing from the game through the shim. The advice was already correct
per wheel; it was being handed the wrong wheel.

**`logi-launch`, a new command, starts a helper inside the game's Proton
prefix.** Put `logi-launch %command%` in a game's Steam launch options and
the telemetry relay that simulated TrueForce needs starts with the game,
instead of being a thing you had to remember to run by hand every session.
It works out which game it is from the appid, so there is nothing to
configure, and a title that needs no helper starts nothing.

This is harder than it looks, which is why it is a command rather than a
line in the documentation. Proton takes the prefix exclusively when it
launches and waits for any existing wineserver to exit, so a helper started
first stops the game from starting at all. And the obvious way to run a
Windows binary in a prefix, `WINEPREFIX=... wine`, uses your distribution's
wine against a prefix Proton built, which prompts to install wine-mono and
can convert the prefix. Our own documentation recommended exactly that.
`logi-launch` starts the helper only once the game's own wineserver exists,
and runs it with the wine build the prefix actually belongs to.

**The apps can now talk to a G923 Xbox edition at all.** Every HID++ query
this project made was sent as a short report, and an interface had to
declare the short and long report ids to be recognised as HID++ in the
first place. That wheel does neither: it carries HID++ on its Joystick
interface with the long and very-long ids and no short one. So the
interface was never found, and a short request would have been refused
before it reached the wheel. Nothing over HID++ had ever reached that
wheel, and the failures read as a wheel that does not answer. It does:
asked properly it replies, and reports the rev-light feature at index
`0x12`. Firmware queries and feature probes work there now.

**A from-source install now installs the apps.** `sudo ./tools/setup.sh`
built the driver, installed the udev rules and the helper scripts, and then
left `logi-wheel`, `logi-ffb` and `logi-tf-sim` to the reader. Every
distribution package has always installed them, so only people building
from a checkout ended up with a current driver next to binaries they had
built by hand months earlier. `doctor` reported those versions but never
said anything when one was missing; it now fails on a missing app and warns
when one is older than the checkout it is run from.

**`logi-wheel --led-probe` tries every interface, numbered.** It used to try
two fixed guesses, and on a wheel whose interfaces are laid out differently
one of them had nowhere to go and silently tested nothing. It now walks
every interface, asks each whether it answers HID++ rather than assuming,
and prints which device node and dialect each numbered test used. `sent` was
never evidence on its own: on a G923 the classic LED command sent to the
wrong interface is accepted by the kernel and does nothing.

**Also:** the per-game cards on the Setup page no longer draw their last
line past their own edge, which was hiding a line of text entirely; every
helper documented as a bare command is now checked to be installed by every
packaging path; and `docs/LAUNCH_OPTIONS.md` collects every launch option
this project asks for, what each does and how they combine.

## 0.31.0 - 2026-08-09

**Both apps now manage every wheel you have plugged in.** Previously they
found one and ignored the rest. The window puts a button for each next to
the title; the terminal app switches with `w`. Settings, values, the live
input monitor and the tests all follow the wheel you pick, and wheels of the
same model are numbered so you can tell them apart. Each wheel keeps its own
settings on its own hardware.

**You can pick which wheel simulated TrueForce drives.** New on the Setup
page, or `wheel = auto | dd | g923` in `tf-sim.conf`. This only matters with
more than one wheel plugged in, where previously one of them could never be
reached. `LOGI_TF_SIM_WHEEL` still overrides it for a single run.

**An app that cannot find your wheel now tells you why.** Instead of one line
and a Retry button, both apps run the install checks and stop at the first
thing that is actually wrong: nothing plugged in, a G923 still in console
mode, the driver not loaded, another driver holding the wheel, permission
rules missing. Each says what it means in plain language and gives the one
command that fixes it. The window offers to run it for you; both let you copy
it, and the full list of checks is one click away for a bug report.

**`logi-rebind-wheel` now ships with the packages.** It moves a wheel that
another driver grabbed at boot onto this driver, without reaching behind the
desk to replug. It used to exist only in a git checkout, which is not much
use when your wheel does not work.

**An RS50 in compatibility mode is no longer named as a G PRO.** It borrows
the G PRO's USB id in that mode, so it was labelled wrong throughout the apps.

**The desktop app has its own taskbar icon** instead of a generic cogwheel.

**`tools/wheel-rotation-watch.py --cmd` takes a test program name and numeric
arguments** (`--cmd sine 50 2 0.3`) rather than a path resolved through
`$PATH`.

## 0.30.0 - 2026-08-08

**Simulated TrueForce is quieter by default: intensity 30, not 60.** An RS50
owner called 60 "way too powerful" and then reported 30 as fine across three
rev rates in the same session. Measured on the steering axis over a sweep, 60
moved the wheel about 604 degrees and 30 about 214. It is also the honest
lever for the low-frequency haptic layers, which move a direct-drive wheel
rather than buzzing it: master intensity scales all of them at once, where a
per-layer frequency curve would have been guesswork. A saved configuration is
unaffected.

**A diagnostic report, from the app or the command line.** In the terminal
app it is `b` on the Info page; in the window it is the Collect button on
the same page, which puts the report on the clipboard and saves a copy. From
a shell it is `logi-wheel --report`. All three print the same thing, with the
parts that identify you left out. Versions, which wheels are bound and to
what, every wheel setting and its value, the simulated-TrueForce config, and
which udev rules are installed.

The alternative advice was "paste your dmesg", and that publishes your
wheel's serial number: the driver logs it at probe, and it sits in sysfs
right next to the settings worth reading. Your profile names and lighting
slot names are worse, being whatever you called them. None of the three
helps diagnose anything, so the report shows them as withheld rather than
printing them, and the dmesg command it suggests filters the serial line out.

**The kernel's own TrueForce stream now runs at 4 kHz too, and the effect
tick that feeds it really runs at 1 kHz.** The tick asked for 2 ms and was
believed to run at 500 Hz. It ran at 333 Hz. It was a jiffies timer that
re-armed itself for the next jiffy, and the timer wheel's contract is that a
timer never fires early, so an expiry set partway through the current jiffy
was too early and slipped to the jiffy after. Every nominal interval came
back one millisecond long, measured across four of them on an RS50: ask for
1 ms and get 2, ask for 2 and get 3. A self-rearming jiffies timer bottoms
out at two jiffies, which is 2 ms where `CONFIG_HZ` is 1000 and 8 ms where
it is 250, so the rate it was asked for was never available to it.

It is now an hrtimer, programmed against the clock hardware, and `CONFIG_HZ`
does not enter into it. Measured on hardware: 1000.2 Hz, median period
1.000 ms, 99th percentile 1.003 ms, no tick longer than 1.046 ms.

**Texture is sampled across the whole timeline now, not half of it.** The
tick emits four texture samples a quarter-millisecond apart, which covers
one millisecond. While the tick actually ran every two, that left every
other millisecond with no samples in it, and the wheel had to hold or repeat
through the gap.

The frequency was never wrong. Measured on an RS50 by capturing the steering
encoder during a periodic effect and reading the dominant frequency off it,
both the old and the new build render a requested 50 Hz at 50.0 Hz and a
requested 100 Hz at 100.0 Hz. What changed is how much of the waveform the
wheel is given to reconstruct from, not what note it plays.

The tick also computes the steering force sum, so this triples the rate at
which game force is sampled. Under a steering force and a TrueForce stream
together the driver now puts 990 packets per second on the wire, every one
accepted at its full 64 bytes with no submission errors. That is close to
the ceiling by design: the wheel is full-speed USB with a `bInterval=1`
interrupt OUT endpoint, which allows exactly one packet per millisecond.

**LIGHTSYNC custom colours now actually appear on the strip.** They never
have, on any RS50, in any release. Setting colours returned success, the
wheel acknowledged every command, and the strip stayed dark, which made this
look like hardware or a wheel-side setting rather than a bug here.

Two faults in the same sequence. The apply switched every LED **off**
immediately before uploading the colours meant to be displayed: it sent a
`0x807A fn6` "pre-config" whose byte 5 is the rev-display *level*, not a LED
count and not a flag, and it sent zero there. The same command layout with a
level of 10 is what lights all ten LEDs for the rev display. Second, the slot
the colours were written into was never activated, so the upload was stored
and never shown. G HUB follows every colour upload with an activate on
`0x807B fn3`, and the constant for it already existed in this driver with
nothing calling it.

Verified on an RS50 after the change: three, then seven, then one lit LED
give three, then seven, then one; all-blue to all-yellow changes; half and
half shows two distinct halves; and alternating neighbours is visible. Before
it, the same sequence left the strip dark.

Two things worth knowing if you go looking at this yourself. The colour
upload is a **very long 64-byte report**, so a capture filter that matches
only the 20-byte and 7-byte HID++ reports will not show it at all and will
make the wire format look invented. And the strip only displays on some
onboard profiles: on a profile that keeps it dark, every write still reports
success.

**The LED state the apps show is now the wheel's, not a guess.** Several
things conspired to make the strip on screen disagree with the strip on the
wheel.

Effects 5 to 9 are the five custom slots rather than five animations, but
only effect 5 was recognised as one, so selecting 6 to 9 switched the wheel
to another slot while everything downstream still reported the first. The
driver also assumed effect 5 at every probe instead of asking, so a wheel
that came up on a built-in sweep was reported as a custom slot until
something wrote the attribute. It asks now, and it follows the wheel's own
effect-change broadcasts to the slot they carry, which is the path the
wheel's on-device menu takes.

The colours themselves were read once at load and never again. They are
re-read when an effect selects a different slot and when the active profile
changes, since a profile swap replaces the wheel's LED configuration
wholesale.

In the app, changing the effect now reloads the LED page, the preview
animates the sweep the selected effect plays rather than the slot's own
direction, and the four built-in sweeps preview as motion without colour:
they render a palette held in the wheel's firmware which nothing reports, so
showing the slot's colours there claimed something untrue. Custom slots are
unchanged, and their colours are shown as before.

**The driver reports which HID++ features a wheel has**, one line at probe.
Userspace cannot find this out: the driver parses HID++ replies and tells the
kernel it consumed them, so a hidraw reader sees nothing on any wheel the
driver is talking to, and reads that silence as absence. `docs/FEATURE_MATRIX.md`
records what both wheels here answered, including four features present on
the hardware that this driver does not implement.

The line says so when it is incomplete, on the line itself. A wheel that
declines to answer one page costs a full send timeout, and the scan's whole
allowance had been exactly one timeout, so a single unanswered page ended it
and dropped every page after. On an RS50 that truncated four scans in five,
reporting between 3 and 15 features where the complete answer was 17, and it
said so only on a separate line that did not travel with the results when
someone pasted them. Since a short list would otherwise read as a complete
one, and concluding a wheel lacks a capability nobody asked about is exactly
the mistake this project keeps having to avoid, the scan now gets room for a
stalled page or two where it runs off probe context, and states on the
results line how many pages it reached. Six consecutive scans after the
change: all complete, 17 features, 0.75 seconds each.

**Simulated TrueForce now streams at 4 kHz instead of 1 kHz.** Logitech's own
figure for TRUEFORCE is a 1 ms processing interval, and both transports were
measured sustaining exactly that: one packet per millisecond carrying four
samples. The old rate was a quarter of it. What that buys is headroom the
engine note did not have before: at 1 kHz nothing above 500 Hz survives, so
the note's upper harmonics had to be faded out as revs rose and a high rev
rate degenerated into a plain tone at the top end. At 4 kHz the third
harmonic does not reach that limit until around 13000 rpm.

On a G923 it improves force feedback as well, for an unrelated reason: the
game's force reaches that wheel through the same packet stream, resampled at
the stream's rate. At 250 packets/sec a game sending force feedback faster
than that was being undersampled; Assetto Corsa Competizione offers 400 Hz.

**Two timing bugs in the haptic layers, both user-visible.** Anything
time-dependent in an effect runs once per rendered block, and two effects
assumed a block was a millisecond. The daemon renders roughly 50 ms at a
time, so the airborne duck's 60 ms ramp really took about three seconds, and
the rev limiter counted blocks against a 150 ms threshold, wanting several
seconds of sustained limit before engaging. Both now work from the block's
real duration. (The airborne duck was unreachable in practice, since no
telemetry decoder sets that flag yet; the rev limiter was not.)

**The default rev rate moves from 25 to 35.** 25 reproduced exactly what the
daemon emitted before it modelled cylinder firing rate, which was the right
way to correct the arithmetic without changing anyone's feel. Hardware
measurement has since shown it sits at the wrong end of the range: sampling
the steering axis through a sweep, 25 moved an RS50 899 degrees where 40
moved it 552, because a lower note is one a direct-drive wheel can follow far
enough to become steering input rather than texture. A saved configuration is
unaffected.

**A direct-drive wheel no longer thrashes when simulated TrueForce plays.**
Streaming an engine note to an RS50 drove the wheel into its stops and left
it oscillating there: measured on the steering axis, a sweep travelled 1258
to 1703 degrees and hit a stop every run. The cause was not anything the
stream sends. The wheel needs a live force-feedback session for its control
loop to stay stable while TrueForce samples arrive, and a game always has
one open, which is why real TrueForce on the same wheel was always fine and
why our own self-test was not. The daemon now holds a zero-level effect open
for as long as it streams. The same sweep travels 204 to 488 degrees and
reaches no stop. Force is zero by design: this keeps the loop alive, it does
not change how the wheel feels.

**The engine note no longer buzzes at high pitch.** Above roughly 5000 rpm
at `pitch = 100` the third harmonic crossed the sample stream's Nyquist limit
and folded back on top of the fundamental, which is felt as a buzz rather
than an engine. Harmonics now fade out as they approach the limit. Default
settings are unaffected: at `pitch = 25` nothing ever reaches the fade.

**Starting TrueForce no longer overwrites your steering range.** The
captured init sequence carried the operating range of the wheel it was
recorded from, 2700 degrees, and replayed it verbatim on every session. The
kernel's range restore healed it within 100 ms, which is why nobody noticed.
It now carries the range your wheel is actually set to.

New diagnostics, neither of which needs anything built:

- `logi-wheel --hidpp-features` lists the HID++ features a wheel implements.
- `logi-wheel --led-probe` sends each known rev-light command in turn, so
  somebody watching the rim can say which one a wheel obeys. The feature
  list cannot answer this: a PlayStation G923 reports LIGHTSYNC and ignores
  it completely for rev lights.
- `tools/wheel-rotation-watch.py` samples the steering axis while something
  drives the wheel, so motion is measured rather than described.
- `tools/hidpp-feature-probe.py` asks the same questions as
  `--hidpp-features` in stdlib Python, for testers without a Rust toolchain.

## 0.29.0 - 2026-08-06

**If you own a G923, this release is mostly for you.** Assetto Corsa
Competizione and Assetto Corsa EVO now produce engine haptics on that wheel
for the first time. Their real TrueForce goes through a Logitech SDK the
G923 does not answer, so it never arrived; this synthesizes an engine note
from the game's own telemetry instead and sends it by a route that needs no
SDK cooperation. Confirmed on hardware, not just written: with the car
stationary in the pit box and the engine revving, the wheel buzzes.

**Two things need action.** The rev lights on a G923 have never worked for
anyone, because their brightness files come up root-owned and no rule
granted access; reinstalling the udev rules fixes that, and a reinstall or
`sudo ./tools/setup.sh` does it. And the pieces that read telemetry out of a
running game are not in the distro packages, because one is a Windows
executable and the other is a plugin the game loads: both are now attached
to this release as downloads.

On a direct-drive wheel nothing here changes. Competizione and EVO still
deliver their own TrueForce through the shim with
`PROTON_ENABLE_HIDRAW=1`, exactly as before.

### Added

- **Simulated TrueForce in every sim that publishes to shared memory**:
  iRacing, RaceRoom Racing Experience, Assetto Corsa, Competizione, EVO,
  rFactor 2 and Le Mans Ultimate. A small relay runs inside the game's
  Proton prefix and forwards what it reads to the daemon. rFactor 2 and Le
  Mans Ultimate also need the community `rF2SharedMemoryMapPlugin`. See
  [`docs/SHARED_MEMORY_RELAY.md`](docs/SHARED_MEMORY_RELAY.md).
- **Simulated TrueForce in Euro Truck Simulator 2 and American Truck
  Simulator.** These publish telemetry through a plugin interface rather
  than over UDP, so a small native Linux plugin now forwards engine speed,
  throttle and gear to the daemon. No Wine involved. See
  [`docs/SCS_PLUGIN.md`](docs/SCS_PLUGIN.md).
- **Simulated TrueForce in GRID (2019) and GRID Legends.** No new code: both
  are the same Codemasters telemetry format the DiRT titles use and the
  parser already read them. What was missing was saying so, and telling you
  to switch the game's UDP output on.
- **The helpers install themselves.** `sudo ./tools/setup.sh` places the
  relay in every Proton prefix and the truck-sim plugin in both truck sims,
  and the settings app has a per-game "Install relay" button (`h` in the
  terminal app). Both are also packaged and attached to each release. The
  intent is that installing this project leaves you needing only Logitech's
  own DLLs, which cannot be redistributed.

The Assetto Corsa family's decoders were confirmed against running games on
2026-08-06, including the two offsets most likely to be wrong: the redline
behind five `wchar_t` arrays in the older titles, and EVO's `currentMaxRpm`,
which has no structural guard at all. The rest are written against published
layouts and range-gated, but nobody has driven them yet, so they carry the
provisional marker in [`docs/GAME_SETUP.md`](docs/GAME_SETUP.md).

### Fixed

- **A G923's rev lights could never light.** Their brightness files are
  root-owned and nothing granted a desktop user access, so every write
  failed silently while the daemon reported it was driving the display. The
  direct-drive wheels use a different attribute that this project already
  made writable, which is why it went unnoticed. Confirmed working after the
  fix by sampling the five brightness files through a rev sweep: they fill
  one at a time to all five and drain back in order.
- **With two wheels attached, the rev display of the wrong one was driven.**
  The daemon vibrated the wheel it had opened and lit whichever wheel sysfs
  listed first.
- **Engine haptics fell further behind the longer you drove.** On a G923 the
  writer consumed samples fractionally slower than the daemon produced them
  and the surplus accumulated without limit, so throttle response lagged
  more with every minute. Roughly 110 ms of added delay per second of
  driving. A steady idle felt fine throughout, which is why it survived: a
  constant signal hides latency and a changing one exposes it.
- **iRacing owners were told to turn on simulated TrueForce instead of being
  given their launch options.** iRacing needs `logi-ffb %command%` to have
  any force feedback at all, and the recipe dropped that the moment the game
  gained a telemetry decoder, so the advice was to enable an engine note for
  a wheel that was not being driven.
- **Assetto Corsa was listed as having no usable telemetry.** It publishes
  both a documented UDP protocol and a shared-memory block.
- **Simulated TrueForce could not be switched on in the GUI for any game the
  relay serves.** A row only carried its per-game switch when simulated
  TrueForce was the game's *primary* setup action, and it never is for these:
  Competizione and EVO want the shim, and iRacing, RaceRoom, rFactor 2 and Le
  Mans Ultimate want logi-ffb. All six had working simulated TrueForce and no
  way to enable it.
- A crate declaring a minimum Rust of 1.74 used an API stable only since
  1.82, so it would not have built on the version this project claims to
  support.

### Changed

- Each telemetry source now carries its own game id, so every relayed title
  gets its own enable switch and intensity instead of sharing one.
- The relay is now built and linted for Windows in CI. Everything in it that
  touches shared memory sits behind `cfg(windows)`, so nothing had ever
  compiled the code users actually run; the first run of that job found a
  real defect.

## 0.28.0 - 2026-08-06

**If you use computer profiles and have a custom pedal or steering curve
loaded, update before saving another profile.** Saving one recorded the curve
as "reset", so applying that profile later reverted the curve to the
built-in one. Profiles saved by earlier versions may already carry that;
check a profile file for `wheel_response_curve=reset` (or the
`wheel_*_curve` equivalents) and delete the line if your wheel has a curve
you want to keep.

Other things you may have run into, and no longer will: the shim installer
exiting without printing anything, `sudo ./tools/setup.sh` skipping the
TrueForce shim and then reporting in the same run that the files were staged,
a re-run of setup quietly undoing the rotation fix for the 90 degree steering
clamp, and being told to set `PROTON_ENABLE_HIDRAW=1` for games where that is
exactly what stops force feedback.

Removes the `revlights` tool, which is why this is a minor rather than a
patch release.

### Fixed

- **Saving a computer profile could wipe the wheel's response curve.** A
  loaded curve read back as "built-in" everywhere in the app, so the profile
  recorded it as `reset` and applying that profile reverted the curve. A
  curve the wheel cannot read back is now left out of the profile rather than
  recorded as an instruction to erase it.
- **Unplugging the wheel while force feedback was still starting up could
  crash the kernel.** If force feedback failed to initialise, the driver
  freed state it went on using for every settings read, and freed it a second
  time on unplug. Force feedback failing now leaves the settings working
  rather than taking them with it.
- **The shim installer could exit with no output at all** when the SDK
  directory existed but held no version-named subdirectory: the exact result
  of copying G HUB's `Logi` folder but dropping the DLLs one level too high.
- **Full setup skipped the shim for anyone who staged the SDK where the README
  says**, because the check ran as root while the install ran as the user. The
  same run then reported both "not staged" and "all four staged".
- **`doctor` could not detect the "wheel steers but produces no force at
  all" state** it was written to catch, so it stayed silent through exactly
  the failure people were reporting.
- **Re-running setup silently reverted the rotation proxy**, restoring the
  90-degree steering clamp a user had deliberately fixed.
- **iRacing and RaceRoom owners got no warning** that `PROTON_ENABLE_HIDRAW=1`
  is what stopped their force feedback; the list carried two of four titles.
  The registry now owns the appids and a test fails when the two disagree.
- `make -C mainline` pointed the build at the wrong directory.
- **`logi-tf-sim` did not recognise the PlayStation/PC G923** (`c267`). It
  kept its own product-id list and that copy still had the gap after the
  other was fixed, so the wheel was named correctly by the settings pages and
  then not found at all by the daemon.
- **The simulated-TrueForce rev rate had three different defaults** (25 in the
  daemon, 50 in the front-end mirror, 100 in the UI's initial value), so a
  fresh install showed 50% while running at 25%.
- **Pedal deadzones could not be edited in the terminal app at all.** The
  editor opened, every key did nothing, and Enter reported success having
  changed nothing. Left/Right now moves the selected half, Up/Down picks
  which half.
- **`--range-proxy` was unreachable for every package user**: no channel
  shipped the proxy DLL, and the installer looked for it only inside a git
  checkout. All four formats install it now.
- **The Arch package installed a udev rule without the binary it calls**,
  leaving a G923 Xbox edition stuck in console mode and looking like dead
  hardware.
- **`usbutils` was declared by no package** while the tooling needs `lsusb`,
  and without it the wheel-capability check fell through to the answer that
  tells G923 owners to set the variable that removes their force feedback.
  The check reads sysfs first now, so it no longer depends on an optional
  binary.
- **The Setup page approved SDK folders the installer then rejected**,
  checking one of the four required DLLs.
- **`--uninstall` could leave a half-uninstalled prefix** that the app still
  reported as installed, and never removed Logitech's own library that
  `--range-proxy` had moved aside.
- **Steam libraries**: three components disagreed about where to look, none
  of them knew about Flatpak Steam, and one counted symlinked libraries
  twice.
- **`doctor` said nothing when an in-tree Logitech driver won the bind race**,
  the exact situation the rebind helper exists for, and its stale-module
  warning was permanently wrong on any tree with uncommitted changes.
- **`dkms-update.sh` skipped the shim unless `winegcc` was installed**, which
  nothing has needed since the shim became a copy of Logitech's own DLLs.
- Documentation that contradicted the code: the rev-light cadence was
  documented as 160 ms against an actual 10 ms floor (sixteenfold), and a
  brightness/sensitivity aliasing that was disproved on hardware was still
  described.

### Removed

- **`userspace/revlights/`**, superseded by `logi-tf-sim`'s rev-light feeder.
  It was in no package and no README, and running it meant two processes
  writing the same sysfs attribute at their own rates.

## 0.27.4 - 2026-08-06

### Fixed

- **The app ignored any SDK version other than 1_3_11.** v0.27.1 taught the
  shell installer to discover whichever version G HUB shipped, but the same
  assumption was left hardcoded in the app: a 1_3_12 install had its SDK
  folder marked invalid on the Setup page, and a prefix that had just been
  staged still read as "shim not installed". The version is discovered on
  both sides now.
- **`doctor` looked for the SDK only inside a repo checkout.** It hardcoded
  `$REPO_ROOT/sdk`, so anyone who installed from a package was told the DLLs
  were missing however correctly they had staged them, and the report never
  said where it had looked. It now asks the installer where the SDK actually
  resolves to, via a new `install-tf-shim.sh --print-sdk-dir`, and names that
  directory in both the pass and the warning.
- **The shim installer could exit silently with nothing staged.** Under
  `set -e`, the version lookup added in v0.27.1 returned non-zero for a
  directory that does not exist yet, killing the script before its own
  fallback: no install, no error. A first-time user now gets the list of
  files it expected to find.
- **`doctor` told DirectInput sims to set `PROTON_ENABLE_HIDRAW=1`.** One
  undifferentiated list of six appids drove the launch-option check, so Le
  Mans Ultimate and rFactor 2 owners were told to set the variable that stops
  force feedback reaching those games, and two more titles that need nothing
  were warned about for no reason. The list is split by what each game
  actually needs now, and the check is wheel-aware: a G923 with the variable
  set is told plainly that it is what is stopping its force feedback.
- **The installer's closing advice told everyone to set
  `PROTON_ENABLE_HIDRAW=1`**, with no mention of which wheels and games that
  is right for. It is now printed only where it applies, and a wheel without
  SDK TrueForce is told to leave the variable alone.
- **Running the full setup from a packaged install failed several steps in**
  on a missing script, instead of saying that the driver is already installed.
- **`doctor` counted every Steam library twice** (`~/.steam/steam` is normally
  a symlink to `~/.local/share/Steam`, and only the strings were deduped), and
  **never read `libraryfolders.vdf`**, so games on a second drive were
  invisible to it while the installer staged into them happily.
- **A populated repo `sdk/` was passed over.** Its marker test compared a
  glob literally, so the check never matched and the installer fell through
  to the XDG directory even with the DLLs sitting in the checkout.

### Added

- **The README now says where the SDK files go**, with the directory layout
  and the ways to point at a different one. Nothing documented this, which is
  most of why it was hard to tell a wrong location from a bug.

## 0.27.3 - 2026-08-05

### Fixed

- **`setup.sh doctor` told every owner about G923 udev rules.** Three checks
  in the permissions section printed regardless of what was plugged in, so an
  RS50 owner saw "G923 (c266/c267/c26e) rebind rule installed" in a report
  about his own machine and reasonably concluded doctor had misidentified his
  wheel. They now appear only on a machine that has a G923, keyed on USB so
  they still show when the in-tree driver grabbed the wheel and it has none
  of our sysfs attributes.
- **The openSUSE publish job could fail before doing anything**, with zypper
  reporting no provider for any package including ones already installed. It
  refreshes the repository metadata explicitly now instead of relying on
  zypper's implicit auto-refresh.

## 0.27.2 - 2026-08-05

### Fixed

- **The Setup page gave G923 owners advice that costs them force feedback.**
  Both front-ends resolved a game's setup recipe from the title alone, so a
  G923 owner with Assetto Corsa Competizione installed was told to install the
  TrueForce shim and set `PROTON_ENABLE_HIDRAW=1`. That wheel does not answer
  the TrueForce SDK, and the variable diverts the game to raw HID reports it
  cannot drive feedback through, so following the advice lost them the force
  feedback they already had. A recipe now resolves from the (game, wheel)
  pair: the SDK titles degrade to "nothing to do" on a wheel that cannot use
  the SDK, and say to leave the variable unset.
- **The G923 PlayStation/PC edition (`046d:c267`) was not recognized by the
  settings app.** The kernel driver binds all three G923 product ids; the app
  knew only two, so that wheel showed up as a generic "Logitech Racing Wheel".

### Added

- **`docs/GAME_SETUP.md`**: every known game against every supported wheel,
  with the exact Steam launch options each pair needs. Generated from the same
  registry the app reads, by a test that fails if the file drifts from it, so
  the doc and the app cannot disagree.
- **Launch options are shown and copied, per game.** The Setup page spells out
  the exact string a game needs on your wheel: `c` copies it in the terminal
  app, a Copy button in the desktop one. Nothing is written to your Steam
  configuration, because Steam rewrites `localconfig.vdf` when it exits and an
  edit made underneath a running Steam is lost.

## 0.27.1 - 2026-08-05

### Added

- **`libtrueforce` now matches the real SDK's calling convention.** Seventeen
  of its fifty-four entry points were declared to return a value where the
  real library reports through an out parameter and returns a status, so no
  program written against Logitech's SDK could call them. All fifty-four now
  agree with the shipped library, checked against its own machine code rather
  than a header. `logiWheelGetVersion` had also invented a leading index
  argument it does not have. This changes the library's ABI, which costs
  nothing, because the old signature could never have been linked against
  successfully. See `docs/SDK_ABI_NOTES.md`.


- **A rotation shim for sims that ask the SDK how far the wheel turns**
  (`./tools/install-tf-shim.sh --all-steam --range-proxy`). Under Proton the
  TrueForce SDK cannot reach G HUB, so it falls back to 90 degrees, the
  minimum of the wheel's legal range, and the game clamps steering at 45 each
  way. The shim forwards every other SDK call to Logitech's own library
  untouched and answers only the rotation question, from this driver. If
  Logitech's library is not beside it, it refuses to load rather than leave a
  wheel that steers correctly and produces no force at all. Not yet confirmed
  in a game (issue #27).

### Fixed

- **`libtrueforce`'s rotation getters had the wrong signature.** They were
  declared as `double f(int)`; the real SDK reports through an out parameter
  and returns a status, `int f(int, double *)`. Verified against the shipped
  library's own code, where the index arrives in RCX, the out pointer in RDX
  is null-checked, and `0x80000001` comes back when it is null. No program
  written against the real SDK could have called ours.

- **`libtrueforce` reported no rotation range for a G923.** Its range getter
  read `wheel_range`, which only the direct-drive wheels have; a G923 calls
  the same setting `range`. It returned 0, which a caller may reasonably read
  as "no wheel" rather than "wheel present, range unknown". It now tries both.

- **`doctor` told every G923 owner their wheel was not plugged in.** It
  looked for the three direct-drive USB ids only, so a G923 was invisible to
  it however well the wheel was working, and the driver-health section was
  skipped as a knock-on. It also read `wheel_*` attributes that a G923 does
  not have, so even once found it would have called a healthy wheel unbound.
  It now knows both wheel families and both attribute sets, reports a rig
  with more than one wheel properly instead of leaving all but the first
  unlabelled, and recognises a G923 Xbox still sitting in console mode
  (`c26d`) as exactly that rather than as no wheel at all (issue #27).

- **`doctor` told people to set a launch option they had already set.** It
  read `PROTON_ENABLE_HIDRAW=1` by finding the first line mentioning a
  game's app id anywhere in Steam's config and then taking the next
  `LaunchOptions` it saw. An app id appears several times in a real
  `localconfig.vdf` (six, in the one measured), and if the block it landed on
  had no launch options the scan ran on and reported a different game's. On a
  real config it got two of three wrong, both false negatives. It now reads
  each game's own block.

- **`doctor` counted every Proton prefix when checking for the TrueForce
  shim**, so the warning scaled with the size of a Steam library rather than
  with anything being wrong: one report read "shim in 50 of 52 prefixes",
  which is 50 more shims than that person needed. Only the sims that load
  Logitech's SDK need it, and only those are counted now.

- **Corrected the explanation shipped in 0.27.0 for a failed range read.**
  Those notes said a mid-session failure was most likely contention with a
  game's force-feedback traffic. An owner's log disproves it: the read
  succeeds at probe and fails seven seconds later with no game running at
  all. The cause is not established, and the driver no longer guesses at one.
  It has no practical effect, since on that wheel the range is never moved
  and so there is nothing to restore either way (issue #27).

## 0.27.0 - 2026-08-04

### Changed

- **The G923 Xbox edition's `range_restore` is now on by default**, matching
  `wheel_range_restore` on the direct-drive wheels. A wheel a game has
  collapsed to 90 degrees is unusable, and leaving the cure behind a switch
  only helps people who read the issue tracker. It still never writes unless
  the wheel disagrees with the range this driver set, never while the rim is
  away from centre, and never more than three times in a session. Set it to
  `0` to stop it.

### Fixed

- **A single failed range read no longer reports the restore as impossible.**
  The message shipped in 0.26.0 said the wheel could not be read at all, on
  the strength of one timeout. The same read succeeds during probe on that
  wheel, so a miss mid-session is most likely contention with a game's
  force-feedback traffic. It now says the read failed and that it is still
  trying, and only remarks on persistence after ten consecutive misses
  (issue #27).

- **`docs/TRUEFORCE_PROTOCOL.md` listed the G923 Xbox edition as honouring
  the operating-range push.** It does not: an owner's rim keeps its full
  travel in ACC's config screen while the limit appears only on track, which
  a real range change could not do. The game clamps its own steering there,
  because the TrueForce SDK falls back to the minimum of the legal range when
  it cannot reach G HUB (issue #27).

## 0.26.0 - 2026-08-03

### Added

- **A haptic effects layer for simulated TrueForce.** Until now the wheel felt
  exactly one thing: the engine. There are now ten layers: the engine note,
  both limiters, gear shifts, the ABS pump, traction loss, surface texture,
  airborne, impacts and DRS. Each has its own gain in `tf-sim.conf`
  (`effect_gear_shift=60` and so on, 0-100), and `effects=0` restores the
  engine-only behaviour exactly.

  How much you feel depends on your game, and each slider says which games
  feed it. Only the engine note and the rev limiter work everywhere; the pit
  limiter, gear shifts, ABS and traction need OutGauge, which here means
  BeamNG.drive. Surface, airborne, impacts and DRS have no source at all yet.
  None of it applies to games with built-in TrueForce, which get their effects
  from the game itself.

  Tunable from both apps, under Setup's "Simulated TrueForce": a switch and a
  slider per layer in the GUI, and the `x` / `l` / `[` `]` / `v` keys in the
  terminal app. Only the engine layer is hardware-validated, so reports on how
  the rest feel are welcome.

- **OutGauge now decodes gear, brake, clutch, and the ABS and traction
  lamps.** These feed the effects above. The gear field also reaches the
  shared-memory relay format, which had carried one all along with nowhere to
  put it.

### Fixed

- **The G923 Xbox edition came up dead after a from-source install.** Upgrade
  if you own that wheel and installed with `tools/setup.sh`. The udev rule
  that switches the wheel out of Xbox console mode was installed, but the
  helper script that rule runs was not, and the rule dispatches it through
  `systemd-run` with the output discarded, so the failure produced no error,
  no log line and no switch. The wheel stayed in console mode and looked like
  hardware that had died. Distro packages (AUR, Debian, COPR, OBS) always
  installed the helper and were never affected. The rule now refuses to fire
  without it, and `tools/setup.sh` reports the helper on its own line, since
  "rule installed" was true throughout and told owners nothing (issue #27).

- **`range_restore` needed a root shell.** The 90-degree restore switch was
  added to the driver but left out of the list of attributes the udev rule
  makes writable, so turning it on meant becoming root rather than writing it
  like every other knob beside it (issue #27).

- **Setup page sections were drawn several times taller than their contents**,
  with each title floating in the middle of an empty box and an open section's
  body rendered outside its own border.

- **Saving the config dropped `cylinders`.** It was read from the file but
  never written back, so any value other than the default was lost the next
  time anything saved. The round-trip test missed it by having picked the
  default for that one field.

## 0.25.0 - 2026-08-02

### Fixed

- **The G923 Xbox mode-switch rule could freeze the machine.** Upgrade if you
  own that wheel. The rule ran `usb_modeswitch` directly from udev, which
  means inside the udev worker, holding the device lock for the whole USB
  transfer. On a handheld whose built-in controllers sit on internal USB
  that read as a total system freeze, and the machine would not finish
  booting while the wheel was plugged in. The switch now runs outside udev,
  and releases whatever already holds the wheel first, since in console mode
  an Xbox controller driver claims it before we get there. Reported with a
  diagnosis better than the bug deserved (issue #52).

  On a system without systemd the rule now does nothing rather than risking
  that, and the switch can be run by hand with `sudo logi-g923-modeswitch`.

### Added

- **`range_restore`**, for wheels whose force feedback runs over HID++,
  which today means the G923 Xbox edition. **Off by default.**

  A game reaching the Logitech SDK directly, which on Linux means
  `PROTON_ENABLE_HIDRAW=1`, pushes its own steering rotation at the wheel and
  can soft-lock you at 45 degrees each way. Every game tested does it; they
  differ only in whether you can steer through it. Turning this on puts your
  range back within a couple of seconds.

  It defaults to off because it writes a rotation range while a game is
  holding your wheel, and on the direct-drive wheels mistiming that
  desynchronised the centre badly. The check that prevents it here is
  believed equivalent to theirs but has not been proven on this hardware, so
  it is opt-in rather than assumed safe. Writing the range back by hand once
  after the game starts remains a complete alternative. See
  `docs/SYSFS_API.md`.

### Documentation

- **The RS50's Dynamic OLED is largely decoded**, contributed by @PeposCJ:
  the command set, ten layouts, and the finding that governs any future
  support, which is that the panel takes typed text fields per layout rather
  than a picture. The driver still sends nothing to it.
- **What a busy HID++ channel tolerates**, from @Mhytee's TF4ALL work and
  corroborated independently: while a game's force is on that channel, any
  other write to it cuts the force, and pacing does not help. This corrects
  guidance in this project's own notes, and it is why the Xbox edition's rev
  lights cannot simply be switched on.
- **The PlayStation G923 ignores the rotation push** described above, tested
  on hardware, so it needs no restore of its own.

## 0.24.0 - 2026-07-30

### The G923 Xbox edition works

**Force feedback and TrueForce both work on the G923 Xbox edition for the
first time.** Hardware-confirmed by the wheel's owner in
[#27](https://github.com/mescon/logitech-trueforce-linux-driver/issues/27):
force feedback in Assetto Corsa Competizione, and TrueForce in
Automobilista 2. Nobody working on this project owns that wheel, so all of
it was found by someone running diagnostics on hardware that was giving him
nothing back. Every fix below came out of that.

- **Force feedback never registered, and took the USB device down with it.**
  Every command this driver sent that wheel went out as a control transfer
  rather than through the interrupt endpoint it answers on, so the driver
  could not read the wheel's force-feedback configuration and gave up on the
  device entirely. The in-tree driver then claimed the same interface, and
  the wheel wedged badly enough that even listing USB devices hung.
- **A force-feedback problem no longer costs you the whole wheel.** The
  driver used to refuse a wheel outright when force feedback would not
  start, which handed it to another driver: strictly worse than simply
  having no force feedback, since steering, buttons and pedals work
  regardless. It now warns and carries on.
- **TrueForce is found on wheels with two USB interfaces.** The transport
  was located by interface number, which was right for every wheel with
  three. The Xbox edition has two, and carries it on the second, so it was
  unreachable. It is now identified by what it announces itself as, which
  works whatever the numbering.
- **The apps no longer insist a working wheel is absent.** Detection
  required three settings files before admitting a wheel exists, which
  describes the PlayStation editions' force-feedback engine; the Xbox
  edition's creates one. Its owner had force feedback working in a game
  while both apps reported no wheel connected.

### Fixed

- **The wheel's own input devices could come up unreadable**, so the apps
  could not show live input. The udev rule only covered the wheel's raw HID
  nodes and left the input nodes to the system's own rules, which do not
  always apply. Fixing it by hand worked and did not survive a reboot.
- **An unidentified wheel showed a photo of an RS50.** Both the default and
  the unknown case pointed at the same picture, so "not identified yet" and
  "this is an RS50" looked identical, and a G923 owner was shown a confident
  photograph of a different wheel. Unidentified now draws no photo and says
  so, and a wheel whose driver has not bound is identified from the name its
  input device reports rather than not at all.

### Known limitation

- On the Xbox edition, a game reaching the Logitech SDK directly (with
  `PROTON_ENABLE_HIDRAW=1`) will have its rotation range pushed to 90
  degrees by the SDK. The direct-drive wheels restore that automatically;
  that mechanism reads the wheel's encoder over a feature the Xbox edition
  does not carry, so it does not cover it yet. Writing the range back once
  after the game starts is a complete workaround, from the Steering page or
  the `range` attribute directly.

## 0.23.0 - 2026-07-29

### Added

- **The rev lights now flash for a pit limiter**, the way they do on
  Windows. This turned out to need nothing from the wheel at all: G Hub
  renders a limiter by alternating the ordinary rev level between full and
  dark at about 1.2 Hz, so `logi-tf-sim` reproduces it from telemetry with
  no driver change and no new protocol support.

  Verified on an RS50: 28 transitions across 12 seconds with a mean gap of
  418 ms, against the 417 ms measured in a G Hub capture on Windows. The rev
  level the same engine speed would otherwise show never appears while the
  limiter is on, so the strip carries no rev information during it, matching
  the captured behaviour exactly.

  **BeamNG is the only telemetry source that reports a limiter so far**, and
  whether the game ever raises that flag is unconfirmed: nobody working on
  this has it, and its cars mostly have no pit limiter. If it never does,
  nothing changes for anyone; a source that stays silent is treated as a car
  without a limiter. F1 carries the same signal in a packet this daemon does
  not read yet, so that is a natural next step for someone who can check it
  against a live game.

### Documentation

- **The Dynamic OLED's transport is identified**: HID++ feature `0x8130`,
  reached on real hardware by @PeposCJ, who displayed both static text and
  live telemetry on the panel. The driver still sends nothing to it, and the
  protocol notes are explicit about what is still unknown, above all whether
  writing the display disturbs force feedback, since both share one channel.
- **The wheel base's screen and the rim's rev lights are now clearly
  separated** in the protocol notes. They are different hardware on
  different features, and the documentation previously used "OLED" for the
  base's menu while describing a flashing strip a few lines further down.
- **The rev-light stream is documented from first-party captures**, also
  contributed by @PeposCJ: the true start-up sequence, an acknowledgement
  for every write, redline being an ordinary full-strip level with no
  special command, and the pit-limiter flash above.

## 0.22.1 - 2026-07-29

Packaging only. Nothing in the driver or the apps changed, so there is no
reason to upgrade unless you install from the AUR.

### Fixed

- **The Arch package could not build in a clean chroot.** It never declared
  `fontconfig`, which the desktop app needs at build time, so anything
  building in a clean environment (`paru --chroot`, aurutils, devtools)
  failed while ordinary builds succeeded on the installed copy every desktop
  already has. The same missing dependency was reported on Fedora in #27.
- **The Arch package's published metadata named the wrong source.** A
  `.SRCINFO` records every field fully expanded, including the tarball URL,
  while the `PKGBUILD` derives that URL from the version. Only three lines
  of the `.SRCINFO` were being stamped at release time, so 0.22.0 reached
  the AUR claiming version 0.22.0, a v0.18.0 tarball, and a checksum for
  neither. Installs were unaffected, since the build reads the `PKGBUILD`.

### Changed

- **The Arch package is now built in CI before it is published**, in a clean
  container with only its declared dependencies available. The Debian,
  Fedora and openSUSE packages have always compiled what they ship; Arch was
  the one channel where a recipe that could not build still reached users.
  Its `.SRCINFO` is generated from the recipe that was just built rather
  than edited alongside it.

## 0.22.0 - 2026-07-29

### Fixed

- **A real G PRO was not recognised as a wheel.** It reports no "G PRO"
  anywhere: its product string is "PRO Racing Wheel", so its input node
  arrives as "Logitech  PRO Racing Wheel", with no G and two spaces. The
  desktop and terminal apps skipped it in the Test view, and `logi-ffb`
  skipped it entirely, which left DirectInput force feedback dead on that
  wheel. Reported and half-fixed by @aderumier in #51; `logi-ffb` keeps its
  own copy of that check, which the pull request could not reach.
- **Most button names in Info / Testing were cut short**, showing
  "R Encoder ..." where the name should be. The panel never grew with the
  window: the layout sized it to its contents and left the rest of the row
  empty, so the names had about 98px to fit in whatever the window size.
  The wheel photo beside it is capped now so it cannot take that room back.
- **Two driver defects found by static analysis.** The accessory rescan
  looped over its candidate list in a way that could only ever run once,
  contradicting the round-robin it documents, and the removal path returned
  a value from a function that returns none, which is not valid C even
  though both compilers accept it. Neither was reachable as a user-visible
  bug; both are gone.

### Changed

- **Every binary this project installs is now named `logi-*`**, so that
  `pgrep logi-` finds all of them. The one exception was the TrueForce SDK
  shim installer, `logitech-trueforce-install-shim`, now **`logi-shim`**.
  The old name stays as a symlink in every package, and the apps look for
  both, so nothing that already calls it needs changing.
- The `ffb-proxy` and `tf-sim` crates are renamed to `logi-ffb` and
  `logi-tf-sim`, matching the binaries they build. No installed name
  changes as a result; this only affects reading the source tree.

### Documentation

- **Logi Wheel has a section of its own in the README**, directly under the
  introduction and with the screenshot beside it. The app you actually open
  every day previously appeared only as an aside, by binary name, even
  though the logo at the top of that page is its logo.
- The Info / Testing screenshot is retaken. The old one predated the
  current wheel artwork and showed the truncated button names described
  above.
- The from-source install row lists the fontconfig headers the GUI needs,
  which were only written down in the userspace README before, not where
  someone following the install table would look.
- The superseded HID-BPF pedal-shaping prototype is kept under
  `docs/prototypes/` for two findings that are recorded nowhere else, with
  a correction to its own design note, whose premise that hardware curves
  do not apply on PC was overturned by the 0.21.0 pedal fix.

## 0.21.0 - 2026-07-28

### Fixed

- **Pedal curves, sensitivity and deadzones never did anything.** They were
  written to the pedal unit, which accepts a curve and reports it back as
  loaded but never applies it to the axis it sends to the PC. They now go to
  the wheel base, where they work. If you set a pedal curve before this
  release and felt no difference, this is why. Proven on an RS50 with a step
  curve, which an applied curve makes impossible to sweep through: on the
  pedal unit the axis swept straight through it, on the base it snapped to
  the step exactly.
- **A G923 could be left undetectable, with `lsusb` hanging.** The rule that
  hands the wheel to this driver could hand it back to the in-tree driver
  mid-handover, which then probed a device already in use and wedged it.
  Reported on a G923 Xbox edition (issue #27).
- **The Fedora/COPR package would not install** - it required a package
  called `wayland`, which is the name of the source package, not anything
  installable. It now requires the libraries that actually exist.
- **Module load order was backwards.** The config asked for the in-tree
  Logitech drivers to load first, which is precisely what makes them claim
  the wheel before this driver can.
- **Settings for hardware you do not have were offered as if you did.** The
  handbrake controls appeared on wheels with no accessory attached, and
  after the pedal fix the pedal controls appeared on wheels with no pedals.
- **Unplugging or plugging in the RS Shifter and Handbrake is now noticed.**
  Presence was decided once at start-up, so the accessory's settings stayed
  hidden until you reloaded the driver.
- **The onboard slot editor offered to "restore slot 0"**, which is not a
  slot - it is what the wheel reports in desktop mode, where the flow is
  entered from. It now says it will go back to desktop mode.

### Added

- **The accessory's mode switch is now read**, `wheel_accessory_mode`. The RS
  Shifter and Handbrake is one of three things at a time, chosen by a switch
  on its base, and most of its settings apply to only one of them. Both apps
  now show which mode it is in and grey out the settings that do not apply,
  saying which mode each one needs. The settings stay writable regardless, so
  you can still set one up before flipping the switch.
- **The RS Shifter and Handbrake's last two settings**, `wheel_shift_actuation`
  and `wheel_handbrake_actuation` (1-100), matching G HUB's Shift Sensitivity
  and Handbrake Actuation sliders. Both appear in the desktop and terminal
  apps. Note that G HUB's own handbrake value overflows above about 69% and
  wraps to a point shorter than 50%; this driver writes the correct value, so
  above that point the two will not agree.

### Changed

- Pedal shaping now targets the wheel base's axes rather than the pedal
  unit's. No attribute names changed, and nothing needs reconfiguring.

### Changed (cosmetic)

- **The wheel pictures in the desktop app** are cleaner: the RS50 now uses a
  photo with its background removed, so it sits on the page rather than in a
  white box, and the button-press highlights on the Info page land on the
  buttons themselves instead of on a diagram's numbered labels.

### Documentation

- The protocol specification and sysfs reference corrected: pedal curves
  apply on the base, a HID++ sub-device index belongs to a physical port
  rather than a device type, and feature `0x80B1` is documented.
- The README now explains that `PROTON_ENABLE_HIDRAW=1` makes some games
  read the pedals inverted (rest reads as fully pressed), and that the
  game's own invert-axis option is the fix.

## 0.20.1 - 2026-07-28

- **Pedal settings worked only when no accessory was attached.** The driver
  asked the pedal unit for its response-curve feature at a fixed HID++
  sub-device index. Attaching the RS Shifter and Handbrake shifts that
  numbering, so the pedals answered elsewhere and all nine pedal settings
  (throttle, brake and clutch sensitivity, deadzones and curves) reported
  "not supported" on hardware that supports them. The index is now
  discovered at runtime, confirmed by the axis count the pedal unit
  reports. Fixed on an RS50 with the accessory attached.
- **The desktop app showed unsupported settings as editable zeros.** A
  setting the wheel rejects now reads as unavailable, matching the terminal
  app, instead of a slider sitting at 0 that silently does nothing.
- **A flaky test** in the terminal app's simulation suite no longer fails on
  fast machines.

## 0.20.0 - 2026-07-26

### Added
- **G923 support, PlayStation edition (`046d:c266`/`c267`).** A classic
  force-feedback engine ported from berarma's new-lg4ff (GPL-2.0-or-later,
  credited in the source along with the original lg4ff authors) drives the
  wheel: constant, spring, damper, friction, inertia, periodic and ramp
  effects plus autocenter, with an automatic PlayStation-to-PC mode switch.
  Settings use the classic `range`/`gain`/`autocenter`/`combine_pedals`
  sysfs names (Oversteer-compatible, distinct from this driver's usual
  `wheel_*` surface, since it is a different FFB engine) plus a read-only
  `ffb_output`; `combine_pedals` rewrites the input report, not just a
  no-op toggle. Rev lights are standard Linux LED devices (5 classdevs,
  `::RPM1` to `::RPM5`, one per mirrored pair), driven by the classic
  G29-family LED command, not the DD wheels' HID++ feature.
  Hardware-verified on a c266:
  constant force and autocenter feel correct in Assetto Corsa Competizione,
  and the LED sweep lights the innermost pair.
- **Simulated TrueForce for the G923.** The wheel speaks the same
  TrueForce stream protocol as the RS50/G PRO - confirmed against TF4ALL
  (Mhytee's Windows SimHub plugin, issue #20) - on its third USB interface,
  which the driver now claims as a hidraw-only node. `logi-tf-sim` streams
  the same telemetry-driven haptics used on the other wheels to it, while
  mirroring the classic engine's live output (`ffb_output`) into the
  stream's force field so force feedback and TrueForce agree instead of
  fighting (an active stream otherwise makes the wheel ignore the classic
  path entirely). Hardware-confirmed: a driven tone reaches the wheel as
  vibration; the feel check under real game telemetry is still pending.
  The rev-display feeder now drives the G923's strip too: since the
  wheel has no `wheel_rev_level` attribute, it lights the 5 `::RPM1`-`::RPM5`
  classdevs directly, mapping the same 0-10 telemetry level onto the 5
  mirrored pairs.
- **G923 support, Xbox edition (`046d:c26e`), routed through the driver's
  existing HID++ 0x8123 (G920-style) force-feedback path.** The
  console-boot mode (`046d:c26d`, no input node at all) now switches to PC
  mode automatically on plug-in via a udev rule and `usb_modeswitch` (a
  recommended, not required, package); the out-of-tree `xone` driver can
  claim `c26d` first and block the switch. Unverified pending an
  Xbox-edition tester.
- **PID-scoped driver pre-emption** for c266/c267/c26e: a udev rule
  reclaims the wheel from a competing driver that wins the bind race
  (unbind, then bind this driver), restoring the previous driver if the
  rebind itself fails, with no blanket blacklisting, so every other
  Logitech device stays on its usual driver. The one exception is
  berarma's new-lg4ff (`hid-logitech-new`), which we blacklist outright to
  stop it racing us for c266/c267 - if you run it for a different
  Logitech wheel (G29, G27, DFGT, ...) that wheel now falls back to the
  in-tree `hid-logitech` driver instead.
- **logi-wheel G923 support**: the TUI and GUI recognise the G923 and expose
  its four classic settings, with its own wheel image on the Info/Testing
  page alongside the RS50 and G PRO. Gain and autocenter show as a rounded
  0-100% in the UI instead of the raw 0-65535 sysfs value (the sysfs
  attribute itself is unchanged, so Oversteer and scripts see the same
  numbers as before). Settings a wheel does not have are hidden instead of
  shown empty; for the G923 that means no LIGHTSYNC, no onboard profile
  slots, and no desktop/onboard mode toggle. Desktop (computer-side)
  profiles now work for any wheel without onboard slots, including the
  G923. The button tester shows the G923's real button labels from a live
  capture (X, Square, Circle, Triangle, the paddles, R2/L2/R3/L3, Share,
  Options, the Plus/Minus pair, the dial's CW/CCW/push, and PS) - only the
  18 buttons the wheel actually has - and the RS50 callout-diagram overlay
  no longer draws over other wheels.
- **Info/Testing is now the first page you see**: it is the first sidebar
  entry and the app's startup view in both frontends, naming the detected
  wheel and showing its serial and firmware. On the G923, which has no
  `wheel_serial`/`wheel_firmware` sysfs at all, both are queried live over
  HID++ feature `0x0003` (DeviceInformation). A scrollbar now appears
  whenever a view scrolls, instead of silently swallowing the mouse wheel
  with no sign there is more below.
- **The force feedback and TrueForce-texture self-tests are full
  sequences, not a single 2-second effect.** Both frontends list the whole
  plan up front - one row per step, with its label and duration - and
  track each row's live state (pending, counting down, playing, done, or
  skipped) off a shared state machine, with a countdown before every step.
  The force feedback plan covers constant force in both directions, a
  rising ramp, spring, damper, friction, inertia, sine, square, triangle
  and sawtooth waves, an envelope demo, simultaneous mixed effects, a gain
  demo and an autocenter demo; the TrueForce plan steps through four
  rising texture frequencies. A step whose effect type the wheel does not
  advertise is marked skipped instead of erroring - the G923 skips
  friction, the direct-drive wheels run everything. Force direction in
  these tests is per-model, hardware-verified: the G923's classic engine
  and the direct-drive engine use opposite sign conventions for the same
  logical direction, so each step resolves to the correct raw value for
  the connected wheel instead of assuming one global convention.

### Changed
- **The settings app is renamed from `logi-dd` to `logi-wheel`** (TUI binary
  `logi-dd` -> `logi-wheel`, GUI binary `logi-dd-gui` -> `logi-wheel-gui`;
  `logi-ffb` and `logi-tf-sim` keep their names). Reason: "dd" meant
  direct-drive, but the driver now also supports the gear-driven G923, and
  the app configures every supported wheel, not just the direct-drive ones.
  - **Config files migrate automatically, once.** Profiles and
    `tf-sim.conf` move to `$XDG_CONFIG_HOME/logi-wheel` (falling back to
    `~/.config/logi-wheel`); the first time the new location is needed and
    the old `logi-dd` one still has data, it is copied over (the originals
    are left in place) and the new location is used from then on, so
    nothing is ever written back to `logi-dd` again.
  - **`LOGI_DD_SYSFS_DIR`/`LOGI_DD_TEST_OVERLAYS`** still work as deprecated
    aliases for the new `LOGI_WHEEL_SYSFS_DIR`/`LOGI_WHEEL_TEST_OVERLAYS`;
    the new name wins if both are set.
  - **Packages upgrade in place.** The AUR, Debian and RPM packages for
    `logi-dd`/`logi-dd-gui` are replaced by `logi-wheel`/`logi-wheel-gui`
    with `Provides`/`Replaces`/`Conflicts` (or `Obsoletes`, on RPM) on the
    old names, so a package manager moves existing installs over cleanly.
  - **Transitional symlinks**: `logi-dd` and `logi-dd-gui` are installed as
    symlinks to `logi-wheel`/`logi-wheel-gui`, so scripts and desktop
    shortcuts referencing the old binary names keep working.

### Notes
- **G923 force feedback needs no Proton launch options**: no
  `PROTON_ENABLE_HIDRAW`, just Steam Input off. TrueForce through
  Logitech's own SDK does not work for the PlayStation G923 on Linux (the
  SDK DLL delegates the actual haptics to G HUB, which Proton does not
  provide); simulated TrueForce via `logi-tf-sim` is the supported path
  for TrueForce on this wheel.

## 0.19.1 - 2026-07-23

### Fixed
- **DirectInput force feedback in Le Mans Ultimate and other DirectInput sims**
  (issue #50), confirmed on hardware. The `logi-ffb` virtual wheel's hidraw node
  was owned by root, so Wine could not open it and silently fell back to a path
  with no force feedback. A udev rule now grants the session user access to that
  node. The proxy also narrows `PROTON_ENABLE_HIDRAW` to the virtual wheel only
  (preserving any value you set) and warns when the node is inaccessible instead
  of failing silently. This path needs Proton 10 / Experimental / GE-Proton 10
  or newer.
- **Packaging: the retired `GETTING_STARTED.md` broke the AUR and OBS builds.**
  The AUR PKGBUILD, its post-install message, the OBS spec and the akmods
  comments still referenced the removed file; they now point at the wiki.

## 0.19.0 - 2026-07-22

### Added
- **Telemetry capture tool.** `logi-tf-sim capture --port <port>` records a
  game's UDP telemetry to a file so its format can be added to the simulated
  TrueForce support. See the wiki's "Add a game" page; this lets any
  UDP-telemetry sim be supported from a short recording.
- **Relay listener for shared-memory sims.** `logi-tf-sim` now also listens for
  a small relay protocol (default port 20780) that feeds the same simulated
  TrueForce and rev-light pipeline, so shared-memory sims (iRacing, rFactor 2
  and similar) can be driven by a Wine-side relay. The relay itself is in
  progress.

### Fixed
- **Rev lights track RPM at the full ~60 Hz** G HUB uses. The feeder was capped
  near 6 Hz, so rev sweeps lagged well behind the engine; the correct cadence
  was measured from a hardware capture (issue #20).

### Changed
- The release workflow now publishes the Arch AUR package on every tag,
  alongside the existing Debian, Fedora COPR and openSUSE OBS channels.

## 0.18.0 - 2026-07-22

### Added
- **Game detection across launchers.** The Setup "Your games" list now finds
  sims installed through Steam (Proton and native Linux builds), Lutris and
  Heroic, each tagged with its source. Native Linux sims (Euro Truck Simulator
  2, American Truck Simulator, and more as the registry grows) are listed too.
- **Add a game.** For a racing sim the registry does not recognise, Setup can
  set it up: pick it from your detected games or point at its Wine prefix, and
  the TrueForce files are installed into it. It then stays in the list to manage.

### Changed
- **Setup shows only recognised sims** (plus any game you added), instead of
  every installed title, so non-wheel games no longer clutter the list.

## 0.17.0 - 2026-07-22

### Added
- **Simulated TrueForce for more sims.** New `logi-tf-sim` telemetry parsers
  for F1 (modern UDP), BeamNG (OutGauge) and EA WRC synthesize engine haptics
  and drive the rev lights for games without native TrueForce.
- **Per-game compatibility registry.** A built-in list of what each known sim
  supports (native FFB, native or simulated TrueForce), surfaced in the GUI
  and TUI Setup views so you can see and toggle per-game support at a glance.
- **Exact numeric entry on settings.** Every slider takes a typed value, not
  just a drag, including the steering rotation range.

### Changed
- **GUI settings redesign.** Pages render as grouped cards with a plain-English
  explanation under every setting, the steering page gains a rotation dial, and
  force feedback versus TrueForce is spelled out where it matters.
- **Setup rebuilt as a per-game panel.** Enable or disable TrueForce per game
  from a "Your games" list instead of a scrolling reference dump; the TUI Setup
  view matches.
- **DirectInput force feedback now uses Wine's hidraw PID path** (issue #50).
  The `logi-ffb` proxy no longer attaches a kernel PID force-feedback layer to
  the virtual wheel, which broke the wheel's detection in some Proton sims. The
  virtual wheel stays a plain device, and the proxy enables
  `PROTON_ENABLE_HIDRAW=1` for the game it launches so Wine drives the virtual
  wheel's PID collection directly, with the proxy forwarding effects to the
  real wheel.
- **No group or terminal step to use the wheel.** udev grants the session user
  access to the wheel settings and the proxy device directly, so first-time
  setup no longer needs a group membership or a manual command.

### Fixed
- **logi-ffb button coverage.** The proxy now forwards the D-pad and the
  extended button block to the virtual wheel.

### Removed
- The standalone Oversteer udev patch, superseded by logi-dd.

## 0.16.2 - 2026-07-20

### Fixed
- **logi-ffb pedals never reached the virtual wheel** (issue #50): the
  proxy listened on guessed axis codes; the RS50 emits throttle, brake
  and clutch on ABS_RX/ABS_RY/ABS_RZ. Verified end to end; DirectInput
  sims can now bind all three pedals.
- **The openSUSE OBS channel had been silently stale since 0.14.0**:
  builders have no network, so the Rust workspace could not fetch
  crates. The publish workflow now vendors the crates into the sources,
  and the spec builds offline and locked; two follow-up spec fixes
  (icon-directory ownership, dropping the dkms-era noarch marking) let
  all three packages build and publish on OBS.

### Added
- All four binaries answer `--version`; `setup.sh doctor` prints driver
  and app versions; the bug-report template requires the driver
  version; the module logs its tag-derived version at load.

## 0.16.1 - 2026-07-20

Branding patch: one universal logo (steel-blue rim, legible on light
and dark surfaces) used identically as launcher icon, README logo,
window icon and in-app header mark; the GUI presents as "Logi DD"
(window title, header, desktop entry - binary and package names are
unchanged); desktop entry gains StartupWMClass; TUI header carries the
rev-light arc signature.

## 0.16.0 - 2026-07-20

The settings app grows a desktop GUI, both frontends gain LIGHTSYNC,
Setup and testing surfaces, simulated TrueForce arrives as a telemetry
daemon, and a hardware-verification campaign decoded three LIGHTSYNC
protocol facts (custom slots are effect values 5-9, effect selects need
a commit, the strip doubles as a level-driven rev display) and fixed
the driver accordingly. Packaging splits into three interdependent
packages.

### Added
- **`logi-tf-sim`, a simulated-TrueForce daemon**: synthesizes engine
  haptics from a game's own UDP telemetry (DiRT Rally 2.0 and the
  classic Codemasters format, Automobilista 2 / Project CARS 2) for
  titles without native TrueForce, and feeds the same RPM to the
  wheel's rev-light display. Per-game enable and intensity, master
  switch, tunable felt rev rate, a consent-gated test sweep, and a
  Setup panel in both frontends. Streams through `libtrueforce`
  (static-linked).
- **RPM rev-light display**: `wheel_rev_level` (0-10) drives the RS50's
  strip as a live rev display (hardware-verified), fed manually, by
  `logi-tf-sim`, or by any telemetry bridge.
- **Per-axis shaping**: throttle, brake, clutch, handbrake and steering
  each choose sensitivity or the full response curve independently.
- **Mode-coupled profiles**: onboard mode shows the wheel's five named
  slots; desktop mode manages computer-side profile presets
  (save/apply/delete under `~/.config/logi-dd/profiles`).
- **Info / Testing page**: serial, firmware, app and driver versions
  (copyable), a live input monitor (rotating wheel diagram, button
  tester with GL/GR, pedal bars) and guarded, cancelable force
  simulations.
- **Per-game Setup**: Steam/Proton game discovery with per-game shim
  install/remove, an SDK folder with live resolution, a games
  compatibility table, and helper discovery that also finds repo
  checkouts.
- **Curve editor polish**: axis legends, a hover ghost showing where a
  click adds a point, numeric per-point entry.
- **Drift watcher**: external profile/mode changes (the rim's buttons)
  refresh whatever page is open within about two seconds.
- **Desktop entry and an original logo** for `logi-dd-gui`.
- **Three-package split** in every channel: the driver package,
  `logi-dd` (TUI + `logi-ffb` + `logi-tf-sim` + shim installer) and
  `logi-dd-gui` (desktop app), with dependency chains so one install
  pulls what it needs.
- **`logi-dd-gui`, a desktop settings app** (Slint): the full `wheel_*`
  settings surface as a GUI - every category with live values, mode
  switching and refresh, a G HUB-style curve editor, an HSV color picker,
  a deadzone pair editor, onboard profile renaming, and a named-profile
  dropdown. Ships in all packaging channels alongside the TUI.
- **LIGHTSYNC redesign** in both frontends: the LED settings are composed
  into a per-slot model (colors per LED, effect, brightness, animation
  direction) with a slot editor, replacing the flat "LEDs" list.
- **Setup section** (GUI page and TUI view): per-game management of the
  TrueForce shim over Steam/Proton game discovery, with an SDK directory
  picker, plus logi-ffb helper setup.
- **Test section** (GUI page and TUI view): a live input monitor and
  guarded force-feedback simulations for checking the wheel end to end.
- **Advanced shaping toggle** in Steering and Pedals: the simple view
  shows the G HUB-equivalent sliders, Advanced reveals the full curve
  and filter set.
- **`LOGI_DD_SYSFS_DIR`** environment override for development: point the
  apps at a directory of plain `wheel_*` files and they run fully
  headless, no wheel or driver needed.
- **`install-tf-shim.sh --uninstall-prefix`** removes the shim from a
  single Wine/Proton prefix.

### Fixed
- **LIGHTSYNC direction wire encoding.** The driver wrote the sysfs 0-3
  direction enum straight into the 0x807B config, but the device expects
  1-4 on the wire; the firmware NAKed the off-by-one config (an `-EIO` on
  writing Outside-In). The driver now translates both ways.
- **logi-ffb virtual wheel identity.** The virtual wheel cloned the real
  wheel's name and IDs, so games (and the proxy itself) could bind the
  wrong device and the wrapper hid steering. It now appears as
  "logi-ffb Virtual Wheel" with its own IDs, and the proxy refuses to
  bind its own virtual device.
- **GUI/TUI fix wave**: the curve plot maps linearly with repaired point
  hit-testing and no drag stalls, editor overlays are actually modal,
  severed widget bindings re-sync from model pushes, pair edits no longer
  race, mode edits no longer desync the header, and slot renames give
  feedback and refresh the profile dropdown.
- **Custom LIGHTSYNC slots never switched on the wheel.** Decoded from
  captures and hardware-confirmed: the five custom slots ARE effect
  values 5-9 (0x05 = CUSTOM 1); the driver hardcoded 0x05, so every
  slot switch rendered slot 1, and its "activate" call was actually a
  name read. Slot selection now visibly repaints the strip.
- **Built-in LED effects never repainted.** fn3 only stages an effect;
  the strip repaints on a zero-parameter fn6 commit, which the driver
  now sends.
- **The rev-arm burst stomped the active effect** (its fn3 0x02 side
  effect); the driver snapshots and restores the user's effect and slot
  around arming.
- **Combined pedals trapped the toggle**: the driver misparsed the
  toggle's own change echo as a "profile 1" broadcast, briefly reporting
  onboard mode; the frontends then locked the row as wrong-mode.
- **GL and GR are their own buttons** (codes 0x2cc/0x2cd), not aliases
  of the shifter paddles; the input tester now maps them.
- **wheel_rev_level is not G PRO-only**: the RS50 accepts the level
  command; docs and labels corrected.

## 0.15.0 - 2026-07-18

The release adding `logi-ffb` plus the pedal-shaping restoration below
(the tag was documented in the GitHub release notes; summarized here for
completeness).

### Added
- **`logi-ffb`, a DirectInput force-feedback proxy** (`ffb-proxy` crate):
  presents a virtual force-feedback wheel to Wine/Proton sims on the
  `PROTON_ENABLE_HIDRAW=1` path and forwards effects to the real wheel's
  evdev FF interface. Run as `logi-ffb %command%` in Steam launch options.
- **Combined pedals.** `wheel_combined_pedals` (0/1, desktop only) toggles G HUB's
  legacy throttle+brake axis merge via feature `0x80D0`. Verified on an RS50: on,
  the two pedals collapse to a single centred axis (`ABS_RX` re-centres, `ABS_RY`
  goes silent); off restores separate axes. Wired into logi-dd.
- **RS Shifter & Handbrake support.** All modes work with no driver change (they
  ride the wheel's existing report): sequential shift = `BTN_TOP2` / `BTN_PINKIE`,
  digital handbrake = `BTN_THUMB2`, analog handbrake = `ABS_Z`, all hardware-mapped
  on an RS50. Added `wheel_handbrake_curve` and `wheel_handbrake_sensitivity` to
  bend the analog handbrake (base `0x80A4` axis 4), the same curve type as the
  pedals, verified live. Wired into logi-dd.
- **Hardware pedal shaping.** The pedal unit (HID++ sub-device `0x02`) applies a
  `0x80A4` response curve to each axis it reports to the PC, the same mechanism
  as the steering wheel. Verified on an RS50 for all three pedals with an
  artifact-proof test (a two-plateau throttle curve, and step curves on the
  load-cell brake and clutch, each producing a bimodal axis a linear pedal
  cannot). Each pedal `<p>` in {throttle, brake, clutch} gains three attributes,
  all writing the single curve the axis holds (last write wins):
  - `wheel_<p>_curve` - raw `in:out` points or `reset`, like `wheel_response_curve`.
  - `wheel_<p>_sensitivity` - the 0-100 G HUB slider (50 = linear).
  - `wheel_<p>_deadzone` - `"lower upper"` percent dead travel (sum <= 99).

  This corrects the v0.14.0 removal, which rested on a single untested capture;
  the pedal MCU does apply the curve, it was a measurement error.
- **logi-dd curve editor.** A G HUB-style modal point editor for the pedal and
  steering curves: edit control points (input/output percent) plus deadzones
  with a live ASCII preview, then upload. Plus registry entries for the nine new
  pedal attributes.

## 0.14.0 - 2026-07-16

An input-dropping bug fixed, onboard profiles renameable, and the pedal
attributes that never did anything removed. Also the first release carrying
`logi-dd`, the settings app. All driver changes are hardware-verified on an RS50
(native mode, kernel 7.1.3).

The sysfs surface loses the pedal shaping attributes (see **Removed**). That is
an API break, but not a behaviour change: pedals were already reported raw.

### Fixed
- **Joystick frames were parsed as HID++ and dropped.** A direct-drive wheel's
  interface 0 is a joystick whose input report declares no report ID, so its
  first byte is data - the 4-bit hat switch plus buttons 1-4. We claim that
  interface (to track the steering axis) and register no `report_table`, so
  `hidpp_raw_event` ran on those frames and switched on that byte as if it were
  a HID++ report ID. D-pad Up + button 1 is `0x10` (`REPORT_ID_HIDPP_SHORT`);
  Up-Right and Right give `0x11` and `0x12`. The 30-byte frame then failed the
  HID++ size check, logged `received hid++ report of bad size (30)` and returned
  1 - telling the HID core the report was consumed, so the frame was dropped
  before reaching the input layer. Holding such a combination discarded every
  input report: steering, pedals and buttons froze while dmesg flooded.
  Reproduced on an RS50 (170 errors in a few seconds of D-pad + button presses;
  zero afterwards). The interfaces claimed for input only are now flagged and
  skip the HID++ demux.

### Added
- **Profile rename.** `wheel_profile_names` is writable (0664): `echo "3:RACE" >
  wheel_profile_names` renames an onboard slot via feature `0x8137` fn4. The
  wheel persists the name to its own NVM on the single write; there is no
  separate save step. Slots are 1-5. An RS50 takes names of up to 9 characters
  (matching its own stock `PROFILE 3`), stores them uppercased, and accepts
  spaces; it refuses a longer name at the HID++ layer, reported as `-EIO`.
- **`logi-dd`, a settings app** (`userspace/logi-dd`): a Rust core plus a
  terminal UI over the `wheel_*` sysfs surface - typed reads/writes, per-setting
  validation, mode gating, and a profile selector that shows the slots by name.
  Onboard slots can be renamed from it (pick the slot, type a name); it caps the
  name at the wheel's 9 characters, so it cannot compose one the wheel refuses.
  First part of a G HUB replacement.

### Removed
- **Pedal shaping attributes** - `wheel_{throttle,brake,clutch}_curve`,
  `wheel_{throttle,brake,clutch}_deadzone`, `wheel_combined_pedals`, the
  Oversteer-compat `combine_pedals`, and `wheel_pedal_response_curve`.
  API-breaking for the sysfs surface, but not a behaviour change: these
  transforms never reached userspace (the rewritten report did not survive to
  the input layer), so the attributes accepted settings that did nothing.
  `wheel_pedal_response_curve` uploaded a hardware curve that a raw-HID capture
  showed the wheel stores but never applies to its PC output. Pedals are
  reported raw, exactly as before; curve them in userspace instead. The steering
  `wheel_response_curve` is unaffected. Oversteer hides its combine-pedals
  control when the attribute is absent; every other Oversteer attribute is
  unchanged.

## 0.13.0 - 2026-07-11

A correctness batch from a review of the G Hub USB captures and an audit of
places where a symptom had been masked instead of fixed, plus validation of the
real G PRO wheel against contributor captures. All driver changes are
hardware-verified on an RS50 (native mode, kernel 7.1.3).

### Fixed
- **Pedal init hang (#30).** The pedal MCU (device index 0x02) silently drops
  HID++ messages sent with software-id 0x01; it accepts 0x0a (what G Hub uses).
  Init drops from ~15-20 s of retry timeouts to ~0.4 s.
- **Damping was zeroed on any settings re-read.** The read used function index
  fn1 (which *sets* damping to 0) instead of fn0 (get). Now fn0.
- **TrueForce current-value read** used fn1 (an event slot) instead of fn2.
- **`wheel_sensitivity`** now uploads the 0x80A4 axis-response Bezier curve (the
  real desktop sensitivity control) instead of aliasing 0x8040 LED brightness.
  Sensitivity and brightness are fully independent.
- **Removed the `05 07` "FFB keepalive"**, which was a DualShock-4 lightbar
  packet, not a wheel command. FFB does not depend on it.
- **RS50 rev-lights** un-gated, with corrected ~100 Hz cadence and DMA-safe
  buffers (the previous stack buffers triggered a USB DMA warning).
- **On-wheel OLED profile edits** now trigger a settings re-read (0x8137 sw0).
- **Transport / error-handling hardening**: output_report falls back to
  SET_REPORT only on -ENOSYS; compat-lookup misses return -EOPNOTSUPP; dropped
  FFB samples are counted rather than silently lost; retry-on-timeout breaks on
  non-BUSY.
- **G PRO compat fallback (#33).** On a real G PRO, a transport-level feature
  lookup failure no longer applies RS50 fallback indices (which are shifted on a
  real G PRO and would cross-wire a setting into a bystander feature); it reports
  the feature absent and logs it.

### G PRO validation
- The real G PRO (`046d:c272`) HID++ configuration protocol is confirmed against
  contributor G Hub captures (#8): identical to the RS50 except a uniform
  feature-index shift, which the driver resolves dynamically. The FFB / TrueForce
  stream itself is not yet verified on a real G PRO (those captures were
  config-only).

### Packaging
- New distribution channels: **Debian/Ubuntu (.deb)**, **Fedora COPR (akmod)**,
  and **openSUSE OBS (DKMS)**, auto-published on each GitHub Release.

### Tooling
- `linux_game_capture.sh` gains a ring-buffer mode (`ring[:N]`) that keeps only
  the last N seconds, for capturing intermittent issues (#31).

### Documentation
- Protocol spec corrected (05-07 is a DS4 packet; sensitivity is 0x80A4; damping
  is fn0; pedal software-id is 0x0a), and the `wheel_sensitivity` sysfs reference
  rewritten to match.

## 0.12.1 - 2026-07-09

Packaging and documentation. No driver code change from v0.12.0 (the module
is byte-identical); this release adds an install path for atomic distros and
corrects the docs.

### Atomic / immutable distros (Bazzite, Silverblue, Kinoite)

DKMS cannot build on rpm-ostree systems (its build tree is read-only during
the transaction), so the module now also ships as a static **kmod RPM**
(`packaging/akmods/logitech-trueforce-kmod.spec`, kmodtool-based). You build
it once in a `toolbox`, layer it with `rpm-ostree install`, and reboot.
Verified end-to-end on Fedora Silverblue 44 (kernel 7.1.3-200.fc44): it
builds, layers, and loads, registering the `logitech-dd` driver with the
three wheel USB IDs. `docs/GETTING_STARTED.md` section 1a documents the flow,
including the post-kernel-update rebuild and the Bazzite custom-kernel
`kernel-devel` note.

### Documentation
- Corrected the RS50 LED description to match the hardware: a horizontal
  10-LED strip across the upper faceplate (rev/shift indicator), numbered
  left to right.
- Accuracy pass across the doc set, checked against the driver code.
- Trimmed verbose historical and development notes from the README.

## 0.12.0 - 2026-07-09

9 commits since the `v0.11.0` tag on 2026-07-08. The fork is now scoped
to only the direct-drive wheels (module `hid-logitech-dd`), coexisting
with the in-tree `hid-logitech-hidpp` instead of shadowing it for every
Logitech device; two LED init-stomp bugs are fixed; and the licensing +
AUR packaging groundwork is in place. All validated on RS50 hardware and
built clean on clang 7.1.3 and gcc 6.18-debug.

### Scoped to the direct-drive wheels (module renamed to hid-logitech-dd)

The driver was a full fork of the in-tree `hid-logitech-hidpp` and shipped
under that same name, so once installed it **replaced the in-tree driver
for every Logitech HID++ device** - mice, keyboards, receivers - freezing
them at the fork's snapshot (which lagged mainline by ~21 recent Bluetooth
devices plus several 7.1/7.2 hardening fixes). It only ever added value for
the direct-drive wheels.

- The module now builds as **`hid-logitech-dd`** (driver name `logitech-dd`)
  and its device table is trimmed to just the direct-drive wheel USB IDs
  (`c276` RS50 native, `c272` G PRO Xbox/PC + RS50-compat, `c268` G PRO
  PS/PC). It runs **alongside** the in-tree `hid-logitech-hidpp`, which
  keeps serving every other Logitech device at its current version. No
  symbol clash (the fork exports none) and no PID conflict (the in-tree
  driver does not claim these wheels), so **no blacklist is needed**.
- `setup.sh` now **migrates** existing installs: it removes the old
  `hid-logitech-hidpp` DKMS package and the stale
  `blacklist-hid-logitech-hidpp.conf`, restoring the in-tree driver for
  your other Logitech hardware.
- Gear-driven **G920/G923 are no longer claimed** by this fork; they use
  the in-tree driver (their standard HID++ FFB is unchanged).

### Licensing and packaging

- Added the missing license texts: `COPYING` (GPL-2.0) for the driver and
  tooling, `userspace/libtrueforce/COPYING` (LGPL-2.1) for the library,
  plus SPDX headers on every libtrueforce source. Required for AUR.
- `install-tf-shim.sh` resolves its SDK-DLL directory (`--sdk-dir` / env /
  repo `sdk/` / `~/.local/share/logitech-trueforce/sdk`) instead of
  hardcoding the repo tree, so it works when installed standalone.
- DKMS packaging skeleton for the AUR under `packaging/aur/`.

### Fixed

- **LED brightness reverting to 100% on connect** ([issue #29]): the
  LIGHTSYNC slot-apply wrote a driver-cached brightness (default 100%,
  never read back from the wheel) on every init and LED change, racing
  the wheel's profile load and stomping the user's saved RPM brightness.
  apply_slot no longer writes brightness; it stays owned by the
  `wheel_led_slot_brightness` handler. Hardware-verified on RS50.
- **LED effect reset to Custom on connect** (same class as #29): the
  load-time apply forced effect mode 5 (Custom) over any animated effect
  (modes 1-4) the wheel restored from its profile, because the effect
  mode is never read back. The init now applies the slot's colours
  without forcing the effect mode. Hardware-verified: an animated effect
  survives a reload, and Custom-mode LEDs still light on load.

[issue #29]: https://github.com/mescon/logitech-trueforce-linux-driver/issues/29

## 0.11.0 - 2026-07-08

35 commits since the `v0.10.0` tag on 2026-07-03. The TrueForce force
path was reworked from TF4ALL protocol findings and feel-verified on
RS50 hardware (clean texture route, host-alive force unaffected by
texture playback, response-curve upload/reset confirmed). SDK-driven
game TrueForce under Proton was additionally packet-confirmed in the
RS50's **native mode** (`046d:c276`, AC EVO, ~2 kHz type-0x01 stream on
ep 0x03), so native mode no longer trades away game TrueForce for its
full 2700 range.

### TrueForce stream reworked from TF4ALL cross-pollination

Protocol findings from the TF4ALL project (a Windows SimHub plugin
built on this project's documentation - issue #20; analysis in
dev/docs/tf4all-analysis.md) fed back into the driver:

- **Unified force+audio stream packets**: bytes 6-9 of a stream packet
  are the motor torque target, with the 13-slot window played
  additively on top - so the driver now sends ONE packet per 2 ms tick
  during texture playback (steering force in the preamble, four window
  slots of texture audio) instead of interleaving 500 Hz force packets
  with 250 Hz audio packets whose preamble wrongly carried the audio
  amplitude. Doubles the texture slot rate to 2 kHz and removes the
  audio-as-torque wart.
- **Texture amplitude cap** at half of full scale: above ~0.5-0.7 FS
  the wheel's DSP crosses from vibration into pulling the steering
  axis; real games stream far below the cap.
- **`wheel_rev_level` (0-10) for the real G PRO rim** - level-based
  rev lights per the TF4ALL G HUB capture decode; the RS50's per-LED
  RGB `wheel_led_*` attributes are hidden on a real G PRO (different
  rim hardware) and vice versa. Untested on real hardware; needs a
  G PRO owner.
- **G923 PIDs added to the udev rule** (c266/c26d/c26e): the G923
  speaks the same TrueForce protocol, so hidraw access lets Logitech's
  SDK DLLs reach it under Proton the way they do for RS50/G PRO.
  Untested; needs a G923 owner.
- Protocol spec corrected: the Windows game-FFB path for these wheels
  is HID++ 0x8123 fn2 (the endpoint stream is the TrueForce/SDK
  session channel and overrides it); stream rates up to ~1000 pkt/s
  observed (AC EVO).

### Overnight hardening pass (2026-07-06)

- **Fixed a regression for Unifying/Lightspeed-paired devices**: the
  device-index answer check added earlier this cycle made every HID++
  sync command on receiver-paired mice/keyboards eat the full timeout
  (the DJ transport rewrites the wire index after the driver's
  snapshot). The check is now applied only to the direct-drive wheels
  it was written for.
- Real G PRO: connect-time LIGHTSYNC initialisation no longer runs
  (wrong protocol dialect for that rim); `wheel_rev_level` hardened
  (pacing underflow, send serialisation, honest errno).
- TrueForce stream: texture window advances only when the packet
  actually queued; session wind-down sends one recentre packet, not
  one per retry.
- New **`wheel_response_curve`**: the steering axis's 64-point
  response curve (G Hub's Sensitivity slider, feature 0x80A4) -
  write `in:out` pairs or `reset`. Implemented from captures, needs
  live validation.
- libtrueforce: all -Wformat-truncation warnings fixed; sparse,
  smatch and both CI kernel builds clean across the week's changes.

### Second overnight batch (2026-07-06)

- **`wheel_pedal_response_curve`**: hardware response curves for the
  pedal unit's three axes (feature 0x80A4 on HID++ sub-device 0x02),
  sharing the steering attribute's upload core. En route, the
  sub-device send helper gained the LONG-report case it was missing
  (13-byte curve chunks would previously have been truncated to a
  SHORT report). Untested on hardware.
- **`wheel_rev_level` is now asynchronous and coalescing**: writes
  return immediately and the driver flushes only the newest level at
  the 160 ms cadence - a fast telemetry feeder no longer blocks
  ~160 ms per write or drains stale intermediate levels to the wire.
- Independent corroboration from our own captures: the 2026-01-26
  gameplay capture streams type-0x01 force packets at 999.8 Hz,
  matching the packet-paced 1 kHz model behind the unified stream.

### Per-model force strength and libtrueforce fixes (2026-07-06)

- **Per-model KF peak torque**: libtrueforce scaled every torque
  request against the RS50's 8 Nm ceiling, so on an 11 Nm G PRO a
  request for 8 Nm mapped to full scale (about 11 Nm actual, ~37% more
  than asked). Peak torque now resolves from the wheel's USB PID
  (RS50 8 Nm, G PRO 11 Nm), and the capability getters report the
  right value. G PRO figures are spec-derived, hardware confirmation
  requested in issue #28.
- **libtrueforce udev permissions gap fixed**: the rule matched only
  the RS50's USB ID (c276), silently locking G PRO owners out of the
  library without root; it now covers c276/c272/c268. File renamed to
  `99-logitech-trueforce.rules`.
- **Dead code removed**: the `gpro_sysfs_init` settings-only path was
  unreachable (the G PRO runs the direct-drive init that already
  provides the full settings surface) and was deleted, along with the
  write-only `is_gpro` marker. No behaviour change.
- Final naming sweep: the remaining `rs50` references in code comments,
  the libtrueforce sources, and the udev rule labels are generalized to
  the direct-drive family where they no longer meant the RS50
  specifically. Em-dashes and en-dashes removed from the tracked docs.

### Naming generalized to the whole direct-drive family

- **dmesg lines are now tagged with the actual wheel model** instead
  of a hardcoded `RS50:`: `RS50 (native):`, `RS50 (G PRO compatibility
  mode):`, or `G PRO:`, resolved from the bound identity at log time.
  The RS50 spoofs the G PRO product ID in compatibility mode but keeps
  its own USB product string (verified live); a real G PRO reports
  "PRO Racing Wheel" (verified from contributor captures) - so the
  compat tag doubles as a mode indicator when debugging.
- Driver symbols renamed `rs50_*` -> `hidpp_dd_*` ("dd" = direct
  drive), quirk `HIDPP_QUIRK_RS50_FFB` -> `HIDPP_QUIRK_DD_FFB`. No
  functional change; no sysfs name changes (those were already
  generic).
- User-facing artifacts renamed: udev rule
  `70-logitech-rs50.rules` -> `70-logitech-trueforce.rules`
  (`dkms-update.sh` removes the old installed filename),
  `oversteer-rs50-support.patch` -> `oversteer-logitech-trueforce.patch`,
  `docs/RS50_PROTOCOL_SPECIFICATION.md` ->
  `docs/PROTOCOL_SPECIFICATION.md` (redirect stub kept).

### Fixed

- **Every HID++ settings command stalled ~5 seconds** (introduced by
  the device-index answer check earlier in this cycle, caught the same
  day via usbmon): the answer matcher compared against a question
  snapshot taken before the transport applied the 0xff device-index
  default, so every first attempt was rejected and only an accidental
  retry-on-timeout made calls succeed. Symptoms: Oversteer appearing
  to hang, `wheel_profile_names` taking up to 50 s, deferred init
  taking minutes. Now the default is applied before the snapshot;
  range writes measure 4 ms.
- **udev permissions race**: the permissions rule fires on the hidraw
  "add" event, which is emitted before probe creates the `wheel_*` /
  compat attribute files, so a plug or driver load could leave the
  settings root-only until a manual `udevadm trigger`. The driver now
  emits a "change" uevent after creating its sysfs group so udev
  replays the rule with the files present.
- **Teardown and concurrency fixes** from adversarial review:
  HID++ answers are matched to questions by device index (a late
  sub-device reply can no longer satisfy a base-device wait and vice
  versa); the wheel sysfs group is removed at the start of teardown,
  closing a window where a store could re-arm the effect timer after
  the final delete (use-after-free); interface 0 no longer takes the
  owner teardown path via its cached FF pointer, and the owner
  invalidates that cache before freeing (use-after-free on partial
  unbind); sysfs handlers and the range-restore worker re-check the
  teardown flag between sync HID++ sends (teardown could stall for
  the full send-timeout multiple).
- **Autocenter is now independent of the game's FF_GAIN**: gain is
  applied to the summed game effects only, then the autocenter spring
  is added - a leftover low gain from a game no longer silently kills
  the user's centring force (matches hardware-autocenter semantics on
  other wheels).
- **Pre-release review hardening**: a non-finite (NaN) KF torque
  request from game force code slipped past the clamp in libtrueforce
  and reached an undefined int16 cast - an unbounded command to a
  direct-drive motor - now treated as zero force; the response-curve
  pair parser rejected trailing junk (`30000:40000x`, `5:5:5`) instead
  of silently accepting the numeric prefix; and two dead `if (ff->wq)`
  guards left by the settings-only path removal were dropped.

### Oversteer

- The bundled Oversteer patch now unlocks the full settings set
  (`gain`, `autocenter`, `spring_level`/`damper_level`/
  `friction_level`, `combine_pedals`) for both G PRO product IDs, not
  just `range` - real G PRO owners get the same Oversteer integration
  as the RS50.

## 0.10.0 - 2026-07-03

188 commits since the `v0.9-pre-simplification` tag on 2026-02-02.
Rather than enumerate all of them, this entry groups them by theme.
See `git log v0.9-pre-simplification..v0.10.0` for the full
chronology.

### The 90-degree saga closed; profile slots done right (2026-07-03)

- **Launch-time 90-degree reset: root-caused and fixed.** A usbmon
  capture of an AC EVO launch showed the game's SDK session pushing an
  operating range of 90 degrees in a TrueForce interface-2 packet
  (type 0x0e - previously misdocumented as a frequency config; its
  canonical init value 2700.0 is the wheel's max range). The new
  `wheel_range_restore` (default on) restores the pre-reset range
  automatically - detection to restore measured under 100 ms against
  a faithful replay of the game traffic - behind safety gates:
  external-and-exactly-90 only, desktop mode only, wheel stationary,
  widen-only, three strikes per session, explicit writes supersede.
  Game-side alternative documented: AC EVO's "Steering lock" setting
  pushes its configured value once touched and re-applied.
- **Profile slot select settled against the wheel's OLED**: fn2 SET is
  the plain profile index (a capture-note misparse had briefly
  suggested a [mode_class, slot] encoding; writing that switches to
  profile 2), fn1 GET returns [profile, mode], and fn3 returns each
  slot's user-assigned NAME - exposed as the new read-only
  `wheel_profile_names` attribute.
- Firmware behaviours documented from the reproduction work: type-0x0e
  is session-scoped, and an idle TF session's range change is
  reverted by the firmware itself after about a minute.

### Hardening, identity, and protocol resolutions (2026-07-02, later)

- **Ten review findings fixed** (commit `c2b3a65`) after an adversarial
  self-review of the KF/TF work: the TF session init and the range
  read-back no longer share a workqueue with the 500 Hz force stream
  (either could stall steering forces); a use-after-free window on
  unplug during TF init is closed; an effect's channel is decided at
  playback start and held (no mid-play migration); fast periodics keep
  their DC offset on the steering axis; spring damping respects the
  effect's saturation caps; TF START/STOP state only advances when the
  packet actually queued; failed TF init retries (bounded); steering
  packets get queue priority over texture; and the profile SET/GET
  wire format is per-device (the GET had been reading onboard slots
  back as "profile 2").
- **New sysfs: `wheel_serial` and `wheel_firmware`** - the real
  12-character serial (matches the USB descriptor) and the firmware
  versions of the wheel base and the motor unit, read from HID++
  DeviceInfo at init and logged in dmesg. Include `wheel_firmware`
  output in bug reports.
- **LED effects 6-9 accepted** - the wheel advertises nine effects,
  not five (live-verified supported-effect list); 6-9 are not yet
  visually labeled. External LED-effect and brightness changes (G Hub
  style tools, the wheel's own menu) now update the sysfs values via
  the wheel's broadcasts instead of going stale.
- **libtrueforce: `logitf_get_stream_feedback()`** - the stream thread
  consumes the wheel's type-0x02 responses (real-time position,
  device-side sample counter); a Linux-native API extension.
- **Protocol documentation majorly extended** - the three
  long-standing unknown features resolved (axis response curves /
  report-HID-usages / brake force), the sub-device map (display
  module, pedal base, motor unit), HID++ error packets, SW_ID and
  0x12-report semantics from Logitech's official specs, DeviceInfo
  identity decode, and corrected feature-catalog rows. See
  docs/RS50_PROTOCOL_SPECIFICATION.md sections 5 and 9.

### Project renamed (2026-07-02)

`logitech-rs50-linux-driver` is now **`logitech-trueforce-linux-driver`**:
the driver covers the whole Logitech TrueForce direct-drive family
(RS50 and G PRO today), not just the RS50, and the name should say so.
Old GitHub URLs and clone remotes redirect automatically. No change for
installed systems: the kernel module and DKMS package were always named
`hid-logitech-hidpp`.

### KF/TF separation and FFB stability (2026-07-02)

- **In-kernel TrueForce texture channel** (`wheel_texture_route`,
  default `tf`). Vibration-class effects (`FF_RUMBLE`, periodic
  effects at 20 Hz or faster) now stream on the wheel's TrueForce
  audio-haptic channel instead of being summed into the steering
  force, matching the Windows KF/TF split. Fixes the "gritty/notchy
  steering under rumble" A/B from issue #8. The TF session init
  (68-packet capture replay, twice) runs lazily on first texture
  playback; verified live on an RS50 (audible texture playback with
  the steering axis still). Texture amplitude respects `FF_GAIN` and
  `wheel_strength` (the firmware does not scale TF samples itself).
- **Spring damping** (`wheel_spring_damping`, default 25%). Emulated
  `FF_SPRING` now carries a synthetic damping term scaled by the
  spring's own coefficient. An undamped host-emulated spring rings on
  a direct-drive motor because of the position-to-force loop latency;
  observed live as AC EVO's map-load centring force oscillating the
  wheel into its over-torque failsafe.
- **Friction chatter fix.** `FF_FRICTION` now ramps through a small
  velocity stick zone (Karnopp model) instead of slamming full-scale
  force on every sign flip of the per-tick encoder delta, which
  buzzed the rim at up to 500 Hz when turning slowly.
- **Honest rotation-range reporting.** Some game launches silently
  reset the physical range to 90 degrees with no HID++ broadcast
  (AC EVO observed); the driver now re-reads the true range on its
  20 s keepalive cadence, updates `wheel_range`, logs the external
  change, and notifies sysfs pollers. Detection only - the driver
  never writes the old range back on its own (unsafe under active
  FFB on a direct-drive wheel).
- **Onboard slot select fixed in compat mode.** `wheel_profile`
  writes for slots 1-5 now encode `[0x02, slot, 0]` per the G Hub
  capture instead of `[slot, 0, 0]`, which had put the slot number
  in the mode-class byte (only desktop mode happened to work).
  On-wheel slot confirmation still pending.
- **Effect-upload debug logging** now includes the full parameters
  (condition coefficients, periodic waveform/period/magnitude, ...)
  for root-causing feel issues via dynamic debug.

### Verified game support (2026-04-26 / 2026-04-29)

End-to-end gameplay verified under Proton on Linux:

- **Assetto Corsa Competizione** (RS50 in G PRO compatibility mode)
- **Assetto Corsa EVO** (RS50 in G PRO compatibility mode)

Both produce full FFB, TrueForce haptics, and complete button /
paddle / encoder binding. The setup is documented as the
"SDK-aware sims" recipe in the README and uses Logitech's own
Authenticode-signed SDK DLLs running unmodified inside Wine via
`tools/install-tf-shim.sh`. No DLL injection, no IAT hooks, no
certificate spoofing. The same setup is expected to work for the
other Logitech-SDK-aware sims (LMU, AMS2, AC, rF2 + Logitech
plugin, iRacing) because they all link against the same SDK.

### Added

- **Full force feedback effect set** via software emulation on top
  of the RS50's constant-force endpoint (commit `d5b7cc0`). The
  driver now accepts and produces `FF_SPRING`, `FF_DAMPER`,
  `FF_FRICTION`, `FF_INERTIA`, `FF_RAMP`, `FF_PERIODIC`
  (SINE/SQUARE/TRIANGLE/SAW_UP/SAW_DOWN) and `FF_RUMBLE` (approximated
  as a low-frequency square shake on the single motor) in addition
  to `FF_CONSTANT`. Condition effects read the live wheel position,
  velocity and acceleration sampled from interface-0 input reports
  at the 500 Hz timer cadence. Motivated by ACC which uploads
  thousands of DAMPER effects and essentially no constant forces,
  revealing the previous constant-only behaviour as a feel-killer.
- **`wheel_ffb_constant_sign` sysfs attribute** (`d7dc398`). Toggles
  the FF_CONSTANT sign compensation the driver applies to line up
  Wine/Proton's DirectInput path with our wire format. Default
  `1` (invert, matching what ACC under Proton expects); set `0` for
  native-evdev apps (`fftest`, SDL FF, custom tools). Only affects
  FF_CONSTANT; condition effects, ramp, periodic, and rumble feel
  identical at either setting. See `docs/SYSFS_API.md` for the full
  rationale and the troubleshooting section in the README for the
  user-facing story.
- **FF-matrix test harness** in `tests/ff_matrix_test.c` + Makefile.
  Walks every effect-type × parameter-combination for uploads
  (16 cases including inverted envelopes, negative coefficients,
  non-zero replay.delay, all periodic waveforms) and observes
  ABS_X motion for CONSTANT direction, RAMP ramp-up, PERIODIC sine
  oscillation, CONSTANT attack envelope, and SPRING centering.
  Auto-toggles `wheel_ffb_constant_sign` off during motion checks
  so the native-convention assertions stay coherent. Found several
  of the bugs below.
- **G PRO Racing Wheel support**, both Xbox/PC (`046d:c272`) and PS/PC
  (`046d:c268`) variants. FFB via the G920-class HID++ 0x8123 path on
  interface 1, TRUEFORCE streaming via the same interface 2 endpoint 0x03
  that the RS50 uses. Every `wheel_*` sysfs attribute relevant to the
  G Pro's hardware is exposed. `gpro_sysfs_init` discovers the
  per-feature SET function numbers and any G Pro-specific sub-device
  features at init time.
- **Wheel calibration** via a new write-only sysfs attribute
  `wheel_calibrate`. Writes a 0..65535 raw encoder value that the wheel
  adopts as the new centre reference. Backed by sub-device `0x05`,
  feature page `0x812C`, function 3 (matching what G Hub does when the
  user clicks Calibrate). Originally only wired up on the G Pro;
  commit `1ed2d80` enabled the same path on RS50 once an RS50 G Hub
  capture (`2026-04-22_re_calibrate.pcapng`) confirmed the sub-device
  layout matches. Closes issue #13.
- **TRUEFORCE full-stack userspace support** in `userspace/libtrueforce/`.
  A shared library that speaks the 64-byte report ID 0x01 stream on
  interface 2 directly via hidraw. Handles the 68-packet two-pass init
  exactly as G Hub does (verified byte-for-byte against both wheels
  across multiple games). Exposes the full Logitech Steering Wheel SDK
  entry-point surface (discover / open / close, set / get torque, TF
  streaming, angle and angular velocity, operating range, damping,
  gain). Forwards range / damping / TF gain to the kernel's `wheel_*`
  sysfs knobs so the library and the driver never disagree.
- **Wine PE shim scaffolding** at `userspace/tf_wine_shim/` (later
  retired - see Removed below). Built a `trueforce_sdk.dll.so` via
  winegcc as an alternative path for Proton games that cannot load
  Logitech's real signed SDK DLL. The real-DLL approach in
  `tools/install-tf-shim.sh` superseded it before end-to-end
  verification, so the shim was moved to `dev/userspace/` (commit
  `08e1c55`).
- **Profile / rotation broadcasts** on interface 1. The wheel emits
  unsolicited notifications on profile button press and rotation-range
  changes; the driver now consumes both and updates cached sysfs state,
  including re-querying dependent settings after a mode change.
- **Onboard and desktop profile/mode support** via `wheel_mode` and
  `wheel_profile`. Switching between `desktop` and onboard profile 1-5
  applies the correct active profile to the wheel and invalidates the
  settings cache so the next sysfs read reflects reality.
- **LIGHTSYNC custom slot control** on RS50. Five user-configurable
  slots with per-LED RGB, per-slot effect/direction, brightness, and
  slot-name write. LED configuration writes are transactional (apply +
  commit) to match G Hub's behaviour.
- **Capture scripts for reverse-engineering** (originally tracked in
  `tools/`, since moved to `dev/tools/` in commit `eb726da` so the
  public repo only carries end-user-relevant tooling). Used to
  decode the G PRO compatibility-mode HID++ feature catalog and the
  desktop-mode entry sequence.
- **CI coverage for userspace**: GitHub Actions builds libtrueforce
  on every push and runs the wire-conversion unit tests
  (`make check`). The earlier Wine PE shim CI job was dropped in
  commit `c4e96b0` after the shim itself was retired (see Removed
  below). Kernel driver continues to build against 5.15 and 6.8.

### Fixed

- **FFB command queue could grow without bound** (issue #8, G920 /
  G923 / G Pro HID++ 0x8123 path): a game replaying a constant force
  re-uploads and re-plays the same effect far faster than the wheel's
  ~300 command/s HID++ drain rate, and the send queue had no coalescing
  or backpressure, so it could reach thousands of entries
  ("command queue contains N commands") and stall feedback. The queue
  now collapses a run of identical-key updates to the latest pending
  one, mirroring how G Hub only ever sends the current state of an effect
  at the device's pace. Implemented as a single drain worker over a
  coalescing FIFO; also switches the queue allocation to GFP_ATOMIC since
  the playback path runs in atomic context. Builds on 6.x and 7.x;
  verified to load and not affect the RS50 path, which uses a separate
  timer-push FFB design and was never affected. Needs confirmation on
  real G920-class hardware.
- **D-pad directions scrambled** (issue #22): the hat reported wrong
  directions in game binding screens, most visibly Left registering as
  Down. Interface 0's HID descriptor already declares a standard hat
  switch that the kernel maps correctly, but the driver also ran a
  hand-rolled byte-0 decode based on a non-standard encoding and emitted
  its own (wrong) hat frame ahead of the correct one. A binding screen
  latches the first frame, so it saw the wrong direction. The redundant
  decode was removed and the native hat mapping left to do its job.
  Verified on a live wheel: Up/Right/Down/Left all report correctly with
  no spurious frames.
- **Build break on kernel 7.x** (issue #24): `hid_report_raw_event()`
  gained a `size_t bufsize` parameter ("HID: pass the buffer size to
  hid_report_raw_event", mainline v7.1, backported into the v7.0.x
  stable series). Because the change was backported partway through a
  point-release range, two kernels with the same `x.y.0` base can carry
  different prototypes, so a `LINUX_VERSION_CODE` check is unreliable.
  Kbuild now probes the actual argument count by syntax-checking a
  six-argument call against the target kernel's own headers and passes
  the new buffer size when present. Builds on 6.x and 7.x with both gcc
  and clang.
- **rmmod regressions on live RS50**: two destroy-path crashes. The
  `ff_hdev` pointer cached on interface 1 became stale if interface 2's
  `hidpp_remove` ran first during rmmod, producing a null-ptr deref
  inside `hid_hw_close`. The thin-probe interfaces also left the
  `hidpp_device` work_structs uninitialised, tripping
  `WARN_ON_ONCE(!work->func)` in `cancel_work_sync`. Both resolved
  (995607f, simplified in 8ab5fc4).
- **FFB filter byte-0 bitfield**: earlier analysis modelled byte 0 as a
  single flag with a per-wheel offset. Cross-capture re-analysis
  decoded it as `bit 0 = user explicit, bit 2 = auto`, identical on
  RS50 and G Pro (63999d8).
- **RS50 damping and trueforce SET function numbers**: damping uses
  fn=1 and trueforce uses fn=3, not the default fn=2 both paths used
  to send. The G Pro init block already had the overrides; the RS50
  path was missing them (c2ee83e).
- **G Pro FFB filter SET**: corrected fn=3 to fn=2 and auto-flag
  encoding to `0x01 / 0x05` after capture analysis on a live G Pro
  (09e2a6c).
- **Profile broadcast handler** previously gated on the wrong nibble of
  the HID++ function byte; missed broadcasts meant the cached
  `wheel_profile` went stale on profile-button presses. Fixed to gate
  on `sw_id == 0` (46914ad).
- **G Pro interface-0 probe path** and the G Pro / RS50 hid_hw_init
  interface iteration (d1a1bd4, 8106b3a) address sixtysecondstosmash's
  "fftest shows 0 effects" report in issue #8. Retest on G Pro still
  pending user confirmation.
- **C90 compliance** on kernel 5.15 builds: three recent additions
  slipped through with C99 inline declarations that the Ubuntu-22.04
  build rejects under `-std=gnu89`. Rolled back to C90-clean
  declarations (7249eef).
- **Batch script line endings**: `tools/*.bat` scripts were LF-only,
  which broke `call :label` resolution in Windows `cmd.exe` past a
  certain file size. Forced CRLF via `.gitattributes` and `-text`
  (35d0eb4).
- **TRUEFORCE init sent twice, not once**: libtrueforce originally
  replayed the 68-packet init on session open but stopped after one
  pass. Live G Hub captures on both wheels show a duplicate pass with
  the sequence counter reset to 1; the library now matches that
  (0aebf70).
- Many smaller correctness fixes: FF_GAIN scaling in the constant-force
  path, constant_force accesses paired under `WRITE_ONCE`/`READ_ONCE`,
  timer re-arming on zero-force release, pedal deadzone overlap
  rejection, rate-limited FFB error counters, wheel_sensitivity numeric
  return in onboard mode, sysfs_emit for show handlers, LIGHTSYNC
  probe cleanup, LED stores that write the device before updating the
  cache.
- **Sensitivity cache aliasing** correctly gated on `mode_known` so a
  failed mode query no longer caches an LED-brightness value as wheel
  sensitivity (`a99847b`).
- **Out-of-tree build portability**: dropped the `usbhid/usbhid.h`
  include and inlined the one `hid_to_usb_dev` macro we used from it,
  so builds succeed on Fedora, CachyOS, Arch and similar distributions
  whose kernel-devel package does not ship that internal header
  (`f2d212c`).

### Changed

- **Phase A audit closed**. The remaining Phase A findings were all
  worked through in commits `0d8918a` (7 trivial findings closed),
  `cc3e46a` (SYS.F29: sysfs attributes moved behind a single
  `attribute_group`, -67 lines), `0cd9fc7` (SYS.F41: extract
  `hidpp_errno` helper, -21 lines across 14 call sites), `934efb7`
  (SYS.F40: document the `params[2] = 0` padding convention), and
  `25fb739` (SYS.F21: split `rs50_ff_discover_features` into settings
  and LIGHTSYNC halves). The remaining strategic items (god-struct
  split, table-drive the settings handlers) were explicitly deferred
  with rationale recorded in `dev/docs/plans/STATUS.md`.
- **Protocol spec (`docs/RS50_PROTOCOL_SPECIFICATION.md`) bumped to
  v6.1**, rescoped to cover both RS50 and G Pro, D-pad rewritten from
  4-way to 8-way, per-feature SET function numbers tabulated,
  centre-calibration section added.
- **TRUEFORCE doc** rewritten from "research only" to the current
  implementation state, including the library layout, the two-pass
  init, and the wheel-coverage table.
- **SYSFS API, README, RS50_SUPPORT** brought in sync with the code.
- **USB_CAPTURE_GUIDE** broadened from "G Pro-specific" to "any
  Logitech wheel beyond the two we already support", with references
  to the `tools/windows_*_captures.bat` scripts and updated protocol
  background.

### Removed

- **`userspace/tf_wine_shim/`** moved to `dev/userspace/`
  (gitignored) in commit `08e1c55`. It was Phase 23.1 scaffolding,
  never end-to-end-verified, and superseded by
  `tools/install-tf-shim.sh`, which copies Logitech's own
  Authenticode-signed SDK DLLs into Wine prefixes. The CI job that
  built the shim was dropped in `c4e96b0`.
- **Reverse-engineering / capture tooling** moved to `dev/`
  (commit `eb726da`): `docs/RS50_SUPPORT.md`,
  `docs/USB_CAPTURE_GUIDE.md`, `docs/WINDOWS_RE_CAPTURE_GUIDE.md`,
  `tools/windows_gpro_compat_capture.bat`,
  `tools/windows_gpro_compat_range_capture.bat`,
  `tools/windows_tf_captures.bat`,
  `tools/windows_wheel_captures.bat`. These are contributor /
  maintainer tools not needed by end users; the public repo now
  carries only end-to-end driver files plus user-facing docs.

### Documentation

- New `userspace/libtrueforce/tests/unit.c` covering the wire-format
  conversions with a 65536-sample monotonicity sweep.
- Phase B gap analysis (`dev/docs/plans/2026-04-16-windows-gap-analysis.md`)
  and the Phase A audit (`dev/docs/plans/2026-04-16-code-audit.md`)
  are archived; `dev/docs/plans/STATUS.md` maps each rank and finding
  ID to its current shipping state.

## v0.9-pre-simplification (2026-02-02)

Tagged snapshot before the simplification + audit sprint. RS50-only,
FFB constant force via the existing `rs50_ff_*` path, basic sysfs
settings, LIGHTSYNC per-slot writes. See `git log
v0.9-pre-simplification` for the full history up to that point.
