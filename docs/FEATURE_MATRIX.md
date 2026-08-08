# HID++ feature matrix

What each wheel reports, against what this driver uses. Enumerated on
hardware 2026-08-08 by asking `ROOT.GetFeature` for every gaming feature in
Logitech's published HID++ 2.0 registry (mirrored in Solaar's
`hidpp20_constants.py`), rather than by reading captures.

The driver logs its own line of this at probe:

```
HID++ features (base): 8040=0A 807A=0B 807B=0C 80A4=0D 80D0=0E ...
```

## Read this before drawing conclusions from it

**These are BASE-DEVICE answers.** HID++ sub-devices answer separately: the
RS Shifter & Handbrake reports at device index `0x04`, and its features
error out here rather than reporting themselves. `0x80B1 BANDED_AXIS` and
`0x812C WHEEL_CENTER_POSITION` show as absent on the RS50 below and the
driver demonstrably uses both. Absence in this table means "not on the base
device", not "not on the wheel".

The G923 column is more trustworthy: it answers a clean "not supported"
rather than erroring, because its classic force-feedback path leaves HID++
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
| `0x8130` DISPLAY_GAME_DATA | 0x12 | - | **no** |
| `0x8132` AXIS_MAPPING | 0x13 | - | **no** |
| `0x8133` GLOBAL_DAMPING | 0x14 | - | yes |
| `0x8134` BRAKE_FORCE | 0x15 | - | yes |
| `0x8136` TORQUE_LIMIT | 0x16 | - | yes |
| `0x8137` CONFIGURATION_PROFILES | 0x17 | - | yes |
| `0x8138` OPERATING_RANGE | 0x18 | - | yes |
| `0x8139` TRUE_FORCE | 0x19 | - | yes |
| `0x8140` FFB_FILTER | 0x1A | - | yes |

The G923 reporting neither `FORCE_FEEDBACK` nor `TRUE_FORCE` confirms
independently what captures already showed: the PlayStation edition drives
force through the classic path, not the 0x8123/0x8139 family.

## The gaps, and what each would actually take

Not filed as issues because none is a small fix, and saying so is more
useful than a ticket that implies otherwise.

**`0x80A3` on the G923.** The two wheels use different response-curve pages:
the RS50 has `0x80A4`, the G923 has the legacy `0x80A3`. So
`wheel_response_curve` is structurally unreachable on a G923.

This is not a page fallback away from working. The G923 exposes **no**
`wheel_*` attributes at all: it takes the classic path, which has no
settings surface, no feature discovery and no sysfs group. Reaching `0x80A3`
means building that path for this wheel and decoding a page whose function
layout is undocumented. Feature work, not a bug fix.

**`0x8127` DUAL_CLUTCH, present on both wheels.** A clutch bite point is a
real thing to want and nothing here offers one. Blocked on not knowing the
page's functions: implementing it blind means guessing at commands sent to a
wheel, and a wrong guess in a force-feedback device is not a harmless
mistake. Needs a G HUB capture of the bite-point control.

**`0x8120` GAMING_ATTACHMENTS, present on both.** Plausibly the accessory
reporting this driver currently infers by scanning sub-device indices, which
is known awkward (index is a physical port, not a device type). Same
blocker: no function layout.

**`0x8130` DISPLAY_GAME_DATA, RS50 only. NOT unknown**, and an earlier
version of this file wrongly said it was. It is the RS50's Dynamic OLED, and
`PROTOCOL_SPECIFICATION.md` 12.3 has it largely decoded from issue #20:
fn0 layout count, fn1 layout descriptor, fn2 clear pending, fn3 set
layout/data, ten layouts A-J, a typed firmware renderer rather than a
framebuffer, reached at interface 1 endpoint 0 by SET_REPORT. Reached on
hardware by a third party with static text and live iRacing telemetry.
Unimplemented here, but the open questions are narrower than "what is it":
function numbers and payload layout are known, and the risk below is not.

**`0x8132` AXIS_MAPPING, RS50 only.** Unknown, and it would duplicate
something already done host-side.

## Before writing to any of these: a live-force hazard

`PROTOCOL_SPECIFICATION.md` 12.5 records, from two independent third
parties, that **while force is present on the HID++ endpoint a non-force
write to that endpoint cuts the force.** Regardless of sender, size or
pacing; a 60 ms floor fixed nothing. Rev-light writes on `0x807A` during a
game's force were the original case.

That is a constraint on every feature in the "not implemented" list, because
each would be exactly such a write. It also bears directly on issue #27: the
G923 Xbox edition is the only wheel here whose force rides HID++, so driving
its rev lights over `0x807A` may cost force feedback while the lights update
even if the feature is present. Finding the feature is therefore necessary
and not sufficient.

The enumeration this file records is safe on that count: it runs once at
probe, before any force exists.

All four want the same thing before code: a capture of G HUB exercising the
control. That is the routine this project has used for every other page it
implements, and there is no shortcut that does not amount to guessing at a
wheel's firmware.
