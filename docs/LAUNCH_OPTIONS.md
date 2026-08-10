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

It knows 28 titles by their Steam appid. A game it does not know still gets
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

**2. It sets `PROTON_ENABLE_HIDRAW` only if that wheel wants it here.**
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
[logi-launch] plan: wheel=direct-drive game=Assetto Corsa EVO (early access) hidraw=1 ffb=native relay=none tfsim=0
[logi-launch] set PROTON_ENABLE_HIDRAW=1
[logi-launch] no in-prefix helper needed for this game
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
| `PROTON_ENABLE_HIDRAW=1 %command%` | Lets the game talk to the wheel's raw HID interface, which is how Logitech's SDK delivers TrueForce | Set by `logi-launch`. By hand: a **direct-drive wheel** (RS50, G PRO) in a game with its own TrueForce (ACC, Assetto Corsa EVO), with the TrueForce files installed |
| *(leave it out)* | The wheel stays an ordinary Linux force-feedback device | By hand: a **G923**, always |
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
`PROTON_ENABLE_HIDRAW=1` to it: `logi-launch` sets that itself on the wheels
that want it, and setting it yourself is how it ends up on a G923, where it
costs that wheel its force feedback.

Le Mans Ultimate, on any wheel:

```
logi-launch %command%
```

`logi-launch` starts `logi-ffb` for you there, because that title uses
DirectInput force feedback. You do not type `logi-ffb` yourself.

### The same two by hand

If you would rather not use the wrapper:

```
PROTON_ENABLE_NVAPI=1 VKD3D_CONFIG=descriptor_heap PROTON_ENABLE_HIDRAW=1 gamemoderun %command%   # AC EVO, direct-drive wheel
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

**One wheel, or several of the same kind, needs none of this.** There is
nothing ambiguous to resolve, and nothing to pass.

## Teaching it a new game

`logi-launch` knows 28 titles by Steam appid. For anything else, or to
override what it decides, write a line in
`~/.config/logi-wheel/games.conf`:

```
# appid    settings
3058630    hidraw=1 relay=ac-evo tfsim=1
1234567    ffb=proxy tfsim=0
```

The appid is the number in the game's Steam store URL, and the same number
as its folder under `steamapps/compatdata`. Your line wins over the built-in
answer.

| setting | values | meaning |
|---|---|---|
| `hidraw` | `1`, `0` | set `PROTON_ENABLE_HIDRAW`. Only for a wheel that can take it: on a G923 this costs you force feedback |
| `ffb` | `proxy` | launch through `logi-ffb`, for games that drive force feedback the DirectInput way |
| `relay` | `acc`, `ac-evo`, `assetto`, `iracing`, `raceroom`, `rf2`, `lmu`, `none` | which decoder the in-prefix telemetry relay should use |
| `tfsim` | `1`, `0` | run `logi-tf-sim`. Set `0` for a game whose own TrueForce already reaches your wheel |

A line that works for you is also exactly the report needed to add the game
properly, so please open an issue with it.

## What it needs installed

`logi-launch` comes with the `logi-wheel` package, along with `logi-ffb`,
`logi-tf-sim` and the relay it installs into prefixes. If you built from
source, `sudo ./tools/setup.sh` installs all of them.
