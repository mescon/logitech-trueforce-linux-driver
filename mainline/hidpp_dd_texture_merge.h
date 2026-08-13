/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Engine-texture merge for the direct-drive TrueForce stream.
 *
 * Pure logic only: no kernel services beyond types and unaligned stores,
 * so tests/texture-merge can compile this file in userspace unchanged.
 * The interceptor in hid-logitech-hidpp.c owns locking and time.
 *
 * What this replicates: on Windows, G HUB synthesises an engine note from
 * the game's Escape RPM and merges it into the SDK's ep3 stream (native
 * c276 mode included; the game itself sends no texture). The tables here
 * are fitted to a real Windows AC EVO capture; see docs/TF_TEXTURE_RECIPE.md.
 */
#ifndef HIDPP_DD_TEXTURE_MERGE_H
#define HIDPP_DD_TEXTURE_MERGE_H

#ifdef __KERNEL__
#include <linux/types.h>
#include <linux/version.h>
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 12, 0)
#include <linux/unaligned.h>
#else
#include <asm/unaligned.h>
#endif
#else
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
typedef uint8_t u8;  typedef uint16_t u16; typedef uint32_t u32;
typedef uint64_t u64; typedef int16_t s16;  typedef int32_t s32;
typedef int64_t s64;
static inline void put_unaligned_le16(u16 v, void *p)
{
	u8 *b = p; b[0] = v & 0xff; b[1] = v >> 8;
}
static inline u32 get_unaligned_le32(const void *p)
{
	const u8 *b = p;

	return (u32)b[0] | ((u32)b[1] << 8) | ((u32)b[2] << 16) |
	       ((u32)b[3] << 24);
}
#endif

#define HIDPP_DD_TEXMERGE_FS		4000	/* sample clock, Hz */
#define HIDPP_DD_TEXMERGE_BLOCK		4	/* samples per spliced packet */
#define HIDPP_DD_TEXMERGE_WINDOW	13	/* window slots in a packet */
#define HIDPP_DD_TEXMERGE_HARMONICS	5
#define HIDPP_DD_TEXMERGE_STALE_NS	(200ULL * 1000 * 1000)
#define HIDPP_DD_TEXMERGE_MIN_RPM_X10	3000	/* below 300 rpm: silence */

struct hidpp_dd_texmerge_band {
	u32 f0_min_x100;			/* band lower edge, Hz x100 */
	u16 gain_q12[HIDPP_DD_TEXMERGE_HARMONICS]; /* h1..h5, Q12 */
	u16 amp_q8;				/* rms -> h1 amplitude, Q8 */
};

struct hidpp_dd_texmerge {
	u32 phase[HIDPP_DD_TEXMERGE_HARMONICS];	/* Q32 turns */
	s16 window[HIDPP_DD_TEXMERGE_WINDOW];	/* rolling sample history */
	u32 debt_q8;				/* fractional samples owed */
	u64 last_ns;				/* last splice decision time */
	u32 rpm_x10;
	u32 max_rpm_x10;
	u64 rpm_stamp_ns;
	u16 intensity;				/* percent, 100 = capture fit */
	u8 cylinders;				/* firing = rpm/60 * cyl/2 */
	bool enabled;
};

/* GENERATED-TABLES-BEGIN (tools/gen_texmerge_tables.py) */
static const s16 hidpp_dd_texmerge_sine_lut[1024] = {
	     0,    201,    402,    603,    804,   1005,   1206,   1407,
	  1608,   1809,   2009,   2210,   2410,   2611,   2811,   3012,
	  3212,   3412,   3612,   3811,   4011,   4210,   4410,   4609,
	  4808,   5007,   5205,   5404,   5602,   5800,   5998,   6195,
	  6393,   6590,   6786,   6983,   7179,   7375,   7571,   7767,
	  7962,   8157,   8351,   8545,   8739,   8933,   9126,   9319,
	  9512,   9704,   9896,  10087,  10278,  10469,  10659,  10849,
	 11039,  11228,  11417,  11605,  11793,  11980,  12167,  12353,
	 12539,  12725,  12910,  13094,  13279,  13462,  13645,  13828,
	 14010,  14191,  14372,  14553,  14732,  14912,  15090,  15269,
	 15446,  15623,  15800,  15976,  16151,  16325,  16499,  16673,
	 16846,  17018,  17189,  17360,  17530,  17700,  17869,  18037,
	 18204,  18371,  18537,  18703,  18868,  19032,  19195,  19357,
	 19519,  19680,  19841,  20000,  20159,  20317,  20475,  20631,
	 20787,  20942,  21096,  21250,  21403,  21554,  21705,  21856,
	 22005,  22154,  22301,  22448,  22594,  22739,  22884,  23027,
	 23170,  23311,  23452,  23592,  23731,  23870,  24007,  24143,
	 24279,  24413,  24547,  24680,  24811,  24942,  25072,  25201,
	 25329,  25456,  25582,  25708,  25832,  25955,  26077,  26198,
	 26319,  26438,  26556,  26674,  26790,  26905,  27019,  27133,
	 27245,  27356,  27466,  27575,  27683,  27790,  27896,  28001,
	 28105,  28208,  28310,  28411,  28510,  28609,  28706,  28803,
	 28898,  28992,  29085,  29177,  29268,  29358,  29447,  29534,
	 29621,  29706,  29791,  29874,  29956,  30037,  30117,  30195,
	 30273,  30349,  30424,  30498,  30571,  30643,  30714,  30783,
	 30852,  30919,  30985,  31050,  31113,  31176,  31237,  31297,
	 31356,  31414,  31470,  31526,  31580,  31633,  31685,  31736,
	 31785,  31833,  31880,  31926,  31971,  32014,  32057,  32098,
	 32137,  32176,  32213,  32250,  32285,  32318,  32351,  32382,
	 32412,  32441,  32469,  32495,  32521,  32545,  32567,  32589,
	 32609,  32628,  32646,  32663,  32678,  32692,  32705,  32717,
	 32728,  32737,  32745,  32752,  32757,  32761,  32765,  32766,
	 32767,  32766,  32765,  32761,  32757,  32752,  32745,  32737,
	 32728,  32717,  32705,  32692,  32678,  32663,  32646,  32628,
	 32609,  32589,  32567,  32545,  32521,  32495,  32469,  32441,
	 32412,  32382,  32351,  32318,  32285,  32250,  32213,  32176,
	 32137,  32098,  32057,  32014,  31971,  31926,  31880,  31833,
	 31785,  31736,  31685,  31633,  31580,  31526,  31470,  31414,
	 31356,  31297,  31237,  31176,  31113,  31050,  30985,  30919,
	 30852,  30783,  30714,  30643,  30571,  30498,  30424,  30349,
	 30273,  30195,  30117,  30037,  29956,  29874,  29791,  29706,
	 29621,  29534,  29447,  29358,  29268,  29177,  29085,  28992,
	 28898,  28803,  28706,  28609,  28510,  28411,  28310,  28208,
	 28105,  28001,  27896,  27790,  27683,  27575,  27466,  27356,
	 27245,  27133,  27019,  26905,  26790,  26674,  26556,  26438,
	 26319,  26198,  26077,  25955,  25832,  25708,  25582,  25456,
	 25329,  25201,  25072,  24942,  24811,  24680,  24547,  24413,
	 24279,  24143,  24007,  23870,  23731,  23592,  23452,  23311,
	 23170,  23027,  22884,  22739,  22594,  22448,  22301,  22154,
	 22005,  21856,  21705,  21554,  21403,  21250,  21096,  20942,
	 20787,  20631,  20475,  20317,  20159,  20000,  19841,  19680,
	 19519,  19357,  19195,  19032,  18868,  18703,  18537,  18371,
	 18204,  18037,  17869,  17700,  17530,  17360,  17189,  17018,
	 16846,  16673,  16499,  16325,  16151,  15976,  15800,  15623,
	 15446,  15269,  15090,  14912,  14732,  14553,  14372,  14191,
	 14010,  13828,  13645,  13462,  13279,  13094,  12910,  12725,
	 12539,  12353,  12167,  11980,  11793,  11605,  11417,  11228,
	 11039,  10849,  10659,  10469,  10278,  10087,   9896,   9704,
	  9512,   9319,   9126,   8933,   8739,   8545,   8351,   8157,
	  7962,   7767,   7571,   7375,   7179,   6983,   6786,   6590,
	  6393,   6195,   5998,   5800,   5602,   5404,   5205,   5007,
	  4808,   4609,   4410,   4210,   4011,   3811,   3612,   3412,
	  3212,   3012,   2811,   2611,   2410,   2210,   2009,   1809,
	  1608,   1407,   1206,   1005,    804,    603,    402,    201,
	     0,   -201,   -402,   -603,   -804,  -1005,  -1206,  -1407,
	 -1608,  -1809,  -2009,  -2210,  -2410,  -2611,  -2811,  -3012,
	 -3212,  -3412,  -3612,  -3811,  -4011,  -4210,  -4410,  -4609,
	 -4808,  -5007,  -5205,  -5404,  -5602,  -5800,  -5998,  -6195,
	 -6393,  -6590,  -6786,  -6983,  -7179,  -7375,  -7571,  -7767,
	 -7962,  -8157,  -8351,  -8545,  -8739,  -8933,  -9126,  -9319,
	 -9512,  -9704,  -9896, -10087, -10278, -10469, -10659, -10849,
	-11039, -11228, -11417, -11605, -11793, -11980, -12167, -12353,
	-12539, -12725, -12910, -13094, -13279, -13462, -13645, -13828,
	-14010, -14191, -14372, -14553, -14732, -14912, -15090, -15269,
	-15446, -15623, -15800, -15976, -16151, -16325, -16499, -16673,
	-16846, -17018, -17189, -17360, -17530, -17700, -17869, -18037,
	-18204, -18371, -18537, -18703, -18868, -19032, -19195, -19357,
	-19519, -19680, -19841, -20000, -20159, -20317, -20475, -20631,
	-20787, -20942, -21096, -21250, -21403, -21554, -21705, -21856,
	-22005, -22154, -22301, -22448, -22594, -22739, -22884, -23027,
	-23170, -23311, -23452, -23592, -23731, -23870, -24007, -24143,
	-24279, -24413, -24547, -24680, -24811, -24942, -25072, -25201,
	-25329, -25456, -25582, -25708, -25832, -25955, -26077, -26198,
	-26319, -26438, -26556, -26674, -26790, -26905, -27019, -27133,
	-27245, -27356, -27466, -27575, -27683, -27790, -27896, -28001,
	-28105, -28208, -28310, -28411, -28510, -28609, -28706, -28803,
	-28898, -28992, -29085, -29177, -29268, -29358, -29447, -29534,
	-29621, -29706, -29791, -29874, -29956, -30037, -30117, -30195,
	-30273, -30349, -30424, -30498, -30571, -30643, -30714, -30783,
	-30852, -30919, -30985, -31050, -31113, -31176, -31237, -31297,
	-31356, -31414, -31470, -31526, -31580, -31633, -31685, -31736,
	-31785, -31833, -31880, -31926, -31971, -32014, -32057, -32098,
	-32137, -32176, -32213, -32250, -32285, -32318, -32351, -32382,
	-32412, -32441, -32469, -32495, -32521, -32545, -32567, -32589,
	-32609, -32628, -32646, -32663, -32678, -32692, -32705, -32717,
	-32728, -32737, -32745, -32752, -32757, -32761, -32765, -32766,
	-32767, -32766, -32765, -32761, -32757, -32752, -32745, -32737,
	-32728, -32717, -32705, -32692, -32678, -32663, -32646, -32628,
	-32609, -32589, -32567, -32545, -32521, -32495, -32469, -32441,
	-32412, -32382, -32351, -32318, -32285, -32250, -32213, -32176,
	-32137, -32098, -32057, -32014, -31971, -31926, -31880, -31833,
	-31785, -31736, -31685, -31633, -31580, -31526, -31470, -31414,
	-31356, -31297, -31237, -31176, -31113, -31050, -30985, -30919,
	-30852, -30783, -30714, -30643, -30571, -30498, -30424, -30349,
	-30273, -30195, -30117, -30037, -29956, -29874, -29791, -29706,
	-29621, -29534, -29447, -29358, -29268, -29177, -29085, -28992,
	-28898, -28803, -28706, -28609, -28510, -28411, -28310, -28208,
	-28105, -28001, -27896, -27790, -27683, -27575, -27466, -27356,
	-27245, -27133, -27019, -26905, -26790, -26674, -26556, -26438,
	-26319, -26198, -26077, -25955, -25832, -25708, -25582, -25456,
	-25329, -25201, -25072, -24942, -24811, -24680, -24547, -24413,
	-24279, -24143, -24007, -23870, -23731, -23592, -23452, -23311,
	-23170, -23027, -22884, -22739, -22594, -22448, -22301, -22154,
	-22005, -21856, -21705, -21554, -21403, -21250, -21096, -20942,
	-20787, -20631, -20475, -20317, -20159, -20000, -19841, -19680,
	-19519, -19357, -19195, -19032, -18868, -18703, -18537, -18371,
	-18204, -18037, -17869, -17700, -17530, -17360, -17189, -17018,
	-16846, -16673, -16499, -16325, -16151, -15976, -15800, -15623,
	-15446, -15269, -15090, -14912, -14732, -14553, -14372, -14191,
	-14010, -13828, -13645, -13462, -13279, -13094, -12910, -12725,
	-12539, -12353, -12167, -11980, -11793, -11605, -11417, -11228,
	-11039, -10849, -10659, -10469, -10278, -10087,  -9896,  -9704,
	 -9512,  -9319,  -9126,  -8933,  -8739,  -8545,  -8351,  -8157,
	 -7962,  -7767,  -7571,  -7375,  -7179,  -6983,  -6786,  -6590,
	 -6393,  -6195,  -5998,  -5800,  -5602,  -5404,  -5205,  -5007,
	 -4808,  -4609,  -4410,  -4210,  -4011,  -3811,  -3612,  -3412,
	 -3212,  -3012,  -2811,  -2611,  -2410,  -2210,  -2009,  -1809,
	 -1608,  -1407,  -1206,  -1005,   -804,   -603,   -402,   -201,
};
static const struct hidpp_dd_texmerge_band hidpp_dd_texmerge_bands[5] = {
	{      0, { 4096, 1556,  901,  655,  573 },  325 },
	{  14000, { 4096,  451,  328,  205,   82 },  358 },
	{  19000, { 4096,  614,  369,  328,  123 },  355 },
	{  24000, { 4096,  942,  532,  328,  287 },  348 },
	{  29000, { 4096, 1106, 1024,  287,  205 },  339 },
};
/* GENERATED-TABLES-END */

static inline u32 hidpp_dd_texmerge_f0_x100(const struct hidpp_dd_texmerge *tm)
{
	/* firing frequency = rpm/60 * cylinders/2 (4-stroke)
	 * f0_x100 = (rpm_x10/10)/60 * cyl/2 * 100 = rpm_x10 * cyl * 10 / 120 */
	return (u32)(((u64)tm->rpm_x10 * tm->cylinders * 10) / 120);
}

static inline const struct hidpp_dd_texmerge_band *
hidpp_dd_texmerge_band_for(u32 f0_x100)
{
	int i;

	for (i = (int)(sizeof(hidpp_dd_texmerge_bands) /
		       sizeof(hidpp_dd_texmerge_bands[0])) - 1; i > 0; i--)
		if (f0_x100 >= hidpp_dd_texmerge_bands[i].f0_min_x100)
			return &hidpp_dd_texmerge_bands[i];
	return &hidpp_dd_texmerge_bands[0];
}

static inline s16 hidpp_dd_texmerge_next_sample(struct hidpp_dd_texmerge *tm,
						u32 f0_x100)
{
	const struct hidpp_dd_texmerge_band *band =
		hidpp_dd_texmerge_band_for(f0_x100);
	/* target rms in counts = 72 + 1.13 * f0_hz, from the capture fit */
	u32 rms = 72 + (113 * (f0_x100 / 100)) / 100;
	/* h1 amplitude in counts */
	u32 amp = (rms * band->amp_q8) >> 8;
	s32 acc = 0;
	int k;

	/* phase increment for h1, Q32 turns: f0 / FS per sample */
	u32 inc = (u32)(((u64)f0_x100 << 32) /
			(100ULL * HIDPP_DD_TEXMERGE_FS));

	for (k = 0; k < HIDPP_DD_TEXMERGE_HARMONICS; k++) {
		s32 lut;

		tm->phase[k] += inc * (k + 1);
		/* Nyquist guard: a harmonic at or past 0.45*FS aliases badly;
		 * drop it from the sum but keep advancing its phase so it
		 * resumes in step if f0 drops back into range. */
		if ((u64)(k + 1) * f0_x100 >= (u64)45 * HIDPP_DD_TEXMERGE_FS)
			continue;
		lut = hidpp_dd_texmerge_sine_lut[tm->phase[k] >> 22]; /* /2^22 = x1024 */
		acc += (lut * (s32)band->gain_q12[k]) >> 12;	/* Q15 of 1.0 */
	}
	/* scale by amplitude and intensity percent; clamp to s16 */
	{
		s32 v = (s32)(((s64)acc * amp) >> 15);

		v = (s32)(((s64)v * tm->intensity) / 100);
		if (v > 32767) v = 32767;
		if (v < -32768) v = -32768;
		return (s16)v;
	}
}

static inline bool hidpp_dd_texmerge_eligible(const u8 *buf, size_t len)
{
	return len == 64 && buf[0] == 0x01 && buf[4] == 0x01 && buf[10] == 0x00;
}

/*
 * Decode the rotation range from an SDK type-0x0e operating-range push.
 * Wire layout, confirmed against a live AC EVO usbmon capture: buf[0]=0x01,
 * buf[4]=0x0e, buf[5]=the push's sequence byte, and the range as an IEEE-754
 * float at bytes 6-9 little-endian. Two captured frames:
 *   90.0   = .. 0e <seq> 00 00 b4 42
 *   2700.0 = .. 0e <seq> 00 c0 28 45
 * Decoded without FP (90.0f = 0x42b40000): the exponent path covers
 * 1.0..4096.0, everything outside decodes to 0. Only the integer part of the
 * float matters. Returns whole degrees, 0 when out of coverage.
 */
static inline u32 hidpp_dd_texmerge_decode_push_deg(const u8 *buf)
{
	u32 fbits = get_unaligned_le32(&buf[6]);
	u32 exp = (fbits >> 23) & 0xff;

	if (exp < 127 || exp > 138)
		return 0;
	return (u32)(((fbits & 0x7fffff) | 0x800000) >> (23 - (exp - 127)));
}

static inline int hidpp_dd_texmerge_splice(struct hidpp_dd_texmerge *tm,
					   u8 *buf, size_t len, u64 now_ns)
{
	u32 f0;
	int i;

	if (!tm->enabled || !hidpp_dd_texmerge_eligible(buf, len))
		return 0;
	if (!tm->rpm_stamp_ns ||
	    now_ns - tm->rpm_stamp_ns > HIDPP_DD_TEXMERGE_STALE_NS)
		return 0;
	if (tm->rpm_x10 < HIDPP_DD_TEXMERGE_MIN_RPM_X10)
		return 0;

	/* sample-debt pacing: owe FS samples per second of wall time.
	 * last_ns == 0 is a legitimate prior timestamp (not just "never
	 * spliced"), so gate on ordering alone. */
	if (now_ns > tm->last_ns) {
		u64 dt = now_ns - tm->last_ns;

		if (dt > 1000000000ULL)	/* cap at 1s: caller was idle/first-call */
			dt = 1000000000ULL;
		tm->debt_q8 += (u32)((dt * HIDPP_DD_TEXMERGE_FS * 256) /
				     1000000000ULL);
		if (tm->debt_q8 > 16 * 256)
			tm->debt_q8 = 16 * 256;
	}
	tm->last_ns = now_ns;
	if (tm->debt_q8 < HIDPP_DD_TEXMERGE_BLOCK * 256)
		return 0;
	tm->debt_q8 -= HIDPP_DD_TEXMERGE_BLOCK * 256;

	f0 = hidpp_dd_texmerge_f0_x100(tm);
	/* shift the rolling window and append a fresh block */
	for (i = 0; i < HIDPP_DD_TEXMERGE_WINDOW - HIDPP_DD_TEXMERGE_BLOCK; i++)
		tm->window[i] = tm->window[i + HIDPP_DD_TEXMERGE_BLOCK];
	for (; i < HIDPP_DD_TEXMERGE_WINDOW; i++)
		tm->window[i] = hidpp_dd_texmerge_next_sample(tm, f0);

	buf[10] = HIDPP_DD_TEXMERGE_BLOCK;
	for (i = 0; i < HIDPP_DD_TEXMERGE_WINDOW; i++) {
		u16 v = (u16)(tm->window[i] + 32768); /* offset binary */

		put_unaligned_le16(v, &buf[12 + 4 * i]);
		put_unaligned_le16(v, &buf[14 + 4 * i]);
	}
	return HIDPP_DD_TEXMERGE_BLOCK;
}

#endif /* HIDPP_DD_TEXTURE_MERGE_H */
