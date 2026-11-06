# Simulated TrueForce in Euro Truck Simulator 2 and American Truck Simulator

These two games publish telemetry through their own plugin interface, not
over UDP. (Telemetry is the live data a game publishes about the vehicle. UDP
is a simple network protocol. The game sends packets to a port.) So
`logi-tf-sim`, the daemon that turns telemetry into haptics, cannot see them
the way it sees Automobilista 2 or DiRT Rally. `logi-tf-scs` is a small
plugin that runs inside the game. It forwards engine speed, throttle and gear
to the daemon. The daemon then drives the wheel exactly as it does for the
UDP titles.

It is a native Linux plugin. No Wine, no Proton, nothing to cross-compile.
(Proton is Steam's layer for running Windows games. Wine is the project it is
built on.)

## Install

**`sudo ./tools/setup.sh` installs it into both games for you.** It also
creates the `plugins` directory the games look for but do not ship. If you
have run that, skip to "Check it is working".

Otherwise, get the plugin file first. Pick one of these:

- If you installed from a package (Debian, Arch, Fedora, openSUSE), the
  master copy is at:

```
/usr/share/logitech-trueforce/liblogi_tf_scs.so
```

- Or download `liblogi_tf_scs-<version>.so` from the
  [latest release](https://github.com/mescon/logitech-trueforce-linux-driver/releases/latest)
  and rename it to `liblogi_tf_scs.so`.
- Or build it yourself:

```bash
cargo build --release -p logi-tf-scs
```

Then install it:

1. Create the game's `plugins` directory. The game only looks for it; it does
   not ship one.
2. Copy the file into that directory.
3. Turn the game on in the app's Setup page, under Simulated TrueForce. This
   is the same switch every other supported title uses. Euro Truck
   Simulator 2 and American Truck Simulator have separate switches and
   separate intensities.

Steps 1 and 2 for Euro Truck Simulator 2:

```bash
mkdir -p ~/.steam/steam/steamapps/common/"Euro Truck Simulator 2"/bin/linux_x64/plugins
cp userspace/logi-wheel/target/release/liblogi_tf_scs.so \
   ~/.steam/steam/steamapps/common/"Euro Truck Simulator 2"/bin/linux_x64/plugins/
```

Steps 1 and 2 for American Truck Simulator:

```bash
mkdir -p ~/.steam/steam/steamapps/common/"American Truck Simulator"/bin/linux_x64/plugins
cp userspace/logi-wheel/target/release/liblogi_tf_scs.so \
   ~/.steam/steam/steamapps/common/"American Truck Simulator"/bin/linux_x64/plugins/
```

If your Steam library lives on another drive, `./tools/setup.sh doctor`
prints the library roots it found.

## Check it is working

The game logs its plugins at startup.

1. Load a session in the game.
2. Search the log:

```bash
grep -i "logi-tf-scs" ~/.local/share/"Euro Truck Simulator 2"/game.log.txt
```

A line that mentions `logi-tf-scs` means the game loaded it.

3. If the wheel is still quiet after that, check the daemon:

```bash
logi-tf-sim --status
```

## What it sends, and what it does not

It sends engine rpm, the engine's redline, throttle position and selected
gear. That is what the engine-note synthesizer needs, and nothing more.

It does not send position, speed, damage, cargo, navigation or anything else
the SCS telemetry interface exposes. It does not read anything back from the
game. It opens one UDP socket to `127.0.0.1` and writes to it.

If `logi-tf-sim` is not running, the packets go nowhere, and the game is
unaffected. Every callback is wrapped, so a fault in the plugin cannot take
the game down with it.

## Port

The daemon listens on UDP 20780 by default. If you changed `port.relay` in
`tf-sim.conf`, tell the plugin the same number:

```bash
LOGI_TF_SIM_RELAY_PORT=20999 steam steam://rungameid/227300
```

## Status

The plugin is written against the official SCS SDK 1.14 headers. Tests pin
every struct layout. But it has **not yet been confirmed in a running
game**. That is why the compatibility table marks these two titles
provisional. If you run either of them, please leave a note on the issue
tracker. Say whether the engine note appeared. That is what moves them to
verified.
