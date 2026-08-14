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

static void test_oscillator_phase_continuity(void)
{
	/* The max-step bound above cannot tell a continued oscillator from
	 * one whose phases were silently reset: a reset landing near a zero
	 * crossing produces no visible step at all. Pin the phase state
	 * directly instead. At 250 Hz the h1 phase increment is exactly
	 * 1/16 turn per sample (2^32 * 250 / 4000 in Q32), so after 4001
	 * samples the continued oscillator's next sample evaluates h1 at
	 * 45 degrees while a phase-zeroed twin's next evaluates it at
	 * 22.5 degrees - all integer math, fully deterministic. */
	struct hidpp_dd_texmerge a = { .intensity = 100 };
	struct hidpp_dd_texmerge b, r;
	s16 cont_a, cont_b, reset;

	for (int i = 0; i < 4001; i++)
		(void)hidpp_dd_texmerge_next_sample(&a, 25000);
	b = a;			/* continued twin: same phases */
	r = a;			/* reset twin: fresh phases */
	memset(r.phase, 0, sizeof(r.phase));

	cont_a = hidpp_dd_texmerge_next_sample(&a, 25000);
	cont_b = hidpp_dd_texmerge_next_sample(&b, 25000);
	reset = hidpp_dd_texmerge_next_sample(&r, 25000);

	/* Twin determinism first, so the inequality below is meaningful. */
	CHECK(cont_a == cont_b, "continued twins diverge (%d vs %d)",
	      cont_a, cont_b);
	/* A reset sequence must be DISTINGUISHABLE from a continued one:
	 * sin(45) vs sin(22.5) of the h1 amplitude alone is ~0.3x amp,
	 * far above any rounding slack. */
	CHECK(cont_a != reset, "phase reset not detected (both %d)", cont_a);
	CHECK(abs((int)cont_a - (int)reset) > 50,
	      "reset barely detectable (%d vs %d)", cont_a, reset);
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

static void test_range_push_decode(void)
{
	/* The two REAL captured type-0x0e frames (AC EVO usbmon captures):
	 * a 90-degree push and a 2700-degree push. Bytes 6-9 carry the range
	 * as an IEEE-754 float little-endian; the driver decodes it without
	 * FP in hidpp_dd_texmerge_decode_push_deg, the same expression
	 * hidpp_dd_texmerge_seen_range_push runs on live pushes. */
	static const u8 push_90[10] = {
		0x01, 0x00, 0x00, 0x00, 0x0e, 0x46, 0x00, 0x00, 0xb4, 0x42,
	};
	static const u8 push_2700[10] = {
		0x01, 0x00, 0x00, 0x00, 0x0e, 0x32, 0x00, 0xc0, 0x28, 0x45,
	};
	u8 tiny[10];

	CHECK(hidpp_dd_texmerge_decode_push_deg(push_90) == 90,
	      "90-deg push decodes to %u, want 90",
	      hidpp_dd_texmerge_decode_push_deg(push_90));
	CHECK(hidpp_dd_texmerge_decode_push_deg(push_2700) == 2700,
	      "2700-deg push decodes to %u, want 2700",
	      hidpp_dd_texmerge_decode_push_deg(push_2700));

	/* exponents outside the 1.0..4096.0 coverage decode to 0 */
	memcpy(tiny, push_90, sizeof(tiny));
	tiny[6] = 0x00; tiny[7] = 0x00; tiny[8] = 0x00; tiny[9] = 0x3f; /* 0.5f */
	CHECK(hidpp_dd_texmerge_decode_push_deg(tiny) == 0,
	      "0.5f decodes to %u, want 0",
	      hidpp_dd_texmerge_decode_push_deg(tiny));
}

static void test_fixture_range_pushes(void)
{
	/* The committed capture fixtures (fixtures/README.md): the three real
	 * type-0x0e frames from the 2026-08-13/14 validation session, decoded
	 * through the same helper the driver runs on live pushes. Run from
	 * the tests/texture-merge directory (as `make run` does). */
	static const struct { const char *file; unsigned want; } fx[] = {
		{ "fixtures/range_push_2700_pass1.bin", 2700 },
		{ "fixtures/range_push_90.bin", 90 },
		{ "fixtures/range_push_2700_pass2.bin", 2700 },
	};
	for (unsigned i = 0; i < sizeof(fx) / sizeof(fx[0]); i++) {
		u8 frame[64];
		FILE *f = fopen(fx[i].file, "rb");

		CHECK(f != NULL, "missing fixture %s", fx[i].file);
		if (!f)
			continue;
		CHECK(fread(frame, 1, sizeof(frame), f) == sizeof(frame),
		      "%s is not 64 bytes", fx[i].file);
		fclose(f);
		CHECK(frame[0] == 0x01 && frame[4] == 0x0e,
		      "%s is not a type-0x0e frame", fx[i].file);
		CHECK(hidpp_dd_texmerge_decode_push_deg(frame) == fx[i].want,
		      "%s decodes to %u, want %u", fx[i].file,
		      hidpp_dd_texmerge_decode_push_deg(frame), fx[i].want);
	}
}

static void mk_stream_pkt(u8 *pkt, u16 cur, u8 seq)
{
	memset(pkt, 0, 64);
	pkt[0] = 0x01; pkt[4] = 0x01; pkt[5] = seq;
	put_unaligned_le16(cur, &pkt[6]); put_unaligned_le16(cur, &pkt[8]);
	pkt[10] = 0; pkt[11] = 0; /* real SDK packets are byte10/11 = 00/00 */
}

static void test_eligibility(void)
{
	u8 pkt[64];

	mk_stream_pkt(pkt, 0x8000, 7);
	CHECK(hidpp_dd_texmerge_eligible(pkt, 64), "stream pkt not eligible");
	CHECK(!hidpp_dd_texmerge_eligible(pkt, 63), "short pkt eligible");
	pkt[4] = 0x0e;
	CHECK(!hidpp_dd_texmerge_eligible(pkt, 64), "range push eligible");
	mk_stream_pkt(pkt, 0x8000, 7); pkt[10] = 4;
	CHECK(!hidpp_dd_texmerge_eligible(pkt, 64),
	      "pkt with own samples eligible");
	mk_stream_pkt(pkt, 0x8000, 7); pkt[0] = 0x02;
	CHECK(!hidpp_dd_texmerge_eligible(pkt, 64), "wrong report id eligible");
}

static void test_splice_preserves_base(void)
{
	struct hidpp_dd_texmerge tm = {
		.enabled = true, .intensity = 100, .cylinders = 8,
		.rpm_x10 = 60000, .rpm_stamp_ns = 1000000,
	};
	u8 pkt[64], orig[64];
	u64 t = 2000000; /* fresh vs rpm_stamp */
	int n;

	mk_stream_pkt(pkt, 0xa1b2, 42);
	memcpy(orig, pkt, 64);
	/* prime the debt so a splice happens immediately */
	tm.last_ns = t - 2000000; /* 2 ms elapsed = 8 samples owed */
	n = hidpp_dd_texmerge_splice(&tm, pkt, 64, t);
	CHECK(n == HIDPP_DD_TEXMERGE_BLOCK, "spliced %d, want %d", n,
	      HIDPP_DD_TEXMERGE_BLOCK);
	CHECK(memcmp(pkt, orig, 10) == 0, "bytes 0-9 modified");
	CHECK(pkt[10] == HIDPP_DD_TEXMERGE_BLOCK, "byte10 = %d", pkt[10]);
	CHECK(pkt[11] == 0x0d, "byte 11 = %#x, want 0x0d (texture marker)", pkt[11]);
	/* window slots are duplicated u16 pairs */
	for (int i = 0; i < HIDPP_DD_TEXMERGE_WINDOW; i++)
		CHECK(memcmp(&pkt[12 + 4 * i], &pkt[14 + 4 * i], 2) == 0,
		      "slot %d not duplicated", i);
}

static void test_splice_gates(void)
{
	struct hidpp_dd_texmerge tm = {
		.enabled = true, .intensity = 100, .cylinders = 8,
		.rpm_x10 = 60000, .rpm_stamp_ns = 1000000, .last_ns = 0,
	};
	u8 pkt[64], orig[64];

	mk_stream_pkt(pkt, 0x8000, 1);
	memcpy(orig, pkt, 64);
	/* stale rpm: stamp 1 ms, now 1 ms + STALE + 1 -> untouched */
	CHECK(hidpp_dd_texmerge_splice(&tm, pkt, 64,
		1000000 + HIDPP_DD_TEXMERGE_STALE_NS + 1) == 0, "spliced stale");
	CHECK(memcmp(pkt, orig, 64) == 0, "stale rpm modified pkt");
	/* disabled -> untouched */
	tm.enabled = false; tm.rpm_stamp_ns = 1000000;
	CHECK(hidpp_dd_texmerge_splice(&tm, pkt, 64, 2000000) == 0,
	      "spliced while disabled");
	/* below idle threshold -> untouched */
	tm.enabled = true; tm.rpm_x10 = HIDPP_DD_TEXMERGE_MIN_RPM_X10 - 1;
	CHECK(hidpp_dd_texmerge_splice(&tm, pkt, 64, 2000000) == 0,
	      "spliced below idle");
}

static void test_splice_pacing(void)
{
	/* At 2000 pkts/s and FS=4000, every second eligible packet gets a
	 * 4-sample block: over 100 packets 500 us apart, ~50 splices. */
	struct hidpp_dd_texmerge tm = {
		.enabled = true, .intensity = 100, .cylinders = 8,
		.rpm_x10 = 60000, .rpm_stamp_ns = 1,
	};
	u8 pkt[64];
	int spliced = 0;
	u64 t = 1;

	tm.last_ns = t;
	for (int i = 0; i < 100; i++) {
		t += 500000; /* 500 us */
		tm.rpm_stamp_ns = t; /* keep fresh */
		mk_stream_pkt(pkt, 0x8000, i & 0xff);
		if (hidpp_dd_texmerge_splice(&tm, pkt, 64, t) > 0)
			spliced++;
	}
	CHECK(spliced >= 45 && spliced <= 55, "spliced %d/100, want ~50", spliced);
}

static void test_nyquist_guard(void)
{
	/* At f0 = 600 Hz (V8 at 9000 rpm), h3 (1800 Hz) sits exactly at the
	 * 0.45*FS threshold and h4/h5 exceed it; all three are silenced
	 * (threshold is >=). The output must contain no alias energy. Compare rms against an h1-h2-only
	 * expectation: with band 290+ gains (h4/h5 small), dropping h4/h5
	 * changes rms by only a few percent, so simply assert the sample
	 * stream stays finite and the rms is within 20% of the target
	 * formula (alias energy folding back would inflate it well past
	 * that at these frequencies).
	 */
	struct hidpp_dd_texmerge tm = { .intensity = 100 };
	double sum2 = 0;
	int n = 8000;
	for (int i = 0; i < n; i++) {
		s16 s = hidpp_dd_texmerge_next_sample(&tm, 60000);
		sum2 += (double)s * s;
	}
	double rms = sqrt(sum2 / n);
	double target = 72 + 1.13 * 600;
	CHECK(fabs(rms - target) / target < 0.20,
	      "rms at 600Hz = %.1f, want ~%.1f (alias guard)", rms, target);
}

int main(void)
{
	test_lut_sanity();
	test_f0();
	test_oscillator_rms();
	test_oscillator_continuity();
	test_oscillator_phase_continuity();
	test_intensity_scales();
	test_range_push_decode();
	test_fixture_range_pushes();
	test_eligibility();
	test_splice_preserves_base();
	test_splice_gates();
	test_splice_pacing();
	test_nyquist_guard();
	printf(failures ? "%d FAILURES\n" : "all tests pass\n", failures);
	return failures ? 1 : 0;
}
