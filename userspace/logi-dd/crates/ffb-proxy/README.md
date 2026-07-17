# ffb-proxy (`logi-ffb`)

A userspace DirectInput force-feedback proxy for the Logitech direct-drive
wheels (RS50, G PRO). It presents a virtual force-feedback wheel that mirrors
the real one and forwards force-feedback effects onto the real wheel's
existing kernel evdev FF interface, so DirectInput games that would otherwise
get no force feedback get the genuine thing.

This is a generic DirectInput force-feedback proxy (constant force, springs,
damper, friction, rumble, periodic waveforms, and so on), not the TrueForce
haptic texture system; TrueForce already works over its own path and is
unaffected by this tool.

## Why it exists

The real wheel's HID report descriptor has no PID (Physical Interface Device,
the USB force-feedback collection) in it. That is fine for the native path:
the `hid-logitech-dd` kernel driver builds a complete evdev force-feedback
interface for the wheel regardless, and games that go through Wine's SDL/evdev
backend (`PROTON_ENABLE_HIDRAW=0`, the default) get full force feedback
already.

The trouble is the other path. Some DirectInput games under Wine/Proton need
`PROTON_ENABLE_HIDRAW=1` (raw hidraw access) for reasons unrelated to force
feedback, for example to see the wheel's full button/axis set correctly. Wine
reads the HID descriptor directly in that mode, finds no PID collection on the
real wheel, and never sends any force-feedback effects. The game runs, but the
wheel never pushes back.

`logi-ffb` closes that gap. It creates a virtual `uhid` device with an
authored descriptor that does include a PID collection, mirrors the real
wheel's inputs onto it, and decodes whatever force-feedback effects the game
sends to the virtual device, then re-issues each one on the real wheel's
kernel evdev FF interface. Both the native path and the proxied hidraw path
end up driving the exact same kernel FF code on the exact same wheel, so
fidelity is identical either way: there is no effect type or nuance available
on one path and missing on the other.

## Requirements

- The `hid-logitech-dd` driver loaded, with the wheel plugged in and bound to
  it.
- Access to `/dev/uhid`. The packaged udev rule grants this to the `input`
  group; if you have not set that up, run `logi-ffb` as root instead.

## Build

```bash
cd userspace/logi-dd
cargo build --release -p ffb-proxy
```

The binary lands at `userspace/logi-dd/target/release/logi-ffb`. Copy it
somewhere on your `PATH` if you like, or run it in place.

## Usage

The common case is prepending `logi-ffb` to the command that launches your
game:

```bash
logi-ffb <game command>
```

`logi-ffb` brings up the virtual wheel, steers the game away from seeing the
real wheel a second time (so it does not enumerate two look-alike devices),
runs the game command to completion, and tears the virtual wheel down when the
game exits.

In Steam, put the whole thing in the title's launch options:

```
logi-ffb %command%
```

For testing or for a setup that is not a single game launch, run the proxy
standalone in the foreground:

```bash
logi-ffb --daemon
```

This brings the virtual wheel up and leaves it running until it receives
`SIGINT`/`SIGTERM` (Ctrl-C), instead of wrapping a game command.

`logi-ffb -h` / `logi-ffb --help` prints a short usage summary.
