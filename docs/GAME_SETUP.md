# Game setup, per game and per wheel

**Generated file. Do not edit.** It is rendered from the
compatibility registry in
`userspace/logi-wheel/crates/logi-wheel-core/src/games.rs` by
`tests/game_setup_doc.rs`, which fails if this file drifts from it.
The settings app resolves your own installed games against that same
registry, so what you read here is what the app will tell you.

**Most people need none of this.** `logi-launch %command%` in a
game's Steam launch options works the whole recipe out for the game
being launched and the wheel attached, and applies it: the raw-HID
setting only where that wheel wants it, the logi-ffb proxy for the
DirectInput games, the telemetry daemon, and the relay inside the
game's prefix for the sims that need one. A game with its own
TrueForce keeps it, and nothing is layered on top. See
LAUNCH_OPTIONS.md.

**This table is for doing it by hand**, or for working out what went
wrong. Read on for why the answer differs per wheel.

What a game needs depends on the wheel as well as the game. The
direct-drive wheels answer Logitech's TrueForce SDK, so a sim with
built-in TrueForce reaches them through the staged SDK DLLs, and
`PROTON_ENABLE_HIDRAW=1` is what lets it.

The G923 does not answer that SDK. Setting the variable there does
not add TrueForce, it diverts the game to raw HID reports the wheel
cannot drive force feedback through, so it costs you the force
feedback you already had. Leave it unset.

That wheel is still capable of haptics in those games, by a
different route: `logi-tf-sim` synthesizes an engine note from the
game's own telemetry, read out of its shared memory by a small relay
(`docs/SHARED_MEMORY_RELAY.md`). Confirmed working on a G923 in
Assetto Corsa Competizione and EVO.

There is a second route that carries the real thing: installing the
shim with `--proxy` puts this project's own SDK proxy in the game's
path, where it copies the TrueForce the game is already producing
and streams it to the wheel. It is the same proxy that ships the
packaged fix for the 90-degree rotation clamp, so it loads and
works; the apps do not install it yet, so
`tools/install-tf-shim.sh --proxy` is how to turn it on.

Launch options go in Steam under the game's Properties. Paste them
exactly, `%command%` included: it is the placeholder Steam replaces
with the game itself, so without it the line replaces the game
instead of wrapping it.

## Recipes

| Game | Runs on Linux | Force feedback | On RS50 / G PRO | On G923 |
|---|---|---|---|---|
| American Truck Simulator * | Native Linux | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Assetto Corsa (original) | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Assetto Corsa Competizione | Proton | TrueForce shim | Install the shim<br>`PROTON_ENABLE_HIDRAW=1 %command%` | Turn on simulated TrueForce<br>and leave `PROTON_ENABLE_HIDRAW` unset |
| Assetto Corsa EVO (early access) | Proton | TrueForce shim | Install the shim<br>`PROTON_ENABLE_HIDRAW=1 %command%` | Turn on simulated TrueForce<br>and leave `PROTON_ENABLE_HIDRAW` unset |
| Assetto Corsa Rally (early access) * | Proton | Native FFB | Nothing to do | Nothing to do |
| Automobilista 2 | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| BeamNG.drive * | Native Linux | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| CarX Drift Racing Online | Proton | Native FFB | Nothing to do | Nothing to do |
| Dakar Desert Rally * | Proton | Native FFB | Nothing to do | Nothing to do |
| DiRT 4 | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| DiRT Rally 2.0 | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| EA Sports F1 (F1 22-25) * | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| EA Sports WRC | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Euro Truck Simulator 2 * | Native Linux | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Forza Horizon 5 | Proton | Native FFB | Nothing to do | Nothing to do |
| Forza Motorsport (2023) | Not on Linux | Not on Linux | - | - |
| Gran Turismo 7 | Not on Linux | Not on Linux | - | - |
| GRID (2019) | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| GRID Legends | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| iRacing * | Proton | logi-ffb | Launch via logi-ffb<br>`logi-ffb %command%` | Launch via logi-ffb<br>`logi-ffb %command%` |
| KartKraft * | Proton | Native FFB | Nothing to do | Nothing to do |
| Le Mans Ultimate | Proton | logi-ffb | Launch via logi-ffb<br>`logi-ffb %command%` | Launch via logi-ffb<br>`logi-ffb %command%` |
| Need for Speed: Shift | Proton | Native FFB | Nothing to do | Nothing to do |
| Project CARS 2 | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| RaceRoom Racing Experience * | Proton | logi-ffb | Launch via logi-ffb<br>`logi-ffb %command%` | Launch via logi-ffb<br>`logi-ffb %command%` |
| Rennsport * | Proton | Native FFB | Nothing to do | Nothing to do |
| rFactor 2 | Proton | logi-ffb | Launch via logi-ffb<br>`logi-ffb %command%` | Launch via logi-ffb<br>`logi-ffb %command%` |
| Richard Burns Rally * | Proton | Native FFB | Nothing to do | Nothing to do |
| TOCA Race Driver 3 | Proton | Native FFB | Nothing to do | Nothing to do |
| Wreckfest | Proton | Native FFB | Nothing to do | Nothing to do |

Rows marked `*` are not confirmed on this driver yet: expected or
documented rather than tested end to end.

## Simulated TrueForce

Games with no TrueForce of their own can still have engine haptics
and rev lights, synthesized by `logi-tf-sim` from whatever telemetry
the game publishes. This works on every supported wheel, including
the G923: it is ordinary force feedback driven from telemetry, not
the SDK.

How the telemetry reaches the daemon depends on the game. Most
broadcast it over UDP and only need that switched on in their own
settings. Euro Truck Simulator 2 and American Truck Simulator use a
plugin instead (`docs/SCS_PLUGIN.md`), and iRacing publishes to
shared memory that a small in-prefix relay forwards
(`docs/SHARED_MEMORY_RELAY.md`). Either way, enable the game in the
app's Setup page afterwards.

| Game | Simulated TrueForce |
|---|---|
| American Truck Simulator | supported today |
| Assetto Corsa (original) | supported today |
| Assetto Corsa Competizione | the game's own TrueForce is the route to use; on a direct-drive wheel the helper never adds haptics of its own, and where it runs beside the game it drives the rev lights and the screen only; simulated haptics are the fallback for a wheel that cannot receive the real thing |
| Assetto Corsa EVO (early access) | the game's own TrueForce is the route to use; on a direct-drive wheel the helper never adds haptics of its own, and where it runs beside the game it drives the rev lights and the screen only; simulated haptics are the fallback for a wheel that cannot receive the real thing |
| Assetto Corsa Rally (early access) | no usable telemetry |
| Automobilista 2 | supported today |
| BeamNG.drive | supported today |
| CarX Drift Racing Online | no usable telemetry |
| Dakar Desert Rally | no usable telemetry |
| DiRT 4 | supported today |
| DiRT Rally 2.0 | supported today |
| EA Sports F1 (F1 22-25) | supported today |
| EA Sports WRC | supported today |
| Euro Truck Simulator 2 | supported today |
| Forza Horizon 5 | no usable telemetry |
| GRID (2019) | supported today |
| GRID Legends | supported today |
| iRacing | supported today |
| KartKraft | possible, needs a telemetry parser first |
| Le Mans Ultimate | supported today |
| Need for Speed: Shift | no usable telemetry |
| Project CARS 2 | supported today |
| RaceRoom Racing Experience | supported today |
| Rennsport | no usable telemetry |
| rFactor 2 | supported today |
| Richard Burns Rally | possible, needs a telemetry parser first |
| TOCA Race Driver 3 | no usable telemetry |
| Wreckfest | no usable telemetry |

## What each recipe means

- **Install the shim.** Stage Logitech's signed SDK DLLs into the game's Proton prefix, from the app's Setup page or `tools/install-tf-shim.sh`. Install the TrueForce shim once, from the app's Setup page; launch options `logi-launch %command%` (it turns raw HID on, stages the proxy that answers the SDK's rotation question, without which the stock library stops steering and force on track, and everything else this game needs); turn Steam Input off. No engine-note texture merge here: ACC produces its own TrueForce audio, so the merge is not wired for it.
- **On a wheel with no SDK TrueForce.** Leave PROTON_ENABLE_HIDRAW unset: on this wheel it costs you force feedback. For haptics, turn this game on under Simulated TrueForce; logi-launch puts the telemetry relay in the game's prefix and starts the daemon for you, and that route is confirmed working on a G923. Installing the shim WITH --proxy carries the game's own TrueForce to the wheel instead, the same proxy that ships the 90-degree rotation fix; the apps install the plain shim, so use tools/install-tf-shim.sh --proxy. Steam Input off.
- **Launch via logi-ffb.** Launch options `logi-launch %command%` (it keeps raw HID off, runs the logi-ffb helper and puts the telemetry relay in the game's prefix for you); Steam Input off. Simulated TrueForce also needs the community rF2SharedMemoryMapPlugin installed in the game.
- **Nothing to do.** The wheel is an ordinary Linux force feedback device and the game drives it directly.

## Confidence

- **verified** (3 titles): confirmed end to end by this project
- **documented** (16 titles): documented by the vendor or a reliable community source
- **expected** (7 titles): expected to work, not confirmed
- **unknown** (4 titles): genuinely unknown
