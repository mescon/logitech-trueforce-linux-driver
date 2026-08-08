# Changelog

This project follows a loose semver: major versions mark API-breaking
changes to the sysfs surface, minor versions add supported wheels or
new attributes, patch versions are bug fixes and documentation. Pre-1.0
the contract is "it works on RS50 and G Pro as listed here".

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
count and not a flag, and it sent zero there. The same command shape with a
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
  nothing, because the old shape could never have been linked against
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
  the command set, ten layouts, and the finding that shapes any future
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
  direct-drive, but the driver now also supports the belt-driven G923, and
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
  shape the analog handbrake (base `0x80A4` axis 4), the same curve type as the
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
  reported raw, exactly as before; shape them in userspace instead. The steering
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
- Belt-driven **G920/G923 are no longer claimed** by this fork; they use
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
