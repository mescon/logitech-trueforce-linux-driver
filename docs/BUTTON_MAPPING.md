# Button Mapping

The physical controls on the RS50 / G PRO wheel and hub, and the joystick button
index each one reports. Buttons use sequential indices matching Windows
DirectInput, so bindings stay consistent across platforms.

This is the reference for binding controls in a game. The wire-level bitmask
(which report bit encodes which button) is in
[PROTOCOL_SPECIFICATION.md](PROTOCOL_SPECIFICATION.md). The G923 has its own
table below: it reports its own classic button layout straight from its HID
descriptor, not the RS50/G PRO one.

![RS50 button layout](images/rs-wheel-hub-button-layout.png)

The **Index** column is the joystick button number games show when binding
(sequential, matching Windows DirectInput). The **Diagram #** column is the
numbered callout box in the layout image above - the two numbering systems
are unrelated, so a button's index and its diagram number usually differ.

| Index | Button | Diagram # |
|-------|--------|-----------|
| 0 | A | 3 |
| 1 | X | 1 |
| 2 | B | 4 |
| 3 | Y | 2 |
| 4 | Right Paddle / Gear Right | 17 |
| 5 | Left Paddle / Gear Left | 16 |
| 6 | RT (Right Trigger) | 5 |
| 7 | LT (Left Trigger) | 15 |
| 8 | Camera / View | 11 |
| 9 | Menu | 9 |
| 10 | RSB (Right Stick) | 6 |
| 11 | LSB (Left Stick) | 14 |
| 21 | Right Encoder CW | 8 |
| 22 | Right Encoder CCW | 8 |
| 23 | Right Encoder Push | 8 |
| 24 | Left Encoder CW | 12 |
| 25 | Left Encoder CCW | 12 |
| 26 | Left Encoder Push | 12 |
| 27 | G1 (Logitech logo) | 10 |
| 28 | GL | 13 |
| 29 | GR | 7 |

GL and GR are their own buttons, not aliases of the shifter paddles
(hardware-verified 2026-07-20 by guided capture: evdev 0x2cc / 0x2cd,
sequential after G1).

The D-pad reports as a hat switch (`ABS_HAT0X` / `ABS_HAT0Y`), not as four
buttons - diagram callout "D".

## RS H-Shifter (046d:c26b)

### Connected to the Wheel Base

When connected to the wheel base accessory port (sub-device 03), gears 1–7 and Reverse report on DirectInput button indices 12 to 19 (hardware-verified 2026-10-29). Index 20 is unused.

| Index | Button / Gear | evdev Code | Wire bitmask (Report 0x08) |
|---|---|---|---|
| 12 | RS H-Shifter: Gear 1 | `0x12c` (`BTN_BASE`) | Byte 2 bit 0 (`0x01`) |
| 13 | RS H-Shifter: Gear 2 | `0x12d` (`BTN_BASE2`) | Byte 2 bit 1 (`0x02`) |
| 14 | RS H-Shifter: Gear 3 | `0x12e` (`BTN_BASE3`) | Byte 2 bit 2 (`0x04`) |
| 15 | RS H-Shifter: Gear 4 | `0x12f` (`BTN_BASE4`) | Byte 2 bit 3 (`0x08`) |
| 16 | RS H-Shifter: Gear 5 | `0x2c0` (`BTN_TRIGGER_HAPPY1`) | Byte 2 bit 4 (`0x10`) |
| 17 | RS H-Shifter: Gear 6 | `0x2c1` (`BTN_TRIGGER_HAPPY2`) | Byte 2 bit 5 (`0x20`) |
| 18 | RS H-Shifter: Gear 7 | `0x2c2` (`BTN_TRIGGER_HAPPY3`) | Byte 2 bit 6 (`0x40`) |
| 19 | RS H-Shifter: Reverse | `0x2c3` (`BTN_TRIGGER_HAPPY4`) | Byte 3 bit 0 (`0x01`) |
| 20 | Unused | `0x2c4` (`BTN_TRIGGER_HAPPY5`) | - |

### Connected Directly to PC via USB

When connected directly to the PC via USB (`046d:c26b Logitech RS H-Shifter`), it enumerates as an independent joystick reporting 8 digital buttons:
- Buttons 0 to 6 (`0x120`..`0x126`): 1st through 7th gear
- Button 7 (`0x127`): Reverse (R)

## G923

The G923's own button layout, hardware-captured 2026-07-27 by a guided live
capture (every physical button pressed in turn, its joystick index
recorded) on a PS-edition unit (PID 0xc266). It reports the same sequential
DirectInput indexing as the RS50/G PRO above, but a different set of
buttons: no G1/GL/GR, and only one dial (right hand; the G923 has no left
encoder at all).

| Index | Button |
|-------|--------|
| 0 | X |
| 1 | Square |
| 2 | Circle |
| 3 | Triangle |
| 4 | Right Paddle |
| 5 | Left Paddle |
| 6 | R2 |
| 7 | L2 |
| 8 | Share |
| 9 | Options |
| 10 | R3 |
| 11 | L3 |
| 19 | Plus (Up) |
| 20 | Minus (Down) |
| 21 | Dial CW |
| 22 | Dial CCW |
| 23 | Dial Push |
| 24 | PS |

Indices 12 to 18 are gaps in the HID descriptor (unused). Indices 19-20
(Plus/Minus) are real buttons on the G923, unlike the RS50/G PRO, which
have no buttons there at all.

The D-pad reports as a hat switch (`ABS_HAT0X` / `ABS_HAT0Y`), not as four
buttons, same as the RS50/G PRO.
