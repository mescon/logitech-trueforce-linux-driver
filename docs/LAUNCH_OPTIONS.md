# Steam launch options, and what each one is for

Everything this project asks you to put in a game's launch options, in one
place. Right-click the game in Steam, **Properties**, **Launch Options**.

`%command%` is the placeholder Steam replaces with the real command that
starts the game. Type it exactly, including the percent signs. Anything to
the left of it runs first; the game itself is what `%command%` becomes.

## The options

| Option | What it does | When you want it |
|---|---|---|
| `PROTON_ENABLE_HIDRAW=1 %command%` | Lets the game talk to the wheel's raw HID interface, which is how Logitech's SDK delivers TrueForce | A **direct-drive wheel** (RS50, G PRO) in a game with its own TrueForce: ACC, Assetto Corsa EVO. Needs the shim installed |
| *(leave it out)* | The wheel stays an ordinary Linux force-feedback device | A **G923**, always. Also DirectInput games, unless you are using `logi-ffb` |
| `logi-ffb %command%` | Presents a virtual wheel that speaks the older DirectInput force-feedback protocol, and forwards it to your real wheel | Games that only do DirectInput FFB: Le Mans Ultimate, rFactor 2, iRacing, RaceRoom |
| `logi-launch %command%` | Starts a Windows helper **inside the game's Proton prefix**, after the game is up | Anything that has to read the game's shared memory from inside the prefix: `logi-tf-relay`, or a telemetry bridge to another PC |

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

### Choosing the helper

| Variable | Default | Meaning |
|---|---|---|
| `LOGI_LAUNCH_EXE` | `c:\sim-teleport.exe` | The helper, as a Windows path inside the prefix |
| `LOGI_LAUNCH_ARGS` | `source` | Arguments passed to it |
| `LOGI_LAUNCH_WAIT` | `120` | Seconds to wait for the game's wineserver |
| `LOGI_LAUNCH_SETTLE` | `15` | Seconds to let the game create its sections first |
| `LOGI_LAUNCH_LOG` | `/tmp/logi-launch.log` | Where it writes what it did |

To drive simulated TrueForce from a shared-memory sim, point it at this
project's own relay instead:

```
LOGI_LAUNCH_EXE='c:\logi-tf-relay.exe' LOGI_LAUNCH_ARGS='--game ac-evo' logi-launch %command%
```

Use `--game acc`, `assetto`, `iracing`, `raceroom`, `rf2` or `lmu` to match
the title. See [SHARED_MEMORY_RELAY.md](SHARED_MEMORY_RELAY.md).

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
then use the default `logi-launch` settings above.

None of this touches force feedback or TrueForce. A telemetry helper only
reads; the native FFB and SDK TrueForce paths are unaffected.
