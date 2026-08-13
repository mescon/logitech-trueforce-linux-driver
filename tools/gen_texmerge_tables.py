#!/usr/bin/env python3
"""Generate the fixed-point tables for hidpp_dd_texture_merge.h.

Run: python3 tools/gen_texmerge_tables.py
Paste the output into mainline/hidpp_dd_texture_merge.h between the
GENERATED-TABLES-BEGIN/END markers. Values derive from the measured
Windows capture (docs/TF_TEXTURE_RECIPE.md, 2026-08-13 table).
"""
import math

N = 1024
print("/* GENERATED-TABLES-BEGIN (tools/gen_texmerge_tables.py) */")
print("static const s16 hidpp_dd_texmerge_sine_lut[%d] = {" % N)
row = []
for i in range(N):
    v = round(32767 * math.sin(2 * math.pi * i / N))
    row.append("%6d," % v)
    if len(row) == 8:
        print("\t" + " ".join(row)); row = []
if row:
    print("\t" + " ".join(row))
print("};")

# f0 bands from the measured table. g ratios are h2..h5 relative to h1.
# amp_q8 = 256 / sqrt(sum(g_k^2)/2) with g1=1: converts target rms counts
# to the h1 amplitude in counts.
bands = [
    #  f0_min_x100, h2,   h3,   h4,   h5      (measured medians)
    (0,      0.38, 0.22, 0.16, 0.14),   # up to 140 Hz (idle band)
    (14000,  0.11, 0.08, 0.05, 0.02),   # 140-190
    (19000,  0.15, 0.09, 0.08, 0.03),   # 190-240
    (24000,  0.23, 0.13, 0.08, 0.07),   # 240-290
    (29000,  0.27, 0.25, 0.07, 0.05),   # 290+ (clamped above)
]
print("static const struct hidpp_dd_texmerge_band hidpp_dd_texmerge_bands[%d] = {"
      % len(bands))
for f0min, h2, h3, h4, h5 in bands:
    ssum = 1.0 + h2*h2 + h3*h3 + h4*h4 + h5*h5
    amp_q8 = round(256.0 / math.sqrt(ssum / 2.0))
    q = lambda g: round(g * 4096)
    print("\t{ %6d, { 4096, %4d, %4d, %4d, %4d }, %4d }," %
          (f0min, q(h2), q(h3), q(h4), q(h5), amp_q8))
print("};")
print("/* GENERATED-TABLES-END */")
