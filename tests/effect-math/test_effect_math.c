/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Userspace tests for hidpp_dd_effect_math.h: the arithmetic that decides
 * what torque the motor is asked for. Same harness style as
 * tests/texture-merge, no kernel needed.
 */
#include <stdio.h>
#include <string.h>
#include "hidpp_dd_effect_math.h"

static int failures;
#define CHECK(cond, ...) do { \
	if (!(cond)) { \
		failures++; \
		fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); \
		fprintf(stderr, __VA_ARGS__); \
		fputc('\n', stderr); \
	} \
} while (0)

/* ---- envelope ---------------------------------------------------------- */

static void test_no_envelope_passes_magnitude_through(void)
{
	struct ff_envelope env;
	memset(&env, 0, sizeof(env));
	CHECK(hidpp_dd_apply_envelope(&env, 1000, 0, 0) == 1000, "flat envelope, t=0");
	CHECK(hidpp_dd_apply_envelope(&env, 1000, 5000, 0) == 1000, "flat envelope, infinite length");
	CHECK(hidpp_dd_apply_envelope(&env, -1000, 5000, 0) == -1000, "sign preserved");
}

static void test_attack_ramps_from_attack_level(void)
{
	struct ff_envelope env;
	memset(&env, 0, sizeof(env));
	env.attack_length = 100;
	env.attack_level = 0;
	s32 start = hidpp_dd_apply_envelope(&env, 1000, 0, 0);
	s32 mid = hidpp_dd_apply_envelope(&env, 1000, 50, 0);
	s32 end = hidpp_dd_apply_envelope(&env, 1000, 100, 0);
	CHECK(start == 0, "attack starts at attack_level (got %d)", start);
	CHECK(mid > 400 && mid < 600, "halfway through the attack is about half (got %d)", mid);
	CHECK(end == 1000, "attack ends at full magnitude (got %d)", end);
}

static void test_fade_only_applies_with_a_finite_length(void)
{
	struct ff_envelope env;
	memset(&env, 0, sizeof(env));
	env.fade_length = 100;
	env.fade_level = 0;
	/* length 0 is infinite: no fade, ever. */
	CHECK(hidpp_dd_apply_envelope(&env, 1000, 100000, 0) == 1000, "infinite effects never fade");
	/* length 1000 ms: the fade covers the last 100 ms. */
	s32 before = hidpp_dd_apply_envelope(&env, 1000, 850, 1000);
	s32 late = hidpp_dd_apply_envelope(&env, 1000, 950, 1000);
	CHECK(before == 1000, "full before the fade window (got %d)", before);
	CHECK(late > 400 && late < 600, "halfway through the fade is about half (got %d)", late);
}

/* ---- conditions -------------------------------------------------------- */

static struct ff_condition_effect spring(s16 coeff, u16 sat, u16 deadband)
{
	struct ff_condition_effect c;
	memset(&c, 0, sizeof(c));
	c.right_coeff = coeff;
	c.left_coeff = coeff;
	c.right_saturation = sat;
	c.left_saturation = sat;
	c.deadband = deadband;
	c.center = 0;
	return c;
}

static void test_centred_wheel_gets_no_condition_force(void)
{
	/*
	 * The 0.39.0 guard reads an unreported wheel as centred and still.
	 * This is what that guard relies on: centre must mean zero for a
	 * spring with no offset, or an untouched wheel would be pulled.
	 */
	struct ff_condition_effect c = spring(0x7fff, 0x7fff, 0);
	CHECK(hidpp_dd_condition_force(&c, 0) == 0, "no deflection, no force");
}

static void test_spring_restores_towards_centre(void)
{
	struct ff_condition_effect c = spring(0x7fff, 0x7fff, 0);
	s32 right = hidpp_dd_condition_force(&c, 1000);
	s32 left = hidpp_dd_condition_force(&c, -1000);
	CHECK(right < 0, "deflected right, pushed back left (got %d)", right);
	CHECK(left > 0, "deflected left, pushed back right (got %d)", left);
	/*
	 * Within one count, not exactly: the coefficient product is scaled
	 * with an arithmetic shift, which rounds towards minus infinity, so
	 * the two sides differ by one LSB in 32767. Found by this test on its
	 * first run; not worth a division in a 1 kHz path.
	 */
	CHECK(abs(right + left) <= 1, "symmetric about centre to within a count (%d vs %d)", right, left);
}

static void test_deadband_is_a_dead_zone(void)
{
	struct ff_condition_effect c = spring(0x7fff, 0x7fff, 200);
	CHECK(hidpp_dd_condition_force(&c, 50) == 0, "inside the deadband, nothing");
	CHECK(hidpp_dd_condition_force(&c, -50) == 0, "inside on the other side, nothing");
	CHECK(hidpp_dd_condition_force(&c, 500) != 0, "outside it, force");
}

static void test_saturation_clips_both_signs(void)
{
	struct ff_condition_effect c = spring(0x7fff, 100, 0);
	s32 right = hidpp_dd_condition_force(&c, 20000);
	s32 left = hidpp_dd_condition_force(&c, -20000);
	CHECK(right == -100, "a hard right deflection clips at -saturation (got %d)", right);
	CHECK(left == 100, "a hard left deflection clips at +saturation (got %d)", left);
}

static void test_anti_spring_is_not_dropped(void)
{
	/*
	 * A negative coefficient is legal (the field is signed) and is how
	 * oversteer effects push AWAY from centre. An earlier revision kept
	 * only restoring forces and silently zeroed these.
	 */
	struct ff_condition_effect c = spring(-0x7fff, 100, 0);
	s32 right = hidpp_dd_condition_force(&c, 20000);
	CHECK(right == 100, "anti-spring pushes further right, clipped at +saturation (got %d)", right);
}

/* ---- acceleration filter (FF_INERTIA) ---------------------------------- */

#define TAU 50
#define TICK 1

static void test_steady_acceleration_reads_as_itself(void)
{
	/*
	 * The whole point of taking the gap behind a one-pole: under a
	 * constant acceleration a0 the lag settles at a0 * tau, so the
	 * estimate settles at a0 and the INERTIA scale needs no retuning.
	 * Ramp the velocity by 3 counts per tick and wait ten time
	 * constants; the result is in 1/256ths.
	 */
	s32 slow = 0, a = 0, vel = 0;
	int t;

	for (t = 0; t < 10 * TAU; t++) {
		vel += 3;
		a = hidpp_dd_accel_filter(&slow, vel, TICK, TAU);
	}
	CHECK(a >= 3 * 256 - 8 && a <= 3 * 256, "steady 3 counts/tick^2 reads as ~768 q8 (got %d)", a);
}

static void test_quantisation_blips_are_spread_not_spiked(void)
{
	/*
	 * A rim turning at half a count per tick reads on a quantised
	 * encoder as velocity 1, 0, 1, 0, ... The old per-tick difference
	 * made that +1, -1, +1, -1: a full count of "acceleration" every
	 * tick, 256 in these units, and grain on the rim. Band-limited,
	 * the estimate never leaves a small band around zero.
	 */
	s32 slow = 0, a, worst = 0;
	int t;

	for (t = 0; t < 20 * TAU; t++) {
		a = hidpp_dd_accel_filter(&slow, t & 1, TICK, TAU);
		if (a > worst)
			worst = a;
		if (-a > worst)
			worst = -a;
	}
	CHECK(worst <= 8, "alternating 1/0 velocity stays within 8 q8 of zero (worst %d, raw would be 256)", worst);
}

static void test_a_velocity_step_peaks_at_step_over_tau_and_decays(void)
{
	/*
	 * A single step of dv shows up as a peak of dv / tau that decays
	 * over tau, instead of a one-tick spike of dv. Step from rest to
	 * 100 counts per tick and hold.
	 */
	s32 slow = 0, first, later, late;
	int t;

	first = hidpp_dd_accel_filter(&slow, 100, TICK, TAU);
	CHECK(first > 0 && first <= 100 * 256 / TAU, "peak is at most dv/tau = 512 q8 (got %d)", first);
	for (t = 0; t < TAU; t++)
		later = hidpp_dd_accel_filter(&slow, 100, TICK, TAU);
	CHECK(later < first / 2, "one tau later it has decayed below half (got %d after %d)", later, first);
	for (t = 0; t < 10 * TAU; t++)
		late = hidpp_dd_accel_filter(&slow, 100, TICK, TAU);
	CHECK(late == 0, "at a steady velocity the estimate returns to exactly zero (got %d)", late);
}

static void test_rest_reads_exactly_zero(void)
{
	/*
	 * After a stop the tracker snaps onto the velocity once it is
	 * within a step of it, so a still wheel reads 0 and not the
	 * residue of an integer division: an INERTIA effect on a parked
	 * car must not hold a standing force.
	 */
	s32 slow = 0, a = 1;
	int t;

	for (t = 0; t < 2 * TAU; t++)
		hidpp_dd_accel_filter(&slow, 40, TICK, TAU);
	for (t = 0; t < 10 * TAU; t++)
		a = hidpp_dd_accel_filter(&slow, 0, TICK, TAU);
	CHECK(a == 0 && slow == 0, "rest reads zero acceleration with the tracker parked (a %d, tracker %d)", a, slow);
}

static void test_filter_is_sign_symmetric(void)
{
	s32 sp = 0, sn = 0, ap = 0, an = 0, vel = 0;
	int t;

	for (t = 0; t < 5 * TAU; t++) {
		vel += 2;
		ap = hidpp_dd_accel_filter(&sp, vel, TICK, TAU);
		an = hidpp_dd_accel_filter(&sn, -vel, TICK, TAU);
	}
	CHECK(ap == -an, "turning left mirrors turning right (%d vs %d)", ap, an);
}

/* ---- wire mapping ------------------------------------------------------ */

static void test_wire_mapping_is_offset_binary_and_saturates(void)
{
	CHECK(hidpp_dd_force_to_offset_binary(0) == 0x8000, "zero force is centre");
	CHECK(hidpp_dd_force_to_offset_binary(S16_MAX) == 0xffff, "full right is top");
	CHECK(hidpp_dd_force_to_offset_binary(S16_MIN) == 0x0000, "full left is bottom");
	CHECK(hidpp_dd_force_to_offset_binary(100000) == 0xffff, "over-range saturates, never wraps");
	CHECK(hidpp_dd_force_to_offset_binary(-100000) == 0x0000, "under-range saturates, never wraps");
}

int main(void)
{
	test_no_envelope_passes_magnitude_through();
	test_attack_ramps_from_attack_level();
	test_fade_only_applies_with_a_finite_length();
	test_centred_wheel_gets_no_condition_force();
	test_spring_restores_towards_centre();
	test_deadband_is_a_dead_zone();
	test_saturation_clips_both_signs();
	test_anti_spring_is_not_dropped();
	test_steady_acceleration_reads_as_itself();
	test_quantisation_blips_are_spread_not_spiked();
	test_a_velocity_step_peaks_at_step_over_tau_and_decays();
	test_rest_reads_exactly_zero();
	test_filter_is_sign_symmetric();
	test_wire_mapping_is_offset_binary_and_saturates();
	if (failures) {
		fprintf(stderr, "%d failure(s)\n", failures);
		return 1;
	}
	puts("all tests pass");
	return 0;
}
