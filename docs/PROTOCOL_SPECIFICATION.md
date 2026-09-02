# Logitech TrueForce Direct-Drive Wheel Protocol Specification

**Document Version**: 6.7
**Date**: 2026-07-06
**Author**: Verified from USB capture analysis
**Status**: Protocol reference for Linux driver development

This document was originally RS50-only. Most of what it describes now applies to the Logitech G Pro Racing Wheel (`046d:c272` Xbox/PC, `046d:c268` PS/PC) as well. G Pro-only or RS50-only differences are called out inline.

---

## 1. Device Identification

### RS50

| Property | Value |
|----------|-------|
| Vendor ID | `0x046D` (Logitech) |
| Product ID | `0xC276` |
| Device Name | RS50 Base for PlayStation/PC |
| USB Version | 2.0 |
| Max Packet Size | 64 bytes |
| Device Class | HID (Human Interface Device) |

### G Pro Racing Wheel

| Property | Value |
|----------|-------|
| Vendor ID | `0x046D` (Logitech) |
| Product ID | `0xC272` (Xbox/PC), `0xC268` (PS/PC) |
| Device Name | Logitech G Pro Racing Wheel |

G Pro exposes additional HID++ sub-devices (indices 0x01, 0x02, 0x05 over the shared interface 1), whereas the RS50 is a single-device base. Centre calibration (section 5, page 0x812C) lives on sub-device 0x05 and is only exercised by the driver on the G Pro.

---

## 2. USB Interface Structure

The RS50 presents **3 HID interfaces** to the host:

### Interface 0 - Game Controller (Joystick)
| Property | Value |
|----------|-------|
| Endpoint | `0x81 IN` (Interrupt) |
| Report Size | 30 bytes |
| Interval | 1ms |
| Purpose | Wheel position, pedals, buttons |

### Interface 1 - HID++ Protocol
| Property | Value |
|----------|-------|
| Endpoint IN | `0x82 IN` (Interrupt) |
| Endpoint OUT | `0x00` (Control, SET_REPORT) |
| Report Size | 64 bytes |
| Purpose | Configuration, settings, feature queries |

### Interface 2 - Force Feedback
| Property | Value |
|----------|-------|
| Endpoint IN | `0x83 IN` (Interrupt, 64 bytes) |
| Endpoint OUT | `0x03 OUT` (Interrupt, 64 bytes) |
| Purpose | Real-time force feedback |

**Important**: FFB uses dedicated endpoint `0x03 OUT`, NOT the HID++ protocol.

---

## 3. Joystick Input Reports (Endpoint 0x81)

### Report Format (30 bytes) - VERIFIED

```
Offset  Size  Type      Description
------  ----  ----      -----------
0-1     2     uint16    Button bitmask, buttons 0-15 (little-endian)
2-3     2     -         Button bitmask high bytes (byte 3 bit 7 = G button; otherwise 0)
4-5     2     uint16    Wheel position (little-endian)
6-7     2     uint16    Accelerator pedal (little-endian)
8-9     2     uint16    Brake pedal (little-endian)
10-11   2     uint16    Clutch pedal (little-endian)
12-17   6     -         Reserved (zeros)
18      1     uint8     Always 0x01
19-29   11    -         Reserved (zeros)
```

### Button Bitmask (4 bytes, offset 0-3) - VERIFIED

Button state is encoded in bytes 0-3 of the input report.

**Byte 0:**
| Bit | Mask   | Button |
|-----|--------|--------|
| 0-3 | `0x0F` | D-pad hat nibble (see D-pad Encoding below; `0x08`+ = released) |
| 4   | `0x10` | **A** |
| 5   | `0x20` | **X** |
| 6   | `0x40` | **B** |
| 7   | `0x80` | **Y** |

**Byte 1:**
| Bit | Mask   | Button |
|-----|--------|--------|
| 0   | `0x01` | **Right Paddle** |
| 1   | `0x02` | **Left Paddle** |
| 2   | `0x04` | **LT** (Left Trigger Button) |
| 3   | `0x08` | **RT** (Right Trigger Button) |
| 4   | `0x10` | **View** (Back/Select) |
| 5   | `0x20` | **Menu** (Start) |
| 6   | `0x40` | **LB** (Left Bumper) |
| 7   | `0x80` | **RB** (Right Bumper) |

**Byte 3:**
| Bit | Mask   | Button |
|-----|--------|--------|
| 7   | `0x80` | **G Button** (Logitech logo) |

### D-pad Encoding (Byte 0 low nibble, bits 0-3) - standard HID hat

Byte 0's low nibble (`byte0 & 0x0F`) is a **standard HID Hat Switch**
(Usage `0x39`). The interface-0 report descriptor declares it as logical
`0-7` over physical `0-315` degrees with a null state, which by HID
convention means value `0` = Up and each step is `45` degrees clockwise.
Values `8-15` are the null (centered / released) state. The high nibble
(bits 4-7) holds the A/X/B/Y buttons listed in the Byte 0 table above.

Because this is a standard hat switch, the kernel's native HID input
mapping decodes it to `ABS_HAT0X`/`ABS_HAT0Y`; the driver does **no**
custom D-pad decoding.

| Hat value | Direction | Angle |
|-----------|-----------|-------|
| `0` | Up | 0 deg |
| `1` | Up-Right | 45 deg |
| `2` | Right | 90 deg |
| `3` | Down-Right | 135 deg |
| `4` | Down | 180 deg |
| `5` | Down-Left | 225 deg |
| `6` | Left | 270 deg |
| `7` | Up-Left | 315 deg |
| `8-15` | Released (null state) | - |

**Detection**: the hat value is `byte0 & 0x0F`; `0-7` are directions and
`8-15` are released (so "released" is also detectable as `byte0 & 0x08`).

**Example (no buttons pressed):**
- Idle / released: `08 00 00 00`
- D-Up: `00 00 00 00`
- D-Right: `02 00 00 00`
- D-Down: `04 00 00 00`
- D-Left: `06 00 00 00`
- D-Up-Right: `01 00 00 00`

> An earlier revision of this section documented a hand-rolled byte-0
> decode with a non-standard direction table (e.g. value `0x02` = Left,
> `0x06` = Down). That decode lived in the driver, mapped several
> directions wrongly - it reported physical Left as Down - and was removed
> in favour of the kernel's native hat mapping (issue #22). The table
> above is the standard HID hat encoding the report descriptor actually
> declares, confirmed against the live wheel after the fix.

### Wheel Position Encoding

| Value | Position |
|-------|----------|
| `0x0000` | Full left |
| `0x8000` | Center |
| `0xFFFF` | Full right |

Resolution depends on configured rotation range (90° to 2700°).

### Pedal Value Encoding

| Value | Position |
|-------|----------|
| `0x0000` | Released |
| `0xFFFF` | Fully pressed |

### Example Reports

```
Centered, pedals released:
08 00 00 00 00 80 00 00 00 00 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00

Throttle full:
08 00 00 00 3C 80 FF FF 00 00 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00
                  ^^^^^ accelerator = 0xFFFF

Brake partial (0x4847):
08 00 00 00 10 80 00 00 47 48 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00
                        ^^^^^ brake = 0x4847
```

---

## 4. Force Feedback Output Commands (Endpoint 0x03 OUT)

> **Endpoint 0x03 carries one 64-byte type-0x01 packet family, not two
> separate protocols.** Byte 10 (the "new samples this packet" count)
> demultiplexes it: `0` is a pure constant-force ("KF") update
> documented here, and `4` is a unified force+audio ("TF") packet whose
> bytes 12-63 carry a rolling haptic sample window on top of the same
> force field. **[`TRUEFORCE_PROTOCOL.md`](TRUEFORCE_PROTOCOL.md) is the
> authoritative reference for the full framing** (window layout, init
> sequence, type-0x0e range, type-0x02 responses). This section covers
> only the constant-force subset the kernel driver's steering path
> emits, plus the encoding and refresh command shared by both.

### Constant-Force Report (64 bytes, byte 10 = 0) - VERIFIED

```
Offset  Size  Type      Description
------  ----  ----      -----------
0       1     uint8     Report ID (0x01)
1-3     3     -         Reserved (0x00 0x00 0x00)
4       1     uint8     Command type (0x01 = stream/sample packet)
5       1     uint8     Sequence counter (0x00-0xFF, wraps)
6-7     2     uint16    Force value / motor torque target ("cur", LE)
8-9     2     uint16    Force value duplicate (LE, must match 6-7)
10      1     uint8     New-sample count (0 here; 4 = TF audio packet)
11-63   53    -         Zero for a constant-force packet. In a TF
                        packet byte 11 is 0x0d and 12-63 are the audio
                        window - see TRUEFORCE_PROTOCOL.md.
```

Bytes 6-9 are the wheel's motor torque target ("cur" in TrueForce
terms) and are honoured whether or not the packet also carries audio,
so a constant-force packet is a TF stream packet with an empty sample
window. **CRITICAL**: the sequence counter is a **single byte** that
wraps at 255 (unlike the two-byte HID++ sequence).

### Force Value Encoding (Offset Binary)

| Hex Value | Decimal | Force Direction |
|-----------|---------|-----------------|
| `0x0000` | 0 | Maximum LEFT |
| `0x4000` | 16384 | Half LEFT |
| `0x8000` | 32768 | NEUTRAL (no force) |
| `0xC000` | 49152 | Half RIGHT |
| `0xFFFF` | 65535 | Maximum RIGHT |

**Conversion from signed to offset binary**:
```c
uint16_t offset_binary = (int16_t)signed_value + 0x8000;
```

### FFB Enable/Refresh Command (64 bytes)

Sent periodically during gameplay (approximately every 30-60 seconds):

```
Offset  Size  Description
------  ----  -----------
0       1     Report ID (0x05)
1       1     Command type (0x07)
2-6     5     Reserved (zeros)
7-8     2     Value (0xFFFF = enabled?)
9-63    55    Padding (zeros)
```

Example: `05 07 00 00 00 00 00 FF FF 00 00 00 00 ...`

> **CORRECTION (capture re-analysis):** this `05 07` packet is NOT a wheel
> command. In every capture the `05 07 .. FF FF` packets are 32-byte
> DualShock-4 lightbar/rumble output reports to a *game controller* that was
> plugged in at capture time - the `FF FF` is the DS4 lightbar colour, not an
> FFB value. G HUB never sends `05 07` to the wheel, and the wheel's FFB
> endpoint is silent at idle. Host-alive is carried entirely by the type-0x01
> force stream. The driver no longer sends this packet.

### Example FFB Commands

```
Neutral (no force), sequence 0x7F:
01 00 00 00 01 7F 00 80 00 80 00 00 00 00 ... (54 zeros)

Maximum force LEFT, sequence 0x00:
01 00 00 00 01 00 00 00 00 00 00 00 00 00 ... (54 zeros)

Maximum force RIGHT, sequence 0x01:
01 00 00 00 01 01 FF FF FF FF 00 00 00 00 ... (54 zeros)
```

### Observed Behavior

- Force update rate: games stream **1000 Hz** (observed in every gameplay
  capture: ACC on RS50, ACC/BeamNG on G PRO - median inter-packet gap
  ~1.0 ms); the kernel driver's own force stream matches that at 1000 Hz
  since 0.30.0. Before that it was a jiffies timer whose nominal 500 Hz was
  really 333 Hz, because a self-rearming jiffies timer never gets the period
  it asks for
- Sequence counter increments with each command (one shared counter across
  all interface-2 packet types on the wire)
- Force value and duplicate must always match
- `05 07` is NOT sent to the wheel (see the correction above - it is a
  DualShock-4 packet); there is no idle FFB keepalive

---

## 5. HID++ Protocol (Interface 1)

The RS50 uses the HID++ 2.0-family protocol for configuration,
reporting protocol number 4. Decoding IRoot `GetProtocolVersion` (fn1)
per Logitech's official x0000 spec, the capture traffic
`10ff001d 0000 39` -> `12ff001d 04 02 39` (`2026-01-26_ghub_startup`)
reads: byte 0 `protocolNum` = 4, byte 1 `targetSw` = 0x02 (bit 1 =
"Logitech Gaming Software is the intended target SW"), byte 2 = echoed
pingData. Officially this is NOT a major.minor version - "4.2" is the
Linux kernel driver's de-facto major.minor reading of the same two
bytes, kept elsewhere in this document for consistency with kernel
terminology. G Hub uses this call with random pingData as a liveness
ping throughout every session.

### Message Formats

**Short Report (0x10)** - 7 bytes:
```
Byte 0: Report ID (0x10)
Byte 1: Device Index (0xFF = wired)
Byte 2: Feature Index
Byte 3: Function (high nibble) | SW ID (low nibble)
Bytes 4-6: Parameters
```

**Long Report (0x11)** - 20 bytes:
```
Byte 0: Report ID (0x11)
Byte 1: Device Index (0xFF)
Byte 2: Feature Index
Byte 3: Function | SW ID
Bytes 4-19: Parameters
```

**Very Long Report (0x12)** - 64 bytes:
```
Byte 0: Report ID (0x12)
Byte 1: Device Index (0xFF)
Byte 2: Feature Index
Byte 3: Function | SW ID
Bytes 4-63: Parameters
```

### ⚠️ CRITICAL: RS50 HID++ Report ID Behavior

**The RS50 has non-standard HID++ report handling:**

1. **G Hub sends SHORT reports (0x10)** for all commands, NOT long reports (0x11)
2. **RS50 ALWAYS responds with VERY LONG reports (0x12)** regardless of the input report type
3. Responses are 64 bytes even for simple queries

This is different from other Logitech HID++ devices which typically respond with the same report type they receive - but it IS officially specified: Logitech's HID vendor-collection usages document defines report ID 0x12 as the "very long" HID++ report (64 bytes, usage page 0xFF43), and the vendor collection usage's high byte is a capability bitmask (bit 0 = short 0x10/7B, bit 1 = long 0x11/20B, bit 2 = very-long 0x12/64B), so a device advertising usage 0x07NN supports all three. A driver can therefore detect very-long support from the report descriptor instead of assuming it. (An earlier revision of this section called 0x12 an undocumented firmware extension; the cpg-docs 2.0 draft indeed only defines 0x10/0x11, but the vendor-usages document covers it.) Note the exception in section 5.3: SUB-DEVICE responses (dev_idx 0x01/0x02/0x05) arrive as 0x11, not 0x12.

**Implication for drivers:**
- When sending 0x10 (short) or 0x11 (long), expect response on 0x12 (very long)
- The kernel driver must handle this asymmetric report ID behavior
- Feature discovery and settings queries work correctly when using 0x10 requests

**Verified via USB capture (2026-01-28):**
```
Host → Device: 10 ff 00 0b 81 38 00  (SHORT: query feature 0x8138)
Device → Host: 12 ff 00 0b 18 00 00... (VERY LONG: feature at index 0x18)
```

### HID++ Communication Method

Commands are sent via **USB Control Transfer (SET_REPORT)** to endpoint 0, NOT via interrupt OUT.
Responses arrive via **Interrupt IN** on endpoint 0x82.

```
Host → Device: SET_REPORT (Control, endpoint 0x00)
Device → Host: Interrupt IN (endpoint 0x82)
```

### Complete Feature Table (Verified)

| Index | Feature ID | Name | G Hub Setting |
|-------|------------|------|---------------|
| 0x00 | `0x0000` | IRoot | Feature discovery |
| 0x01 | `0x0001` | IFeatureSet | List all features |
| 0x02 | `0x0003` | DeviceInfo | Serial, firmware entities (live-verified 2026-07-02; see below) |
| 0x03 | `0x0005` | DeviceNameType | Device name string (fn0 = length, fn1 = name at offset, fn2 = type) |
| 0x04 | `0x00C3` | SecureDFU | Firmware update |
| **0x09** | **`0x1BC0`** | **ReportHidUsages** | Enables extra Button-page HID usages; optional (see 5.1) |
| 0x0A | `0x8040` | Brightness | **LED brightness only** (both modes). The steering "Sensitivity" slider is a `0x80A4` response-curve upload, NOT this feature (see below) |
| **0x0B** | **`0x807A`** | **LIGHTSYNC** | LED effect mode selection |
| **0x0C** | **`0x807B`** | **RGBZoneConfig** | LED RGB color data (see Section 9) |
| 0x0D | `0x80A4` | AxisResponseCurve | Per-axis 64-point response curves (see 5.1) |
| 0x0F | `0x8120` | GamingAttachments | Attachment/module management (openlogi registry name) |
| 0x10 | `0x8123` | ForceFeedback | HID++ FFB (unused by this driver; documented at openlogi.org) |
| 0x11 | `0x8127` | (undecoded, hidden) | fn2 result is constant across every onboard slot - not slot content; not sent by this driver (see "Onboard Profile Authoring" above) |
| 0x12 | `0x8130` | **DisplayGameData** | The Dynamic OLED's transport. Hardware-confirmed by a third party, not by this driver; not touched by any sysfs attribute (see 12.3) |
| 0x13 | `0x8132` | (undecoded) | Not touched by any sysfs attribute |
| **0x14** | **`0x8133`** | **Damping** | Damping slider |
| **0x15** | **`0x8134`** | **BrakeForce** | Brake Force slider |
| **0x16** | **`0x8136`** | **FFBStrength** | Strength slider |
| **0x17** | **`0x8137`** | **Profile** | Profile switching (called before LED changes) |
| **0x18** | **`0x8138`** | **RotationRange** | Rotation Range slider |
| **0x19** | **`0x8139`** | **TRUEFORCE** | TRUEFORCE slider |
| **0x1A** | **`0x8140`** | **FFBFilter** | FFB Filter + Auto toggle |

**DeviceInfo identity readout (live-verified 2026-07-02).** Feature
`0x0003` fn0 getDeviceInfo returns `[entityCnt][unitId x4][transport
x2][modelId...][capabilities @ byte 14]`; capabilities bit 0 gates fn2
`getDeviceSerialNumber`, which returns the real 12-character ASCII
serial (verified identical to the USB `iSerial`). fn1
`getFwInfo(entity)` returns `[type][name x3 ASCII][BCD number][BCD
rev][BE16 BCD build][active][trPid]...`; the wheel base has 3 entities
(type 1 bootloader "BL2", type 0 active main FW "U1 65.03.B0038", type
2 hardware) and the motor unit at sub-device `0x05` carries its own
DeviceInfo whose type-0 entity is the servo firmware ("SC
02.01.B0042"). Querying past the last entity returns a standard
InvalidArgument error frame. The driver reads all of this once at init
(`serial ..., base FW ..., motor FW ...` in dmesg, prefixed with the model tag) and exposes
`wheel_serial` / `wheel_firmware` in sysfs.

**Feature-type flags** (per Logitech's official x0000/x0001 specs,
which define the full byte): bit 7 `obsl` (obsolete), bit 6 `hidden`,
bit 5 `eng` (engineering), bit 4 `manuf_deact`
(manufacturing-deactivatable), bit 3 `compl_deact`
(compliance-deactivatable), bits 2-0 reserved. Every feature the Linux
driver touches (0x8040, 0x807A/B, 0x80A4, 0x8133-0x8140, and the
dev-0x05 calibration cluster 0x812B/0x812C) advertises flags 0x00 =
public. The undocumented catalog entries not listed above decode as:
0x40 = hidden; 0x60 = hidden + engineering; 0x70 = hidden +
engineering + manufacturing-deactivatable - features deliberately not
exposed to normal software. A useful signal that the driver's surface
sits entirely on Logitech's public feature set. (IFeatureSet
`getFeatureID` also returns a featureVersion byte the catalog dumps in
this section do not record; versions gate function availability, e.g.
x8040 gained its illumination on/off functions in v1.)

### Setting Commands (All Verified)

Each setting feature exposes a handful of HID++ functions. The encoding in byte 3 of the short report is `(function_number << 4) | SW_ID`. In the G Hub captures this analysis is derived from, SW_ID is `0xD` across the board; the Linux driver uses SW_ID `0x0a` (it must not use `0x01` - the pedal sub-device's MCU silently drops sw-id `0x01`), so the same SET appears on the wire as e.g. `0x2a` rather than `0x2d`. GET semantics are *mostly* `fn=0` queries capabilities/limits and `fn=1` reads current value - **but damping is an exception: its current value is read with `fn=0`; `fn=1` is the SETTER and an empty-payload `fn=1` sets damping to 0.** The SET function number also varies **per feature and per wheel**. Do not assume all settings use `fn=2`: damping and TRUEFORCE each have their own SET fn, and the two wheels agree on the exceptions.

| Feature | Page | GET caps | GET value | SET (RS50 + G Pro) |
|---------|------|----------|-----------|--------------------|
| Rotation range | 0x8138 | `fn=0` (`0x0D`) | `fn=1` (`0x1D`) | `fn=2` (`0x2D`) |
| FFB strength | 0x8136 | `fn=0` | `fn=1` | `fn=2` |
| FFB filter | 0x8140 | `fn=0` | `fn=1` | `fn=2` |
| Brake force | 0x8134 | `fn=0` | `fn=1` | `fn=2` |
| Sensitivity / brightness | 0x8040 | `fn=0` | `fn=1` | `fn=2` |
| Damping | 0x8133 | `fn=0` | `fn=1` | **`fn=1`** (`0x1D`) [1] |
| TRUEFORCE | 0x8139 | `fn=0` | `fn=1` | **`fn=3`** (`0x3D`) |
| Centre calibration | 0x812C | - | - | **`fn=3`** (`0x3D`) on sub-device 0x05 (RS50 + G Pro, both verified) |

[1] Damping reuses `fn=1` for both GET-value and SET; the device disambiguates by payload length (empty on GET, 3 bytes on SET).

The driver uses the same SET fn numbers for both wheels (`hidpp_dd_ff_data::fn_set_*` defaulted for RS50, overridden where G Pro differs). In practice the two wheels agreed on every SET fn we've captured so far.

All non-calibration commands use Short HID++ (`0x10`) with device index `0xFF`.

#### FFB Strength (Feature 0x8136, Index 0x16)
```
Set: 10 FF 16 2D [Value_Hi] [Value_Lo] 00
```
**Encoding**: Value = Nm × 8192 (range 1.0-8.0 Nm with 0.1 Nm steps)

| Value | Nm | Formula |
|-------|-----|---------|
| `0x1FFF` | 1.0 Nm | 1.0 × 8192 ≈ 8191 |
| `0x4FFF` | 2.5 Nm | 2.5 × 8192 ≈ 20479 |
| `0x7FFF` | 4.0 Nm | 4.0 × 8192 ≈ 32767 |
| `0xC998` | 6.3 Nm | 6.3 × 8192 ≈ 51608 |
| `0xFFFF` | 8.0 Nm | Maximum torque |

**Driver Formula**: `value = (uint16_t)(nm * 8191.875)` or `value = nm * 0xFFFF / 8.0`

#### Damping (Feature 0x8133, Index 0x14)
```
Set: 10 FF 14 1D [Value_Hi] [Value_Lo] 00
```
| Value | Percentage |
|-------|------------|
| `0x0000` | 0% |
| `0x7FFF` | ~50% |
| `0xFFFF` | 100% |

#### FFB Filter (Feature 0x8140, Index 0x1A)
```
Set: 10 FF 1A 2D [Flags] 00 [Level]
```

**Flags (Byte 4) - bitfield:**

| Bit | Mask | Meaning |
|-----|------|---------|
| 0 | `0x01` | User explicitly set this level right now (slider move) |
| 2 | `0x04` | Auto filter mode enabled |

The four values observed across RS50 and G Pro captures (`auto_ffb_filter`, `ffb_filter_sweep`, and the 2026-04-18 G Pro run) are `0x00`, `0x01`, `0x04`, `0x05`. Any combination of the two bits is legal:

| Flags | Interpretation |
|-------|----------------|
| `0x00` | Auto OFF, level held (auto-toggle path, user did not touch the slider this write) |
| `0x01` | Auto OFF, user just set the level (slider move) |
| `0x04` | Auto ON, level held (auto-toggle path) |
| `0x05` | Auto ON, user just set the level |

The driver splits this across two sysfs writes: `wheel_ffb_filter` always sets bit 0 and OR's bit 2 from the current auto state; `wheel_ffb_filter_auto` writes bare `0x00`/`0x04` to mirror G Hub's auto-toggle behaviour. See commits `63999d8` (decode) and `8ab5fc4` (driver simplification).

**Filter Level (Byte 6):** 1-15

| Value | Level (G Hub label) |
|-------|---------------------|
| `0x01` | Minimum |
| `0x07` | Low |
| `0x0B` | Medium |
| `0x0F` | Maximum |

**Examples:**
- `10 FF 1A 2D 01 00 0B` = user set level 11, auto OFF
- `10 FF 1A 2D 05 00 0B` = user set level 11, auto ON
- `10 FF 1A 2D 04 00 0B` = auto ON, level held at 11
- `10 FF 1A 2D 00 00 0B` = auto OFF, level held at 11

#### Rotation Range (Feature 0x8138, Index 0x18)
```
Set: 10 FF 18 2D [Degrees_Hi] [Degrees_Lo] 00
```
**Encoding**: Value = Degrees (direct, 16-bit big-endian)

| Value | Degrees | Notes |
|-------|---------|-------|
| `0x005A` | 90° | Minimum |
| `0x0168` | 360° | |
| `0x021C` | 540° | |
| `0x0384` | 900° | Common default |
| `0x0438` | 1080° | |
| `0x0A8C` | 2700° | Maximum |

**Range**: 90° to 2700° in 10° increments

> The 90° minimum is not a curiosity. When Logitech's TrueForce SDK cannot
> reach G HUB, which is its normal state under Proton, a game asking how far
> the wheel turns receives this floor rather than the real range, and clamps
> its steering to 45° each way. See `docs/TRUEFORCE_PROTOCOL.md`, "The SDK's
> own IPC, and where 90 degrees comes from".

#### TRUEFORCE (Feature 0x8139, Index 0x19)
```
Set: 10 FF 19 3D [Value_Hi] [Value_Lo] 00
```
| Value | Percentage |
|-------|------------|
| `0x0001` | Minimum |
| `0x4CCC` | ~30% |
| `0xFFFF` | 100% |

#### Brake Force (Feature 0x8134, Index 0x15) - ONBOARD MODE ONLY
```
Set: 10 FF 15 2D [Value_Hi] [Value_Lo] 00
```
**Encoding**: Value = Percentage × 655.35 (0x0000 = 0%, 0xFFFF = 100%)

| Value | Percentage |
|-------|------------|
| `0x028F` | ~1% |
| `0x3FFF` | ~25% |
| `0x4CCC` | ~30% |
| `0x7FFF` | ~50% |
| `0xBFFF` | ~75% |
| `0xFFFF` | 100% |

> ⚠️ **Note**: This setting is ONLY available in Onboard mode (profiles 1-5).
> It is NOT available in Desktop mode (profile 0).

#### Sensitivity - it is a 0x80A4 response curve, NOT 0x8040

> **CORRECTION (capture re-analysis + hardware):** the steering "Sensitivity"
> slider does NOT write feature 0x8040. `0x8040` is LED BrightnessControl only.
> G HUB's Sensitivity is a full 64-point `0x80A4` AxisResponseCurve upload on
> the steering axis (a cubic Bezier (0,0)->(1,1) with control points
> P1=(1-s, s), P2=(s, 1-s) for s=slider/100; 50 = identity = revert to the
> built-in curve). The `0x8040` "Set" shown below only changes LED brightness.
> The driver's `wheel_sensitivity` now uploads the 0x80A4 curve accordingly.

The 0x8040 write shown here changes LED brightness (both modes):
```
Set: 10 FF 0A 2D 00 [Value] 00
```
| Value | Setting |
|-------|---------|
| `0x00` | 0 |
| `0x19` | 25 |
| `0x32` | 50 |
| `0x4B` | 75 |
| `0x64` | 100 (default) |

> ⚠️ **Note**: This setting is ONLY available in Desktop mode (profile 0).
> It is NOT available in Onboard mode (profiles 1-5).

#### LED Brightness (Feature 0x8040, Index 0x0A) - Same as Sensitivity
```
Set: 10 FF 0A 2D 00 [Value] 00
```
| Value | Brightness |
|-------|------------|
| `0x00` | 0% (off) |
| `0x32` | 50% |
| `0x64` | 100% |

*Note: Shares the same Feature Index as Sensitivity.*

#### LIGHTSYNC Effect (Feature 0x807A, Index varies) - SIMPLIFIED

> ⚠️ **Note**: This is the simplified "quick effect" command. For full per-LED RGB
> control with custom colors, see **Section 9: LIGHTSYNC RGB LED Control**.

The feature index is discovered dynamically via `hidpp_root_get_feature(0x807A)`.
Typical index: `0x0B` or `0x0C` depending on firmware.

```
Set: 10 FF [idx] 3C [Effect] 00 00
```
| Effect | Name |
|--------|------|
| `0x01` | Inside→Out |
| `0x02` | Outside→In |
| `0x03` | Right→Left |
| `0x04` | Left→Right |
| `0x05`-`0x09` | CUSTOM 1-5 (renders custom slot `effect - 5`) |

The value only STAGES the effect; the strip repaints on the following
zero-parameter fn6 commit (see section 9.3.2).

#### Profile/Mode Switch (Feature 0x8137, Index 0x17)

**Get Current Mode:**
```
Query: 10 FF 17 1D 00 00 00
Response: 12 FF 17 1D [Profile] [Mode] 00 ...
```

**Set Mode/Profile:**
```
Set: 10 FF 17 2D [ProfileIndex] 00 00
```

| Index | Mode | Description |
|-------|------|-------------|
| `0x00` | Desktop | Single profile, Sensitivity available, Brake Force NOT available |
| `0x01` | Onboard 1 | First onboard profile, Brake Force available |
| `0x02` | Onboard 2 | Second onboard profile |
| `0x03` | Onboard 3 | Third onboard profile |
| `0x04` | Onboard 4 | Fourth onboard profile |
| `0x05` | Onboard 5 | Fifth onboard profile |

**Mode Differences:**
| Feature | Desktop (0x00) | Onboard (0x01-0x05) |
|---------|----------------|---------------------|
| Sensitivity | ✅ Available | ❌ Not available |
| Brake Force | ❌ Not available | ✅ Available |
| LED Colors | ✅ Full LIGHTSYNC | ❌ Effect+Brightness only |
| Profile Count | 1 | 5 fixed profiles |

**Examples:**
```
Switch to Desktop:  10 FF 17 2D 00 00 00
Switch to Onboard 1: 10 FF 17 2D 01 00 00
Switch to Onboard 3: 10 FF 17 2D 03 00 00
```

#### Onboard Profile Authoring

There is no block-write/upload protocol for an onboard slot's contents.
Confirmed by a full USB capture of G Hub's onboard settings page
(`dev/captures/2026-07-14_profile_save.pcapng`, 342 HID++ frames) plus a
read-only live cross-check against a connected RS50: **a slot is authored
by activating it (the Set Mode/Profile call above) and then sending the
same per-setting SET commands every other view uses.** The wheel persists
each value to the active slot's own storage immediately; there is no
separate commit/save call afterwards.

**Select-then-set model:**
```
1. Set: 10 FF 17 2D [1-5] 00 00      activate the slot to author
2. Set: 10 FF 18 2A [range] 00       wheel_range's normal SET, RotationRange
3. Set: 10 FF 16 2A [strength] 00    wheel_strength's normal SET, FFBStrength
4. ...                               any other setting's normal SET
```
Each SET is answered by the same unsolicited broadcast event the driver
already consumes when running live in that mode; there is no additional
frame that means "saved" or "committed". Reading a setting back always
reports the ACTIVE slot's stored value: switching to a different slot and
back shows the values it was last written with, confirming the writes
land in that slot's own storage, not a shared/desktop-only location.

**What a slot stores** (per-slot, confirmed by the capture and the live
cross-check): rotation range (`0x8138`), FFB strength (`0x8136`),
TrueForce intensity (`0x8139`), damping (`0x8133`), the FFB filter pair
(`0x8140`), brake force (`0x8134`, onboard-mode-only on the wheel itself),
LED effect + brightness, and the slot's own name (`0x8137` fn3/fn4). NOT
slot content: the steering sensitivity curve (G Hub actively reverts axis
0 to linear at the start of every onboard settings burst - the Sensitivity
slider is a desktop-only concept), combined-pedals, and the pedal/
handbrake response curves (a desktop-only software shaping layer; ignored
by the pedal MCU in native PC mode).

**Slot-0/name edge case:** `0x8137` fn3 (get slot name) against slot
argument `0x00` (the desktop state) returns an empty name rather than an
error - desktop has no onboard name to report, which is expected, not a
fault.

**Feature indices pinned this session** (request/response pairing against
a live G Hub startup capture; previously missing from the table below):
index `0x11` = `0x8127` (a hidden feature whose fn2 result reads back
identical across every slot, i.e. NOT part of a slot's content - some
kind of commit/sync call G Hub issues but this driver has never needed to
send), `0x12` = `0x8130` (DisplayGameData, the Dynamic OLED's transport;
see 12.3), `0x13` = `0x8132` (undecoded; not touched by any sysfs
attribute).

**Application-level implementation:** see `docs/SYSFS_API.md`'s "Editing
an onboard slot" section for how logi-wheel's userspace turns this wire
model into a guarded "select a slot, edit its values, optionally revert"
flow - the kernel driver itself needs no new attributes for any of it,
since authoring a slot is just writing the existing per-setting attrs
while `wheel_profile` names that slot.

#### Centre Calibration (Feature 0x812C, sub-device 0x05)

Both RS50 and G Pro expose centre calibration on sub-device `0x05`, not the root (`0xFF`). The feature index must be discovered by querying page `0x812C` on device index `0x05` and the resulting SET must also go to device index `0x05`. The index differs per wheel; RS50 captures show index `0x0f`, G Pro varies.

G Hub's calibrate button is a three-step exchange:

```
Query:   10 05 [idx] 1A 00 00 00           host -> device: fn=1 GET
Reply:   11 05 [idx] 1A [Pos_Hi] [Pos_Lo]  device -> host: raw encoder position
Set:     10 05 [idx] 3D [Pos_Hi] [Pos_Lo] 00  host -> device: fn=3 SET centre
```

**SET parameters:**
- Bytes 4-5: absolute encoder position (big-endian u16) to adopt as the new centre
- Byte 6: reserved, `0x00`

Two sysfs attributes drive this (see `docs/SYSFS_API.md`): `wheel_calibrate` is the raw SET primitive - the game or userspace tool samples the current wheel position from evdev and passes it verbatim as the new centre; `wheel_calibrate_here` performs the fn=1 GET then the fn=3 SET internally, adopting the wheel's current physical position in one write. Verified on RS50 from `2026-04-22_re_calibrate.pcapng` and on G Pro from `2026-04-18_calibrate.pcapng`.

### 5.1 G PRO PID Feature Set (RS50-in-compat-mode and real G PRO)

The G PRO Racing Wheel for Xbox/PC (`046d:c272`) and PS/PC (`046d:c268`) PIDs cover two physically distinct cases:

- **Real G PRO Racing Wheel.** Direct-drive wheel. iProduct string is "Logitech PRO Racing Wheel".
- **RS50 in "G PRO compatibility" mode.** RS50 hardware re-enumerated as the same VID/PID via the wheel's OLED menu. iProduct string is "Logitech RS50 Base for PlayStation/PC".

Both wheels are direct-drive and run the same modern firmware architecture. They share the same HID++ 4.2 feature catalog at the same indices and the same dedicated 64-byte FFB endpoint on interface 2. The driver gives both `HIDPP_QUIRK_DD_FFB` from the id-table (no iProduct-string sniff), so both go through the `hidpp_dd_ff_*` code path rather than the inherited G920 HID++ FFB path. This is what makes basic FFB, TrueForce streaming, and the wheel-config sysfs surface all work on a real G PRO without the queue-saturation / "Failed to send command" failures the G920 path inherits from the older gear-driven generation (issue #8).

Catalog note (corrected 2026-07-02 after a full IFeatureSet cross-capture
comparison): the RS50's compat-mode device-0xff feature catalog is
**byte-identical to its native catalog** (the full index 0x04-0x25 table
matches between `2026-01-26_ghub_startup` and
`2026-04-26_compat_ghub_init`). Only the **real G PRO's** catalog differs
- same canonical feature IDs, shifted to different indices (e.g. what
sits at index 0x0d on the RS50 is at 0x0b on the G Pro). An earlier
revision of this section described the compat catalog as "reduced";
that was wrong. What remains true operationally: `ROOT.GetFeature(<id>)`
lookups did not reliably return the expected indices on the G PRO PID in
early bring-up, so the driver still tries native-mode IDs first and then
the `HIDPP_DD_COMPAT_*` fallback indices (range / strength / trueforce /
damping / FFB filter / profile-mode switch / LIGHTSYNC / centre
calibration all wired).

A different feature set, observed only in compat mode, controls live host-pushed wheel settings. All commands below are short HID++ reports (`0x10`) sent on the corded device index `0xff` with sw_id `d`.

| Feature ID (best-guess) | Index | Fn | Purpose | Params |
|---|---|---|---|---|
| `0x8138` | 0x18 | 2 | Set live steering angle | `[angle_hi, angle_lo, 0x00]` (16-bit BE degrees) |
| `0x8136` | 0x16 | 2 | Set FFB strength | `[value_hi, value_lo, 0x00]` (16-bit BE, encoded as `Nm × 8192 - 1`, saturates at `0xFFFF` ≈ 8 Nm) |
| `0x8139` | 0x19 | 3 | Set TRUEFORCE strength | `[value_hi, value_lo, 0x00]` (16-bit BE 0..0xFFFF; 0..100% scale) |
| `0x8133` | 0x14 | 1 | Set wheel damping | `[value_hi, value_lo, 0x00]` (16-bit BE 0..0xFFFF; 0..100% scale) |
| `0x8140` | 0x1A | 2 | Set FFB filter level | `[0x00, 0x00, level]` (level 1..15) |

The fallback indices in the table are what GHUB uses on a 2026-04-26 firmware revision. The driver tries `ROOT.GetFeature(<id>)` first so a future firmware that reorders the table still works; if that returns an unknown index, the hardcoded fallback is used.

**Mode switching in compat mode**: feature `0x8137` (advertised at index `0x17` in compat, the same canonical "Profile" feature ID as in native) carries the mode switch:

- `fn=1` with no parameters reads `[profile, mode, ...]`: `params[0]` is the profile index (0 = desktop, 1..5 = onboard slot), `params[1]` a mode flag. (An earlier revision of this section misread the response as `[mode_class, slot]`; that was disproven live 2026-07-02 - a decode built on it reported "profile 1" while the wheel's OLED sat on slot 2.)
- `fn=2` with `[0x00, 0x00, 0x00]` switches the wheel into desktop mode. Subsequent live host SETs (range, strength, trueforce, damping, filter) take effect on the motor immediately. Verified end-to-end on 2026-04-26 against the live wheel.
- `fn=2` with `[slot, 0x00, 0x00]` selects onboard slot 1..5 - the same plain-index encoding as native, confirmed live 2026-07-02 (OLED landed on the correct slot name, sysfs read-back matched, and the slot's stored range applied). This is also what G Hub's own packets send (`10ff172d 03` in the compat profile-sweep captures).
- `fn=3` with `[slot]` returns the slot's user-assigned profile NAME: `[slot][length][ASCII name]` (e.g. `12ff173c 01 04 "RACE"`). Exposed as the `wheel_profile_names` sysfs attribute; verified against the wheel's OLED profile list.

The wheel boots in onboard mode by default. The OLED menu cycles between onboard profile slots only; it does not expose a desktop indicator separately, but the slot name displayed reflects the active state.

An earlier draft of this document and the driver shipped with a "force_desktop_mode" helper that sent `10ff1a2d 00 00 0b` to feature `0x8140`; the dedicated filter-only capture (`dev/captures/2026-04-26_compat_range_filter_only_desktop.pcapng`) proved that command actually sets the FFB filter level to `0x0b` (= 11), not switching modes. The helper was removed and replaced with the discovery above.

**Format notes**:

- Damping uses `fn=1` (other settings use `fn=2` or `fn=3`); this matches the native RS50 convention where damping is also `fn=1`.
- FFB filter wire format in compat mode is simpler than native: bytes 0-1 are zero, byte 2 carries the 1..15 level. There is no `flags` byte and no observable auto-mode encoding from compat-mode captures, so the driver leaves `wheel_ffb_filter_auto` as `-EOPNOTSUPP` in compat.
- Onboard mode silently ignores live host-pushed SETs. Compat-mode sysfs writes only take physical effect on the motor while the wheel is in desktop mode; the wheel boots in onboard, so write `0` to `wheel_profile` first to switch into desktop.

**Previously-unknown features, resolved 2026-07-02** (full cross-capture
analysis; index-to-ID mapping derived from IFeatureSet fn1 pairing in
`ghub_startup`, `compat_ghub_init`, and the G Pro contributor captures):

- **Feature `0x80A4` (AxisResponseCurve; index `0x0d` on RS50 native AND
  compat, `0x0b` on the real G Pro; also present on pedal sub-device
  `0x02`).** Per-axis 64-point response-curve store, NOT a torque LUT.
  `fn0` caps returns `[axes=7][3][3]` on the wheel base (`[3][3][3][1]`
  on the pedal unit); `fn1 [axis]` returns `[axis][00 01 00][HID usage]
  [bit width 0x10][loaded_points u16][max_points u16 = 0x0040]`; an
  upload is `fn3 [axis]` (open; empty param = axis 0 / steering),
  22x `fn4 [n][(in,out) u16-BE x n]` chunks (n <= 3; 64 monotonic points
  from `(0,0)` to `(0xFFFF,0xFFFF)`), then `fn5` commit (echoes
  `[axis][00][0040]`); `fn6 [axis]` reverts an axis to the built-in
  curve. This is what G Hub's **Sensitivity** slider uploads for the
  steering axis, exposed as `wheel_response_curve`; the wheel applies it
  to the steering axis it reports to the PC.

  The same feature applies curves on three axis groups, each of which the
  driver drives:
  - **Steering** (base dev `0xff`, axis 0): `wheel_response_curve` and
    `wheel_sensitivity`.
  - **Pedals** (base dev `0xff`, axes **1/2/3** = throttle/brake/clutch):
    `wheel_{throttle,brake,clutch}_{curve,sensitivity,deadzone}`.

    **On the BASE, not the pedal MCU.** The pedal sub-device exposes its own
    `0x80A4` store and will accept a 64-point upload, reporting it back as
    loaded - but it never applies it to the axis it reports to the PC. The
    curve that actually bends a pedal lives on the base, one axis up from
    the pedal index. Hardware-proven 2026-07-28 with a step curve (output
    plateaus `0x2000`/`0xE000`, which an applied curve makes the band between
    unreachable):

    | destination | result |
    |---|---|
    | pedal MCU | readback said 64/64 points, axis swept straight through the forbidden band |
    | base axis 1 | axis snapped to exactly `0x2000` with the pedals untouched |

    Each mapping was then confirmed individually, nothing touched: base axis 1
    moves only `ABS_RX` (throttle), axis 2 only `ABS_RY` (brake), axis 3 only
    `ABS_RZ` (clutch). G Hub writes its throttle curve to base axis 1 as well
    (capture, same date), so this is also the path the wheel is designed
    around.

    Earlier revisions of this document said the pedal MCU applied its curve
    and that the base's pedal axes were inert. Both were wrong, and the driver
    was built on them - every pedal curve, sensitivity and deadzone was
    silently doing nothing until it was retargeted. Note the trap: `fn1`
    readback reports what a sub-device has STORED, which is not the same as
    what it APPLIES, and only the latter can be seen on evdev.
  - **Analog handbrake** (base dev `0xff`, axis 4, HID usage 0x32 = Z, evdev
    `ABS_Z`): `wheel_handbrake_curve` and `wheel_handbrake_sensitivity`.

  A measurement note: the wheel emits no HID reports while an axis is held
  still, so an upload does not change the reported axis until it next moves;
  read the point count back with `fn1` for a motion-free check.
  Unknowns: the second `03` in caps, pedal-unit
  `fn9 [axis]` (called on every G Hub init; possibly axis refresh), and
  whether curves persist across power cycles.
- **Feature `0x80D0` (combined pedals; on the wheel base dev `0xff`).** A
  boolean: `fn1` sets it (`10 ff <idx> 1a 01` on / `...00` off), `fn0` reads it
  back. When on, the wheel merges the throttle and brake into a single centred
  axis for legacy games. Exposed as `wheel_combined_pedals` (desktop mode only).
  The same feature also emits the profile-change broadcast events the driver
  consumes, so its index is shared.
- **Feature `0x1BC0` (REPORT_HID_USAGE; index `0x09` on RS50, `0x07` on
  G Pro).** On every config apply, G Hub sends `fn2` (empty) followed by
  LONG `fn1` writes of `01 0009 00XX` with XX iterating `{0x0d..0x12,
  0x15}` on the RS50 PID and `{0x0d..0x13, 0x15}` on the G PRO PID -
  byte-identical across wheels, so XX is not a feature index. Given the
  registered feature name, the likely reading is "enable reporting of
  Button-page (usage page 0x0009) usages 13..21"; the effect on the HID
  interface was never isolated in captures. Optional: every SET works
  without it, and the Linux driver omits it with no observed downside.
  Naming sources: Solaar's feature registry and openlogi.org both list
  `0x1BC0` as REPORT_HID_USAGE / "report HID usage pages to host"
  (corroborating the payload reading); cvuchener/hidpp's older table
  calls it "Persistent remappable action", but that is settled
  definitively by Logitech's own spec set, which contains
  `x1c00_persistentremappableaction` and no 0x1BC0 document -
  PersistentRemappableAction is `0x1C00`, and `0x1BC0` =
  ReportHidUsages stands. No public source documents 0x1BC0's
  functions; the fn map above (from captures) is the best available.
- **Compat index `0x15` = `0x8134` Brake Force - mystery closed.** The
  compat catalog is identical to native (see above), so index 0x15 is
  the already-documented Brake Force feature. The previously mysterious
  `10ff152a XXXX` pushes during "unrelated" slider sweeps are the G Hub
  Brake Force slider (u16 BE, 0..0xFFFF = 0..100%), e.g. `028f / 4ccc /
  7fff / ffff` in `compat_range_other_sliders_desktop`, each answered by
  broadcast `12ff1500 XXXX` - wire-identical to the native
  `2026-01-26_brake_force_sweep`. The sw_id byte carries no protocol
  meaning: G Hub runs parallel client sessions on sw_ids `a`..`e` (plus
  `f` for DFU checks) and the wheel broadcasts with sw_id 0. Note these
  captures show brake-force writes accepted in DESKTOP mode too, so the
  native section's "onboard mode only" restriction deserves a re-check.
- Mode-change broadcasts: when the wheel transitions between onboard profiles (via the OLED) or between onboard and desktop modes (when Windows G Hub takes / releases control), the wheel may emit an unsolicited notification on EP 0x82. Not yet captured or consumed.

**Evidence**: `dev/captures/2026-04-26_compat_ghub_init.pcapng` (full GHUB bring-up enumeration); `..._compat_range_ghub_slider_{desktop,onboard}.pcapng` (steering angle 90→1080°); `..._compat_range_strength_slider_{desktop,onboard}.pcapng` (FFB strength 0→100%); `..._compat_range_damping_only_desktop.pcapng` (isolated damping sweep, source for the 0x14/fn=1 wiring); `..._compat_range_filter_only_desktop.pcapng` (isolated filter sweep, source for the 0x1a/fn=2 wiring and proof that "force_desktop_mode" was actually the filter setter).

### 5.2 Compatibility Mode Behavior That Is NOT a Driver Bug

A few behaviors observed in compat mode look like driver problems but are firmware-side defaults verified to match Windows GHUB on the same wheel firmware. Listed here so future readers do not re-investigate them:

- **Default centering spring**: in compat mode the wheel applies its own self-centering spring whenever it is in onboard mode and no game / host-side FFB is actively writing. This is the same on Windows with GHUB running. There is no known host command to disable it; users see it as "the wheel won't stop pushing back to center" when nothing is talking to the wheel.
- **Default steering angle is 90°**: the factory default angle in compat mode is 90°, not the wheel's 1080° hardware maximum. This is correct firmware behavior. Set it from Linux via `wheel_profile=0` then `wheel_range=<degrees>`, or from the OLED by editing the active onboard profile.
- **LIGHTSYNC works in compat mode**: feature `0x807A` is advertised in compat at the same index discovery picks up in native, and `wheel_led_*` writes drive the LED strip end-to-end (verified 2026-04-29). An earlier draft of this document claimed otherwise; that claim was incorrect.

### 5.3 Sub-device Addressing (HID++ dev_idx 0x01 / 0x02 / 0x05)

Cross-capture census (2026-07-02, all 62 captures): besides the wheel
base at dev_idx `0xff` (~100k packets each way), three sub-devices carry
real HID++ traffic on BOTH the RS50 and the real G Pro - dev `0x01`
(~390 packets each way), dev `0x02` (~560), dev `0x05` (~420). No other
sub-index appears. Each has its own feature catalog, enumerated by G Hub
on every init:

- **dev `0x01`** - display / rim module: `0x8091` (per-key/LED matrix),
  `0x18A2`, `0x9315`.
- **dev `0x02`** - pedal base: `0x80A4` (axis response curves, 3 axes
  with HID usages 0x31/0x33/0x32), `0x80D0`, **`0x8134` Brake Force**,
  `0x8135`, `0x9209`/`0x9215`. G Hub calls `0x80A4` fn0/fn1/fn9 here on
  every init. Pedal-curve and brake-force support for the G Pro's
  modular pedals should target this dev_idx.
- **dev `0x05`** - motor / base unit: `0x8128`, `0x8129`, `0x812B`,
  **`0x812C` centre calibration**, `0x92D1`. The whole 0x812x
  calibration cluster lives here; live calibrate traffic confirmed on
  both wheels (`2026-04-22_re_calibrate` on RS50: `10050f1a` read
  position -> `10050f3a` write centre; same pattern at index 0x0c on the
  G Pro contributor captures). The driver's `wheel_calibrate*` already
  targets this dev_idx.
- **dev `0x04` - RS Shifter & Handbrake accessory** (hardware-verified
  2026-07-28, RS50 only; not part of the 2026-07-02 census because the
  accessory was not attached during it). An optional accessory in the
  base's USB-A port, not a separate USB device: only a HID++ sub-device
  plus fields already reserved in the base's interface-0 input report
  (evdev `BTN_TOP2`/`BTN_PINKIE`/`BTN_THUMB2`/`ABS_Z`, section 5.1's
  input mapping table). IFeatureSet at idx `0x01`, count 18 (HID++
  4.2). Names: `0x0005` reports **"RS Shifter & Handbrake"**, device
  type byte `0x1a` (not in any known registry); `0x0007`
  DeviceFriendlyName reports **"RS Handbrake & Shifter"** (reversed word
  order) and is accessory-only, absent on every other sub-device.
  `0x0003` DeviceInformation: 3 entities, own serial-number capability
  bit set, main firmware "MPO 13.00.B0013" on its own SecureDFU
  (`0x00C3`). Full feature list: core `0x0001`, `0x0003`, `0x0005`,
  `0x0007`, `0x00C3`; unknown/engineering `0x1602`, `0x1802`
  (DeviceReset, do not call), `0x1807` (likely
  ConfigurableProperties), `0x180B`, `0x18B1`, `0x1BC0` ReportHidUsages,
  `0x1E00` EnableHiddenFeatures (disabled), `0x1E02`
  ManageDeactivatableFeatures, `0x1EB0`, `0x9315`; and three public,
  non-core, non-DFU features that are exactly the surface a "3 sliders +
  mode" G HUB accessory page needs:
  - **`0x80A4` AxisResponseCurve** - the accessory's own store, 1 axis
    (HID usage `0x32` = Z, same usage as the base's handbrake axis 4),
    0/64 points loaded (built-in linear response active). Same fn0/fn1
    wire layout as every other `0x80A4` store (section 5.1); the driver
    uses this axis count to identify the accessory during discovery
    (`HIDPP_DD_SHIFTER_AXIS_COUNT`).
  - **`0x80B1` BANDED_AXIS** - unique to this sub-device across the
    whole wheel (verified absent on `0xff`/`0x01`/`0x03`/`0x05`), so the
    driver uses `Root.getFeature(0x80B1)` as the accessory confirmation
    test. Name is first-party (Solaar commit b9e0cf8, sourced from LGHUB
    itself; listed in Solaar's "racing peripherals" group).

    **Decoded 2026-07-28** from G Hub captures. This is the actuation-point
    store behind the Shift Sensitivity and Handbrake Actuation sliders.

    A band is addressed by `[group][band]`, and carries a signed 32-bit
    position on the lever's travel:

    | fn | direction | params | meaning |
    |---|---|---|---|
    | `fn0` | read | - | `[groups][?]` - 2 groups on this accessory |
    | `fn1 [group]` | read | | group info; byte 5 is the band count |
    | `fn2 [group][band]` | read | | `[group][band][value s32-LE]` |
    | `fn3 [group][band][value s32-LE]` | write | | set the band |

    Group 0 is the sequential shifter and holds **two** bands, one per shift
    direction; group 1 is the digital handbrake and holds **one**. Asking for
    a band that does not exist (e.g. `(1,1)`) returns error `0x2a`.

    G Hub's scales, both linear in the 1-100 its UI accepts (it refuses 0):

    - **shift**: `+/- pct * 0.008` of full scale, written to BOTH of group 0's
      bands, symmetric about centre. The slider therefore spans 80% of the
      lever's travel, not all of it.
    - **handbrake**: `pct * 944.64` in the same units, negative, group 1
      band 0.

    **G Hub overflows here.** The handbrake scale exceeds 16 bits above about
    69%, and G Hub truncates: it writes 75% as `5312` rather than `70848` -
    a point far SHORTER than 50%. That wraparound is what makes a captured
    sweep look non-monotonic and undecodable until you spot it. The wheel
    itself accepts the full 32-bit value (`-94464` stored and read back
    verbatim), so this driver writes the true value and stays monotonic;
    above ~69% our behaviour and G Hub's will not agree.

    The firmware validates nothing - it stored `+/-2000000000` without
    complaint - so there is no range to discover by probing, and a bad value
    is not rejected.
  - **`0x1B30` DEVICE_MODE** - also unique to this sub-device. Same
    first-party naming source as `0x80B1`.

    **Decoded 2026-07-28**: it reports the physical mode switch's current
    position. `fn2` (no params) returns the mode in its first byte; `fn0`
    returns a static `1a 03 01` on this firmware and is NOT the mode.
    `fn1` and `fn3` return errors.

    | value | switch position |
    |---|---|
    | `0` | sequential shifter (LEFT) |
    | `2` | digital handbrake (MIDDLE) |
    | `1` | analog handbrake (RIGHT) |

    Note the ordering: middle and right are **2 and 1**, not 1 and 2. It
    was measured by moving the switch through all three positions while
    watching `fn2`, precisely because assuming left-to-right numbering is
    the obvious guess and would have been wrong.

    A caution for anyone probing this feature: match a reply on the
    function/sw byte as well as the feature index. Matching on the feature
    index alone hands an `fn2` reply back as the answer to an `fn0` request,
    which is what first made `fn0` look like it was changing with the
    switch.

  The driver discovers this sub-device, resolves the three feature
  indices, exposes presence via `wheel_accessory`, and drives `0x80B1`
  through `wheel_shift_actuation` and `wheel_handbrake_actuation`.
  `0x1B30` is read through `wheel_accessory_mode`. See `docs/SYSFS_API.md`.

  The accessory's own `0x80A4` store is deliberately NOT used: G Hub writes
  the handbrake curve there, but on this hardware a curve written to the
  accessory has no observable effect, while the same curve on base axis 4
  bends `ABS_Z` immediately (both tested 2026-07-28 with the unit confirmed
  in analog-handbrake mode). So `wheel_handbrake_curve` targets the base.
  "G Hub writes somewhere else" and "our target is wrong" are different
  claims, and only the second justifies a change.

**A sub-device index belongs to a physical port, not to a device type.**
The base has two USB-A ports, and the SAME accessory reports at dev `0x04`
on the accessory port and at dev `0x03` on the pedals' port
(hardware-verified 2026-07-28: with the pedals unplugged and the accessory
in their port, `0x03` was the accessory and no 3-axis sub-device existed at
all). On this wheel the pedal MCU answers at `0x03` whether or not the
accessory is attached. The index is the physical port, not the device type.

Two consequences, both enforced in the driver: never key on an index to
identify a device, and never skip an index because another device was once
found there - a rescan optimisation that skipped the pedals' index made the
accessory permanently undiscoverable on the second port. The
driver never hardcodes a sub-device index for either device: it probes
a small candidate list (`0x02, 0x03, 0x01, 0x04, 0x05, 0x06`) and
classifies whatever answers by feature content (axis count for
`0x80A4`, then `0x80B1` confirmation for the accessory), in a single
pass shared by both discoveries. A future firmware or attach order could
place either device at any of these indices, or move the pedal MCU
somewhere the current candidate list does not cover - if that ever
happens, the fix is to extend the candidate list, not to special-case an
index.

Two driver-relevant quirks:

- **Sub-device responses arrive as report ID `0x11`**, not the `0x12`
  the wheel base uses for its responses.
- **Feature ID `0x0009`** (unregistered; index `0x08` on RS50, `0x06` on
  G Pro, at dev `0xff`) looks like the sub-device topology/presence
  feature: G Hub calls its `fn2` (empty) and the wheel answers with a
  burst of unsolicited events `12ff0800 [unit][flags] 14` for units
  01/02 (flags 0xf0), 03/04 (0x60), 05 (0xc0) - likely how G Hub learns
  which dev_idx values exist. Units 3-4 are announced but never
  addressed via HID++ in any capture.

To enumerate a sub-device's catalog on a live wheel from Linux (no
Windows capture round-trip needed), the `hidpp-list-features` tool from
cvuchener/hidpp works over hidraw and takes a `--device-index` option -
useful for verifying the dev `0x02` pedal-base map on contributors' G
Pro hardware.

---

## 6. Initialization Sequence

Recommended initialization order:

1. **USB Enumeration** - Get device descriptors
2. **Claim Interfaces** - Claim interfaces 0, 1, 2
3. **Feature Discovery** - Use IRoot (index 0x00) to find feature indices
4. **Read Settings** - Query current rotation, FFB gain
5. **Start FFB Loop** - Begin sending force commands (the driver runs a
   fixed 1 kHz timer). It is an hrtimer, programmed against the clock
   hardware, so the period is the one asked for and `CONFIG_HZ` does not
   enter into it. It was a jiffies timer until 0.30.0, which could not
   deliver this rate: the timer wheel never fires a timer early, so a
   callback re-arming itself for the next jiffy always landed on the one
   after, giving 2 ms where HZ is 1000 and 8 ms where HZ is 250. The
   nominal 500 Hz of earlier releases was really 333 Hz for the same
   reason

---

## 7. Driver Implementation Notes

### Linux Kernel Driver

1. **Multi-Interface Device**: Claim interface 2 for FFB, interface 0 for input
2. **FFB via hid_hw_output_report()**: Send 64-byte reports to interface 2
3. **Workqueue**: FFB must be sent from process context
4. **Full software effect pipeline**: The wheel firmware only understands raw constant forces on endpoint 0x03, but the driver emulates the complete Linux FFB effect set on top of it (FF_CONSTANT, FF_RAMP, FF_PERIODIC with SINE/SQUARE/TRIANGLE/SAW_UP/SAW_DOWN, and the four condition effects FF_SPRING, FF_DAMPER, FF_FRICTION, FF_INERTIA). Condition effects read the live wheel state (position from interface-0 byte 4-5, with velocity and acceleration derived at the timer tick) and apply the standard Linux `ff_condition_effect` formula; waveform and envelope semantics match `Documentation/input/ff.rst`.

### Timer-Based Force Updates

The driver uses a 500Hz timer to send continuous force commands while any effect is active. The RS50 requires continuous commands to maintain force (unlike some wheels that hold state). Each tick walks the active-effect slots, sums each effect's instantaneous contribution, applies `FF_GAIN`, and sends a single net force value. The timer keeps running whenever any effect is playing, even if the current net force happens to be zero (a SPRING at exact centre or a DAMPER with a stationary wheel) so condition effects produce force the moment the wheel moves.

### Settings Query on Init

The driver queries current device settings via HID++ on initialization:
- Rotation range, FFB strength, damping, TRUEFORCE, brake force
- FFB filter level and auto mode
- LED brightness

This ensures sysfs values reflect actual device state.

### C Structure Definition

```c
#define HIDPP_DD_FF_REPORT_ID       0x01
#define HIDPP_DD_FF_EFFECT_CONSTANT 0x01
#define HIDPP_DD_FF_REPORT_SIZE     64

struct hidpp_dd_ff_report {
    u8 report_id;       /* 0x01 */
    u8 reserved[3];     /* 0x00, 0x00, 0x00 */
    u8 effect_type;     /* 0x01 = constant force */
    u8 sequence;        /* 0x00-0xFF, wraps */
    __le16 force;       /* 0x0000=left, 0x8000=center, 0xFFFF=right */
    __le16 force_dup;   /* duplicate of force value */
    u8 padding[54];     /* zeros */
} __packed;

static_assert(sizeof(struct hidpp_dd_ff_report) == HIDPP_DD_FF_REPORT_SIZE,
              "RS50 FFB report structure size mismatch");
```

### Force Value Conversion

```c
static u16 rs50_force_to_offset_binary(s32 force)
{
    return (u16)((s32)signed_force + 0x8000);
}
```

### Linux Sysfs Interface

The driver exposes all runtime settings (FFB, rotation range, LIGHTSYNC,
onboard profiles, pedal curves and deadzones, Oversteer compatibility
shims, optional HID++ debug shell) as sysfs attributes under
`/sys/class/hidraw/hidrawX/device/`.

The authoritative reference for the full attribute surface (types,
ranges, read/write semantics, availability per wheel, onboard vs desktop
mode differences) is [`SYSFS_API.md`](SYSFS_API.md). This spec does not
re-enumerate it.

---

## 8. Differences from G920/G923

> This section describes the G920 and the G923 Xbox edition, both of which
> use the HID++ 0x8123 path below. The G923 PlayStation edition (`c266`/
> `c267`) is a different case: this driver now supports it directly via a
> ported classic force-feedback engine (plain HID output reports, the same
> wire format as G29/G920's in-kernel `hid-logitech` driver) - neither
> 0x8123 nor the RS50/G PRO TrueForce-stream path documented below drives
> its force feedback. The PS G923 does still carry the same interface-2
> TrueForce stream transport as the DD wheels (confirmed by TF4ALL's
> Windows captures, issue #20, and on hardware 2026-07-26): the driver
> exposes that interface as a hidraw node and `logi-tf-sim` streams
> simulated TrueForce over it. See the G923 section in the README and
> CHANGELOG, and the coverage note in
> [`TRUEFORCE_PROTOCOL.md`](TRUEFORCE_PROTOCOL.md).

| Feature | G920/G923 | RS50 |
|---------|-----------|------|
| FFB Method | HID++ Feature 0x8123 | Dedicated endpoint 0x03 |
| FFB Commands | Via HID++ FAP messages | Raw HID output reports (05 XX) |
| Report ID | 0x11/0x12 | 0x01 (custom) |
| Sequence Field | 2 bytes | 1 byte |
| Max Rotation | 900° | 2700° |
| Motor Type | Gear | Direct drive |
| FFB refresh keepalive | Not needed | Not needed |
| USB Interfaces | Unified HID++ | 3 separate (joystick, HID++, FFB) |

### Architecture Note

The RS50/G PRO expose **two independent force transports**, and which one a
host uses is a choice, not a hardware constraint (established 2026-07-04 by
the TF4ALL project's Windows-side kernel-filter captures, cross-checked
against our catalog):

- **HID++ Feature 0x8123 fn2** - the path Logitech's own Windows runtime
  uses for normal game FFB on these wheels too, not just on G920/G923:
  report 0x11/0x12, device index 0xff, the wheel's 0x8123 feature *index*
  (native RS50 catalog: 0x10; G-PRO-PID catalog: 0x0e), function 2, signed
  int16 BE motor target at payload offset 10-11, sent at the game's rate
  (~140-333 Hz observed). Set-and-hold: the wheel maintains the last
  commanded force indefinitely with no keepalive.
- **Dedicated endpoint 0x03 on Interface 2** (raw report-0x01 packets) -
  the TrueForce session channel, used by SDK-native games (AC EVO, ACC)
  and by this Linux driver for ALL force output. While a TrueForce session
  is active, bytes 6-9 of the stream packet ("cur") are the motor torque
  target and OVERRIDE the 0x8123 path; the 13-slot window plays additively
  on top as audio. A packet with zero new samples (byte 10 = 0) is a pure
  force command - this driver's classic "KF" packet.

The G920/G923 comparison above therefore describes *defaults*, not
capabilities: the G920/G923 has only 0x8123, while the DD wheels have both
and privilege the endpoint stream.

**Driver implication**: this driver uses the endpoint path exclusively
(`HIDPP_QUIRK_DD_FFB` / `hidpp_dd_ff_*`), which needs the 1 kHz refresh
semantics below; the G920 `hidpp_ff_*` 0x8123 code path with its slot-based
effect engine still cannot be reused as-is (the issue #8 queue-saturation
failures), but a minimal 0x8123-fn2 force-target sender remains an untested
alternative transport - potentially relevant if the wheel's FFB-filter
smoothing turns out to apply only to that path.

**Interface initialization**: FFB must be initialized on Interface 1 (HID++), not Interface 0 (joystick). Interface 0 has no HID++ support.

---

## 9. LIGHTSYNC RGB LED Control (Feature 0x807A)

The RS50 has a horizontal strip of **10 individually addressable RGB LEDs**
across the upper section of the faceplate, used as an engine-RPM / shift
indicator. The LIGHTSYNC feature provides per-LED colour control plus a
direction setting for the built-in sweep effects.

> **This section describes RS50 rim hardware only** (in both native and
> compat enumeration - the rim does not change with the PID). The **real
> G PRO rim** uses the same 0x807A feature page for a completely different,
> LEVEL-based rev-light protocol with no per-LED RGB. The direct-drive
> rev path, decoded from G HUB's own live captures (2026-08-14): at
> attach time the host performs **discovery reads only** - SHORT fn0,
> fn1, fn2, fn0, once - and then the level stream is a bare repeated
> pair: SHORT fn2 GET_STATE + LONG fn6 with params
> `00 01 00 0a 00 LEVEL` (level 0-10). **No arming exists on this
> path**: fn3, fn4 and fn7 are NEVER used, and from the onboard
> power-on state the strip renders bare fn2+fn6 levels exactly
> (hardware-proven; the earlier "must be fn3-armed or it refuses every
> level" belief is disproven). Colours, direction and scaling belong to
> the wheel's onboard profile. Exposed by this driver as
> `wheel_rev_level`, with the RGB attributes hidden on real G PROs.
> Caution from the TF4ALL project's original decode: bursting these
> writes starves the wheel's shared HID++ command processor and cuts FFB
> out on the Windows FFB path, so level writes are paced. The ~160 ms
> figure quoted from that source is wrong: a first-party G HUB capture
> (issue #20, iRacing) measures ~16.5 ms per pair, about 60 Hz. The driver
> enforces a ~10 ms floor.
>
> **G923 editions only: the display has to be switched on before it
> accepts a level** (hardware-verified 2026-08-11, both G923 editions).
> On those wheels `fn2` reports the live effect, and a wheel answering
> **0** is displaying nothing: in that state every `fn6` level is
> refused with HID++ error 5, `LogitechInternal`, until an
> `fn3 param 0x02` starts the display, after which the identical level
> is accepted. This is a G923 quirk, not part of the direct-drive
> path: the DD wheels need no enable call (see above), and the G923
> editions keep their own separate arming code in the driver. `fn3` is
> not a boolean switch either way: values 1-4 and 6-9 are the built-in
> sweeps and the owner's custom slots, which a careless `fn3` would
> overwrite.
>
> **`fn0` reports the strip length** in its second parameter: `0x0a` on the
> direct-drive wheels, `0x05` on a G923. The level is a fraction of that
> length, so a wheel told the wrong length shows the wrong fill.
>
> **RS50 accepts the same level command (hardware-verified 2026-07-20,
> re-verified with the bare fn2+fn6 form 2026-08-14):** writes to
> `wheel_rev_level` visibly drive the RS50's 10-LED strip too -
> 0 = all dark, 10 = all lit, intermediate values a partial fill. The
> display renders **center-out by default**, so the 10-LED strip shows
> 5 mirrored visual steps across a 0-10 level sweep. The display
> pattern is a separate concern from the level protocol: the level says
> how much of the bar is lit, the wheel's stored display config
> (colours, direction) says how that amount is drawn. Reconfiguring the
> pattern mid-session via `0x807B` is NOT part of G HUB's mid-session
> vocabulary and is untested during live TrueForce sessions, so it must
> not be assumed safe there. The arm-semantics question this note once
> deferred to a future capture is settled: there ARE no arm semantics on
> the direct-drive path (see the attach-time discovery reads above). A
> written level holds until the next level write; writing
> `wheel_led_effect` restores the idle pattern.
>
> **Historical: the old arm burst stomped the active effect
> (hardware-verified 2026-07-20).** When the driver still sent an arm
> burst, its "fn3 param 0x02" was a plain SET_EFFECT that force-switched
> the wheel to effect 2 (Outside-In), so the driver had to re-assert the
> pre-arm effect afterwards. The burst is gone from the direct-drive
> path (2026-08-14, see above) - and had to go: sent during a native
> TrueForce SDK session it killed the session at init and killed wheel
> input mid-session, while the bare fn2+fn6 form coexists with a live
> session (see section 12.5). This paragraph is kept only to explain
> older captures and driver versions.

### 9.1 Physical LED Layout

```
[LED1] [LED2] [LED3] [LED4] [LED5] [LED6] [LED7] [LED8] [LED9] [LED10]
```

LEDs are numbered 1-10, left to right across the faceplate strip.
The protocol uses **reversed order** - see section 9.4.

### 9.2 Feature Discovery

The LIGHTSYNC feature uses Page ID `0x807A`. The actual feature index varies by device/firmware
and must be discovered at runtime:

```
Query: 10 FF 00 0X 80 7A 00    (ROOT.getFeatureIndex for page 0x807A)
Reply: 12 FF 00 0X [index] 00 00...
```

Typical indices observed: `0x0B` or `0x0C`

### 9.3 HID++ Function Codes

LIGHTSYNC uses multiple report types depending on the function:

| Function | Code  | Report Type | Description |
|----------|-------|-------------|-------------|
| Get RGB Zone Config | `0x1C` | 0x12 (Very Long) | Read slot configuration |
| Set RGB Zone Config | `0x2C` | 0x12 (Very Long) | Write slot configuration |
| Set Effect Mode | `0x3C` | 0x10 (Short) | Select effect 1-9 (5-9 = custom slots); stages, fn6 commits |
| Get Zone Name | `0x3C` | 0x12 (Very Long) | Read custom slot name |
| Set Zone Name | `0x4C` | 0x12 (Very Long) | Write custom slot name |
| **Enable LED Subsystem** | **`0x6C`** | **0x11 (Long)** | **REQUIRED before LEDs work** |

**Function Code Format**: The code is `(function_number << 4) | 0x0C`
- Function 1 (GET CONFIG): `0x10 | 0x0C` = `0x1C`
- Function 2 (SET CONFIG): `0x20 | 0x0C` = `0x2C`
- Function 3 (EFFECT/NAME): `0x30 | 0x0C` = `0x3C` (dual use based on report type)
- Function 4 (SET NAME): `0x40 | 0x0C` = `0x4C`
- Function 6 (ENABLE): `0x60 | 0x0C` = `0x6C`

### 9.3.1 Enable LED Subsystem (Function 0x6C)

G Hub sends this command during LED control operations. The driver uses fn7 (0x7C) instead for the enable/refresh step.

**Request Format (LONG report 0x11, 20 bytes):**
```
Byte    Field           Description
----    -----           -----------
0       Report ID       0x11 (long)
1       Device Index    0xFF
2       Feature Index   [discovered LIGHTSYNC index]
3       Function        0x6C (Enable LED subsystem)
4       Unknown         0x00
5       Enable          0x01 (enable LEDs)
6       Unknown         0x00
7       Num LEDs        0x0A (10 LEDs)
8-19    Padding         0x00
```

**Example from G Hub capture:**
```
11 FF 0B 6C 00 01 00 0A 00 00 00 00 00 00 00 00 00 00 00 00
```

### 9.3.2 Detailed Function Reference (Empirical - 2026-01-30)

Both LIGHTSYNC features (0x807A and 0x807B) share a similar function numbering scheme.
The following tables document what we've observed from driver testing and G Hub captures.

**HID++ Function Code Encoding:**
The function byte encodes: `(function_number << 4) | SW_ID`
- SW_ID = 0x0C for SHORT reports, 0x0D for responses/LONG reports
- Example: fn3 on SHORT = `(3 << 4) | 0x0C` = `0x3C`

#### Feature 0x0B (Page 0x807A) - LIGHTSYNC Effect Control

| fn# | Code | Name (Confirmed) | Request Params | Response Data | Notes |
|-----|------|------------------|----------------|---------------|-------|
| 0 | 0x0C | GET_INFO | (none) | `01 0a 0a 00 00 00` | Version=1, LEDcount=10, LEDcount=10 |
| 1 | 0x1C | GET_CAPS | (none) | `00 02 01 03 04 05 06 07 08 09` | Capability flags or effect IDs |
| 2 | 0x2C | GET_STATE | (none) | `05 00 00 00` | Current effect (>= 5 means custom slot `effect - 5` is selected) |
| 3 | 0x3C | SET_EFFECT | `[mode] 00 00` | `[mode] 00 00` | Set effect 1-9 (stages; fn6 commits) |
| 4 | 0x4C | Unknown | `00 0a 00` | - | Context-dependent; works during config only |
| 5 | 0x5C | Unknown | `[index]` | `[index] 00 00` | Context-dependent; echoes param during config |
| 6 | 0x6C | PRE_CONFIG/COMMIT | 16-byte LONG | - | Required before/after RGB changes |
| 7 | 0x7C | ENABLE | `00 00 00` | - | Refreshes LED display |
| 8+ | 0x8C+ | - | - | ERROR 7 | Not supported |

> **Note**: fn4 and fn5 are context-dependent - they succeed during LED configuration
> but return error 5 (ERR_COUNT) at other times. Possibly related to config mode state.

**fn0 Response Breakdown:**
```
01 0a 0a 00 00 00
 │  │  │
 │  │  └── LED count again (10)
 │  └───── LED count (10)
 └──────── Version or flags (1)
```

**fn1 Response Breakdown:**
```
00 02 01 03 04 05 06 07 08 09
 │  │  │  │  │  │  │  │  │  └── Unknown (zone 9?)
 │  │  │  │  │  │  │  │  └───── Unknown (zone 8?)
 │  │  │  │  │  │  │  └──────── Unknown (zone 7?)
 │  │  │  │  │  │  └─────────── Unknown (zone 6?)
 │  │  │  │  │  └────────────── Unknown (zone 5?)
 │  │  │  │  └─────────────────  Unknown (zone 4?)
 │  │  │  └────────────────────  Unknown (zone 3?)
 │  │  └───────────────────────  Unknown (zone 2?)
 │  └──────────────────────────  Unknown (zone 1?)
 └─────────────────────────────  Unknown flags
```
Interpretation: May enumerate effect types or zone capabilities (appears to be zone IDs 1-9).

**fn2 Response Breakdown:**
```
05 00 00 00
 │
 └── Current effect mode (5 = CUSTOM 1 / slot 0 selected)
```

**Effect Mode Values (fn3 parameter, decoded 2026-07-20):**
| Value | Effect | G Hub Name |
|-------|--------|------------|
| 0x01 | Inside→Out animation | FRÅN INSIDAN UT |
| 0x02 | Outside→In animation | FRÅN UTSIDAN IN |
| 0x03 | Right→Left animation | HÖGER TILL VÄNSTER |
| 0x04 | Left→Right animation | VÄNSTER TILL HÖGER |
| 0x05 | CUSTOM 1 (renders custom slot 0) | the slot's name |
| 0x06 | CUSTOM 2 (renders custom slot 1) | the slot's name |
| 0x07 | CUSTOM 3 (renders custom slot 2) | the slot's name |
| 0x08 | CUSTOM 4 (renders custom slot 3) | the slot's name |
| 0x09 | CUSTOM 5 (renders custom slot 4) | the slot's name |

> **Effect values 5-9 ARE the five custom slots** - there is no separate
> "activate slot" command; selecting a slot IS selecting its effect
> number, staged by fn3 and repainted by the zero-parameter fn6 commit.
> Decoded from `2026-01-30_desktop_led_colors.pcapng` (every G Hub
> dropdown switch to CUSTOM N sends fn3 `[0x04 + N]` and reads back slot
> `N - 1`'s config and name: frames 219/339/715 and 279/525/775) and
> hardware-confirmed live (fn3 `0x08` + fn6 repainted the strip to
> slot 3). Earlier revisions misread 0x05 as a lone "Static/Custom" mode
> and called 6-9 unlabeled. Corollary: fn2 GET_STATE returns the selected
> custom slot as `effect - 5` when the value is >= 5.

#### Feature 0x0C (Page 0x807B) - RGB Zone Config

| fn# | Code | Name (Confirmed) | Request Params | Response Data | Notes |
|-----|------|------------------|----------------|---------------|-------|
| 0 | 0x0C | GET_INFO | (none) | `05 05 08 01 0a` | Slots=5, ?, maxNameLen=8, ?, LEDs=10 |
| 1 | 0x1C | GET_SLOT_CONFIG | `[slot]` | `[slot] [type] [RGB...]` | Returns slot's RGB colors (partial) |
| 2 | 0x2C | SET_SLOT_CONFIG | 64-byte RGB | - | Write RGB colors (VERY_LONG report) |
| 3 | 0x3C | GET_NAME | `[slot]` | `[slot] [len] [name...]` | Get slot name (a pure read; earlier revisions mislabeled it "activate slot") |
| 4 | 0x4C | SET_NAME | `[slot] [len] [name]` | - | Set slot name (confirmed working) |
| 5+ | 0x5C+ | - | - | ERROR 7 | Not supported |

> **IMPORTANT**: fn6/fn7 do NOT exist on feature 0x0C! They only work on 0x0B.
> Sending fn6/fn7 to 0x0C returns HID++ error 7 (ERR_INVALID_FUNCTION_ID).

**fn0 Response Breakdown:**
```
05 05 08 01 0a
 │  │  │  │  │
 │  │  │  │  └── LED count (10)
 │  │  │  └───── Unknown (1)
 │  │  └──────── Max name length (8 characters)
 │  └─────────── Number of slots (5)
 └────────────── Number of slots (5, duplicated)
```

**fn1 Response (GET_SLOT_CONFIG) - Returns actual RGB data:**
```
Slot 0: 00 03 ff ff ff ff ff ff ff ff ff ff ff ff ff ff
         │  │  └──────────── LED colors (RGB triplets, partial view)
         │  └─────────────── Type (0x03)
         └────────────────── Slot index

Slot 1: 01 03 00 ff 00 00 ff 00 00 ff 00 00 ff 00 00 ff  (green/cyan pattern)
Slot 2: 02 03 ff 00 00 ff 00 00 ff 00 00 ff 00 00 ff 00  (red pattern)
Slot 3: 03 03 00 00 ff ff ff 00 00 00 ff ff ff 00 00 00  (blue/yellow pattern)
Slot 4: 04 04 00 ff 00 00 ff 00 ff 00 00 ff 00 00 ff 00  (type=4, green/cyan)
```
Note: Response shows first ~4-5 LEDs in 16-byte SHORT response. Full RGB data requires VERY_LONG report.

**fn3 Response (GET_NAME) - Returns slot name:**
```
Slot 0: 00 00 00 00 00 00 00 00 00 00...  (empty/unnamed)
Slot 1: 01 08 43 55 53 54 4f 4d 20 32...  = "CUSTOM 2" (len=8)
Slot 2: 02 08 43 55 53 54 4f 4d 20 33...  = "CUSTOM 3" (len=8)
Slot 3: 03 08 43 55 53 54 4f 4d 20 34...  = "CUSTOM 4" (len=8)
Slot 4: 04 08 43 55 53 54 4f 4d 20 35...  = "CUSTOM 5" (len=8)
         │  │  └───────────────────────── ASCII name string
         │  └──────────────────────────── Name length
         └─────────────────────────────── Slot index
```

**fn4 (SET_NAME) - Confirmed working:**
```
To set slot 0 name to "TEST": 0c 4c 00 04 54 45 53 54
                               │  │  │  │  └───────── ASCII "TEST"
                               │  │  │  └──────────── Length (4)
                               │  │  └─────────────── Slot (0)
                               │  └────────────────── fn4 code
                               └───────────────────── Feature 0x0C
```

#### G Hub Init Sequence (from ghub_init_wheel_on.pcapng)

When G Hub starts with wheel already on, it queries feature 0x0B:
```
1. 10 FF 0B 0C 00 00 00   - fn0: GET_INFO
2. 10 FF 0B 1C 00 00 00   - fn1: GET_CAPS
3. 10 FF 0B 2C 00 00 00   - fn2: GET_STATE
4. 10 FF 0B 4C 00 0A 00   - fn4: SET_LEDS (param = LED count?)
5. 10 FF 0B 7C 00 00 00   - fn7: REFRESH
```

> **Correction (2026-08-14)**: current G HUB captures show **neither
> fn4 nor fn7 on the rev-light path**. The attach-time reads on that
> path are fn0, fn1, fn2, fn0, and nothing else (see the section 9
> intro). The listing above is kept as recorded from its own capture;
> treat fn4/fn7 as belonging to the RGB slot-upload flow, not to rev
> levels.

#### G Hub Color Change Sequence (from lightsync_custom_save.pcapng)

When user applies new custom colors:
```
1. 10 FF 17 0C ...        - Profile query (feature 0x8137)
2. 10 FF 09 0C 00 03 00   - Sync call (feature 0x1BC0)
3. 10 FF 0B 3C 05 00 00   - Select slot 0's effect (feature 0x0B fn3, 0x05 = CUSTOM 1)
4. 11 FF 0B 6C ...        - Pre-config (feature 0x0B fn6, LONG report)
5. 12 FF 0C 2C ...        - 64-byte RGB data (feature 0x0C fn2)
6. 11 FF 0B 6C ...        - Commit (feature 0x0B fn6, LONG report)
7. 10 FF 0C 3C 00 00 00   - Read back slot 0's name (feature 0x0C fn3 GET_NAME - a read, not an activate)
8. 10 FF 0B 7C 00 00 00   - Enable/refresh display (feature 0x0B fn7)
```

(Earlier versions of this section had the pre-config / commit / enable
steps listed against feature 0x0C; they actually live on 0x0B. The
summary table at the end of this section was already correct.)

#### Key Discovery: First Update Works, Subsequent Updates Don't

Testing revealed that:
1. **First color change after driver init**: LEDs display correctly
2. **Second and subsequent changes**: HID++ commands succeed but LEDs don't update

This suggests the device has internal state that gets "consumed" on the first update.
G Hub may reset this state via the init sequence (fn0→fn1→fn2→fn4→fn7) before each change,
or there's a cold-start initialization we haven't captured yet.

### 9.4 Set RGB Zone Config (Function 0x2C) - VERIFIED

This is the primary command to configure LED colors and animation direction.

**Request Format (64 bytes):**
```
Byte    Field           Description
----    -----           -----------
0       Report ID       0x12 (very long)
1       Device Index    0xFF (wired)
2       Feature Index   [discovered, typically 0x0B or 0x0C]
3       Function        0x2C (Set RGB Zone Config)
4       Slot Index      0x00-0x04 (CUSTOM 1-5)
5       Direction       0x01-0x04 (animation direction wire value, see 9.4.1)
6-8     LED10 RGB       R, G, B (0x00-0xFF each)
9-11    LED9 RGB        R, G, B
12-14   LED8 RGB        R, G, B
15-17   LED7 RGB        R, G, B
18-20   LED6 RGB        R, G, B
21-23   LED5 RGB        R, G, B
24-26   LED4 RGB        R, G, B
27-29   LED3 RGB        R, G, B
30-32   LED2 RGB        R, G, B
33-35   LED1 RGB        R, G, B
36-63   Padding         0x00
```

> ⚠️ **CRITICAL: LED Order is REVERSED!**
> Protocol sends LED10 first (bytes 6-8) and LED1 last (bytes 33-35).
> Driver code must reverse user-facing LED1-10 to protocol order LED10-1.

**Slot Index Values:**
| Value | G Hub Name |
|-------|------------|
| 0x00 | CUSTOM 1 |
| 0x01 | CUSTOM 2 |
| 0x02 | CUSTOM 3 |
| 0x03 | CUSTOM 4 |
| 0x04 | CUSTOM 5 |

##### 9.4.1 Direction Values

Byte 5 carries a **1-based wire value (1-4)**, NOT the driver's 0-3
`HIDPP_DD_LIGHTSYNC_DIR_*` enum.
`dev/captures/2026-07-19_lightsync_direction.pcapng` contains only
device-to-host GET (fn1) echoes of the state G Hub had already applied for
each direction - no host-to-device 0x0C SET frames were captured. The wire
values below were derived from those GET echoes (GET/SET symmetry: a Get
Zone Config response carries the same byte 5 a Set would have sent),
corroborated by the old driver's `direction + 2` encoding being accepted by
the firmware for 2/3/4 and NAKed for the out-of-range 5:

| Wire (byte 5) | Effect | G Hub Swedish | G Hub English | Driver enum |
|---------------|--------|---------------|---------------|-------------|
| `0x01` | Inside to Outside (expand)  | FRÅN INSIDAN UT     | From Inside Out | 2 |
| `0x02` | Outside to Inside (contract) | FRÅN UTSIDAN IN     | From Outside In | 3 |
| `0x03` | Left to Right sweep          | VÄNSTER TILL HÖGER  | Left to Right   | 0 |
| `0x04` | Right to Left sweep          | HÖGER TILL VÄNSTER  | Right to Left   | 1 |

**Hardware-verified 2026-07-20** (RS50, live rev-fill sweeps): all four
driver-enum directions were watched on the physical strip and match the
table above exactly - enum 0 fills left to right, 1 right to left, 2 from
the centre outward, 3 from the edges inward. The verification used the
rev-level fill (0x807A fn2+fn6, see the rev-light note at the top of
section 9), which follows the SELECTED slot's stored
direction, so the same test confirmed both the wire mapping and the
fill/direction coupling. The 0x807A effect-select values are a separate
encoding entirely (1-4 = built-in animations, 5-9 = the custom slots;
see 9.3.2).

> **Historical bug:** an earlier driver encoded byte 5 as `enum + 2`
> (L->R=2, R->L=3, IO=4, **OI=5**). That both mislabelled the sweeps (its
> "Left to Right" actually sent Outside-In's value) and sent `5` for
> Outside-In, which is out of the device's 1-4 range - the firmware NAKs it
> and the write surfaces as `-EIO`. The driver now maps enum<->wire
> explicitly via `hidpp_dd_lightsync_dir_to_wire()` /
> `hidpp_dd_lightsync_wire_to_dir()`.

**Response Format:**
The device echoes back the configuration in a 0x12 response.

### 9.5 Get RGB Zone Config (Function 0x1C)

Read the current configuration for a slot.

**Request (64 bytes):**
```
Byte    Field           Description
----    -----           -----------
0       Report ID       0x12
1       Device Index    0xFF
2       Feature Index   [discovered]
3       Function        0x1C (Get RGB Zone Config)
4       Slot Index      0x00-0x04
5-63    Padding         0x00
```

**Response:** Same format as Set request (slot, direction, 10 RGB values).

### 9.6 Get/Set Zone Name (Functions 0x3C/0x4C)

Custom slots can have user-defined names (shown in G Hub UI).

**Get Name Request:**
```
12 FF [idx] 3C [slot] 00 00...
```

**Get Name Response:**
```
12 FF [idx] 3C [slot] [len] [ASCII name, null-padded]...
```

**Set Name Request:**
```
12 FF [idx] 4C [slot] [len] [ASCII name, null-padded]...
```

Name is up to 15 ASCII characters. Default names: "CUSTOM 1" through "CUSTOM 5".

### 9.7 Protocol Examples

**Example 1: Set CUSTOM 1 to Rainbow Pattern (Left→Right direction)**

User wants: LED1=Red, LED2=Orange, LED3=Yellow, LED4=Green, LED5=Cyan,
            LED6=Blue, LED7=Indigo, LED8=Violet, LED9=Pink, LED10=White

Protocol payload (after reversing LED order):
```
12 FF 0C 2C 00 03     Header: slot=0, direction=0x03 (Left->Right)
FF FF FF              LED10: White   (0xFFFFFF)
FF 69 B4              LED9:  Pink    (0xFF69B4)
8B 00 FF              LED8:  Violet  (0x8B00FF)
4B 00 82              LED7:  Indigo  (0x4B0082)
00 00 FF              LED6:  Blue    (0x0000FF)
00 FF FF              LED5:  Cyan    (0x00FFFF)
00 FF 00              LED4:  Green   (0x00FF00)
FF FF 00              LED3:  Yellow  (0xFFFF00)
FF 80 00              LED2:  Orange  (0xFF8000)
FF 00 00              LED1:  Red     (0xFF0000)
00 00 00 00...        Padding
```

**Example 2: Read CUSTOM 3 Configuration**

Request:
```
12 FF 0C 1C 02 00 00 00...   (slot index 0x02 = CUSTOM 3)
```

Response (device returns current config):
```
12 FF 0C 1C 02 03 ...RGB data...   (slot 2, direction 3)
```

### 9.8 LED Mirroring Based on Direction

For the two **symmetric** directions - **Inside-Out** (driver enum 2, wire
`0x01`) and **Outside-In** (driver enum 3, wire `0x02`) - G Hub collapses the
10 LEDs into **5 mirrored pairs**, so the colours are a palindrome about the
strip's centre:

- LED1 ↔ LED10 (outermost pair)
- LED2 ↔ LED9
- LED3 ↔ LED8
- LED4 ↔ LED7
- LED5 ↔ LED6 (innermost pair)

Painting one LED sets its mirror to the same colour (G Hub draws a bracket
linking the pair). Confirmed 2026-07-19 from G Hub screenshots. The two sweep
directions (Left-to-Right, Right-to-Left) leave all 10 LEDs independent.

This is application behaviour, not protocol-level enforcement - the wire
format still carries all 10 colours - but a UI that mirrors these directions
(as G Hub and `logi-wheel` do) must send a palindrome so the animation looks
right. The `wheel_led_colors` sysfs attribute always takes 10 colours.

### 9.9 Linux Driver Implementation

The driver exposes LIGHTSYNC control via sysfs attributes:

| Attribute | Type | Description |
|-----------|------|-------------|
| `wheel_led_slot` | R/W | Active slot (0-4). Writing applies that slot's config. |
| `wheel_led_direction` | R/W | Direction for current slot (0-3). |
| `wheel_led_colors` | R/W | All 10 LED colors as hex (see format below). |
| `wheel_led_apply` | W | Trigger to re-send current config to device. |
| `wheel_led_brightness` | R/W | Overall LED brightness (0-100%). |

**Color Format (wheel_led_colors):**
```
RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB
 LED1   LED2   LED3   LED4   LED5   LED6   LED7   LED8   LED9   LED10
```

Space-separated 6-digit hex values. Example:
```bash
# Set rainbow pattern
echo "FF0000 FF8000 FFFF00 00FF00 00FFFF 0000FF 4B0082 8B00FF FF69B4 FFFFFF" > wheel_led_colors

# Set all white
echo "FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF" > wheel_led_colors

# Read current colors
cat wheel_led_colors
```

**Driver Implementation Notes:**
1. Colors are stored in user-friendly order (LED1-10)
2. `rs50_lightsync_apply_slot()` reverses to protocol order before sending
3. Writes to `wheel_led_direction` or `wheel_led_colors` auto-apply
4. Each slot maintains independent direction and color config

### 9.10 Official Lineage: x8070 ColorLedEffects / x8071 RGBEffects

LIGHTSYNC 0x807A/0x807B has no public documentation, but Logitech's
published x8070 (ColorLedEffects) and x8071 (RGBEffects) specs are its
clear conceptual ancestors. The packet layouts do NOT carry over
(function numbering, effect ID enumerations, and the RS50's 64-byte
very-long RGB frames are all different), but four concepts map
directly and reframe open questions in this section:

- **SW/FW ownership arbitration.** x8070 fn8 `setSwControl` ("SW takes
  control of the color and the effect") and x8071 fn5
  `manageSwControl` ("disables all FW RGB clusters handling") are the
  official concept behind 0x807A's fn6 (pre-config) + fn7 (enable)
  being required before LED writes work. It also reframes the "First
  Update Works, Subsequent Updates Don't" mystery of 9.3.2: if fn6 is
  an ownership grab the firmware later reclaims, re-arbitrating before
  every update - exactly what G Hub's per-change sequence does - is
  the officially expected pattern, not a workaround.
- **Custom slots.** 0x807B's 5 named slots correspond to x8071's
  effect 12 "Custom Onboard Stored" ("stored and played frame by frame
  at/from a memory chunk in the device", with a "several slots"
  capability bit and per-slot state/defaults/UUID/name metadata) -
  promoted from getInfo-multiplexing into a dedicated feature.
- **Direction vocabulary.** The RS50's directions 0-3 (L-to-R, R-to-L,
  inside-out, outside-in) match the official Color Wave direction
  semantics (horizontal, reverse-horizontal, center-out, center-in),
  differently encoded.
- **The 9.3.2 fn1 response `00 02 01 03 04 05 06 07 08 09`**,
  previously labeled "zone IDs?", is byte 0 = zone/cluster index and
  bytes 1..9 = the list of supported effect IDs - CONFIRMED live
  2026-07-02 by re-reading fn1 on the wheel. Effect IDs 1..9 exist:
  1-4 are the built-in animations and 5-9 are the five custom slots
  (0x05 = CUSTOM 1 .. 0x09 = CUSTOM 5, decoded 2026-07-20); the
  G Hub effect-change broadcasts carrying 6 and 9 were slot switches.

x8071's `rgbClusterChangedEvent` DOES have a 0x807A equivalent,
confirmed across seven captures and now consumed by the driver:
`12ff<idx>00 <effect>` (sw_id 0) fires on every effect change with
the new effect ID as the single payload byte. The driver updates its
led_effect cache and notifies wheel_led_effect pollers.

### 9.11 Capture Files

| File | Description |
|------|-------------|
| `2026-01-29_lightsync_custom_leds.pcapng` | Per-LED color assignment (10 distinct colors) |
| `2026-01-29_lightsync_custom_save.pcapng` | Save/apply custom slot configuration |
| `2026-01-26_lightsync.pcapng` | Basic LED effect + brightness |

### 9.12 LIGHTSYNC Command Sequence

LIGHTSYNC requires a **specific 5-step sequence** using **both features** (0x0B and 0x0C).
fn6 (pre-config) and fn6/fn7 (commit/enable) must go to feature 0x0B, while RGB data goes to feature 0x0C.

#### Command Sequence

```
Step  Feature  Function  Purpose                    Parameters
----  -------  --------  -------------------------  ---------------------------
1     0x0B     fn3       Select the slot's effect   [05+slot] 00 00
2     0x0B     fn6       Pre-config (LONG report)   00 01 00 0a 00 00 00 ...
3     0x0C     fn2       Set RGB colors (64-byte)   [slot] [dir] [30 bytes RGB]
4     0x0B     fn6       Commit (LONG report)       00 01 00 0a 00 0a 00 ...
5     0x0B     fn7       Enable/refresh display     00 00 00
```

**Important Notes:**
- Step 1's effect value is `0x05 + slot` (see 9.3.2): selecting a custom
  slot IS selecting its effect number. There is NO separate "activate
  slot" command - the `10 FF 0C 3C [slot]` frame earlier revisions listed
  here as an "Activate slot" step is 0x807B fn3 GET_NAME, a pure read
  (G Hub issues it as a read-back after switching; the driver's old
  "activate" call did nothing).
- params[1] in the RGB config (byte 5) IS the animation direction, as a 1-4
  wire value (see 9.4.1): 1=Inside-Out, 2=Outside-In, 3=L->R, 4=R->L. An
  earlier note here claimed it "must be 0x03, not direction" - that was from
  before the direction encoding was decoded and is incorrect.
- fn6/fn7 only work on feature 0x0B (0x807A), not on 0x0C (0x807B)

#### Two-Feature Architecture

| Feature Index | Page ID | Purpose | Functions Used |
|---------------|---------|---------|----------------|
| **0x0B** | **0x807A** | Effect/slot select & commit | fn3 (effect, 5-9 = slots), fn6 (pre/commit), fn7 (enable) |
| **0x0C** | **0x807B** | RGB Zone Config | fn2 (set colors), fn3 (get name) |

#### Driver Init Sequence (runs once at startup)

```
10 FF 0B 0C ...           fn0: Query feature info
10 FF 0B 1C ...           fn1: Query capabilities
10 FF 0B 2C ...           fn2: Query current state
10 FF 0C 0C ...           fn0: Query RGB config info
10 FF 0C 1C [slot] ...    fn1: Query slot config
10 FF 0B 7C 00 00 00      fn7: Enable LED subsystem
```

Scoping note (2026-08-14, verified against the current driver): this
fn7, and this whole sequence, belong to the **RGB slot path only**
(`hidpp_dd_lightsync_enable()` at probe and the slot-upload flow
below). The rev-light level path sends no fn7, no fn4 and no fn3 at
any point: its only traffic is the attach-time discovery reads and
the bare fn2+fn6 level pair (see the section 9 intro).

#### Color Change Sequence (runs for each update)

```
10 FF 0B 3C [05+slot] 00 00       Step 1: Select the slot's effect (0x05 = CUSTOM 1 .. 0x09 = CUSTOM 5)
11 FF 0B 6C 00 01 00 0a 00 ...    Step 2: Pre-config on 0x0B
12 FF 0C 2C [slot] [dir] [RGB...] Step 3: RGB data to 0x0C (byte 5 = direction 1-4)
11 FF 0B 6C 00 01 00 0a 00 0a ... Step 4: Commit on 0x0B (params[5]=0x0a)
10 FF 0B 7C 00 00 00              Step 5: Enable/refresh on 0x0B
```

#### Linux Driver Implementation

The `hidpp_dd_lightsync_apply_slot()` function implements the full 5-step sequence.
All sysfs attributes (`wheel_led_colors`, `wheel_led_slot`, etc.) trigger this function.

---

## 10. Unimplemented Features

The following features exist but are not yet implemented in the driver:

1. **In-game slot activation** - Can games trigger LED slot changes via HID++ or only via sysfs?
2. **Firmware update feature** - Standard HID++ devices often have a DFU feature (0x00C0 or similar). **DO NOT PROBE WRITE FUNCTIONS** on unknown features to avoid corrupting firmware.

(Onboard profile / mode switching is implemented via feature 0x8137 - see
Section 5 - and exposed as the `wheel_profile` / `wheel_mode` attributes.)

> **SAFETY WARNING**: Some HID++ features may be related to firmware updates or critical
> device configuration. Always use GET (read-only) functions when probing unknown features.
> Never blindly send SET commands to uncharacterized features.

### Note on Autocenter / Centering Spring

The `autocenter` sysfs attribute (and the evdev `FF_AUTOCENTER` upload)
drives a real driver-side centring spring. The driver stores the
magnitude, and its 1 kHz effect timer sums a position-fed centring force
on top of the game's own effects while the value is nonzero (firm within
roughly the central eighth of travel). A game that writes autocenter 0
disables it for its session.

The spring is computed host-side from the motor: these direct-drive
wheels have no separate hardware autocenter setting (G Hub exposes none),
and unlike belt/gear-driven wheels they produce centring force through the
motor directly. The same attribute backs Oversteer's autocenter control.

---

## 11. Capture Files Reference

| File | Description |
|------|-------------|
| `rs50_ffb_game3.pcapng` | Gameplay FFB (~320k commands, includes `05 07`) |
| `2026-01-26_ghub_startup.pcapng` | G Hub init, feature enumeration |
| `2026-01-26_ffb_strength_sweep.pcapng` | FFB Strength slider |
| `2026-01-26_damping_sweep.pcapng` | Damping slider |
| `2026-01-26_ffb_filter_sweep.pcapng` | FFB Filter slider |
| `2026-01-26_rotation_sweep.pcapng` | Rotation Range slider |
| `2026-01-26_trueforce_sweep.pcapng` | TRUEFORCE slider |
| `2026-01-26_brake_force_sweep.pcapng` | Brake Force slider |
| `2026-01-26_profile_desktop.pcapng` | Profile switch + Sensitivity |
| `2026-01-26_wheel_input.pcapng` | Wheel position input |
| `2026-01-26_pedal_throttle.pcapng` | Throttle pedal input |
| `2026-01-26_pedal_brake.pcapng` | Brake pedal input |
| `2026-01-26_button_mapping.pcapng` | Button press bitmask mapping |
| `2026-01-26_lightsync.pcapng` | LED effect + brightness |
| `2026-01-26_auto_ffb_filter.pcapng` | Auto FFB Filter toggle |
| `2026-01-28_boot_no_ghub.pcapng` | RS50 boot WITHOUT G Hub - raw USB init |
| `2026-01-28_boot_with_ghub.pcapng` | RS50 boot WITH G Hub - full init sequence |
| `2026-01-28_ghub_init_wheel_on.pcapng` | G Hub init with wheel already powered |
| `2026-01-29_lightsync_custom_leds.pcapng` | **LIGHTSYNC**: Per-LED color assignment (10 distinct colors) |
| `2026-01-29_lightsync_custom_save.pcapng` | **LIGHTSYNC**: Save/apply custom slot configuration |

---

## 12. HID++ Protocol Details

### Feature Discovery Sequence (from G Hub)

G Hub uses this sequence to discover features:
1. Protocol ping: `10 ff 00 1X 00 00 5A` → Response includes protocol version (4.2)
2. Feature query: `10 ff 00 0X HH LL 00` → Response byte 4 = feature index

Where `HH LL` is the 16-bit Page ID (e.g., 0x8138 for rotation range).

### Software ID (SW_ID) Behavior

The HID++ protocol uses a Software ID (SW_ID) in the lower nibble of the function byte to correlate requests with responses. G Hub uses SW_ID `0xB` (11), while the Linux driver uses SW_ID `0x0a` (it must not use `0x01`: the pedal sub-device silently drops requests carrying sw-id `0x01`).

**Observed RS50 behavior:**
- G Hub sends: `10 ff 00 0b 81 38 00` (function 0, SW_ID 11)
- RS50 responds: `12 ff 00 0c 18 00 00...` (function 0, SW_ID echoed as 12?)

The RS50 appears to echo the SW_ID in responses, though some HID++ 2.0 devices may leave it as 0. The driver should handle both cases by comparing only the function index (upper nibble) when SW_ID is 0.

Official semantics (Logitech HID++ 2.0 draft specification, 2012-06-04)
confirm the observed behavior: the firmware must copy the software
identifier into the response but does not otherwise use it, and SW_ID 0
is reserved ("do not use - allows to distinguish a notification from a
response"). This matches the capture census exactly: every unsolicited
packet - the `12ff1500 XXXX`-style settings broadcasts, profile and
rotation events - carries SW_ID 0 (officially "broadcast events"),
while responses echo the requester's SW_ID (G Hub runs parallel client
sessions on SW_IDs `a`..`e`, plus `f` for DFU checks; the Linux driver
is `1`). The same spec also states all request parameters must be
repeated in the response, which explains the echoed values in e.g.
`10ff182d 0438` -> `12ff182d 0438`.

### HID++ Error Packets

Errors arrive as a response with feature index `0xFF` in byte 2,
followed by the failing feature index, the failing fn|sw byte, and an
error code (per the official 2.0 draft): 0 NoError, 1 Unknown,
2 InvalidArgument, 3 OutOfRange, 4 HWError, 5 "Logitech internal",
6 InvalidFeatureIndex, 7 InvalidFunctionId, 8 Busy, 9 Unsupported.
The wheel uses this standard mechanism, including in very-long frames.
Decoded examples from the captures:

- `12 ff ff 0a 3c 09` - feature idx 0x0a (0x8040) fn3: Unsupported.
- `12 ff ff 0b 4c 05` - LIGHTSYNC 0x807A fn4: Logitech internal.
- `11 05 ff 0c 3b 02` - sub-device 0x05 calibration write rejected
  with InvalidArgument (G Pro contributor capture; note the 0x11
  report ID, matching the sub-device response quirk of section 5.3).

### Alternative: FeatureSet Enumeration

Instead of querying ROOT.getFeatureIndex for each PAGE ID, G Hub uses FeatureSet (index 0x01) to enumerate all features:

```
Query FeatureSet.getFeatureID(index):
  10 ff 01 1X [index] 00 00
Response:
  12 ff 01 1X [pageID_hi] [pageID_lo] [type] ...
```

This returns the PAGE ID at each index. G Hub queries indices 0x00 through ~0x1F to build the complete feature map. This is more efficient than individual ROOT queries because:
1. Single function call per index (vs. searching for each PAGE)
2. Discovers all features, including unknown/undocumented ones
3. Faster overall enumeration

### 12.3 Dynamic OLED (Feature 0x8130, DisplayGameData)

> **Not the rev lights.** This wheel has two separate light-emitting
> surfaces, and they share nothing:
>
> | | Where | Feature | Driver support |
> |---|---|---|---|
> | **Dynamic OLED** | the screen on the wheel **base** | `0x8130` (this section) | none |
> | **Rev-light strip** | the 10 LEDs across the **rim** | `0x807A` (12.4, section 9) | `wheel_rev_level`, `wheel_led_*` |
>
> The OLED is the same panel that shows the profile and settings menu
> referred to elsewhere in this document as "the wheel's OLED menu";
> "Dynamic" is the role it plays when a game feeds it, not a second
> display. The rev strip is what `logi-tf-sim` drives from telemetry, and
> what the pit-limiter flash in 12.4 alternates. Nothing in this section
> reaches the strip, and nothing in 12.4 reaches the OLED.

**Status: confirmed on hardware by a third party, not by this driver.** The
driver neither sends nor exposes anything here. This section records what is
known so the next person does not start from the enumeration table again.

The RS50's Dynamic OLED is driven through HID++ feature `0x8130`, which
enumerates at index `0x12` on this wheel. Reported by @PeposCJ in issue #20
(2026-07-28), who reached the panel over that feature and displayed both
static text and live iRacing telemetry on it.

**Functions** (@PeposCJ, 2026-07-30, RS50 hardware):

| fn | Purpose |
|----|---------|
| 0 | layout count |
| 1 | layout descriptor |
| 2 | clear pending Dynamic data |
| 3 | set layout / data |

The wheel reports **10 layouts, A to J**. Static and repeated writes are
both hardware-confirmed (@PeposCJ). Every layout's descriptor, read
first-hand from an RS50 here (2026-09-02, read-only fn1 sweep, feature at
index `0x12`, confirmed as `0x8130` through IFeatureSet):

| index | layout | text fields (widths) |
|---|---|---|
| 0 | A | none |
| 1 | B | none |
| 2 | C | none |
| 3 | D | 11 |
| 4 | E | 7, 3 |
| 5 | F | 1, 3 |
| 6 | G | 1, 3 |
| 7 | H | 21, 10 |
| 8 | I | 19, 10, 19, 10 |
| 9 | J | 19, 10, 19, 10 |

fn0 answered `0x0a`, ten, matching. A G PRO fn1 sweep (@Mhytee, 2026-08-06)
matches this table on all ten layouts, counts and widths identical.

**fn3, validated on hardware** (@Mhytee on a G PRO, layouts F to J driven
since 2026-08-06 and shipped in TF4ALL; A to E from @PeposCJ's working
writer and gallery record; both sources corroborate each other's numbers
wherever they overlap. Posted in issue #75, 2026-09-02). Every setter is
the 64-byte VERY_LONG report, layout byte first:

```
12 ff <featureIdx> 3<swid> <layout 0..9> <layout payload, zero padded to 64>
```

Text is ASCII, left aligned, fixed width, packed back to back; spaces are
content; zero padding after the last field is fine. Firmware behaviour on
non-printable bytes is uncharacterised (TF4ALL clamps to space). Payload
offsets count the report ID as byte 0:

| layout | payload |
|---|---|
| A (0) | none accepted; renders a black frame |
| B (1) | none accepted; the firmware's own four-bar test composition |
| C (2) | byte 5 = gauge fill `0..255` |
| D (3) | byte 5 = gauge fill, byte 6 = thin indicator mark, bytes 7-17 = 11-char label |
| E (4) | byte 5 = gauge fill, byte 6 = thin indicator, bytes 7-9 = 3-char (draws RIGHT), bytes 10-16 = 7-char (draws LEFT) |
| F (5) | byte 5 = 1-char (medium), bytes 6-8 = 3-char (very large) |
| G (6) | byte 5 = 1-char (very large), bytes 6-8 = 3-char (medium) |
| H (7) | bytes 5-25 = 21-char small row, bytes 26-35 = 10-char larger row |
| I (8) | bytes 5-23, 24-33, 34-52, 53-62 = 19/10/19/10; rows 2 and 4 larger, right-aligned |
| J (9) | same offsets as I; every row centred |

The gauge's unfilled remainder is a diagonal-stripe background that the
fill replaces left to right. G is the classic gear-and-speed screen: a
very large one-character slot, a firmware-drawn separator, a medium
three-character value; F is its mirror. Those two carry the panel's
largest font (37 px), which exists nowhere else.

**Two traps in "build frames from the descriptor".** fn1's capability
bytes describe text fields only: D and E also take two normalised value
bytes, and those come *first* in the payload, so a writer that starts
text at byte 5 with the descriptor's widths corrupts D's gauge with the
label's first two characters. And the descriptor lists fields left to
right *as drawn*, not in payload order; E is the one layout where the two
differ. Descriptor widths are trustworthy, descriptor order is not, and
D/E need their value bytes prepended.

**I versus J** have identical descriptors and different rendering: I
right-aligns its two large rows, J centres every row. Only a write tells
them apart.

**Two-zone rule.** The wide large rows (H's lower row, I's large rows)
draw the first character pinned to the left edge and the remainder
right-aligned: "112%" draws "1" left and "12%" right with a gap, on a
correctly described single field. Spaces steer it: a leading space skips
the left zone for a clean right-aligned value, trailing spaces pull it
back, character-exact. Used deliberately that is a one-character value
left and a longer one right on one row. In F and G padding moves nothing.
No large-font layout centres, so J's 18 px rows are the largest centred
text the panel produces.

**The panel does not hold a frame.** When the host goes quiet the
firmware drops the Dynamic surface back to the wheel's own menu; the
timeout is under two seconds (a 2 s refresh visibly flickered back to the
menu between frames). An unchanged frame must still be resent; TF4ALL
resends at 50 Hz and the panel is stable. A driver therefore needs a
refresh tick, not only write coalescing. **fn2 is the immediate handback**:
without it the wheel reclaims its screen only after that timeout.

**The feature ends at fn3.** fn4 through fn15 answer HID++ error `0x07`
INVALID_FUNCTION_ID on a G PRO; there is no claim-the-display call.

**Verified here, first-hand** (RS50, base firmware U1 65.04.B0039,
2026-09-03, watched): the fn3 frame above is accepted at the dynamically
resolved index; layout J drew four rows, the second and fourth larger, all
centred; layout G drew the very large gear digit with the three-character
value beside it; layout H drew `112%` on its lower row as `1` pinned left
and `12%` right with a gap, which is the two-zone rule; and fn2 returned
the wheel's own menu immediately each time. Held at 20 Hz for the viewing,
and stable at that rate.

A cheaper first-hand probe than a watched write: fill bytes 5..63 with
distinct printable characters (a ruler frame) and the panel labels its
own offsets, one frame per layout. A full-width ruler masks the two-zone
rule, which needed a short real value such as "112%" to expose.

Still unknown, at both ends: the 12.5 mechanism, the exact hold timeout,
firmware behaviour on non-printable bytes, and features 0x18A2, 0x18B1
and 0x9315.

**fn1 is self-describing** (@WnDTech's tester, RS50 hardware, 2026-08-15):
requesting the descriptor for layout index 9 answers

```
12 FF 12 1A  09 0A 13 0A 13 0A 00 ...
```

payload `[requested layout index][one-based layout ID][four capability
bytes]`. So `09` is the index asked for, `0A` is layout ID 10 (layout J),
and `13 0A 13 0A` are the four field widths 19 / 10 / 19 / 10. A client
can build frames from the descriptor instead of hardcoding per-layout
widths.

The apparent fifth capacity entry was the layout ID being read as a
width: decoded by @PeposCJ from the RS50's own firmware handler and
confirmed against the same readback (issue #20, 2026-08-31). There are
four text fields, not five. The fn3 frame byte order (layout byte first,
field order, space padding) remains unvalidated on hardware.

**Read that run's "collision retry" carefully.** 0x8130 appeared at index
0x12 only on a second attempt, which looks like the wheel handing out a
colliding index and is more likely to be a mismatched answer. A Root
getFeature response does not echo the feature ID it was asked about, so
every such query looks identical on the wire, and a late answer to one
query is indistinguishable from the answer to the next. @PeposCJ reports
that exact failure being reproduced elsewhere, an OLED discovery answer
latched as the rev-light index. Resolve the index dynamically, never
assume it, and treat a discovery answer as suspect unless the request it
belongs to is still the one outstanding.

**It is a typed renderer, not a framebuffer.** The firmware holds a 128x64
monochrome buffer, but no `0x8130` command accepts framebuffer bytes,
coordinates, pixels or partial regions. Each layout exposes bounded text
fields and normalised values, and the firmware draws them:

```
host -> typed Layout A-J data -> firmware renderer -> framebuffer -> OLED
```

So a driver-side interface for this would be a small set of typed fields per
layout, not a bitmap surface. That rules out the character-device interface a
framebuffer would have implied.

**Transport: interface 1, endpoint 0, HID SET_REPORT.** Deliberately not
Logitech's DirectInput Escape path. That path reaches the panel, but its
exclusive Acquire/Unacquire lifecycle emits `RESET_ALL`,
`SET_GLOBAL_GAINS(0xFFFF)`, `RESET_ALL` on `0x8123`, which killed iRacing's
shift LEDs and centre feel until the game rebuilt its wheel state. Do not
use it.

**Safety: not settled, and the caveat is sharper than it looks.** See 12.5.
Coexistence has been observed but only in the one case where the endpoint
happens to be quiet: iRacing has native TrueForce, so it carries steering
force in `cur` on the TrueForce stream and never writes force to HID++. Five
writes at 1 Hz rendered and acknowledged while TrueForce ran at ~1000
submissions/sec, with no observable disturbance, car stationary in the pits.
That is not evidence that OLED writes are safe in a title that does write
force to HID++, and 12.5 gives reason to expect they are not.

**Related undecoded candidates.** An earlier round of enumeration flagged
`0x18A2`, `0x18B1` and `0x9315` as the interesting unknowns on `dev_idx
0x01`. The OLED turned out not to be any of them, so they remain unexplained
rather than ruled out.

### 12.4 Rev-light stream: attach-time discovery reads, acknowledgements and flashing

This section is entirely about the **rim's 10-LED rev strip**, feature
`0x807A`. It has nothing to do with the base's OLED screen (12.3): the two
are different hardware on different features, and no command here reaches
the display.

From four first-party G Hub/iRacing captures contributed by @PeposCJ in
issue #20 (2026-07-27). These settled three questions and found one bug.

**Attach-time discovery reads** (this used to be called an "arm sequence";
2026-08-14 hardware work settled that nothing here arms anything - they are
plain reads, done once at wheel attach, and the strip renders bare fn2+fn6
levels from the onboard power-on state without them). After `0x807A`
resolves to its runtime index, G Hub sends fn0, fn1, fn2, then fn0 again,
and only then begins the fn2 + fn6 stream:

```
10 ff 00 0c 80 7a 00   ->  12 ff 00 0c 0b 00 00      resolve 0x807A to index 0x0B
10 ff 0b 0c 00 00 00   ->  12 ff 0b 0c 01 0a 0a      fn0
10 ff 0b 1c 00 00 00   ->  12 ff 0b 1c 00 02 01 ...  fn1
10 ff 0b 2c 00 00 00   ->  12 ff 0b 2c 02 00 00      fn2
10 ff 0b 0c 00 00 00                                  fn0 again
11 ff 0b 6c ...                                       then the fn2 + fn6 stream
```

This driver used to send an extra `fn3` with parameter 2 that G Hub never
sends. `fn3` is SET_EFFECT, so it switched the wheel to effect 2 and
destroyed whatever LIGHTSYNC effect the user had configured, which the
driver then papered over by snapshotting and restoring the lighting. Both
the stray write and its workaround were removed in v0.21.0 (`75090ad`). The
sequence above is the first-party evidence that settled it; the `fn3` came
from a third-party capture.

**Acknowledgements.** Every write draws a `0x12` response. Over 301 short
plus 301 long writes the pattern held without exception: `0x10` fn2 answered
in ~2.49 ms median, then ~0.15 ms later `0x11` fn6 answered in ~2.85 ms
median. Consistent with flow control rather than fire-and-forget.

**Redline.** Only `LL = 0..10` is ever used. At redline the stream simply
keeps sending `LL = 10` at ~60 Hz. There is no value above `0x0A`, and no
separate flash command or alternate function.

**Pit-limiter flash.** iRacing's full-strip flash is not a device effect at
all: it is `LL = 10` and `LL = 0` alternating at ~416.7 ms, about 1.2 Hz,
over the same fn2/fn6 pair. A telemetry feeder can therefore reproduce it
without any new protocol support, which is why it lives in `logi-tf-sim`
rather than the driver.

Reproduced on an RS50 on 2026-07-29, from synthetic OutGauge packets rather
than a game: 28 transitions across 12 seconds, strictly `10` and `0`, mean
gap 418 ms against the captured 417 ms. The rev level the same RPM would
otherwise show (`5`, held steady throughout a control run) never appeared,
confirming the limiter overrides the rev display entirely rather than
blending with it. So a Windows behaviour was reproduced on Linux to within
measurement noise from the capture alone, with no vendor documentation.

### 12.5 HID++ endpoint contention: a non-force write cuts live force

Third-party finding (@Mhytee / TF4ALL, 2026-07-30, corroborated by
@PeposCJ's independent OLED captures in issue #20). Observation, not
mechanism: nobody has established why.

**While any force is present on the HID++ endpoint, a write to that endpoint
which is not force appears to cut the force.** Regardless of who sends it,
how small it is, or how gently it is paced. Rev-light writes on `0x807A`
during a game's DirectInput force were the original case, on Windows with a
G PRO. Pacing was tuned down to a 60 ms floor and fixed nothing.

Two things it is *not*:

- **Not a multiple-writers problem.** Two independent programs writing the
  endpoint at once disturbed nothing, and one program doing both force and
  LEDs still dropped out.
- **Not solved by cadence.** See above.

The only configuration where it stops is force carried on the TrueForce
stream with the HID++ endpoint kept completely force-free.

**"Ignored is not the same as absent."** This is the part that took longest
to see, and the part most likely to mislead a reading of a capture. While the
TrueForce stream carries force, the wheel obeys `cur` and appears to ignore
force sent to HID++, so the endpoint looks free. It is not: the game's writes
are still on the wire and still break things until they actually stop.

So a quiet-looking endpoint proves only that the current title has native
TrueForce and never writes force there.

**Unexplained counter-evidence.** In a capture of Logitech's own stack,
`0x8123` force pairs interleave with `0x807A` rev-light updates as close as
37 ms apart with no measurable degradation:

```
FFB inter-packet gaps near an LED write    : median 3.00 ms  p99  15 ms  max 107 ms
FFB inter-packet gaps away from LED writes : median 3.01 ms  p99 240 ms  max 969 ms
```

No third party has reproduced that. Whatever Logitech does to make them
coexist is not known.

**What this means for this driver.** Mostly nothing today, and that is worth
stating explicitly so it is not mistaken for a latent bug:

- **Direct-drive wheels are not exposed to THIS interference class.** Their
  force rides endpoint `0x03`, not HID++, so `wheel_led_*` and the rev-level
  writes never compete with force on the same endpoint. Games cannot put
  force there either: force reaches the wheel through this driver, which
  sends it to `0x03`. They turned out to have an interference class of
  their own, though (hardware, 2026-08-14): while a **native TrueForce SDK
  session** is live, the driver's old rev-LED form - arm burst plus
  per-level fn3 - killed the session at init and killed wheel input
  mid-session. The fix was making the driver speak exactly what G HUB
  speaks: bare fn2+fn6 levels, no arming (commit `9d4b68a`). In that form,
  base FFB + TrueForce texture + telemetry LEDs are hardware-validated
  running together.
- **The PlayStation G923 is not exposed.** Its force is the classic engine's
  plain HID output reports, and its rev LEDs are `led_classdev` entries
  driven by the same engine. Neither is HID++.
- **The G923 Xbox edition is the one wheel where this could bite**, because
  its force genuinely does ride HID++ `0x8123`. It has `0x807A` in its
  feature map, so rev lights are *technically* reachable. Today this driver
  exposes no LED surface for it at all: the `led_classdev` entries come from
  the classic engine it does not use, and `wheel_led_*` belongs to the
  direct-drive path it does not use either.

### 0x807A rev level: the 4th parameter is a denominator (hardware-measured)

Measured on an RS50 on 2026-08-10, with the strip switched to Left-to-Right
so a fill reads as a count from one end rather than closing in from both
(Outside-In makes the two cases below look nearly identical, which cost one
inconclusive round):

| count | level | LEDs lit |
|---|---|---|
| 10 | 10 | 10 |
| 10 | 5 | 5 |
| **5** | **5** | **10** |

The third row settles it. If the level were a raw LED count, `5` would light
five. It lights the whole strip, so the level is a fraction of the count:
the 4th parameter is the denominator, and the wheel scales the fill to its
own strip.

**But the two wheels do not treat a wrong denominator the same.** The RS50
accepts `0a` and `05` and lights for both. The G923 Xbox edition lit nothing
at all for `0a` across five framings (issue #27), while Windows drives it
with `05`, which is that wheel's real LED count. So on the RS50 the value
only scales the fill, and on the Xbox G923 it appears to be validated. That
asymmetry is unexplained and worth remembering: do not conclude from the
RS50's tolerance that the field is cosmetic.

The rule either way is the same, and it is what Windows does: send the
wheel's actual LED count, ten for the direct-drive wheels and five for a
G923.

**Superseded 2026-08-10 by a capture of that wheel (issue #27).** Its rev
lights are driven over `0x807A` after all, with `fn2`+`fn6` pairs, and the
reason ours never lit is a single parameter: the level command states the
strip length, and the G923 has **five** LEDs where the direct-drive wheels
have ten. This project sent `0a` to every wheel. The same capture shows that
wheel's force arriving on the `0xFFFD` stream rather than HID++, so the
endpoint-contention argument below does not apply to its rev lights.

**Consequence for future work:** adding rev-light support to the G923 Xbox
edition is not a matter of wiring up `0x807A`. On that wheel it would be
writing non-force to the same endpoint its force arrives on, which is exactly
the configuration described above. It would need force moved off HID++ first,
or evidence that this wheel behaves differently from the G PRO the finding
came from.

---

## 13. Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-01-26 | Initial specification from USB capture analysis |
| 2.0 | 2026-01-26 | Verified input format, all button mappings, D-pad encoding |
| 3.0 | 2026-01-26 | Pedal response curves, combined pedals mode, deadzones |
| 4.0 | 2026-01-28 | HID++ report ID behavior (0x12 responses), SW_ID handling |
| 5.0 | 2026-01-29 | LIGHTSYNC RGB LED control (10-LED per-zone colors) |
| 5.7 | 2026-02-03 | FFB simplified to FF_CONSTANT only (500Hz timer) |
| 6.0 | 2026-02-04 | Verified feature table, SW_ID behaviour, LIGHTSYNC two-feature architecture |
| 6.1 | 2026-04-21 | 8-way D-pad, G Pro coverage, per-feature SET fn numbers (damping fn=1, TRUEFORCE fn=3), FFB filter flags bitfield, centre calibration (feature 0x812C, G Pro sub-device 0x05), `wheel_calibrate` sysfs attribute |
| 6.2 | 2026-06-29 | Corrected the D-pad section: the hat is a standard HID Hat Switch (value 0 = Up, clockwise) decoded natively by the kernel, not a custom byte-0 decode. The previous direction table and the `rs50_process_dpad()`/`RS50_DPAD_*` references were the removed, scrambled implementation (issue #22) |
| 6.3 | 2026-07-02 | Resolved the three unknown features (0x80A4 axis response curves, 0x1BC0 ReportHidUsages, compat 0x15 = Brake Force); compat catalog identical to native; sub-device map (5.3); HID++ error packets, SW_ID and 0x12-report semantics from Logitech's official specs; DeviceInfo identity decode; LIGHTSYNC official lineage (9.10); registry cross-checks |
| 6.4 | 2026-07-03 | Profile feature settled live against the OLED: fn2 SET = plain [profile,0,0], fn1 GET = [profile, mode], fn3 = per-slot profile names; catalog rows 0x02/0x03 corrected (DeviceInfo / DeviceNameType); launch-time 90-degree reset root-caused to the SDK's type-0x0e operating-range push (see TRUEFORCE_PROTOCOL.md) |
| 6.5 | 2026-07-03 | Renamed from RS50_PROTOCOL_SPECIFICATION.md (covers the whole direct-drive family); driver symbols updated to the hidpp_dd_ prefix; documented that the RS50 keeps its own USB product string in G PRO compatibility mode ("RS50 Base for PlayStation/PC" under PID c272) while a real G PRO reports "PRO Racing Wheel" - the driver uses this to tag log output per model |
| 6.6 | 2026-07-04 | Cross-pollination from the TF4ALL project (issue #20): the Windows game-FFB path for DD wheels is HID++ 0x8123 fn2 (int16 BE at offset 10-11; catalog index 0x10 native / 0x0e G-PRO-PID); stream-packet bytes 6-9 ("cur") are the motor torque target and override 0x8123 while a session is active, with the window additive on top; AC EVO streams unified cur+audio packets at up to ~1000 pkt/s; texture amplitudes above ~0.5-0.7 FS cross from vibration into steering pull; the REAL G PRO rim has level-based rev lights on 0x807A (SHORT fn2 + LONG fn6, byte 9 = 0-10) with no per-LED RGB - section 9 describes RS50 hardware only |
| 6.7 | 2026-07-06 | Section 4 corrected and de-duplicated against TRUEFORCE_PROTOCOL.md: byte 10 is the new-sample count that demuxes the shared ep-0x03 packet family (0 = constant force, 4 = unified force+audio), not padding; bytes 6-9 named as the "cur" motor torque target; force rate note updated (games 250-1000 Hz, driver 500 Hz); TRUEFORCE_PROTOCOL.md pointed to as the authoritative framing reference, with a reciprocal link back. Removed the RS50_PROTOCOL_SPECIFICATION.md redirect stub (rename complete). |
| 6.8 | 2026-07-19 | LIGHTSYNC slot-direction wire encoding decoded from `dev/captures/2026-07-19_lightsync_direction.pcapng`: byte 5 of the 0x807B RGB config is a 1-4 value (1=Inside-Out, 2=Outside-In, 3=Left->Right, 4=Right->Left), not the driver's 0-3 enum (9.4.1). Fixed the driver's `direction + 2` encoding, which both mislabelled the sweeps and sent an out-of-range 5 for Outside-In (firmware NAK -> -EIO). Documented the 5-mirrored-pair (palindrome) behaviour of the two symmetric directions (9.8). |
| 6.9 | 2026-07-20 | Hardware-verified on the RS50: (a) all four 9.4.1 slot-direction wire values watched live via rev-fill sweeps - the driver-enum labels are correct as tabled; (b) the RS50 accepts the G PRO level-based rev-light command (0x807A fn2+fn6, byte 9 = 0-10) - the fill renders the active slot's colours and follows its direction, making a live RPM rev display possible without G HUB's own feed format; (c) fn3 SET_EFFECT only STAGES an effect - the strip repaints on a zero-parameter fn6 commit (driver now sends the pair). |
| 7.0 | 2026-07-20 | LIGHTSYNC slot selection decoded (`2026-01-30_desktop_led_colors.pcapng` frames 219/339/715 + 279/525/775, hardware-confirmed live): 0x807A fn3 effect values 5-9 ARE the five custom slots (0x05 = CUSTOM 1 .. 0x09 = CUSTOM 5) - selecting a slot is selecting its effect number, staged by fn3 and repainted by the fn6 commit (zero-parameter standalone; the full-config forms bracket an RGB upload). No separate "activate slot" function exists: 0x807B fn3 is GET_NAME, a pure read (the driver's old "activate" step did nothing, and its hardcoded fn3 = 0x05 pinned the strip to CUSTOM 1 on every slot switch; both fixed). Section 9 effect tables and sequences corrected; rev-display note reconciled - the rev fill renders the SELECTED slot's config (the earlier "not the active slot" reading was an artifact of the broken switch; post-fix colour source expected, re-verify); the arm burst's effect stomp is healed by re-asserting the pre-arm effect value after the one-time burst. |
| 7.2 | 2026-07-28 | Accessory fully decoded and driven. `0x80B1` BANDED_AXIS: `[group][band]` addressing, fn2 reads / fn3 writes a signed 32-bit lever position, group 0 = shifter (two bands, one per direction), group 1 = digital handbrake (one); both scales given, including G Hub's 16-bit overflow above ~69% which this driver does not reproduce. `0x1B30` DEVICE_MODE: fn2 returns the switch position, values 0/2/1 for shifter/digital/analog. Pedal curves corrected to the BASE axes 1/2/3 - the pedal MCU stores an upload and never applies it. Sub-device index established as a property of the physical PORT, not the device type. |
| 7.1 | 2026-07-28 | RS Shifter & Handbrake accessory (dev `0x04`) catalogued (5.3): full 18-feature map, names via 0x0005/0x0007, own 0x80A4 store, and the two accessory-unique public features `0x80B1` BANDED_AXIS / `0x1B30` DEVICE_MODE (first-party names, no captured wire format). Driver now discovers the accessory alongside the pedal MCU in one candidate-list pass and exposes presence via `wheel_accessory`; its own three settings were unimplemented at that revision (see 7.2). Sub-device index instability documented explicitly: no index is fixed for either the pedal MCU or the accessory. |
| 7.3 | 2026-07-29 | Two contributions from @PeposCJ (issue #20), neither verified by this driver. `0x8130` DisplayGameData identified as the Dynamic OLED's transport, reached on hardware with static text and live iRacing telemetry on the panel (12.3); function numbers, payload layout and, critically, whether writing it disturbs the FFB stream all remain open, and `0x18A2`/`0x18B1`/`0x9315` stay unexplained rather than ruled out. Rev-light stream documented from four first-party captures (12.4): the true arm sequence is fn0/fn1/fn2/fn0 before the fn2+fn6 stream, with no `fn3` (the driver's extra `fn3` SET_EFFECT stomped the user's LIGHTSYNC effect and was removed in v0.21.0); every write draws a `0x12` acknowledgement; redline is plain `LL = 10` at ~60 Hz with no flash command; and iRacing's pit-limiter flash is only `LL` 10/0 alternating at ~416.7 ms, so a telemetry feeder can reproduce it with no new protocol support. |
| 7.4 | 2026-07-30 | Dynamic OLED largely decoded and the HID++ endpoint's contention behaviour recorded, both from issue #20 and neither verified by this driver. `0x8130`: fn0 layout count, fn1 layout descriptor, fn2 clear pending, fn3 set layout/data; 10 layouts A-J, layout J exposing four text fields at 19/10/19/10; a typed firmware renderer rather than a framebuffer (the firmware has a 128x64 buffer but no command accepts pixels, coordinates or regions), reached at interface 1 endpoint 0 by SET_REPORT, explicitly NOT via Logitech's DirectInput Escape path whose Acquire/Unacquire lifecycle emits RESET_ALL / SET_GLOBAL_GAINS / RESET_ALL on 0x8123 (12.3). New 12.5: while any force is present on the HID++ endpoint, a non-force write to it cuts the force, independent of sender count and unimproved by pacing; a quiet-looking endpoint only means the title has native TrueForce and never writes force there ("ignored is not the same as absent"). Recorded with its consequence: the G923 Xbox edition is the only wheel here whose force rides HID++, so rev-light support for it cannot simply reuse 0x807A. |
| 7.5 | 2026-08-08 | Force stream rates corrected throughout: the kernel driver's own stream runs at **1000 Hz** from 0.30.0, matching what games send and Logitech's stated 1 ms TRUEFORCE interval, having really run at 333 Hz before. The timer was a jiffies timer that re-armed itself for the next jiffy, and the timer wheel never fires a timer early, so the expiry always slipped to the jiffy after: measured across four nominal intervals on an RS50, every one came back a millisecond long. It is now an hrtimer, so the period is the one requested and `CONFIG_HZ` does not enter into it. Texture samples span one millisecond per tick, so a two-millisecond tick left every other millisecond unsampled and the wheel held through the gap. This did not shift pitch: measured from the steering encoder on an RS50, both the old and new builds render a requested 50 Hz and 100 Hz exactly. Feature-page names reconciled against Logitech's published HID++ 2.0 registry: `0x807A` is RPM_INDICATOR (the rev display) rather than the general LIGHTSYNC this project called it, `0x807B` is RPM_LED_PATTERN, `0x80D0` is COMBINED_PEDALS (which explains its profile-change broadcast), `0x8136` is TORQUE_LIMIT. New docs/FEATURE_MATRIX.md enumerates both wheels here against that registry: notably the RS50 and G923 use **different** response-curve pages (`0x80A4` versus the legacy `0x80A3`), and DUAL_CLUTCH `0x8127` and GAMING_ATTACHMENTS `0x8120` are present on both wheels and implemented on neither. |
