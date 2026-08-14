# Simulated TrueForce for sims that publish to shared memory

Most sims broadcast telemetry over UDP, which `logi-tf-sim` reads directly.
A few do not: iRacing, RaceRoom, the Assetto Corsa family, rFactor 2
and Le Mans Ultimate publish into a named Windows shared-memory section that
only the game's own SDK reads. Nothing on the Linux side can see it.

`logi-tf-relay` is a small Windows executable that runs inside the game's
Proton prefix, reads that section with the ordinary Win32 API (which Wine
implements), and forwards engine speed, redline, throttle and gear to the
daemon over localhost UDP.

## What works today

| Game | State |
|---|---|
| iRacing | Decoder written. Unconfirmed against a live session. |
| RaceRoom Racing Experience | Decoder written. Unconfirmed against a live session. |
| Assetto Corsa | Decoder written. Layout confirmed via Competizione, which shares it. |
| Assetto Corsa Competizione | **Confirmed end to end on a G923** (2026-08-06). |
| Assetto Corsa EVO | **Layout confirmed on a live session** (2026-08-06). |
| rFactor 2 | Decoder written. Unconfirmed against a live session. |
| Le Mans Ultimate | Decoder written. Unconfirmed against a live session. |

What separates these is not difficulty, it is whether the layout can be
trusted without a capture. Each earned that trust a different way.

**iRacing's telemetry is self-describing.** The section starts with a small
header pointing at a table of variable descriptors, each carrying a
variable's name next to its offset and type, so the decoder looks up `RPM`,
`Throttle` and `Gear` by name at runtime. Nothing about where those values
live is guessed.

**RaceRoom's layout is published by the people who write it.** KW Studios
ship `r3e.h` at `github.com/sector3studios/r3e-api`, in the public domain,
and they own both ends of the interface with no plugin in between. The struct
is byte-packed, so no compiler can disagree about where a field sits, and it
opens with a major version number the decoder checks before reading anything
else.

**Assetto Corsa only needs the head of its struct.** Kunos have appended
fields to the physics block for a decade, but appending does not move what
came before, and throttle, gear and rpm have been in its first 32 bytes since
1.0. The redline comes from a second block where the offset is less obviously
safe, so the decoder verifies that block's layout in-band before trusting it.
Competizione publishes the same section names and the same layout, byte for
byte through every field used, so it is read by the same decoder.

EVO is the one that changed. Kunos renamed every section and took the car
spec sheet out of the static block, so the redline the older two read there
does not exist. Its replacement, `currentMaxRpm`, sits in the physics block
and is republished every tick. That leaves EVO with one section and no layout
guard, so the check is the value itself: a redline is a distinctive number,
and a wrong offset in a physics block lands on a temperature, a pressure or a
pedal, none of which comes near one.

Confirmed on a live session on 2026-08-06: `acevo_pmf_physics` opened while
`acpmf_physics` stayed absent, which is itself evidence the rename is real,
and offset 588 read 6200 next to an engine turning 4662 in first gear. A
number that plausible, that consistent with its neighbours, is not a
temperature read by accident.

**rFactor 2 and Le Mans Ultimate say whether a read was good.** Their layout
comes from a community plugin rather than a vendor, which is why they were
written last: the struct depends on which fork and build the user installed.
What makes them decodable anyway is that the format carries its own
consistency state. Each buffer is preceded by a pair of counters the plugin
bumps before and after writing, so a read that caught a write in progress is
detectable and gets dropped. And the player's car is found by matching a slot
id between two independently written buffers, which a misaligned read fails
rather than passes with plausible numbers.

Assetto Corsa Competizione was confirmed end to end on 2026-08-06, on a
G923. Both sections opened; the physics head read a coherent live car; the
static block's UTF-16 guard passed on real bytes; and `maxRpm` at offset 412
read 8650, a genuine GT3 redline rather than plausible garbage.

Then the whole chain was run: game shared memory, relay wire format, daemon,
synth, wheel. With the car **stationary in the pit box** and the engine
revving, the wheel produced an engine note. That is the test worth repeating,
because force feedback cannot fake it: a parked car generates no tyre or
suspension force, so anything felt while stationary came from telemetry. Same
offsets confirm Assetto Corsa, which shares this layout.

The rest have not been run against a live game. Every read in them is bounds
checked and range gated, and each drops a sample it cannot vouch for rather
than sending a wrong number, but that is a design argument, not evidence.

## Get it

**You normally do not have to place this yourself.** `sudo ./tools/setup.sh`
puts it in every Proton prefix, and the settings app has an "Install relay"
button on each game that needs one (in the terminal app, `h` on the selected
game). It lands at the prefix's drive root, which
makes the in-prefix path `C:\logi-tf-relay.exe`.

If you installed from a package (Debian, Arch, Fedora, openSUSE), the master
copy is at:

```
/usr/share/logitech-trueforce/logi-tf-relay.exe
```

It is a Windows executable, so it lives in the shared data directory rather
than in `bin`: you do not run it directly, you run it inside a game's Proton
prefix. It ships prebuilt because no distro builder has a Rust Windows
target, the same reason `tf-range-proxy.dll` is prebuilt.

Otherwise download `logi-tf-relay-<version>.exe` from the
[latest release](https://github.com/mescon/logitech-trueforce-linux-driver/releases/latest).

To build it yourself, or to refresh the committed copy after changing the
relay's sources:

```bash
rustup target add x86_64-pc-windows-gnu
tools/build-relay.sh
```

If `cargo` comes from your distribution rather than rustup, put rustup's
first: a system cargo usually has only the host target, and the failure is
an unhelpful "can't find crate for `core`".

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Do not try to satisfy it by unpacking an upstream `rust-std` into the system
`/usr/lib/rustlib`. A precompiled std has to come from the exact rustc build
that will use it, and a distribution's patched rustc is not that build; the
result is E0514, "found crate `core` compiled by an incompatible version of
rustc".

That uses the `relay-dist` cargo profile, which is what keeps the committed
binary small, and writes `tools/logi-tf-relay.exe`. Refresh it that way
rather than by hand: CI runs `tools/build-relay.sh --check` and fails if the
committed binary is older than the sources it was built from, so a packaged
relay cannot silently lag the code. The build needs a MinGW linker, no
Windows machine and no Wine.

## Run it

Run the relay in the same Proton prefix as the game, while the game is
running. The prefix is named after the game's Steam appid:

| Game | `--game` | Steam appid | Also needs |
|---|---|---|---|
| iRacing | `iracing` | 266410 | |
| RaceRoom Racing Experience | `raceroom` | 211500 | |
| Assetto Corsa | `assetto` | 244210 | |
| Assetto Corsa Competizione | `acc` | 805550 | |
| Assetto Corsa EVO | `ac-evo` | 3058630 | |
| rFactor 2 | `rf2` | 365960 | `rF2SharedMemoryMapPlugin` |
| Le Mans Ultimate | `lmu` | 2399420 | `rF2SharedMemoryMapPlugin` |

The easy way is to let the game start it, by putting this in the game's
Steam launch options:

```
logi-launch %command%
```

That starts the relay inside the prefix once the game is up, and works out
which game it is from the appid, so there is nothing else to set. See
[LAUNCH_OPTIONS.md](LAUNCH_OPTIONS.md).

**Starting it by hand is fiddlier than it looks, and the obvious command is
a trap.** `WINEPREFIX=... wine ...` uses your distribution's wine, which is
a different build from the one Proton made the prefix with: it runs prefix
initialisation, prompts to install wine-mono, and can convert the prefix.
Use the prefix's own wine instead, and only while the game is already
running, because Proton waits for any existing wineserver to exit before it
will launch:

```bash
PFX=~/.steam/steam/steamapps/compatdata/244210
PROTON=$(sed -n 's#^\(/.*\)/files/.*#\1#p' "$PFX/config_info" | head -1)
WINEPREFIX="$PFX/pfx" "$PROTON/files/bin/wine" 'c:\logi-tf-relay.exe' --game assetto
```

Leave it running. It re-reads the section about 60 times a second and sends
what it finds to `logi-tf-sim`, which must also be running. Then turn the
game on in the app's Setup page under Simulated TrueForce.

**Competizione and EVO are special cases.** Both have real TrueForce of
their own, and on a direct-drive wheel that is the route to use: install the
shim and skip the relay entirely. The relay is here for the G923, which
cannot receive the SDK's TrueForce at all, so a synthesized engine note is
the difference between haptics and silence rather than a second-best.

None of these needs anything switched on inside the game: unlike the UDP
titles, the shared memory is always published. rFactor 2 and Le Mans Ultimate
do need the community `rF2SharedMemoryMapPlugin` in the game's `Plugins`
directory, though, or the game publishes nothing at all.

If your Steam library is on another drive, `./tools/setup.sh doctor` prints
the roots it found.

If the daemon uses a non-default relay port, tell the relay too:

```bash
LOGI_TF_SIM_RELAY_PORT=20999 wine logi-tf-relay.exe --game iracing
```

## Capture a fixture

Every decoder here is written against a published layout, and none has yet
been confirmed against a running game. If one of them stays silent, or sends
something obviously wrong, a dump is what turns that into a fix.

With the game running and a session actually **live** (sitting in the menus
is not always enough):

```bash
WINEPREFIX=<the game's prefix> \
  wine logi-tf-relay.exe --game lmu --dump lmu-dump.bin
```

Attach `lmu-dump.bin` to an issue, saying which game and which build. The
dump is what a decoder gets re-tested against, so a real one settles a
question that no amount of reading headers can.

The dump contains vehicle telemetry for the session that was running. It
carries no account details, no keys and no personal data, but it does reflect
what you were driving at that moment.

## The relay datagram is a generic RPM contract

Everything above describes the producers this project ships, but the port
is a contract, not a private channel: **any** telemetry producer that emits
the LTFR datagram to `127.0.0.1:20780` plugs into the same consumers. That
includes `logi-rpm-bridge`, which forwards `rpm max_rpm` into the driver's
`wheel_texture_rpm` sysfs attribute and therefore feeds the native
TrueForce texture merge - so a homegrown bridge for an unlisted sim gets
the on-wheel engine texture for its title as soon as the game's recipe
grants `texture=merge` (the registry in
`userspace/logi-wheel/crates/logi-wheel-core/src/games.rs`; today only
AC EVO, because grants ride hardware evidence).

`logi-rpm-bridge` is also the rev-light feeder: besides
`wheel_texture_rpm`, it drives `wheel_rev_level` from the same datagrams.
The default mapping is a full rev bar (LED 1 as soon as rpm > 0, all 10 at
the limiter; works for 28-byte senders too). `LOGI_REV_MODE=shift`
selects the dash band instead: dark below the first-shift-light rpm,
level 1 exactly there, 10 at the limiter (needs the 32-byte form below).
`LOGI_REV_SYSFS` overrides the LED target attribute and `LOGI_RPM_PORT`
the UDP port. The strip darkens on telemetry loss (1 s) and on bridge
exit.

The wire format, LTFR version 2 (32 bytes since 2026-08-14, little-endian,
append-only - the first 28 bytes are the original version-2 layout, the
version byte is unchanged, and old consumers keep reading just those 28;
the authoritative copy lives in `logi-wheel-core`'s `relay.rs`):

| offset | field    | type   | notes |
|---|---|---|---|
| 0  | magic    | 4 bytes | `LTFR` |
| 4  | version  | u8      | 2 |
| 5  | flags    | u8      | bit 0 = airborne; other bits reserved, send 0 |
| 6  | game id  | 8 bytes | ASCII, NUL-padded (`ac-evo`, `acc`, ...) |
| 14 | rpm      | f32 LE  | engine speed, rpm |
| 18 | max_rpm  | f32 LE  | engine redline, rpm |
| 22 | throttle | f32 LE  | 0.0-1.0; 0 = the sender cannot tell |
| 26 | gear     | i16 LE  | -1 reverse, 0 neutral, 1..N; 0 also = unknown |
| 28 | shift_rpm | f32 LE | first-shift-light rpm (appended field; absent from 28-byte senders, which is fine) |

Send at roughly 60 Hz; consumers rate-limit on their side, and
`logi-rpm-bridge` drops packets whose rpm is not in `[0, 30000)`. Fields a
producer cannot supply are zero, which the format defines as "the sender
cannot tell".

## Troubleshooting

**"not readable yet"** repeated: the relay is running but the game is not
publishing. Check the game is actually in a session, and for rFactor 2 and Le
Mans Ultimate that the shared-memory plugin is installed.

**Nothing happens although the relay says it is streaming**: check
`logi-tf-sim` is running, that the game is switched on in the Setup page, and
that both agree on the port.

**The relay exits immediately on Linux**: that is the stub. It only does
anything inside a Wine prefix.
