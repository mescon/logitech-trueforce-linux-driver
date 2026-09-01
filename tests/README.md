# Kernel-driver tests that need no kernel

The driver's arithmetic lives in header-only files that compile either inside
the kernel or as ordinary C, with the same source either way:

| header | what it decides | harness |
|---|---|---|
| `mainline/hidpp_dd_effect_math.h` | envelope shaping, condition-effect force, force-to-wire mapping | `tests/effect-math` |
| `mainline/hidpp_dd_texture_merge.h` | the engine-texture synthesis spliced into a native TrueForce stream | `tests/texture-merge` |

Run one with `make -C tests/<name> run`. Each needs a C compiler and the
kernel's uapi headers (`linux/input.h`), which every distribution ships. CI
runs both on every push to the driver.

## Why headers

A function that takes a struct and some integers and returns a number does
not need a kernel to be wrong in, and the bugs that hurt most here have been
exactly that kind: a wheel read as fully deflected before its first report,
a spring's sign dropped for negative coefficients, a value that wrapped where
it should have saturated. Anything in the driver that is pure belongs in one
of these headers, where a five-line test can hold it, rather than in
`hid-logitech-hidpp.c`, where only a wheel can.

The rule when adding to the driver: if it does not touch hardware or driver
state, put it in a header and give it a test here. If it does, keep it in the
`.c` and call in.
