# Where this driver is, and what it still cannot do

This page is a companion to [FEATURE_MATRIX.md](FEATURE_MATRIX.md). That
page says what the wheels report. This one says what has been done with
that, and what has not.

The honest answer to "does this work?" differs for each wheel and each
claim. A reader deserves to know which parts are measured, which are
argued, and which are hoped.

## Wheels

| wheel | force feedback | TrueForce | settings | rev lights |
|---|---|---|---|---|
| RS50 (`c276`, both editions) | yes, native path | yes, the game's own and simulated | full `wheel_*` surface | yes, and LIGHTSYNC colours |
| G PRO (`c272`/`c268`) | yes, same path | yes | full surface | level-based, see below |
| G923 PS (`c266`/`c267`) | yes, classic path | simulated only | **none** | yes, classic command |
| G923 Xbox (`c26e`) | yes, HID++ 0x8123 by default; this driver's own engine with `g923_xbox_dd_engine=1` | the game's own with the SDK files present, else simulated; both with force | **none** | yes, follows revs on the engine (#72) |

The Xbox editions of the RS50 (`c275`) and the G923 (`c26d`) start up
speaking the console's own protocol. In that mode they have no HID++
interface to bind. HID++ is Logitech's configuration protocol over USB.
The driver switches both wheels to their PC id on every plug-in. This
needs `usb_modeswitch` installed. `sudo logi-wheel-modeswitch` does the
switch by hand. After the switch, each wheel is the wheel in its row above.

The G923 Xbox edition is the one wheel here whose force this driver does
not compute. The driver downloads its effects into the wheel's own
firmware over HID++ 0x8123. The firmware sums them there. So nothing in
the kernel knows the force the wheel produces. TrueForce is Logitech's
high-rate haptic stream, and its torque field takes the motor. On this
wheel that field can only carry a zero. `logi-tf-sim` therefore refuses
that wheel instead of silencing it (#72). Its own
`g923.stream_without_ffb_mirror` streams anyway, for anyone who wants the
haptics and not the force.

`g923_xbox_dd_engine=1` is the way to have both. It moves that wheel onto
the engine the direct-drive wheels use. That engine sums the effects here
and carries force and texture in one packet. It is off by default. Its
owner has now measured it on the hardware (issue #72, 2026-09-02). Force
is linear in the commanded level above a static-friction floor of about
3.6% of full scale. The engine's gain matches the firmware path to within
the measurement's own noise. So the scaling needed no change. As of this
release, the condition effects under it get the steering position. The
driver finds the axis by usage in the report descriptor, not at the RS50's
offset. All four conditions were measured alive on a real unit in
September 2026 (#72). Spring returns to centre. Damper, friction and
inertia each slow a fixed push by a distinct, repeatable factor. The rev
lights stay on their own LED device. That device speaks the command this
wheel obeys, not the direct-drive one. Two owners found that this engine
with raw HID off gives force and simulated TrueForce together on that
wheel (#72, #84). 0.40.1 taught the engine the wheel's real range and
strength, its own strength scaling, the firmware centring spring, and the
2 ms position reports. Its velocity estimate had aliased against those
reports (#82, #85, #86).

That wheel can also wedge after a stalled motor. HID++ commands then fail,
as a mix of timeouts and fast submit errors. Init has already reported
success, and steering still works. Reloading the driver or replugging the
wheel recovers it. Its owner measured this against an eight-minute control
(#72). An earlier claim that only a power cycle recovered it was
withdrawn. The driver notices a run of transport failures. It says so once
in dmesg (the kernel log), with that recovery. It declares recovery only
after a run of answers. A wedged wheel can also answer with HID++ errors
instead of silence. #72 traced this: every effect download was refused as
an invalid argument. Since 0.40.1 those errors no longer count as a
healthy answer.

Whether force feedback comes up at all on that wheel depends on one thing.
Its HID++ must answer while this driver is still in probe (the first
set-up of the device). A fix that keeps waiting instead of giving up lives
on `g923-xbox-ffb-retry`. It is unmerged for the same reason (#52).

Two things about the RS50's light strip are not faults. First, it only
displays on some onboard profiles. A profile can keep it dark, and writes
still succeed there. Second, the four built-in sweeps render a palette
held in the wheel's firmware. No function in `0x807A`/`0x807B` reports
that palette. G HUB (Logitech's Windows software) never reads it either.
So their colours cannot be previewed or copied.

The G923 exposes no `wheel_*` attributes at all. That is not an oversight.
It takes the classic force-feedback path. That path has no settings
surface, no feature discovery and no sysfs group to hang them on. sysfs is
the kernel's settings file tree under `/sys`. Its response curves would
also need `0x80A3`, a page this driver does not implement. The
direct-drive wheels use `0x80A4` instead.

## What is measured, what is argued

Everything here has been tested somehow. The distinction that matters is
how.

**Measured on hardware.** The 4 kHz sample rate on both transports (1000
packets per second with four samples each, sustained, no drops, matching
Logitech's stated 1 ms interval).
The direct-drive stability fix (steering-axis travel 1258 to 1703 degrees
before, 204 to 488 after). The rev-rate default (899 degrees of travel at
25 against 611 at 35). Which rev-light command each wheel obeys. Which
HID++ features each wheel has.

The RS50's light strip, end to end. `wheel_rev_level` fills it in
proportion. Custom colours follow what is written, across count, position,
global colour, half-and-half and per-LED alternation. The wire traffic for
the rev display matches a G HUB capture from the same model byte for byte,
including the transport (control SET_REPORT). The driver sends the bare
fn2+fn6 level pair and no setup packet. The arm form proves fatal to a
live native TrueForce session (see `PROTOCOL_SPECIFICATION.md` section 9).

The kernel effect tick, on an RS50 with a kprobe (a kernel trace point) on
the timer callback: 1000.2 Hz on bare metal, median period 1.000 ms, p99
1.003 ms, no tick over 1.5 ms. Under a steering force and a TrueForce
stream together: 990 packets/sec, every one accepted at its full 64 bytes,
no submission errors. That leaves no headroom. The wheel is full-speed USB
with a `bInterval=1` interrupt OUT endpoint. That is exactly 1000 packets
per second. So a feature that wants a second packet per tick has to
displace one, not join it. This is why exactly one program streams to the
wheel at a time. It is also why the driver stands aside when userspace
takes the stream (`docs/TRUEFORCE_PROTOCOL.md`, "One writer at a time").

Telemetry latency through the simulated-TrueForce path, measured off the
wire after fifteen seconds of continuous streaming: a change in engine
speed starts to come out of the wheel 15 ms later. It has fully taken over
the note by 90 ms.

The driver is validated on a debug kernel with the wheel attached. That
kernel runs KASAN (a memory-error detector), lockdep (a lock-order
checker), RCU proving and timer-object debugging. No splats (kernel error
reports) appeared across probe, repeated bind and unbind,
erase-while-playing, arm and disarm churn, twelve concurrent effects,
attribute reads racing an unbind, module unload during an active stream,
or hot-unplug during an active stream. Memory is flat across all of it.

**Argued from the code, not exercised.** The rev-limiter dwell fix is
unit-tested against a simulated clock. Its end-to-end timing has never
been measured in a trustworthy way. The bench instrument for it has six
stages between cause and observation. The `hrtimer_init` compatibility
path for kernels older than 6.15 compiles but has not run. No such kernel
was available.

**Not confirmed at all.** Whether Assetto Corsa Competizione populates
`wheelLoad`. That decides whether the airborne flag ever fires. The
airborne haptic layer's gain, which has never been heard. `LOGI_TF_REARM`,
the experimental pre-launch re-arm for a wheel whose session latched. It
is off by default until it is validated on hardware.

## Known problems

**The force-feedback keepalive is best-effort.** evdev is the Linux input
event interface. If the wheel's evdev node cannot be opened, a
direct-drive wheel can still drive itself into its stops while TrueForce
streams. The self-test refuses to run in that state. The daemon carries
on.

**Low-frequency haptic layers move the wheel rather than buzzing it.** The
pit limiter is 10 Hz, ABS 15 Hz, the rev limiter 25 Hz. Excursion for a
given torque goes roughly as 1/f^2. Their gains were chosen as torque
levels, but what you feel is excursion. This is deliberately not "fixed"
with a frequency curve. `effects.rs` gives the reasoning: the wheel is
normally held, and a hand already damps exactly those frequencies.

**The KF/TF crossover admits texture the wheel can follow.** Anything at
or above 20 Hz routes to the texture channel. 20 to 40 Hz is where these
wheels still track the waveform. This is flagged rather than moved,
because changing it changes which effects route where.

**A session that dies without its teardown latches the wheel.** The next
TrueForce session opens fine and never streams. A power cycle of the base
clears it.

**Three features the wheels have and this driver does not use.**
`DUAL_CLUTCH`, `GAMING_ATTACHMENTS` and `AXIS_MAPPING`. (`DISPLAY_GAME_DATA`,
the base OLED, is driven through `wheel_oled` as of this release.)
Each needs a capture of G HUB exercising the control. Each would be a
non-force write to the HID++ endpoint, and specification 12.5 says such a
write cuts live force.

## Waiting on other people

- **#27**, Xbox G923 rev lights. The driver reports which features that
  wheel has. No owner has run it. Even a positive answer is not
  sufficient, because that wheel's force rides HID++ and 12.5 applies.
- **#52**, Xbox G923 force feedback coming up reliably, on the branch
  named above.
- **#87**, whether this driver's own engine can drive that wheel in every
  game. That is what would give it force feedback and TrueForce together.
- **#8**, a G PRO capture. That is what would let the real-G-PRO rev-light
  work start.
- **#20** and **#62**, an OLED descriptor readback (`0x8130` fn1) offered by
  a Windows implementation.

## Coverage numbers, for honesty about the compatibility table

Of the games listed: 6 verified end to end by this project, 18 documented
from a vendor or reliable source, 10 expected, 8 genuinely unknown.
Simulated TrueForce is live for 23 titles, impossible for 12 (no usable
telemetry), and possible-with-a-parser for 4.

So the table is mostly *not* first-party tested, and says so per row.
