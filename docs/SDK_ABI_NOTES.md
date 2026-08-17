# TrueForce SDK: verified ABI notes

Companion to `TRUEFORCE_PROTOCOL.md`, which covers the wire protocol to the
wheel and why the SDK reports 90 degrees under Proton. This one is about
calling the SDK itself.

What the shipped `trueforce_sdk_x64.dll` actually expects, taken from its own
machine code rather than from any header. Written after three signatures in
`userspace/libtrueforce/include/trueforce.h` turned out to be wrong, one of
which made `tools/tf-range-proxy.c` incapable of working and cost a tester
several rounds of testing (issue #27).

**Read this before adding to `libtrueforce` or the proxy.** The header in
that tree is not authoritative and never was.

## How to check a signature

Find the RVA in `sdk/trueforce_1_3_11/exports_x64.txt`, then:

```bash
D=sdk/trueforce_1_3_11/trueforce_sdk_x64.dll
A=$(python3 -c "print(hex(0x180000000 + 0x18620))")   # image base + RVA
x86_64-w64-mingw32-objdump -d "$D" --start-address="$A" \
  --stop-address="$(python3 -c "print(hex(int('$A',16)+0x40))")"
```

Windows x64 passes integer and pointer arguments in RCX, RDX, R8, R9, and
floating point in XMM0-3 by position. So:

- a `test %rcx,%rcx` followed by a `mov $0x80000001,%eax` return means the
  first argument is a pointer that must not be null
- a `movsd %xmm1,...` means the second argument is a double
- a write through a saved register (`mov %eax,(%rbx)`) means an out parameter,
  and the width of that write is the type

`0x80000001` is the library's own bad-parameter status. Success is 0.

Calling one of these cold to see what it does will not work: they dereference
internal state that only exists after the SDK has been opened, and crash on a
null context. Verified by trying it.

## Verified

| Function | Signature | How |
|---|---|---|
| `logiWheelGetOperatingRangeDegrees` | `int f(int index, double *out)` | RCX index, RDX null-checked out pointer, `0x80000001` when null |
| `logiWheelGetOperatingRangeRadians` | `int f(int index, double *out)` | same layout |
| `logiWheelGetOperatingRangeBoundsDegrees` | `int f(int index, double *lo, double *hi)` | RCX, RDX, R8; RDX null-checked |
| `logiTrueForceSetGainTF` | `int f(int index, double gain)` | `movsd %xmm1` = second argument is a double |
| `logiTrueForceAvailable` | first argument is a **pointer**, not an index | `test %rcx,%rcx` then `0x80000001` |
| `GetOperatingRangeBounds*` | the least and greatest range that can be **set** (90 and 2700), not the rim's angular extremes | see below |

### Why bounds means the settable limits

Worth writing down, because the other reading is plausible and was in this
tree for a while. Five things agree:

- the name: bounds *of the operating range*, and the operating range is a
  single total-degrees number, which Logitech's own documentation states
  (`LogiGetOperatingRange` "fills the range parameter with the current
  controller operating range")
- the library's own strings, `ANGULAR_RANGE_MIN` and `ANGULAR_RANGE_MAX`
- `docs/PROTOCOL_SPECIFICATION.md`: settable range is 90 to 2700 in 10
  degree steps, and 90 being the minimum is exactly why a failed lookup
  produces a 90 degree wheel
- the function reads its answer from the device through a vtable rather than
  from constants, which is what a capability query looks like
- a game asking is most plausibly validating a rotation setting, and both
  ACC and the SDK pair the getter with a setter

The competing reading, `-range/2 .. +range/2`, rested on `450.0` appearing
four times in the binary. Those are all in `.text`: coincidental bytes inside
instructions, not double constants, which would live in `.rdata`. There is no
evidence for it at all.


The legacy Steering Wheel SDK is a different library with different
conventions, and its equivalent writes an `int`, not a `double`:

| Function | Signature |
|---|---|
| `LogiGetOperatingRange` (`logi_steering_wheel_x64.dll`) | `bool f(int index, int *range)` |

## Audit of `libtrueforce`, 2026-08-05

All 54 declarations checked against the real library's prologues. **Seventeen
are wrong**, and they fail the same way: the real library reports through an
out parameter and returns a status, while the header declares a value return
and no out parameter. Nothing written against the real SDK can call these.

```
bool   logiTrueForceAvailable(int)          bool   logiTrueForceIsPaused(int)
bool   logiTrueForceSupported(int)          bool   logiWheelSdkHasControl(int)
double logiTrueForceGetAngleDegrees(int)    double logiTrueForceGetAngleRadians(int)
double logiTrueForceGetAngularVelocityDegrees(int)
double logiTrueForceGetAngularVelocityRadians(int)
double logiTrueForceGetDamping(int)         double logiTrueForceGetDampingMax(int)
double logiTrueForceGetGainKF(int)          double logiTrueForceGetGainTF(int)
double logiTrueForceGetHapticRate(int)      double logiTrueForceGetTorqueKF(int)
double logiTrueForceGetMaxContinuousTorqueKF(int)
double logiTrueForceGetMaxPeakTorqueKF(int)
int    logiWheelGetVersion(int index, ...)  -- first argument is a pointer
```

Each should become `int f(..., T *out)` returning 0 or `0x80000001`, the signature
already applied to the rotation getters. The setters, which pass values in
rather than out, check out clean: `logiTrueForceSetGainTF(int, double)` and
the `SetTorqueTF*` family match.

This is an ABI break for `libtrueforce`, and deliberately not rushed in
alongside a release. It costs nothing to defer, because no caller written
against the real SDK could ever have linked against the current signature.

Reproduce with the method above, or the throwaway script in the commit that
added this section.

## Not established

- Whether the out parameter of each getter above is a `double`, an `int` or a
  `bool`. The status-return signature is certain; the width is not, and the safe
  way to settle each is the write instruction in the body rather than the
  prologue.

## Which SDK a game uses

Grep the game binary. Assetto Corsa Competizione resolves 56 symbols from
the TrueForce SDK, including all four rotation getters, and none at all from
the legacy Steering Wheel SDK:

```bash
strings -n 6 AC2-Win64-Shipping.exe | grep -E "^(logi|Logi|dll)[A-Za-z]" | sort -u
```

It also looks up four symbols the library has never exported (`dllVersion`
and three viscosity calls). Those lookups fail on Windows too, so a proxy
should match the real library exactly rather than inventing them.
