# Game setup, per game and per wheel

**Generated file. Do not edit.** The compatibility registry in
`userspace/logi-wheel/crates/logi-wheel-core/src/games.rs` is the
source. `tests/game_setup_doc.rs` renders this file from it. The test
fails if this file drifts from the registry. The settings app checks
your own installed games against that same registry. So what you
read here is what the app will tell you.

**Most people need none of this.** Put `logi-launch %command%` in a
game's Steam launch options. Launch options are the command Steam
runs to start a game. `logi-launch` works out the whole recipe for
the game and the attached wheel, and applies it. It sets the raw-HID
setting only where that wheel wants it. (Raw HID gives a game direct
access to the wheel's USB reports.) It starts the logi-ffb proxy for
the DirectInput games. (DirectInput is the older Windows
force-feedback interface. The proxy is a virtual wheel that forwards
force to the real one.) It starts the telemetry daemon. (Telemetry
is the live data a game publishes about the car: rpm, gear, speed.)
It puts the relay inside the game's prefix for the sims that need
one. (The prefix is the private Windows folder that Proton, Steam's
layer for running Windows games, makes for each game.) A game with
its own TrueForce keeps it. Nothing is layered on top. See
LAUNCH_OPTIONS.md.

**This table is for doing it by hand**, or for finding out what went
wrong. Read on to see why the answer differs per wheel.

What a game needs depends on the wheel as well as the game. The
direct-drive wheels answer Logitech's TrueForce SDK. (The SDK is
Logitech's library that a game loads to send TrueForce haptics.) A
sim with built-in TrueForce reaches them through the staged SDK
DLLs. `PROTON_ENABLE_HIDRAW` is what lets it, and `logi-launch` sets it
scoped to the wheel, never the bare `1`.

The G923 does not answer that SDK. Setting the variable there does
not add TrueForce. It diverts the game to raw HID reports. The wheel
cannot drive force feedback through those. So it costs you the force
feedback you already had. Leave it unset.

That wheel can still have haptics in those games, by a different
route. `logi-tf-sim` synthesizes an engine note from the game's own
telemetry. A small relay reads that telemetry out of the game's
shared memory (`docs/SHARED_MEMORY_RELAY.md`). This is confirmed
working on a G923 in Assetto Corsa Competizione and EVO.

The shim's `--proxy` option is not a second route in these games.
(The shim is the set of Logitech SDK files staged into a game's
prefix.) Both games check the library's signature before they load
it. They refuse the unsigned proxy, so it carries nothing. On a wheel
that uses Logitech's library, it also costs the game that library's
force feedback. `./tools/setup.sh doctor` flags a prefix that has
it.

To set launch options:

1. In Steam, open the game's Properties.
2. Paste the line into Launch Options, exactly, `%command%`
included.

`%command%` is the placeholder Steam replaces with the game itself.
Without it, the line replaces the game instead of wrapping it.

## Recipes

| Game | Runs on Linux | Force feedback | On RS50 / G PRO | On G923 |
|---|---|---|---|---|
| American Truck Simulator * | Native Linux | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Assetto Corsa (original) | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| Assetto Corsa Competizione | Proton | TrueForce shim | Install the shim<br>`logi-launch %command%` | Turn on simulated TrueForce<br>and leave `PROTON_ENABLE_HIDRAW` unset |
| Assetto Corsa EVO (early access) | Proton | TrueForce shim | Install the shim<br>`logi-launch %command%` | Turn on simulated TrueForce<br>and leave `PROTON_ENABLE_HIDRAW` unset |
| Assetto Corsa Rally (early access) * | Proton | TrueForce shim | Install the shim<br>`logi-launch %command%` | Plain force feedback<br>leave `PROTON_ENABLE_HIDRAW` unset |
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
| iRacing * | Proton | logi-ffb | DirectInput force feedback<br>`logi-launch %command%` | DirectInput force feedback<br>`logi-launch %command%` |
| KartKraft * | Proton | Native FFB | Nothing to do | Nothing to do |
| Le Mans Ultimate | Proton | logi-ffb | DirectInput force feedback<br>`logi-launch %command%` | DirectInput force feedback<br>`logi-launch %command%` |
| Need for Speed: Shift | Proton | Native FFB | Nothing to do | Nothing to do |
| Project CARS 2 | Proton | Native FFB | Turn on simulated TrueForce | Turn on simulated TrueForce |
| RaceRoom Racing Experience | Proton | logi-ffb | DirectInput force feedback<br>`logi-launch %command%` | DirectInput force feedback<br>`logi-launch %command%` |
| Rennsport * | Proton | Native FFB | Nothing to do | Nothing to do |
| rFactor 2 | Proton | logi-ffb | DirectInput force feedback<br>`logi-launch %command%` | DirectInput force feedback<br>`logi-launch %command%` |
| Richard Burns Rally * | Proton | Native FFB | Nothing to do | Nothing to do |
| TOCA Race Driver 3 | Proton | Native FFB | Nothing to do | Nothing to do |
| Wreckfest | Proton | Native FFB | Nothing to do | Nothing to do |

Rows marked `*` are not confirmed on this driver yet. They are
expected or documented, not tested end to end.

## Simulated TrueForce

Games with no TrueForce of their own can still have engine haptics
and rev lights. `logi-tf-sim` synthesizes them from whatever
telemetry the game publishes. This works on every supported wheel,
including the G923. It is ordinary force feedback driven from
telemetry, not the SDK.

How the telemetry reaches the daemon depends on the game. Most games
broadcast it over UDP. (UDP is a simple network protocol. The game
sends packets to a port.) Those games only need that switched on in
their own settings. Euro Truck Simulator 2 and American Truck
Simulator use a plugin instead (`docs/SCS_PLUGIN.md`). iRacing
publishes to shared memory, and a small in-prefix relay forwards it
(`docs/SHARED_MEMORY_RELAY.md`). In every case, enable the game in
the app's Setup page afterwards.

| Game | Simulated TrueForce |
|---|---|
| American Truck Simulator | supported today |
| Assetto Corsa (original) | supported today |
| Assetto Corsa Competizione | use the game's own TrueForce; on a direct-drive wheel the helper adds no haptics of its own, and where it runs beside the game it drives only the rev lights and the screen; simulated haptics are the fallback for a wheel that cannot receive the real thing |
| Assetto Corsa EVO (early access) | use the game's own TrueForce; on a direct-drive wheel the helper adds no haptics of its own, and where it runs beside the game it drives only the rev lights and the screen; simulated haptics are the fallback for a wheel that cannot receive the real thing |
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

- **Install the shim.** Stage Logitech's signed SDK DLLs into the game's Proton prefix, from the app's Setup page or `tools/install-tf-shim.sh`. Install the TrueForce shim once, from the app's Setup page. Set the launch options to `logi-launch %command%`. logi-launch turns raw HID on. It stages the proxy that answers the SDK's rotation question. Without that proxy, the stock library stops steering and force on track. It also sets up everything else this game needs. Turn Steam Input off. There is no engine-note texture merge here. ACC produces its own TrueForce audio, so the merge is not wired for it.
- **On a wheel with no SDK TrueForce.** Leave PROTON_ENABLE_HIDRAW unset. On this wheel it costs you force feedback. For haptics, turn this game on under Simulated TrueForce. logi-launch puts the telemetry relay in the game's prefix. It also starts the daemon for you. That route is confirmed working on a G923. Turn Steam Input off.
- **Launch via logi-ffb.** Set the launch options to `logi-launch %command%`. logi-launch keeps raw HID off. It runs the logi-ffb helper. It puts the telemetry relay in the game's prefix for you. Turn Steam Input off. Simulated TrueForce also needs the community rF2SharedMemoryMapPlugin. Install that plugin in the game.
- **Nothing to do.** The wheel is an ordinary Linux force feedback device. The game drives it directly.

## Confidence

- **verified** (4 titles): confirmed end to end by this project
- **documented** (16 titles): documented by the vendor or a reliable community source
- **expected** (7 titles): expected to work, not confirmed
- **unknown** (3 titles): genuinely unknown
