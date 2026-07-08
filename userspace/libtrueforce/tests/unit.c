// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * Unit tests for libtrueforce's pure-logic helpers.
 *
 * Covers the wire-format conversions used by the streaming path,
 * which are free of hidraw/evdev I/O and therefore safe to run in
 * CI without a wheel attached. Exits 0 on success, 1 on the first
 * assertion failure (printing the test name so the failing case is
 * obvious in a CI log).
 */

#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include <trueforce.h>
#include "internal.h"

#define EXPECT_EQ(label, got, want)					\
	do {								\
		uint32_t _g = (uint32_t)(got);				\
		uint32_t _w = (uint32_t)(want);				\
		if (_g != _w) {						\
			fprintf(stderr, "FAIL %s: got 0x%04x want 0x%04x\n", \
				label, _g, _w);				\
			return 1;					\
		}							\
	} while (0)

#define EXPECT_NEAR(label, got, want, tol)				\
	do {								\
		int32_t _g = (int32_t)(got);				\
		int32_t _w = (int32_t)(want);				\
		int32_t _d = _g - _w;					\
		if (_d < -(tol) || _d > (tol)) {			\
			fprintf(stderr, "FAIL %s: got 0x%04x want ~0x%04x (tol %d)\n", \
				label, (unsigned)_g, (unsigned)_w,	\
				(int)(tol));				\
			return 1;					\
		}							\
	} while (0)

static int test_s16_to_wire(void)
{
	/*
	 * logitf_s16_to_wire shifts signed int16 range [-32768..32767]
	 * to offset-binary [0..65535] with 0x8000 as the neutral
	 * centre. Match what G Hub writes on the wire.
	 */
	EXPECT_EQ("s16:zero",      logitf_s16_to_wire(0),       0x8000);
	EXPECT_EQ("s16:max_pos",   logitf_s16_to_wire(32767),   0xFFFF);
	EXPECT_EQ("s16:max_neg",   logitf_s16_to_wire(-32768),  0x0000);
	EXPECT_EQ("s16:one",       logitf_s16_to_wire(1),       0x8001);
	EXPECT_EQ("s16:neg_one",   logitf_s16_to_wire(-1),      0x7FFF);
	EXPECT_EQ("s16:half_pos",  logitf_s16_to_wire(16384),   0xC000);
	EXPECT_EQ("s16:half_neg",  logitf_s16_to_wire(-16384),  0x4000);
	return 0;
}

static int test_float_to_wire(void)
{
	/*
	 * logitf_float_to_wire clamps to [-1.0, +1.0] and scales by
	 * 32767 before adding the 0x8000 centre offset. Because the
	 * conversion truncates via (int) cast rather than rounding,
	 * boundary outputs are exact but interior points can be off
	 * by up to 1 LSB from a naive scaling, hence the tol=1 near
	 * checks for fractional inputs.
	 */
	EXPECT_EQ("f:zero",        logitf_float_to_wire(0.0f),   0x8000);
	EXPECT_EQ("f:plus_one",    logitf_float_to_wire(1.0f),   0xFFFF);
	EXPECT_EQ("f:minus_one",   logitf_float_to_wire(-1.0f),  0x0001);
	EXPECT_EQ("f:over_plus",   logitf_float_to_wire(2.5f),   0xFFFF);
	EXPECT_EQ("f:over_minus",  logitf_float_to_wire(-3.0f),  0x0001);
	EXPECT_NEAR("f:half_pos",  logitf_float_to_wire(0.5f),   0xC000, 1);
	EXPECT_NEAR("f:half_neg",  logitf_float_to_wire(-0.5f),  0x4000, 1);
	EXPECT_NEAR("f:quarter",   logitf_float_to_wire(0.25f),  0xA000, 1);
	return 0;
}

static int test_wire_monotonic(void)
{
	/*
	 * The streaming code relies on s16_to_wire being strictly
	 * monotonic: increasing input must produce non-decreasing
	 * output. If this ever breaks we'd get audible TF jitter at
	 * zero crossings.
	 */
	uint16_t prev = 0;
	int32_t s;

	for (s = -32768; s <= 32767; s++) {
		uint16_t w = logitf_s16_to_wire((int16_t)s);

		if (s != -32768 && w < prev) {
			fprintf(stderr,
				"FAIL s16 monotonic: at s=%d got 0x%04x < prev 0x%04x\n",
				(int)s, w, prev);
			return 1;
		}
		prev = w;
	}
	return 0;
}

int main(void)
{
	struct {
		const char *name;
		int (*fn)(void);
	} tests[] = {
		{ "s16_to_wire",      test_s16_to_wire },
		{ "float_to_wire",    test_float_to_wire },
		{ "wire_monotonic",   test_wire_monotonic },
	};
	size_t i;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		int rc = tests[i].fn();

		if (rc) {
			fprintf(stderr, "test %s failed\n", tests[i].name);
			return 1;
		}
		printf("ok %s\n", tests[i].name);
	}
	printf("1..%zu\n", sizeof(tests) / sizeof(tests[0]));
	return 0;
}
