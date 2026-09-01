/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Pure force-feedback arithmetic for the direct-drive engine: envelope
 * shaping, condition effects, and the force-to-wire mapping.
 *
 * Header-only and free of kernel state on purpose, like
 * hidpp_dd_texture_merge.h: it compiles in the kernel and in an ordinary
 * userspace test binary (tests/effect-math/), so the maths that decides
 * what torque the motor gets can be checked without a wheel, a kernel
 * build, or an afternoon on somebody's rig. The wheel-position guard that
 * shipped in 0.39.0 was a torque-to-the-stop bug in exactly this kind of
 * code, and would have been a five-line test.
 *
 * Nothing here touches hardware or driver state. Everything that does
 * stays in hid-logitech-hidpp.c and calls in.
 */
#ifndef HIDPP_DD_EFFECT_MATH_H
#define HIDPP_DD_EFFECT_MATH_H

#ifdef __KERNEL__
#include <linux/types.h>
#include <linux/input.h>
#include <linux/kernel.h>
#else
#include <stdint.h>
#include <stdlib.h>
#include <linux/input.h>
typedef uint8_t u8;  typedef uint16_t u16; typedef uint32_t u32;
typedef int16_t s16;  typedef int32_t s32; typedef int64_t s64;
#ifndef S16_MAX
#define S16_MAX ((s16)0x7fff)
#define S16_MIN ((s16)-0x8000)
#endif
#ifndef clamp
#define clamp(v, lo, hi) ((v) < (lo) ? (lo) : (v) > (hi) ? (hi) : (v))
#endif
#endif

/*
 * Apply an FF envelope (attack + fade) to a signed magnitude.
 *
 * Envelope shape per Linux Documentation/input/ff.rst:
 *   - attack: linear ramp from attack_level to |magnitude| over attack_length ms
 *   - hold:   magnitude held at full level in the middle
 *   - fade:   linear ramp from |magnitude| down to fade_level over fade_length ms
 * For effects without envelope (all u16 fields zero), the magnitude passes
 * through unchanged. length == 0 means infinite duration: no fade applies.
 *
 * Works in signed domain so the sign of the input magnitude is preserved
 * through the attack/fade scaling.
 */
static s32 hidpp_dd_apply_envelope(const struct ff_envelope *env,
			       s32 magnitude, u32 elapsed_ms, u32 length_ms)
{
	s32 abs_mag;
	s32 scaled;
	s32 attack_level, fade_level;
	int sign = magnitude < 0 ? -1 : 1;
	u32 fade_start;

	if (!env || (env->attack_length == 0 && env->fade_length == 0))
		return magnitude;

	abs_mag = sign < 0 ? -magnitude : magnitude;
	attack_level = (s32)env->attack_level;
	fade_level = (s32)env->fade_level;

	if (env->attack_length && elapsed_ms < env->attack_length) {
		/*
		 * Lerp attack_level -> abs_mag over attack_length. Work in
		 * signed domain so an "inverted" envelope (attack_level >
		 * abs_mag, legal per spec and used by games that want a
		 * decay-to-rest shape) doesn't underflow the subtraction.
		 */
		u32 span = env->attack_length;
		u32 t = elapsed_ms;

		scaled = attack_level +
			 (s32)(((s64)(abs_mag - attack_level) * (s32)t) /
			       (s32)span);
	} else if (length_ms && env->fade_length &&
		   length_ms >= env->fade_length &&
		   elapsed_ms > (fade_start = length_ms - env->fade_length)) {
		/*
		 * Lerp abs_mag -> fade_level over fade_length. Guard the
		 * fade-window computation with length_ms >= fade_length
		 * so a short effect with a long fade_length (legal but
		 * unusual) does not underflow length_ms - fade_length
		 * into ~4 billion, which previously pinned the branch off
		 * permanently.
		 */
		u32 span = env->fade_length;
		u32 t = elapsed_ms - fade_start;

		if (t > span)
			t = span;
		scaled = abs_mag -
			 (s32)(((s64)(abs_mag - fade_level) * (s32)t) /
			       (s32)span);
	} else {
		scaled = abs_mag;
	}

	return sign * scaled;
}

/*
 * Condition-effect force formula.
 *
 * The output force is always "restoring" relative to the metric: for a
 * SPRING fed wheel position, a positive displacement from centre produces
 * a negative (leftward) force that pulls the wheel back. Same shape
 * applies to DAMPER (force opposes velocity), FRICTION (force opposes
 * motion direction), INERTIA (force opposes acceleration).
 *
 *   if   metric >  center + deadband/2:
 *        f = -right_coeff * (metric - center - deadband/2) / 0x8000
 *        clamp to [-right_saturation, 0]
 *   elif metric <  center - deadband/2:
 *        f = -left_coeff * (metric - center + deadband/2) / 0x8000
 *        clamp to [0, left_saturation]
 *   else:
 *        f = 0
 *
 * The negation is what makes positive right_coeff mean "stiff spring
 * pulling left when wheel is right of centre" rather than "amplify
 * rightward displacement". An earlier version of this helper had the
 * sign inverted and produced a positive-feedback loop: displacement
 * grew instead of damping, and on a live RS50 + ACC session the wheel
 * felt actively unstable, tipping over in whichever direction the
 * driver was nudged. This matches the Linux kernel's ff documentation
 * and every real game's expectation.
 *
 * All four condition effect types (SPRING/DAMPER/FRICTION/INERTIA)
 * reuse struct ff_condition_effect with identical field semantics;
 * only what gets fed in as `metric` differs.
 */
static s32 hidpp_dd_condition_force(const struct ff_condition_effect *c,
				s32 metric)
{
	s32 half_db = (s32)c->deadband >> 1;
	s32 delta;
	s32 force;

	if (metric > c->center + half_db) {
		delta = metric - c->center - half_db;
		force = -(((s32)c->right_coeff * delta) >> 15);
		/*
		 * right_saturation caps the OUTPUT magnitude in this
		 * branch regardless of force sign. A positive right_coeff
		 * produces a restoring (negative) force; a negative
		 * right_coeff (legal per struct ff_condition_effect.coeff
		 * being __s16, used by anti-spring / oversteer effects)
		 * produces a destabilising (positive) force. Both need
		 * their magnitude clipped against right_saturation.
		 * Earlier revisions only kept the force when it was
		 * negative and zeroed any positive result, which silently
		 * dropped the anti-spring case.
		 */
		if (force > (s32)c->right_saturation)
			force = c->right_saturation;
		else if (force < -(s32)c->right_saturation)
			force = -(s32)c->right_saturation;
	} else if (metric < c->center - half_db) {
		delta = metric - c->center + half_db;
		force = -(((s32)c->left_coeff * delta) >> 15);
		if (force > (s32)c->left_saturation)
			force = c->left_saturation;
		else if (force < -(s32)c->left_saturation)
			force = -(s32)c->left_saturation;
	} else {
		return 0;
	}
	return force;
}

/*
 * The wire form of a signed force: offset binary around 0x8000, which is
 * what the stream's cur field and the KF packet both carry. Clamped to the
 * s16 range first, so an out-of-range sum saturates at the stops rather
 * than wrapping to the opposite side.
 */
static u16 hidpp_dd_force_to_offset_binary(s32 force)
{
	force = clamp(force, (s32)S16_MIN, (s32)S16_MAX);
	return (u16)(force + 0x8000);
}

#endif /* HIDPP_DD_EFFECT_MATH_H */
