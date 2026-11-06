# HID++ feature matrix

HID++ is Logitech's configuration protocol over USB. This page lists what
each wheel reports, against what this driver uses. The list was made on
hardware on 2026-08-08. The method: ask `ROOT.GetFeature` for every gaming
feature in Logitech's published HID++ 2.0 registry (mirrored in Solaar's
`hidpp20_constants.py`). It was not read from captures.

The driver logs its own line of this at probe (the first set-up of the
device):

```
HID++ features (base): 8040=0A 807A=0B 807B=0C 80A4=0D 80D0=0E ...
```

## Read this before drawing conclusions from it

**These are BASE-DEVICE answers.** HID++ sub-devices answer separately.
The RS Shifter & Handbrake reports at device index `0x04`. The RS
H-Shifter (`0x812D RS_H_SHIFTER`) reports at the index of the port it is
plugged into. Their features error out here instead of reporting
themselves. `0x80B1 BANDED_AXIS` and `0x812C WHEEL_CENTER_POSITION` show
as absent on the RS50 below, and the driver demonstrably uses both.
Absence in this table means "not on the base device". It does not mean
"not on the wheel".

The G923 column is more trustworthy. It answers a clean "not supported"
instead of erroring, because its classic force-feedback path leaves HID++
alone.

## The matrix

| feature | RS50 (c276) | G923 PS (c266) | driver uses |
|---|---|---|---|
| `0x8040` BRIGHTNESS_CONTROL | 0x0A | - | yes |
| `0x807A` RPM_INDICATOR | 0x0B | 0x11 | yes |
| `0x807B` RPM_LED_PATTERN | 0x0C | - | yes |
| `0x80A3` LEGACY_AXIS_RESPONSE_CURVE | - | **0x12** | **no** |
| `0x80A4` AXIS_RESPONSE_CURVE | 0x0D | - | yes |
| `0x80D0` COMBINED_PEDALS | 0x0E | 0x13 | yes |
| `0x8120` GAMING_ATTACHMENTS | 0x0F | **0x0F** | **no** |
| `0x8123` FORCE_FEEDBACK | 0x10 | - | yes |
| `0x8127` DUAL_CLUTCH | 0x11 | **0x10** | **no** |
| `0x8130` DISPLAY_GAME_DATA | 0x12 | - | **yes** (`wheel_oled`) |
| `0x8132` AXIS_MAPPING | 0x13 | - | **no** |
| `0x8133` GLOBAL_DAMPING | 0x14 | - | yes |
| `0x8134` BRAKE_FORCE | 0x15 | - | yes |
| `0x8136` TORQUE_LIMIT | 0x16 | - | yes |
| `0x8137` CONFIGURATION_PROFILES | 0x17 | - | yes |
| `0x8138` OPERATING_RANGE | 0x18 | - | yes |
| `0x8139` TRUE_FORCE | 0x19 | - | yes |
| `0x8140` FFB_FILTER | 0x1A | - | yes |

The G923 reports neither `FORCE_FEEDBACK` nor `TRUE_FORCE`. This confirms
on its own what captures already showed: the PlayStation edition drives
force through the classic path, not the 0x8123/0x8139 family.

## The gaps, and what each would actually take

These are not filed as issues. None is a small fix, and saying so is more
useful than a ticket that implies otherwise.

**`0x80A3` on the G923.** The two wheels use different response-curve
pages. The RS50 has `0x80A4`. The G923 has the legacy `0x80A3`. So
`wheel_response_curve` is structurally unreachable on a G923.

This is not a page fallback away from working. The G923 exposes **no**
`wheel_*` attributes at all. It takes the classic path, which has no
settings surface, no feature discovery and no sysfs group. sysfs is the
kernel's settings file tree under `/sys`. Reaching `0x80A3` means building
that path for this wheel. It also means decoding a page whose function
layout is undocumented. That is feature work, not a bug fix.

**`0x8127` DUAL_CLUTCH, present on both wheels.** A clutch bite point is a
real thing to want, and nothing here offers one. The page's functions are
now known. They came from a third-party listen on a G923 Xbox edition, not
from a G HUB capture (@fsfarmscaper, [#97](../../issues/97)). G HUB is
Logitech's Windows software. The wheel configures the bite point and the
paddle from button combinations on the wheel itself. `fn2` reads the
result back. Every change arrives as an unsolicited event on the feature
index. Writes are acknowledged and change nothing. So the host's whole job
here is to read. See PROTOCOL_SPECIFICATION.md 12.6. It is not implemented
yet. The natural form is a read-only attribute plus the event. It needs a
wheel with a dual clutch to verify against, and no wheel here has one.

**`0x8120` GAMING_ATTACHMENTS, present on both.** This is plausibly the
accessory reporting the driver currently infers by scanning sub-device
indices. That scan is known to be awkward: an index is a physical port,
not a device type. The blocker is the same: no function layout.

**`0x8130` DISPLAY_GAME_DATA, RS50 only. NOT unknown**, and an earlier
version of this file wrongly said it was. It is the RS50's Dynamic OLED.
`PROTOCOL_SPECIFICATION.md` 12.3 has it largely decoded from issue #20:
fn0 layout count, fn1 layout descriptor, fn2 clear pending, fn3 set
layout/data, ten layouts A-J, a typed firmware renderer rather than a
framebuffer, reached at interface 1 endpoint 0 by SET_REPORT. A third
party reached it on hardware with static text and live iRacing telemetry.
Since then it has been reached first-hand on an RS50. Implemented:
`wheel_oled` and `wheel_oled_layouts` in sysfs, a dashboard in
`logi-tf-sim`, and an editor with presets in both apps. What is still open
is the risk below. It is the same risk the rev lights carry.

**`0x8132` AXIS_MAPPING, RS50 only.** Unknown. It would also duplicate
something already done host-side.

## Before writing to any of these: a live-force hazard

`PROTOCOL_SPECIFICATION.md` 12.5 records this, from two independent third
parties: **while force is present on the HID++ endpoint, a non-force write
to that endpoint cuts the force.** This holds regardless of sender, size
or pacing. A 60 ms floor fixed nothing. Rev-light writes on `0x807A`
during a game's force were the original case.

That is a constraint on every feature in the "not implemented" list,
because each would be exactly such a write. It also bears directly on
issue #27. The G923 Xbox edition is the only wheel here whose force rides
HID++. So driving its rev lights over `0x807A` may cost force feedback
while the lights update, even if the feature is present. Finding the
feature is therefore necessary and not sufficient.

The feature scan this file records is safe on that count. It runs once at
probe, before any force exists.

All four want the same thing before code: a capture of G HUB exercising
the control. That is the routine this project has used for every other
page it implements. There is no shortcut that does not amount to guessing
at a wheel's firmware.
