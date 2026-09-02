# Where this driver is, and what it still cannot do

A companion to [FEATURE_MATRIX.md](FEATURE_MATRIX.md), which says what the
wheels report; this one says what has been done with that, and what has not.

The point of it is that the honest answer to "does this work?" is different
for each wheel and each claim, and a reader deserves to know which parts are
measured, which are argued, and which are hoped.

## Wheels

| wheel | force feedback | TrueForce | settings | rev lights |
|---|---|---|---|---|
| RS50 (`c276`, both editions) | yes, native path | yes, the game's own and simulated | full `wheel_*` surface | yes, and LIGHTSYNC colours |
| G PRO (`c272`/`c268`) | yes, same path | yes | full surface | level-based, see below |
| G923 PS (`c266`/`c267`) | yes, classic path | simulated only | **none** | yes, classic command |
| G923 Xbox (`c26e`) | yes, HID++ 0x8123 by default; this driver's own engine with `g923_xbox_dd_engine=1` | simulated; alongside force only on the driver's engine | **none** | registered, unconfirmed |

The Xbox editions of the RS50 (`c275`) and the G923 (`c26d`) boot speaking
the console's own protocol, with no HID++ interface to bind. Both are
switched to their PC id automatically on every plug-in, which needs
`usb_modeswitch` installed; `sudo logi-wheel-modeswitch` does it by hand.
After the switch each is the wheel in its row above.

The G923 Xbox edition is the one wheel here whose force this driver does
not compute. Its effects are downloaded into the wheel's own firmware over
HID++ 0x8123 and summed there, so nothing in the kernel knows the force
being produced, and a TrueForce stream, whose torque field takes the
motor, can only carry a zero. `logi-tf-sim` therefore refuses that wheel
rather than silencing it (#72), and its own `g923.stream_without_ffb_mirror`
streams anyway for anyone who wants the haptics and not the force.

The way to have both is `g923_xbox_dd_engine=1`, which moves that wheel
onto the engine the direct-drive wheels use, summing the effects here and
carrying force and texture in one packet. Off by default, and now measured
on the hardware by its owner (issue #72, 2026-09-02): force is linear in
the commanded level above a static-friction floor of about 3.6% of full
scale, and the engine's gain matches the firmware path to within the
measurement's own noise, so the scaling needed no change. Under it the
condition effects stay silent, because that edition's steering reports do
not reach the engine yet, and the rev lights stay on their own LED device,
which speaks the command that wheel obeys rather than the direct-drive one.

That wheel can also wedge: every HID++ command times out while init has
already reported success, and only a power cycle of the wheel recovers it,
not a module reload or a USB replug. The driver now notices a run of
unanswered commands and says so, once, in dmesg, since the natural response
to force quietly stopping is exactly the pair of things that do not work.

Whether force feedback comes up at all on that wheel depends on its HID++
answering while this driver is still in probe. A fix that keeps waiting
instead of giving up lives on `g923-xbox-ffb-retry`, unmerged for the same
reason (#52).

Two things about the RS50's light strip that are not faults. It only
displays on some onboard profiles: a profile can keep it dark, and writes
still succeed there. And the four built-in sweeps render a palette held in
the wheel's firmware, which no function in `0x807A`/`0x807B` reports and
which G HUB never reads either, so their colours cannot be previewed or
copied.

The G923 exposes no `wheel_*` attributes at all. That is not an oversight:
it takes the classic force-feedback path, which has no settings surface, no
feature discovery and no sysfs group to hang them on. Its response curves
would also need `0x80A3`, a page this driver does not implement, because the
direct-drive wheels use `0x80A4` instead.

## What is measured, what is argued

Everything here has been tested somehow. The distinction that matters is
how.

**Measured on hardware.** The 4 kHz stream rate on both transports (1000
packets/sec sustained, no drops, matching Logitech's stated 1 ms interval).
The direct-drive stability fix (steering-axis travel 1258-1703 degrees
before, 204-488 after). The rev-rate default (899 degrees of travel at 25
against 611 at 35). Which rev-light command each wheel obeys. Which HID++
features each wheel has.

The RS50's light strip, end to end: `wheel_rev_level` fills it
proportionally, and custom colours follow what is written across count,
position, global colour, half-and-half and per-LED alternation. The wire
traffic for the rev display matches a G HUB capture from the same model
byte for byte, including the transport (control SET_REPORT). The driver
sends the bare fn2+fn6 level pair and no setup packet, because the arm form
proves fatal to a live native TrueForce session (see
`PROTOCOL_SPECIFICATION.md` section 9).

The kernel effect tick, on an RS50 with a kprobe on the timer callback:
1000.2 Hz on bare metal, median period 1.000 ms, p99 1.003 ms, no tick
over 1.5 ms. Under a steering force and a TrueForce stream together, 990
packets/sec with every one accepted at its full 64 bytes and no submission
errors. That leaves no headroom: the wheel is full-speed USB with a
`bInterval=1` interrupt OUT endpoint, which is exactly 1000 packets per
second, so a feature wanting a second packet per tick has to displace one
rather than join it. This is why exactly one program streams to the wheel
at a time, and why the driver stands aside when userspace takes the stream
(`docs/TRUEFORCE_PROTOCOL.md`, "One writer at a time").

Telemetry latency through the simulated-TrueForce path, measured off the
wire after fifteen seconds of continuous streaming: a change in engine speed
starts coming out of the wheel 15 ms later and has fully taken over the note
by 90 ms.

The driver is validated on a KASAN, lockdep, RCU-proving and
timer-object-debugging kernel with the wheel attached: no splats across
probe, repeated bind and unbind, erase-while-playing, arm and disarm churn,
twelve concurrent effects, attribute reads racing an unbind, module unload
during an active stream, or hot-unplug during an active stream. Memory is
flat across all of it.

**Argued from the code, not exercised.** The rev-limiter dwell fix is
unit-tested against a simulated clock but its end-to-end timing has never
been trustworthily measured, because the bench instrument for it has six
stages between cause and observation. The `hrtimer_init` compatibility path
for kernels older than 6.15 compiles but has not run, because no such kernel
was available.

**Not confirmed at all.** Whether Assetto Corsa Competizione populates
`wheelLoad`, which decides whether the airborne flag ever fires. The
airborne haptic layer's gain, which has never been heard. `LOGI_TF_REARM`,
the experimental pre-launch re-arm for a wheel whose session latched, which
is off by default until it is validated on hardware.

## Known problems

**The force-feedback keepalive is best-effort.** If the wheel's evdev node
cannot be opened, a direct-drive wheel can still drive itself into its stops
while TrueForce streams. The self-test refuses to run in that state; the
daemon carries on.

**Low-frequency haptic layers move the wheel rather than buzzing it.** The
pit limiter is 10 Hz, ABS 15 Hz, the rev limiter 25 Hz, and excursion for a
given torque goes roughly as 1/f^2. Their gains were chosen as torque levels
while what is felt is excursion. Deliberately not "fixed" with a frequency
curve: see the reasoning in `effects.rs`, which is that the wheel is
normally held and a hand already damps exactly those frequencies.

**The KF/TF crossover admits texture the wheel can follow.** Anything at or
above 20 Hz routes to the texture channel, and 20-40 Hz is where these
wheels still track the waveform. Flagged rather than moved, because
changing it changes which effects route where.

**A session that dies without its teardown latches the wheel.** The next
TrueForce session opens fine and never streams. Power-cycling the base
clears it.

**Three features the wheels have and this driver does not use.**
`DUAL_CLUTCH`, `GAMING_ATTACHMENTS` and `AXIS_MAPPING`. (`DISPLAY_GAME_DATA`,
the base OLED, is driven through `wheel_oled` as of this release.)
Each needs a capture of G HUB exercising the control, and each would be a
non-force write to the HID++ endpoint, which specification 12.5 says cuts
live force.

## Waiting on other people

- **#27**, Xbox G923 rev lights. The driver reports which features that
  wheel has; no owner has run it. Even a positive answer is not sufficient,
  because that wheel's force rides HID++ and 12.5 applies.
- **#52**, Xbox G923 force feedback coming up reliably, on the branch
  named above.
- **#72**, whether that wheel can be driven by this driver's own engine,
  which is what would give it force feedback and TrueForce together.
- **#8**, a G PRO capture, which is what would let the real-G-PRO rev-light
  work start.
- **#20** and **#62**, an OLED descriptor readback (`0x8130` fn1) offered by
  a Windows implementation.

## Coverage numbers, for honesty about the compatibility table

Of the games listed: 6 verified end to end by this project, 18 documented
from a vendor or reliable source, 10 expected, 8 genuinely unknown.
Simulated TrueForce is live for 23 titles, impossible for 12 (no usable
telemetry), and possible-with-a-parser for 4.

So the table is mostly *not* first-party tested, and says so per row.
