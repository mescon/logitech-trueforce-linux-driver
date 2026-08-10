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

## The options

| Option | What it does | When you want it |
|---|---|---|
| `PROTON_ENABLE_HIDRAW=1 %command%` | Lets the game talk to the wheel's raw HID interface, which is how Logitech's SDK delivers TrueForce | A **direct-drive wheel** (RS50, G PRO) in a game with its own TrueForce: ACC, Assetto Corsa EVO. Needs the shim installed |
| *(leave it out)* | The wheel stays an ordinary Linux force-feedback device | A **G923**, always. Also DirectInput games, unless you are using `logi-ffb` |
| `logi-ffb %command%` | Presents a virtual wheel that speaks the older DirectInput force-feedback protocol, and forwards it to your real wheel | Games that only do DirectInput FFB: Le Mans Ultimate, rFactor 2, iRacing, RaceRoom |
| `logi-launch %command%` | Works out and applies everything below for this game and this wheel | Every racing game. It is the only line most people need |

`gamemoderun` is not ours. It is from `gamemode` and composes fine with all
of the above.

## Combining them

Order is: **environment variables first, then wrappers, then `%command%`.**
Each wrapper runs the next one along, so they chain left to right.

A direct-drive wheel in Assetto Corsa EVO, with gamemode and a telemetry
helper:

```
PROTON_ENABLE_NVAPI=1 PROTON_ENABLE_HIDRAW=1 gamemoderun logi-launch %command%
```

A G923 in Le Mans Ultimate:

```
logi-ffb %command%
```

Note there is no `PROTON_ENABLE_HIDRAW=1` there. On a G923 it costs you
force feedback, and `logi-ffb` is the route to FFB in a DirectInput game.

## Why `logi-launch` exists

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
| `LOGI_LAUNCH_EXE` | `c:\logi-tf-relay.exe` | The helper, as a Windows path inside the prefix |
| `LOGI_LAUNCH_ARGS` | `--game <from appid>` | Arguments passed to it |
| `LOGI_LAUNCH_WAIT` | `120` | Seconds to wait for the game's wineserver |
| `LOGI_LAUNCH_SETTLE` | `15` | Seconds to let the game create its sections first |
| `LOGI_LAUNCH_LOG` | `/tmp/logi-launch.log` | Where it writes what it did |

For example, to run a bridge that forwards telemetry to another machine
instead of driving simulated TrueForce here:

```
LOGI_LAUNCH_EXE='c:\sim-teleport.exe' LOGI_LAUNCH_ARGS=source logi-launch %command%
```

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
`Local\acevo_pmf_physics`. Put `sim-teleport.exe` in the prefix's `drive_c`
and set `LOGI_LAUNCH_EXE` as above.

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
