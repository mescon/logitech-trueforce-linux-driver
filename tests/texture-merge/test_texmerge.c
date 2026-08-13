/* Userspace tests for the pure texture-merge logic. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "hidpp_dd_texture_merge.h"

static int failures;
#define CHECK(cond, ...) do { \
	if (!(cond)) { failures++; printf("FAIL %s:%d: ", __FILE__, __LINE__); \
		       printf(__VA_ARGS__); printf("\n"); } \
} while (0)

static void test_lut_sanity(void)
{
	CHECK(hidpp_dd_texmerge_sine_lut[0] == 0, "sin(0) != 0");
	CHECK(hidpp_dd_texmerge_sine_lut[256] == 32767, "sin(pi/2) != max");
	CHECK(hidpp_dd_texmerge_sine_lut[512] == 0, "sin(pi) != 0");
	CHECK(hidpp_dd_texmerge_sine_lut[768] == -32767, "sin(3pi/2) != min");
}

static void test_f0(void)
{
	struct hidpp_dd_texmerge tm = { .cylinders = 8, .rpm_x10 = 60000 };
	/* 6000 rpm V8: 6000/60*4 = 400 Hz = 40000 x100 */
	CHECK(hidpp_dd_texmerge_f0_x100(&tm) == 40000,
	      "f0 for 6000rpm V8 = %u, want 40000", hidpp_dd_texmerge_f0_x100(&tm));
	tm.cylinders = 6; tm.rpm_x10 = 30000;
	/* 3000 rpm V6: 3000/60*3 = 150 Hz */
	CHECK(hidpp_dd_texmerge_f0_x100(&tm) == 15000,
	      "f0 for 3000rpm V6 = %u, want 15000", hidpp_dd_texmerge_f0_x100(&tm));
}

static void test_oscillator_rms(void)
{
	/* At f0=250 Hz (band 240-290), target rms = 72 + 1.13*250 = 354.5
	 * counts at intensity 100. Accept 15% tolerance (integer tables). */
	struct hidpp_dd_texmerge tm = { .intensity = 100 };
	double sum2 = 0;
	int n = 8000;
	for (int i = 0; i < n; i++) {
		s16 s = hidpp_dd_texmerge_next_sample(&tm, 25000);
		sum2 += (double)s * s;
	}
	double rms = sqrt(sum2 / n);
	CHECK(fabs(rms - 354.5) / 354.5 < 0.15,
	      "rms at 250Hz = %.1f, want ~354.5", rms);
}

static void test_oscillator_continuity(void)
{
	/* No sample-to-sample jump may exceed the physical slew of the
	 * loudest legal waveform: sum of per-harmonic max slews. */
	struct hidpp_dd_texmerge tm = { .intensity = 100 };
	s16 prev = hidpp_dd_texmerge_next_sample(&tm, 30000);
	double max_step = 0;
	for (int i = 0; i < 4000; i++) {
		s16 s = hidpp_dd_texmerge_next_sample(&tm, 30000);
		double d = fabs((double)s - prev);
		if (d > max_step) max_step = d;
		prev = s;
	}
	/* 300 Hz h5 = 1500 Hz max component; sin step <= 2*pi*f/FS * A.
	 * Generous bound: 1500 counts. A discontinuity bug shows as ~2x rms. */
	CHECK(max_step < 1500, "max step %.0f too large (discontinuity)", max_step);
}

static void test_intensity_scales(void)
{
	struct hidpp_dd_texmerge a = { .intensity = 100 };
	struct hidpp_dd_texmerge b = { .intensity = 50 };
	double s2a = 0, s2b = 0;
	for (int i = 0; i < 4000; i++) {
		s16 sa = hidpp_dd_texmerge_next_sample(&a, 20000);
		s16 sb = hidpp_dd_texmerge_next_sample(&b, 20000);
		s2a += (double)sa * sa; s2b += (double)sb * sb;
	}
	double ratio = sqrt(s2b / s2a);
	CHECK(fabs(ratio - 0.5) < 0.05, "intensity 50 ratio %.2f, want 0.50", ratio);
}

int main(void)
{
	test_lut_sanity();
	test_f0();
	test_oscillator_rms();
	test_oscillator_continuity();
	test_intensity_scales();
	printf(failures ? "%d FAILURES\n" : "all tests pass\n", failures);
	return failures ? 1 : 0;
}
