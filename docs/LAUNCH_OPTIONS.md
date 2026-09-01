# Steam launch options, and what each one is for

Everything this project asks you to put in a game's launch options, in one
place. Right-click the game in Steam, **Properties**, **Launch Options**.

`%command%` is the placeholder Steam replaces with the real command that
starts the game. Type it exactly, including the percent signs. Anything to
the left of it runs first; the game itself is what `%command%` becomes.

## The short version

Put this in every racing game's launch options and stop thinking about it:

```
logi-launch %command%
```

It works out what that game needs on the wheel you have attached, and does
it: sets `PROTON_ENABLE_HIDRAW` only where that wheel wants it, launches
through `logi-ffb` for the DirectInput games, starts `logi-tf-sim`, and
starts the telemetry relay inside the game's Proton prefix for the sims that
need one. A game that needs none of it starts none of it, so leaving the
line in place costs nothing.

It knows 29 titles by their Steam appid. A game it does not know still gets
the daemon, which is all the UDP-telemetry games need, and you can describe
it yourself (see [Teaching it a new game](#teaching-it-a-new-game)).

**It will not put simulated TrueForce on a game that has its own.** On a
direct-drive wheel, Assetto Corsa Competizione and Assetto Corsa EVO get
their real TrueForce through the shim and nothing is layered on top. On a
G923, where that cannot arrive, the same titles get the simulated kind
instead.

The rest of this page is what `logi-launch` is doing on your behalf, for
when you want to do it by hand or understand what went wrong.

## What `logi-launch` actually does

In order, when you start a game with it:

**1. It asks the app what this game needs.** `logi-wheel --launch-plan
<appid>` answers from the compatibility registry, for the wheel currently
attached. Steam already provides the appid in the environment, so nothing
has to be configured. The registry is the same one behind the app's Setup
page: the wrapper decides nothing itself, because a second copy of that
logic in shell is a second copy to drift.

**2. It sets `PROTON_ENABLE_HIDRAW` only if that wheel wants it here**, and
scoped to that wheel (`0x046D/0xC276`) rather than the bare `1`, which
Proton reads as every HID device on the machine.
Games with their own TrueForce need the raw HID interface to reach the
wheel, and the SDK only delivers it to a direct-drive wheel. On a G923 the
same setting **removes** force feedback, so it is never set speculatively:
no wheel, or two different kinds and none named, means it is withheld and
the log says why.

**3. It chains through `logi-ffb` for DirectInput games.** Le Mans Ultimate,
rFactor 2, iRacing and RaceRoom drive force feedback the old Windows way,
which needs the proxy in front of the game. `logi-launch` execs it rather
than asking you to add a second wrapper.

**4. It starts `logi-tf-sim` if the game needs it.** The daemon turns a
game's telemetry into TrueForce haptics and rev-light levels. It is left
running afterwards, because it idles when nothing is streaming. If a wheel
was named, the daemon is aimed at that same wheel, so the game and the
haptics cannot end up on different ones.

It is **not** started for a game whose own TrueForce already reaches your
wheel. On a direct-drive wheel, Assetto Corsa Competizione and Assetto
Corsa EVO get the real thing through the shim, and the simulated kind on
top of it would be two engine notes at once. On a G923, where the real one
cannot arrive, those same titles do get it.

**5. It starts the game.**

**6. Once the game has the prefix, it starts `logi-tf-relay` inside it.**
Only for the sims that publish telemetry to Windows shared memory rather
than over the network. This step is last for a reason, and the reason is
not obvious: Proton takes the prefix exclusively when it launches, running
`wineserver -w` and waiting for any existing wineserver to exit. A helper
started **before** the game stops the game from starting at all. So the
wrapper launches the game first and waits for the game's own wineserver to
appear before attaching anything.

The relay is run with the wine build the prefix itself belongs to, read
from its `config_info`. The distribution's `wine` is a different build, and
pointing it at a Proton-made prefix triggers prefix initialisation, prompts
to install wine-mono, and can convert the prefix. If that build cannot be
found, the relay is skipped rather than run the wrong way.

### What it never does

- Set `PROTON_ENABLE_HIDRAW` on a guess. Wrong here silently costs a G923
  owner their force feedback, and nothing in the game would explain it.
- Put simulated TrueForce on a game that already has its own on your wheel.
- Start anything for a game that needs nothing, so leaving the line in
  every game's launch options costs nothing.
- Offer advice for a title that does not run on Linux at all.

### Seeing what it decided

Everything it does is logged to `/tmp/logi-launch.log`, starting with the
plan it resolved:

Assetto Corsa EVO on a direct-drive wheel, where the game's own TrueForce
is the route and nothing is simulated:

```
[logi-launch] plan: wheel=direct-drive game=Assetto Corsa EVO (early access) hidraw=0x046D/0xC276 ffb=native relay=none tfsim=0
[logi-launch] set PROTON_ENABLE_HIDRAW=0x046D/0xC276
[logi-launch] no in-prefix relay needed for this game
```

The same game on a G923, where it cannot be, so the simulated kind and its
relay are used instead:

```
[logi-launch] plan: wheel=classic game=Assetto Corsa EVO (early access) hidraw=unset ffb=native relay=ac-evo tfsim=1
[logi-launch] starting logi-tf-sim, aimed at g923
[logi-launch] starting c:\logi-tf-relay.exe --game ac-evo in .../compatdata/3058630/pfx
```

Note there is no `PROTON_ENABLE_HIDRAW` in the second one. That is the
difference that costs a G923 owner their force feedback if it is guessed.

If something did not happen, that file says which step declined and why.

## The options

| Option | What it does | When you want it |
|---|---|---|
| `logi-launch %command%` | Works out everything below for this game and this wheel, and applies it | **Every racing game.** It is the only line most people need |
| `PROTON_ENABLE_HIDRAW=0x046D/0xC276 %command%` | Lets the game talk to the wheel's raw HID interface, which is how Logitech's SDK delivers TrueForce | Set by `logi-launch`, scoped to your wheel. By hand: a **direct-drive wheel** (RS50, G PRO) in a game with its own TrueForce (ACC, Assetto Corsa EVO), with the TrueForce files installed. Use your own product id, not `0xC276` |
| *(no `PROTON_ENABLE_HIDRAW` at all)* | The wheel stays an ordinary Linux force-feedback device | By hand on a **G923**, always. This row is about that one variable, not about `logi-launch`, which every wheel wants |
| `logi-ffb %command%` | Presents a virtual wheel that speaks the older DirectInput force-feedback protocol, and forwards it to your real wheel | Applied by `logi-launch`. By hand: games that only do DirectInput FFB (Le Mans Ultimate, rFactor 2, iRacing, RaceRoom), on **any** wheel including a G923 |

The bottom three rows describe what `logi-launch` does for you. You only type
them yourself if you would rather not use it.

`gamemoderun` is not ours. It is from `gamemode` and composes fine with all
of the above.

## Combining them

Order is: **environment variables first, then wrappers, then `%command%`.**
Each wrapper runs the next one along, so they chain left to right.

`logi-launch` composes with things that are not ours. Assetto Corsa EVO, with
gamemode and the graphics settings that title wants:

```
PROTON_ENABLE_NVAPI=1 VKD3D_CONFIG=descriptor_heap gamemoderun logi-launch %command%
```

That line is the same on an RS50 and on a G923. Do not add
`PROTON_ENABLE_HIDRAW` to it: `logi-launch` sets that itself on the wheels
that want it, and setting it yourself is how it ends up on a G923, where it
costs that wheel its force feedback.

### With the TrueForce files it is an upgrade; without them it is a loss

**With them**, this is the configuration to want. The game's own TrueForce
reaches the wheel: the haptics its developers authored, rather than the
engine note `logi-tf-sim` synthesises from telemetry. That is a real
difference, and it is the reason this setting exists.

**Without them**, it is a pure loss. Turning it on makes Proton hand the game
the raw HID device instead of the one backed by the Linux input layer, and
**this wheel's raw descriptor carries no force-feedback protocol at all**.
Logitech's SDK is what fills that gap, and the SDK is what those files are.
So on a prefix without them you lose force feedback and gain nothing.

`logi-launch` checks the prefix and declines rather than make that trade,
saying so in the log, and falls back to simulated TrueForce so the wheel is
not left with nothing. That is issue #60, where it read as "logi-launch gives
me no FFB".

The corollary is worth stating plainly: **for Assetto Corsa Competizione and
Assetto Corsa EVO on a direct-drive wheel, Logitech's files are what carry
force feedback**, not only TrueForce. They are optional only in the sense
that you can leave the raw interface off and keep the ordinary path.

One thing this does **not** change: force feedback itself is never simulated
by this project. `logi-tf-sim` synthesises TrueForce and nothing else, so the
forces you feel through the evdev path are the game's own, exactly as its
developers wrote them. What the raw interface changes is which route those
forces travel, and whether the game's TrueForce can travel at all. No
measurement here says the SDK route feels better than the evdev one; the
gain that is established is native TrueForce over simulated.

### The engine buzz, and a stale variable to remove

Real TrueForce through the SDK carries the game's steering forces but not the
fine engine-note texture. On Windows that texture is G HUB's own addition,
synthesised from the game's RPM and merged into the same stream, not
something the game sends itself. Since 2026-08-13 the driver does the same
merge, and `logi-launch` wires all of it up for you: for Assetto Corsa EVO on
a direct-drive wheel, plain `logi-launch %command%` stages the proxy DLL,
starts `logi-rpm-bridge`, and turns on the merge, tearing both down again
when the game exits. The same telemetry lights the rev strip: the proxy
relays the game's live rpm, first-shift-light rpm and redline, and
`logi-rpm-bridge` drives `wheel_rev_level` with them. Nothing else to add,
and nothing to type differently from any other title.

**IMPORTANT: remove `LOGI_ESCAPE_RELAY=0` if it is sitting in a game's launch
options from an older manual recipe.** That variable turns the dinput8 proxy
into capture-only, so it stops relaying the RPM the texture merge needs, and
the wheel goes back to force with no buzz for no reason that shows up
anywhere. `logi-launch` never sets it, and the relay must stay on.

### Why it names your wheel rather than saying `1`

Proton matches this variable as a **substring** against each device's own
`0xVID/0xPID` (`dlls/winebus.sys/main.c`). The bare value `1` short-circuits
that test and returns true for **every HID device on the machine**, so your
keyboard, headset and any other controller are handed to the game as raw HID
alongside the wheel. Naming the wheel is what the pattern form is for, and
what `logi-launch` now does. Reported as issue #60, where `1` cost an RS50
owner their force feedback in Assetto Corsa EVO.

Le Mans Ultimate, on any wheel:

```
logi-launch %command%
```

`logi-launch` starts `logi-ffb` for you there, because that title uses
DirectInput force feedback. You do not type `logi-ffb` yourself.

### The same two by hand

If you would rather not use the wrapper:

```
PROTON_ENABLE_NVAPI=1 VKD3D_CONFIG=descriptor_heap PROTON_ENABLE_HIDRAW=0x046D/0xC276 gamemoderun %command%   # AC EVO, RS50
PROTON_ENABLE_NVAPI=1 VKD3D_CONFIG=descriptor_heap gamemoderun %command%                          # AC EVO, G923
logi-ffb %command%                                                                                # Le Mans Ultimate, either wheel
```

The first two differ by one variable, and that variable is the difference
between TrueForce on one wheel and no force feedback at all on the other.
Doing this by hand means getting it right per game and per wheel, every time.
You also start `logi-tf-sim` yourself, and install the relay per game from the
app's Setup page.

## Why the relay has to run inside the prefix

Some sims never send telemetry over the network. The Assetto Corsa family
(including EVO), iRacing, RaceRoom, rFactor 2 and Le Mans Ultimate publish
into a named Windows shared-memory section instead. Assetto Corsa EVO has no
UDP output at all: its binary carries a `SharedMemoryPhysicsWriter` and
`CreateFileMappingA`, and every socket in it belongs to the multiplayer
stack.

Nothing on the Linux side can read that section, and nothing on another
machine can either. Reading it needs a Windows process inside the same
Proton prefix. That is what `logi-tf-relay` is, and it is how a remote
SimHub, a buttkicker or a phone dashboard can be fed from a game running
here.

**The catch is ordering, and it is worth understanding before you fight it.**
Proton takes the prefix exclusively when it launches: it runs `wineserver -w`
and waits for any existing wineserver to exit first. So a helper started
before the game stops the game from starting at all, and one started by hand
afterwards is a step you have to remember every session.

`logi-launch` runs the game immediately and starts the helper afterwards,
once the game's own wineserver exists. It also runs the helper with the
**same wine build the game is using**, read from the prefix's `config_info`.
That matters: the distribution's own `wine` is a different build, and
pointing it at a Proton-made prefix triggers prefix initialisation, prompts
to install wine-mono, and can convert the prefix. `logi-launch` refuses to
attach at all rather than do that.

### You should not have to configure it

With nothing set, `logi-launch` runs **this project's own `logi-tf-relay`**
and works out which game it is from the appid Steam already provides. So
for simulated TrueForce the whole setup is:

```
logi-launch %command%
```

It knows the seven titles that publish to shared memory (iRacing, RaceRoom,
Assetto Corsa, Competizione, EVO, rFactor 2, Le Mans Ultimate). For any
other game it starts nothing and simply launches it, so leaving it in the
launch options of a game that does not need it costs nothing.

If the relay is not in that game's prefix yet it says so in the log and
launches the game anyway, rather than failing silently. Install it from the
app's Setup page, **Install relay**.

### Running something else instead

| Variable | Default | Meaning |
|---|---|---|
| `LOGI_LAUNCH_EXE` | `c:\logi-tf-relay.exe` | Run this **instead of** the relay, as a Windows path inside the prefix |
| `LOGI_LAUNCH_ARGS` | `--game <from appid>` | Arguments passed to it |
| `LOGI_LAUNCH_HELPERS` | none | Run these **as well as** the relay (see below) |
| `LOGI_LAUNCH_WAIT` | `120` | Seconds to wait for the game's wineserver |
| `LOGI_LAUNCH_SETTLE` | `15` | Seconds to let the game create its sections first |
| `LOGI_LAUNCH_TF_SIM` | `1` | `0` leaves the `logi-tf-sim` daemon alone, for running it yourself |
| `LOGI_LAUNCH_LOG` | `/tmp/logi-launch.log` | Where it writes what it did |
| `LOGI_REV_MODE` | full bar | `shift` makes `logi-rpm-bridge` map the rev strip to the dash band: dark below the car's first shift light, level 1 exactly there, 10 at the limiter. The default lights LED 1 as soon as rpm > 0 and all 10 at the limiter |
| `LOGI_FFB_DEVICE` | the wheel `--wheel` resolved | Which evdev node `logi-ffb` drives (`eventN` or a full path). `logi-launch` sets it from the wheel it set the session up for; set it yourself only to override that |
| `LOGI_TF_CAPTURE` | worked out from the wheels attached | `1` makes the TrueForce SDK shim relay the game's own TrueForce to `logi-tf-sim`, `0` stops it. Only a G923 wants it: on a direct-drive wheel Logitech's library streams those samples itself, and a second writer on the wheel's one-packet-per-millisecond endpoint takes turns with it rather than sharing, which buzzes. `logi-launch` sets it from `--wheel` when you name one |
| `LOGI_TF_REARM` | `0` | **Experimental.** `1` makes `logi-launch` re-arm the TrueForce session before the game starts (the stop/start pair plus the 68-packet init, twice, from `tools/tf-init.bin`), for recovering from a previous session that died without teardown. Off by default pending hardware validation; a power cycle of the base remains the proven recovery |
| `LOGI_ESCAPE_LOG` | `1` | The escape proxy staged into a game writes what the title's TrueForce SDK did to `dinput8-escape.log` beside itself. `0` switches that off. On by default because it is the one record that exists when a report comes in; since 0.39.2 its cost is a buffered write, flushed at most twice a second. |

`LOGI_LAUNCH_EXE` replaces the relay, so simulated TrueForce and the rev
lights lose their telemetry. That is the right choice only when you want
nothing driven on this machine.

### Running your own helpers inside the prefix

`LOGI_LAUNCH_HELPERS` starts extra Windows programs inside the game's
prefix, alongside whatever the plan already decided. Our relay is one such
program; there is nothing special about it, and anything else that has to
run beside the game can go the same way.

```
LOGI_LAUNCH_HELPERS='c:\sim-teleport.exe source' logi-launch %command%
```

Semicolons separate several:

```
LOGI_LAUNCH_HELPERS='c:\sim-teleport.exe source; c:\dash-bridge.exe --port 8080'
```

#### Why it has to be in the prefix

The sims that matter here publish telemetry into a **named Windows
shared-memory section**: the Assetto Corsa family including EVO, iRacing,
RaceRoom, rFactor 2 and Le Mans Ultimate. That section exists inside the
Wine prefix and nowhere else. No Linux process can open it, and neither can
a program on another machine. Reading it takes a Windows process in the same
prefix, which is the whole reason this mechanism exists.

Games that broadcast telemetry over **UDP** instead (Automobilista 2, the F1
titles, the Codemasters rally games, BeamNG) need none of this. Point the
game's own telemetry setting at `127.0.0.1` and read it from Linux directly.

#### Getting a program in there

Copy the `.exe` into the prefix's `drive_c`, which is where `c:\name.exe`
resolves to:

```
cp yourhelper.exe ~/.steam/steam/steamapps/compatdata/<appid>/pfx/drive_c/
```

Then name it in `LOGI_LAUNCH_HELPERS` as `c:\yourhelper.exe`.

#### What logi-launch handles for you

The hard part is not starting the program, it is starting it at the right
moment, and this is the reason to go through the wrapper rather than
launching it yourself:

- **Order.** Proton takes the prefix exclusively at launch: it runs
  `wineserver -w` and waits for any existing wineserver to exit first. Start
  a helper first and **the game does not start at all**, it sits waiting for
  your helper to quit. `logi-launch` execs the game immediately and starts
  helpers afterwards, once the game's own wineserver exists.
- **The right wine.** Helpers run with the same Proton build the game is
  using, read from the prefix's `config_info`. The distribution's `wine` is
  a different build against a Proton-made prefix: it prompts to install
  wine-mono and can convert the prefix.
- **Settling.** It waits `LOGI_LAUNCH_SETTLE` seconds (15 by default) so the
  game has created its sections before the first probe.

#### What suits this, and what does not

Good candidates are programs that **attach to a running game**: telemetry
readers, bridges to a dashboard or a second PC, bass-shaker feeders, logging
tools. Several reading the same telemetry is fine, and not a conflict: a
Windows shared-memory section takes any number of readers.

It does not suit anything that must run **before** the game, such as a
launcher, a patcher, or a mod manager. Those need the prefix to themselves,
which is the exact situation this design avoids.

#### Limits worth knowing

- The program name is whatever precedes the first space, so it cannot itself
  contain one. In `drive_c` the path is `c:\name.exe` and the question does
  not arise.
- Each helper gets its own wine process, and they are all started together
  rather than one after another. A helper that never exits does not hold up
  the next one.
- Helpers stop when the game does, because Proton tears the prefix down on
  exit. That is observed rather than promised: a helper of your own that
  survives it would keep a wineserver alive and delay the next launch. If a
  game refuses to start, check for a stray `wineserver` first.
- Output goes to `/tmp/logi-launch.log`, with `WINEDEBUG=-all` unless you
  set `WINEDEBUG` yourself.

See [SHARED_MEMORY_RELAY.md](SHARED_MEMORY_RELAY.md) for the relay itself.

### Feeding SimHub on another PC

`sim-teleport` ([upstream](https://github.com/t-hovestadt/sim-teleport)) is a
third-party tool, not part of this project. Its **source** half runs on the
gaming PC and its **target** half on the SimHub PC, where it recreates the
game's shared-memory sections locally so SimHub reads the real layout and
identifies the game correctly, rather than being fed a re-encoded
approximation.

Its source half runs correctly under Proton: confirmed on 2026-08-09 with
Assetto Corsa EVO, which it detected from the menu via
`Local\acevo_pmf_physics`. Put `sim-teleport.exe` in the prefix's `drive_c`,
then add it to the helpers:

```
LOGI_LAUNCH_HELPERS='c:\sim-teleport.exe source' logi-launch %command%
```

Use `LOGI_LAUNCH_HELPERS` and not `LOGI_LAUNCH_EXE` here. The second one
replaces our relay, which leaves SimHub fed on the other machine and the rev
lights and simulated TrueForce dark on this one. Feeding SimHub and driving
the wheel are not alternatives, and both bridges can read the same telemetry
at once.

Its own documentation warns that the **target** half must never run on the
gaming PC, because it creates shared-memory sections using the game's own
names. Only the source half belongs here.

None of this touches force feedback or TrueForce. A telemetry helper only
reads; the native FFB and SDK TrueForce paths are unaffected.

## Seeing every game's recipe

```
logi-wheel --launch-plan --list
```

prints all 30 titles the registry knows, with the name to pass to `--game`,
the Steam appid, and what each one resolves to **on the wheel you have
attached**. The same command with an appid instead of `--list` shows one
game's answer, which is what `logi-launch` itself asks for.

Titles that do not run on Linux at all (the Forza games, Gran Turismo 7) say
so and get no recipe, rather than advice nobody can test.

## Naming a game it does not recognise

An appid identifies a Steam title, but not every install has one: a game
added as a non-Steam shortcut gets an id Steam makes up locally, a copy
bought elsewhere has none, and a delisted game may not resolve. Name it
instead:

```
logi-launch --game dirt-4 %command%
```

Names come from `--launch-plan --list`. A partial name works if it is
unambiguous; `--game dirt` refuses and lists the candidates rather than
choosing between DiRT 4 and DiRT Rally 2.0.

## Two wheels at once

`logi-launch` works out what to set from the wheel you have attached. With
two of them, and of different kinds, it stops guessing: the game picks which
wheel it uses in its own settings and never tells us, so there is nothing to
detect. Getting it wrong is not a small mistake either, because
`PROTON_ENABLE_HIDRAW` on a G923 costs that wheel its force feedback.

So on a mixed rig it applies what is safe, says what it withheld, and waits
to be told:

```
logi-launch --wheel dd %command%      # the RS50 or G PRO
logi-launch --wheel g923 %command%    # the G923
```

That choice also aims `logi-tf-sim` at the same wheel, so the game and the
haptics cannot end up on different ones. If the daemon was already running
from an earlier session it keeps whatever wheel it was started with, and
the log says so rather than letting the flag look honoured.

**Everything else this script starts is aimed there too.** One wheel is
resolved at the top of the run and handed to the rest: `logi-rpm-bridge`
gets that wheel's `wheel_texture_rpm` (and so its rev strip, the attribute
next door), `logi-ffb` gets that wheel's force-feedback event node through
`LOGI_FFB_DEVICE`, the texture merge is switched on at that wheel's
`wheel_tf_merge`, and the TrueForce teardown sent after the game exits goes
to that wheel's raw interface. The log names the directory it settled on:

```
[logi-launch] acting on the wheel at /sys/bus/hid/devices/0003:046D:C276.0003
[logi-launch] the wheel's force-feedback node is event21
```

Before this, the merge was switched on for **every** direct-drive wheel
attached and switched off again on every one at exit, and the teardown took
whichever raw node sorted first: quitting one game could reach into a
session running on the other wheel. The exit path now undoes exactly the
attribute this run wrote, and nothing else.

**One wheel, or several of the same kind, needs none of this.** There is
nothing ambiguous to resolve, and nothing to pass.

## Teaching it a new game

`logi-launch` knows 29 titles by Steam appid. For anything else, or to
override what it decides, write a line in
`~/.config/logi-wheel/games.conf`:

```
# appid    settings
3058630    hidraw=1 relay=ac-evo tfsim=1
1234567    ffb=proxy tfsim=0
```

The appid is the number in the game's Steam store URL, and the same number
as its folder under `steamapps/compatdata`. Your line wins for the keys it
states; a key it leaves out keeps the built-in answer's value, so a line
written for an older release does not quietly turn off pieces added since
(an old `hidraw=1` line, for example, keeps the kernel texture merge the
built-in plan asks for). To force something off, state it: `texture=none`,
`tfsim=0`.

| setting | values | meaning |
|---|---|---|
| `hidraw` | `0`, or a wheel id like `0x046D/0xC276` | what to set `PROTON_ENABLE_HIDRAW` to. `0` turns the raw interface off for this game, which is how you keep force feedback when the raw path spoils a game's input. A bare `1` still works and means every HID device, which is rarely what you want |
| `ffb` | `proxy` | launch through `logi-ffb`, for games that drive force feedback the DirectInput way |
| `relay` | `acc`, `ac-evo`, `assetto`, `iracing`, `raceroom`, `rf2`, `lmu`, `none` | which decoder the in-prefix telemetry relay should use |
| `tfsim` | `1`, `0` | run `logi-tf-sim`. Set `0` for a game whose own TrueForce already reaches your wheel. Asking for it alongside `texture=merge` is a combination only a hand-written line can produce, and it works: the two read the same telemetry port, so whichever holds it forwards to the other and both are fed (see "One socket gets the datagrams, every reader gets the telemetry" in [SHARED_MEMORY_RELAY.md](SHARED_MEMORY_RELAY.md)) |
| `texture` | `merge`, `none` | mix the driver's engine-note texture into the game's own TrueForce on the wheel. `merge` makes `logi-launch` stage the dinput8 escape proxy into the game's directory, start `logi-rpm-bridge` and switch `wheel_tf_merge` on, undoing all of it when the game exits. The same chain also lights the rev strip from the game's own telemetry: the proxy relays live rpm, first-shift-light rpm and redline, and `logi-rpm-bridge` drives `wheel_rev_level` with them (full bar by default, `LOGI_REV_MODE=shift` for the dash band). Only does anything for a direct-drive wheel in an SDK title with the TrueForce files installed |
| `revleds` | `bar`, `shift` | how `logi-rpm-bridge` maps the rev strip while `texture=merge` drives it: `bar` (the default) lights LED 1 as soon as the engine turns and all 10 at the limiter, `shift` is the dash band (dark below the car's first shift light, level 1 exactly there). The apps persist this choice in `~/.config/logi-wheel/launch.conf` and it shows on a merge title's Setup card; a games.conf line overrides it per game like any other key |

A line that works for you is also exactly the report needed to add the game
properly, so please open an issue with it.

## What it needs installed

`logi-launch` comes with the `logi-wheel` package, along with `logi-ffb`,
`logi-tf-sim` and the relay it installs into prefixes. If you built from
source, `sudo ./tools/setup.sh` installs all of them.
