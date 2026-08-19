// SPDX-License-Identifier: GPL-2.0-or-later
/*
 *  Classic Logitech wheel force feedback for the G923 (PlayStation variants),
 *  ported into hid-logitech-dd from berarma/new-lg4ff.
 *
 *  Copyright (c) 2010 Simon Wood <simon@mungewell.org>
 *  Copyright (c) 2019 Bernat Arlandis <berarma@hotmail.com>
 */

#include <linux/bitops.h>
#include <linux/bits.h>
#include <linux/fixp-arith.h>
#include <linux/hid.h>
#include <linux/hrtimer.h>
#include <linux/input.h>
#include <linux/jiffies.h>
#include <linux/math.h>
#include <linux/module.h>
#include <linux/spinlock.h>
#include <linux/timer.h>
#include <linux/usb.h>
#include <linux/version.h>
#ifdef CONFIG_LEDS_CLASS
#include <linux/leds.h>
#endif

#include "dd-lg4ff.h"
#include "hid-ids.h"

/*
 * Upstream in-tree drivers include "usbhid/usbhid.h" to get hid_to_usb_dev().
 * That header is not exported by kernel-devel on several distributions (see
 * hid-logitech-hidpp.c's identical note), so the one macro this file needs
 * from it is inlined here too; dd-lg4ff.c is its own translation unit and
 * does not share hid-logitech-hidpp.c's definition.
 */
#ifndef hid_to_usb_dev
#define hid_to_usb_dev(hid_dev) \
	to_usb_device((hid_dev)->dev.parent->parent)
#endif

/*
 * Scaling/translation helpers, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:74-95). These convert between the kernel's FF core value
 * ranges and the wire formats the classic Logitech wheel commands expect.
 */
#define DD_LG4FF_CLAMP_VALUE_U16(x) ((unsigned short)((x) > 0xffff ? 0xffff : (x)))
#define DD_LG4FF_CLAMP_VALUE_S16(x) ((unsigned short)((x) <= -0x8000 ? -0x8000 : ((x) > 0x7fff ? 0x7fff : (x))))
#define DD_LG4FF_SCALE_VALUE_U16(x, bits) (DD_LG4FF_CLAMP_VALUE_U16(x) >> (16 - bits))
#define DD_LG4FF_SCALE_COEFF(x, bits) DD_LG4FF_SCALE_VALUE_U16(abs(x) * 2, bits)
#define DD_LG4FF_TRANSLATE_FORCE(x) ((DD_LG4FF_CLAMP_VALUE_S16(x) + 0x8000) >> 8)
#define DD_LG4FF_STOP_EFFECT(state) ((state)->flags = 0)
#define DD_LG4FF_JIFFIES2MS(jiffies) ((jiffies) * 1000 / HZ)
#undef fixp_sin16
#define fixp_sin16(v) (((v % 360) > 180) ? -(fixp_sin32((v % 360) - 180) >> 16) : fixp_sin32(v) >> 16)
#define DD_LG4FF_DEBUG(...) pr_debug("dd_lg4ff: " __VA_ARGS__)
#define DD_LG4FF_TIME_DIFF(a, b) ({ \
		typecheck(unsigned long, a); \
		typecheck(unsigned long, b); \
		((a) - (long)(b)); })

#define DD_LG4FF_MAX_EFFECTS 16
#define DD_LG4FF_DEFAULT_TIMER_PERIOD 2
#define DD_LG4FF_CAP_FRICTION 1

#define DD_LG4FF_FF_EFFECT_STARTED 0
#define DD_LG4FF_FF_EFFECT_ALLSET 1
#define DD_LG4FF_FF_EFFECT_PLAYING 2
#define DD_LG4FF_FF_EFFECT_UPDATING 3

/* dd_lg4ff_handle_multimode_wheel() return values, ported from new-lg4ff. */
#define DD_LG4FF_MMODE_IS_MULTIMODE 0
#define DD_LG4FF_MMODE_SWITCHED 1
#define DD_LG4FF_MMODE_NOT_MULTIMODE 2

/*
 * new-lg4ff numbers its multimode-wheel mode bits 0..8, one per supported
 * wheel family (native, DF-EX, DFP, G25, DFGT, G27, G29, G923 PS, G923).
 * We only carry the G923 family, so the bit indices are renumbered to a
 * self-contained 0..1 range instead of keeping the upstream 7/8 slots.
 */
#define DD_LG4FF_MODE_G923_PS_IDX 0
#define DD_LG4FF_MODE_G923_IDX 1
#define DD_LG4FF_MODE_MAX_IDX 2

#define DD_LG4FF_MODE_G923_PS BIT(DD_LG4FF_MODE_G923_PS_IDX)
#define DD_LG4FF_MODE_G923 BIT(DD_LG4FF_MODE_G923_IDX)

#define DD_LG4FF_G923_TAG "G923"
#define DD_LG4FF_G923_NAME "G923 Racing Wheel"
#define DD_LG4FF_G923_PS_TAG "G923"
#define DD_LG4FF_G923_PS_NAME "G923 Racing Wheel (Playstation mode)"

struct dd_lg4ff_effect_state {
	struct ff_effect effect;
	struct ff_envelope *envelope;
	unsigned long start_at;
	unsigned long play_at;
	unsigned long stop_at;
	unsigned long flags;
	unsigned long time_playing;
	unsigned long updated_at;
	unsigned int phase;
	unsigned int phase_adj;
	unsigned int count;
	unsigned int cmd;
	unsigned int cmd_start_time;
	unsigned int cmd_start_count;
	int direction_gain;
	int slope;
	unsigned int slot;
};

struct dd_lg4ff_effect_parameters {
	int level;
	int d1;
	int d2;
	int k1;
	int k2;
	unsigned int clip;
};

struct dd_lg4ff_slot {
	int id;
	struct dd_lg4ff_effect_parameters parameters;
	u8 current_cmd[7];
	int cmd_op;
	int is_updated;
	int effect_type;
};

struct dd_lg4ff_wheel_data {
	const u32 product_id;
	u16 combine;
	u16 range;
	u16 autocenter;
	u16 master_gain;
	u16 gain;
	const u16 min_range;
	const u16 max_range;
#ifdef CONFIG_LEDS_CLASS
	u8  led_state;
	struct led_classdev *led[5];
#endif
	const u32 alternate_modes;
	const char * const real_tag;
	const char * const real_name;
	const u16 real_product_id;
	const u16 capabilities;

	void (*set_range)(struct hid_device *hid, u16 range);
};

struct dd_lg4ff_device_entry {
	spinlock_t report_lock; /* Protect output HID report */
	spinlock_t timer_lock;
	struct hid_report *report;
	struct dd_lg4ff_wheel_data wdata;
	struct hid_device *hid;
	struct timer_list timer;
	struct hrtimer hrtimer;
	struct dd_lg4ff_slot slots[4];
	struct dd_lg4ff_effect_state states[DD_LG4FF_MAX_EFFECTS];
	unsigned peak_ffb_level;
	s32 ffb_output; /* Slot-0 net force, post-gain; lockless, WRITE_ONCE/READ_ONCE */
	int effects_used;
#ifdef CONFIG_LEDS_CLASS
	int has_leds;
#endif
};

static const signed short dd_lg4ff_wheel_effects[] = {
	FF_CONSTANT,
	FF_SPRING,
	FF_DAMPER,
	FF_AUTOCENTER,
	FF_PERIODIC,
	FF_SINE,
	FF_SQUARE,
	FF_TRIANGLE,
	FF_SAW_UP,
	FF_SAW_DOWN,
	FF_RAMP,
	FF_FRICTION,
	FF_INERTIA,
	-1
};

struct dd_lg4ff_wheel {
	const u32 product_id;
	const signed short *ff_effects;
	const u16 min_range;
	const u16 max_range;
	const u16 capabilities;
	void (*set_range)(struct hid_device *hid, u16 range);
};

struct dd_lg4ff_compat_mode_switch {
	const u8 cmd_count;	/* Number of commands to send */
	const u8 cmd[];
};

struct dd_lg4ff_wheel_ident_info {
	const u32 modes;
	const u16 mask;
	const u16 result;
	const u16 real_product_id;
};

struct dd_lg4ff_multimode_wheel {
	const u16 product_id;
	const u32 alternate_modes;
	const char *real_tag;
	const char *real_name;
};

struct dd_lg4ff_alternate_mode {
	const u16 product_id;
	const char *tag;
	const char *name;
};

/* Forward declaration: defined below, needed by the device table row. */
static void dd_lg4ff_set_range_g25(struct hid_device *hid, u16 range);

/* Device table, trimmed to the G923 (c266) row. */
static const struct dd_lg4ff_wheel dd_lg4ff_devices[] = {
	{USB_DEVICE_ID_LOGITECH_G923_WHEEL,
		dd_lg4ff_wheel_effects, 40, 900, 0, dd_lg4ff_set_range_g25},
};

/* Multimode wheel table, trimmed to the G923 PS (c267) and G923 (c266) rows. */
static const struct dd_lg4ff_multimode_wheel dd_lg4ff_multimode_wheels[] = {
	{USB_DEVICE_ID_LOGITECH_G923_PS_WHEEL,
	 DD_LG4FF_MODE_G923_PS | DD_LG4FF_MODE_G923,
	 DD_LG4FF_G923_PS_TAG, DD_LG4FF_G923_PS_NAME},
	{USB_DEVICE_ID_LOGITECH_G923_WHEEL,
	 DD_LG4FF_MODE_G923,
	 DD_LG4FF_G923_TAG, DD_LG4FF_G923_NAME},
};

static const struct dd_lg4ff_alternate_mode dd_lg4ff_alternate_modes[DD_LG4FF_MODE_MAX_IDX] = {
	[DD_LG4FF_MODE_G923_PS_IDX] = {USB_DEVICE_ID_LOGITECH_G923_PS_WHEEL,
					DD_LG4FF_G923_PS_TAG, DD_LG4FF_G923_PS_NAME},
	[DD_LG4FF_MODE_G923_IDX] = {USB_DEVICE_ID_LOGITECH_G923_WHEEL,
				     DD_LG4FF_G923_TAG, DD_LG4FF_G923_NAME},
};

/* Multimode wheel identificator for the G923 family. */
static const struct dd_lg4ff_wheel_ident_info dd_lg4ff_g923_ident_info = {
	DD_LG4FF_MODE_G923_PS | DD_LG4FF_MODE_G923,
	0xff00,
	0x3800,
	USB_DEVICE_ID_LOGITECH_G923_WHEEL
};

/* Multimode wheel identification checklist, reduced to the G923 entry. */
static const struct dd_lg4ff_wheel_ident_info *dd_lg4ff_main_checklist[] = {
	&dd_lg4ff_g923_ident_info,
};

/*
 * Module parameters for the hrtimer effect engine, ported from new-lg4ff
 * (hid-lg4ff.c:423-455). The exposed parameter names are additionally
 * given a dd_lg4ff_ prefix (on top of the module's own hid-logitech-dd
 * namespace) so they cannot be confused with an in-tree lg4ff.ko's
 * timer_msecs/timer_mode/etc if both happen to be loaded at once.
 */
static int dd_lg4ff_timer_msecs = DD_LG4FF_DEFAULT_TIMER_PERIOD;
module_param_named(dd_lg4ff_timer_msecs, dd_lg4ff_timer_msecs, int, 0660);
MODULE_PARM_DESC(dd_lg4ff_timer_msecs, "Timer resolution in msecs.");

static int dd_lg4ff_fixed_loop;
module_param_named(dd_lg4ff_fixed_loop, dd_lg4ff_fixed_loop, int, 0);
MODULE_PARM_DESC(dd_lg4ff_fixed_loop, "Put the device into fixed loop mode.");

static int dd_lg4ff_timer_mode = 2;
module_param_named(dd_lg4ff_timer_mode, dd_lg4ff_timer_mode, int, 0660);
MODULE_PARM_DESC(dd_lg4ff_timer_mode, "Timer mode: 0) fixed, 1) static, 2) dynamic (default).");

static int dd_lg4ff_spring_level = 30;
module_param_named(dd_lg4ff_spring_level, dd_lg4ff_spring_level, int, 0);
MODULE_PARM_DESC(dd_lg4ff_spring_level, "Level of spring force (0-100).");

static int dd_lg4ff_damper_level = 30;
module_param_named(dd_lg4ff_damper_level, dd_lg4ff_damper_level, int, 0);
MODULE_PARM_DESC(dd_lg4ff_damper_level, "Level of damper force (0-100).");

static int dd_lg4ff_friction_level = 30;
module_param_named(dd_lg4ff_friction_level, dd_lg4ff_friction_level, int, 0);
MODULE_PARM_DESC(dd_lg4ff_friction_level, "Level of friction force (0-100).");

/*
 * Single choke point for reaching the ported engine's per-device state.
 * new-lg4ff keeps this pointer in lg_drv_data->device_props; we have no
 * lg_drv_data, so it lives directly on struct hidpp_device (lg4ff_entry)
 * and is reached through the hidpp_dd_lg4ff_slot() accessor exported by
 * hid-logitech-hidpp.c, which is the only place that knows that struct's
 * layout. This replaces new-lg4ff's lg4ff_get_device_entry
 * (hid-lg4ff.c:457-478); every later ported function calls this instead
 * of touching drv_data->device_props directly.
 */
static struct dd_lg4ff_device_entry *dd_lg4ff_get_entry(struct hid_device *hdev)
{
	void **slot;

	if (!hdev) {
		hid_err(hdev, "HID not found!\n");
		return NULL;
	}

	slot = (void **)hidpp_dd_lg4ff_slot(hdev);
	if (!slot) {
		hid_err(hdev, "Private driver data not found!\n");
		return NULL;
	}

	return (struct dd_lg4ff_device_entry *)*slot;
}

/*
 * 7-byte SET_REPORT command senders, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:480-514). dd_lg4ff_send_cmd_with_id() forces the report's
 * id first; it is used by the PS-mode switch sequence below, which must
 * address report id 0x30 explicitly. dd_lg4ff_send_cmd() is called from
 * the hrtimer effect engine below.
 */
static void dd_lg4ff_send_cmd_with_id(struct dd_lg4ff_device_entry *entry, u8 *cmd, u8 id)
{
	unsigned long flags;
	s32 *value = entry->report->field[0]->value;

	spin_lock_irqsave(&entry->report_lock, flags);
	entry->report->id = id;
	value[0] = cmd[0];
	value[1] = cmd[1];
	value[2] = cmd[2];
	value[3] = cmd[3];
	value[4] = cmd[4];
	value[5] = cmd[5];
	value[6] = cmd[6];
	hid_hw_request(entry->hid, entry->report, HID_REQ_SET_REPORT);
	spin_unlock_irqrestore(&entry->report_lock, flags);
	DD_LG4FF_DEBUG("send_cmd: %02X %02X %02X %02X %02X %02X %02X %02X\n", id, cmd[0], cmd[1], cmd[2], cmd[3], cmd[4], cmd[5], cmd[6]);
}

static void dd_lg4ff_send_cmd(struct dd_lg4ff_device_entry *entry, u8 *cmd)
{
	unsigned long flags;
	s32 *value = entry->report->field[0]->value;

	spin_lock_irqsave(&entry->report_lock, flags);
	value[0] = cmd[0];
	value[1] = cmd[1];
	value[2] = cmd[2];
	value[3] = cmd[3];
	value[4] = cmd[4];
	value[5] = cmd[5];
	value[6] = cmd[6];
	hid_hw_request(entry->hid, entry->report, HID_REQ_SET_REPORT);
	spin_unlock_irqrestore(&entry->report_lock, flags);
	DD_LG4FF_DEBUG("send_cmd: %02X %02X %02X %02X %02X %02X %02X", cmd[0], cmd[1], cmd[2], cmd[3], cmd[4], cmd[5], cmd[6]);
}

/*
 * Wire-format packer, ported verbatim from new-lg4ff (hid-lg4ff.c:516-618).
 * This is the heart of the classic command protocol: it fills
 * slot->current_cmd[0..6] with the F8/3E slot-select byte plus the
 * per-effect-type payload (CONSTANT 0x00, SPRING 0x0b, DAMPER 0x0c,
 * FRICTION 0x0e; op3 stops the slot). Called from the hrtimer effect
 * engine below.
 */
static void dd_lg4ff_update_slot(struct dd_lg4ff_slot *slot, struct dd_lg4ff_effect_parameters *parameters)
{
	u8 original_cmd[7];
	int d1;
	int d2;
	int k1;
	int k2;
	int s1;
	int s2;

	memcpy(original_cmd, slot->current_cmd, sizeof(original_cmd));

	if ((original_cmd[0] & 0xf) == 1) {
		original_cmd[0] = (original_cmd[0] & 0xf0) + 0xc;
	}

	if (slot->effect_type == FF_CONSTANT) {
		if (slot->cmd_op == 0) {
			slot->cmd_op = 1;
		} else {
			slot->cmd_op = 0xc;
		}
	} else {
		if (parameters->clip == 0 || slot->effect_type == 0) {
			slot->cmd_op = 3;
		} else if (slot->cmd_op == 3) {
			slot->cmd_op = 1;
		} else {
			slot->cmd_op = 0xc;
		}
	}

	slot->current_cmd[0] = (0x10 << slot->id) + slot->cmd_op;

	if (slot->cmd_op == 3) {
		slot->current_cmd[1] = 0;
		slot->current_cmd[2] = 0;
		slot->current_cmd[3] = 0;
		slot->current_cmd[4] = 0;
		slot->current_cmd[5] = 0;
		slot->current_cmd[6] = 0;
	} else {
		switch (slot->effect_type) {
			case FF_CONSTANT:
				slot->current_cmd[1] = 0x00;
				slot->current_cmd[2] = 0;
				slot->current_cmd[3] = 0;
				slot->current_cmd[4] = 0;
				slot->current_cmd[5] = 0;
				slot->current_cmd[6] = 0;
				slot->current_cmd[2 + slot->id] = DD_LG4FF_TRANSLATE_FORCE(parameters->level);
				break;
			case FF_SPRING:
				d1 = DD_LG4FF_SCALE_VALUE_U16(((parameters->d1) + 0x8000) & 0xffff, 11);
				d2 = DD_LG4FF_SCALE_VALUE_U16(((parameters->d2) + 0x8000) & 0xffff, 11);
				s1 = parameters->k1 < 0;
				s2 = parameters->k2 < 0;
				k1 = abs(parameters->k1);
				k2 = abs(parameters->k2);
				if (k1 < 2048) {
					d1 = 0;
				} else {
					k1 -= 2048;
				}
				if (k2 < 2048) {
					d2 = 2047;
				} else {
					k2 -= 2048;
				}
				slot->current_cmd[1] = 0x0b;
				slot->current_cmd[2] = d1 >> 3;
				slot->current_cmd[3] = d2 >> 3;
				slot->current_cmd[4] = (DD_LG4FF_SCALE_COEFF(k2, 4) << 4) + DD_LG4FF_SCALE_COEFF(k1, 4);
				slot->current_cmd[5] = ((d2 & 7) << 5) + ((d1 & 7) << 1) + (s2 << 4) + s1;
				slot->current_cmd[6] = DD_LG4FF_SCALE_VALUE_U16(parameters->clip, 8);
				break;
			case FF_DAMPER:
				s1 = parameters->k1 < 0;
				s2 = parameters->k2 < 0;
				slot->current_cmd[1] = 0x0c;
				slot->current_cmd[2] = DD_LG4FF_SCALE_COEFF(parameters->k1, 4);
				slot->current_cmd[3] = s1;
				slot->current_cmd[4] = DD_LG4FF_SCALE_COEFF(parameters->k2, 4);
				slot->current_cmd[5] = s2;
				slot->current_cmd[6] = DD_LG4FF_SCALE_VALUE_U16(parameters->clip, 8);
				break;
			case FF_FRICTION:
				s1 = parameters->k1 < 0;
				s2 = parameters->k2 < 0;
				slot->current_cmd[1] = 0x0e;
				slot->current_cmd[2] = DD_LG4FF_SCALE_COEFF(parameters->k1, 8);
				slot->current_cmd[3] = DD_LG4FF_SCALE_COEFF(parameters->k2, 8);
				slot->current_cmd[4] = DD_LG4FF_SCALE_VALUE_U16(parameters->clip, 8);
				slot->current_cmd[5] = (s2 << 4) + s1;
				slot->current_cmd[6] = 0;
				break;
		}
	}

	if (memcmp(original_cmd, slot->current_cmd, sizeof(original_cmd))) {
		slot->is_updated = 1;
	}
}

/*
 * Per-effect-type force math, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:620-741). All __always_inline; called from the timer tick
 * (dd_lg4ff_timer, below) that drives these once per effect.
 */
static __always_inline int dd_lg4ff_calculate_constant(struct dd_lg4ff_effect_state *state)
{
	int level_sign;
	int level = state->effect.u.constant.level;
	int d, t;

	if (state->time_playing < state->envelope->attack_length) {
		level_sign = level < 0 ? -1 : 1;
		d = level - level_sign * state->envelope->attack_level;
		level = level_sign * state->envelope->attack_level + d * state->time_playing / state->envelope->attack_length;
	} else if (state->effect.replay.length) {
		t = state->time_playing - state->effect.replay.length + state->envelope->fade_length;
		if (t > 0) {
			level_sign = level < 0 ? -1 : 1;
			d = level - level_sign * state->envelope->fade_level;
			level = level - d * t / state->envelope->fade_length;
		}
	}

	return state->direction_gain * level / 0x7fff;
}

static __always_inline int dd_lg4ff_calculate_ramp(struct dd_lg4ff_effect_state *state)
{
	struct ff_ramp_effect *ramp = &state->effect.u.ramp;
	int level_sign;
	int level = INT_MAX;
	int d, t;

	if (state->time_playing < state->envelope->attack_length) {
		level = ramp->start_level;
		level_sign =  level < 0 ? -1 : 1;
		t = state->envelope->attack_length - state->time_playing;
		d = level - level_sign * state->envelope->attack_level;
		level = level_sign * state->envelope->attack_level + d * t / state->envelope->attack_length;
	} else if (state->effect.replay.length && state->time_playing >= state->effect.replay.length - state->envelope->fade_length) {
		level = ramp->end_level;
		level_sign = level < 0 ? -1 : 1;
		t = state->time_playing - state->effect.replay.length + state->envelope->fade_length;
		d = level_sign * state->envelope->fade_level - level;
		level = level - d * t / state->envelope->fade_length;
	} else {
		t = state->time_playing - state->envelope->attack_length;
		level = ramp->start_level + ((t * state->slope) >> 16);
	}

	return state->direction_gain * level / 0x7fff;
}

static __always_inline int dd_lg4ff_calculate_periodic(struct dd_lg4ff_effect_state *state)
{
	struct ff_periodic_effect *periodic = &state->effect.u.periodic;
	int magnitude = periodic->magnitude;
	int magnitude_sign = magnitude < 0 ? -1 : 1;
	int level = periodic->offset;
	int d, t;

	if (state->time_playing < state->envelope->attack_length) {
		d = magnitude - magnitude_sign * state->envelope->attack_level;
		magnitude = magnitude_sign * state->envelope->attack_level + d * state->time_playing / state->envelope->attack_length;
	} else if (state->effect.replay.length) {
		t = state->time_playing - state->effect.replay.length + state->envelope->fade_length;
		if (t > 0) {
			d = magnitude - magnitude_sign * state->envelope->fade_level;
			magnitude = magnitude - d * t / state->envelope->fade_length;
		}
	}

	switch (periodic->waveform) {
		case FF_SINE:
			level += fixp_sin16(state->phase) * magnitude / 0x7fff;
			break;
		case FF_SQUARE:
			level += (state->phase < 180 ? 1 : -1) * magnitude;
			break;
		case FF_TRIANGLE:
			level += abs(state->phase * magnitude * 2 / 360 - magnitude) * 2 - magnitude;
			break;
		case FF_SAW_UP:
			level += state->phase * magnitude * 2 / 360 - magnitude;
			break;
		case FF_SAW_DOWN:
			level += magnitude - state->phase * magnitude * 2 / 360;
			break;
	}

	return state->direction_gain * level / 0x7fff;
}

static __always_inline void dd_lg4ff_calculate_spring(struct dd_lg4ff_effect_state *state, struct dd_lg4ff_effect_parameters *parameters)
{
	struct ff_condition_effect *condition = &state->effect.u.condition[0];

	parameters->d1 = ((int)condition->center) - condition->deadband / 2;
	parameters->d2 = ((int)condition->center) + condition->deadband / 2;
	parameters->k1 = condition->left_coeff;
	parameters->k2 = condition->right_coeff;
	parameters->clip = (unsigned)condition->right_saturation;
}

static __always_inline void dd_lg4ff_calculate_resistance(struct dd_lg4ff_effect_state *state, struct dd_lg4ff_effect_parameters *parameters)
{
	struct ff_condition_effect *condition = &state->effect.u.condition[0];

	parameters->k1 = condition->left_coeff;
	parameters->k2 = condition->right_coeff;
	parameters->clip = (unsigned)condition->right_saturation;
}

static __always_inline struct ff_envelope *dd_lg4ff_effect_envelope(struct ff_effect *effect)
{
	switch (effect->type) {
		case FF_CONSTANT:
			return &effect->u.constant.envelope;
		case FF_RAMP:
			return &effect->u.ramp.envelope;
		case FF_PERIODIC:
			return &effect->u.periodic.envelope;
	}

	return NULL;
}

/*
 * Effect scheduling state machine, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:743-795). Advances start/play/stop timestamps and the
 * playing/updating flags off the FF core's ff_effect fields; called once
 * per effect from the timer tick (dd_lg4ff_timer, below).
 */
static __always_inline void dd_lg4ff_update_state(struct dd_lg4ff_effect_state *state, const unsigned long now)
{
	struct ff_effect *effect = &state->effect;
	unsigned long phase_time;

	if (!__test_and_set_bit(DD_LG4FF_FF_EFFECT_ALLSET, &state->flags)) {
		state->play_at = state->start_at + effect->replay.delay;
		if (!test_bit(DD_LG4FF_FF_EFFECT_UPDATING, &state->flags)) {
			state->updated_at = state->play_at;
		}
		state->direction_gain = fixp_sin16(effect->direction * 360 / 0x10000);
		if (effect->type == FF_PERIODIC) {
			state->phase_adj = effect->u.periodic.phase * 360 / effect->u.periodic.period;
		}
		if (effect->replay.length) {
			state->stop_at = state->play_at + effect->replay.length;
		}
	}

	if (__test_and_clear_bit(DD_LG4FF_FF_EFFECT_UPDATING, &state->flags)) {
		__clear_bit(DD_LG4FF_FF_EFFECT_PLAYING, &state->flags);
		state->play_at = state->updated_at + effect->replay.delay;
		state->direction_gain = fixp_sin16(effect->direction * 360 / 0x10000);
		if (effect->replay.length) {
			state->stop_at = state->updated_at + effect->replay.length;
		}
		if (effect->type == FF_PERIODIC) {
			state->phase_adj = state->phase;
		}
	}

	state->envelope = dd_lg4ff_effect_envelope(effect);

	state->slope = 0;
	if (effect->type == FF_RAMP && effect->replay.length) {
		state->slope = ((effect->u.ramp.end_level - effect->u.ramp.start_level) << 16) / (effect->replay.length - state->envelope->attack_length - state->envelope->fade_length);
	}

	if (!test_bit(DD_LG4FF_FF_EFFECT_PLAYING, &state->flags) && time_after_eq(now,
				state->play_at) && (effect->replay.length == 0 ||
					time_before(now, state->stop_at))) {
		__set_bit(DD_LG4FF_FF_EFFECT_PLAYING, &state->flags);
	}

	if (test_bit(DD_LG4FF_FF_EFFECT_PLAYING, &state->flags)) {
		state->time_playing = DD_LG4FF_TIME_DIFF(now, state->play_at);
		if (effect->type == FF_PERIODIC) {
			phase_time = DD_LG4FF_TIME_DIFF(now, state->updated_at);
			state->phase = (phase_time % effect->u.periodic.period) * 360 / effect->u.periodic.period;
			state->phase += state->phase_adj % 360;
		}
	}
}

/*
 * Partial mirror of struct usbhid_device from the kernel's
 * drivers/hid/usbhid/usbhid.h, trimmed to the fields dd_lg4ff_timer() below
 * needs: outhead/outtail, the USB output-report FIFO indices used to detect
 * a stalled SET_REPORT queue. That header is kernel-internal and is not
 * exported by kernel-devel on several distributions (see the hid_to_usb_dev
 * note in hid-logitech-hidpp.c for the same class of problem with a
 * different symbol from it), so entry->hid->driver_data is read through
 * this local, offset-compatible mirror instead of including it. Field
 * order and types must track upstream exactly up to and including outtail;
 * checked against drivers/hid/usbhid/usbhid.h as shipped in Linux 7.1.
 */
struct dd_lg4ff_usbhid_device {
	struct hid_device *hid;
	struct usb_interface *intf;
	int ifnum;
	unsigned int bufsize;
	struct urb *urbin;
	char *inbuf;
	dma_addr_t inbuf_dma;
	struct urb *urbctrl;
	struct usb_ctrlrequest *cr;
	struct hid_control_fifo ctrl[HID_CONTROL_FIFO_SIZE];
	unsigned char ctrlhead, ctrltail;
	char *ctrlbuf;
	dma_addr_t ctrlbuf_dma;
	unsigned long last_ctrl;
	struct urb *urbout;
	struct hid_output_fifo out[HID_CONTROL_FIFO_SIZE];
	unsigned char outhead, outtail;
};

/*
 * hrtimer effect engine, ported from new-lg4ff (hid-lg4ff.c:797-968). Sums
 * CONSTANT/RAMP/PERIODIC into slot 0 and condition effects (SPRING/DAMPER/
 * FRICTION/INERTIA) into slots 1-3, applies master/wheel gain and the
 * spring/damper/friction level scalers, then pushes any slot whose command
 * changed out over SET_REPORT. The timer_mode back-off below is load-bearing:
 * without it a stalled USB output queue gets more SET_REPORT commands piled
 * onto it every tick, which only makes the stall worse.
 *
 * new-lg4ff's LED calibration output (its CONFIG_LEDS_CLASS block, gated on
 * the ffb_leds param) is intentionally not ported here: dd_lg4ff_set_leds()
 * now exists (see below) for the rev-LED classdevs, but ffb_leds/profile
 * are not among this task's module params, so this timer never drives the
 * LEDs itself.
 */
static __always_inline int dd_lg4ff_timer(struct dd_lg4ff_device_entry *entry)
{
	struct dd_lg4ff_usbhid_device *usbhid = entry->hid->driver_data;
	struct dd_lg4ff_slot *slot;
	struct dd_lg4ff_effect_state *state;
	struct dd_lg4ff_effect_parameters parameters[4];
	unsigned long jiffies_now = jiffies;
	unsigned long now = DD_LG4FF_JIFFIES2MS(jiffies_now);
	unsigned long flags;
	unsigned gain;
	int current_period;
	int count;
	int effect_id;
	int i;
	int ffb_level;

	if (dd_lg4ff_timer_mode > 0 && usbhid->outhead != usbhid->outtail) {
		current_period = dd_lg4ff_timer_msecs;
		if (dd_lg4ff_timer_mode == 1) {
			dd_lg4ff_timer_msecs *= 2;
			hid_info(entry->hid, "Commands stacking up, increasing timer period to %d ms.", dd_lg4ff_timer_msecs);
		} else {
			DD_LG4FF_DEBUG("Commands stacking up, delaying timer.");
		}
		return current_period;
	}

	memset(parameters, 0, sizeof(parameters));

	gain = (unsigned)entry->wdata.master_gain * entry->wdata.gain / 0xffff;

	spin_lock_irqsave(&entry->timer_lock, flags);

	count = entry->effects_used;

	for (effect_id = 0; effect_id < DD_LG4FF_MAX_EFFECTS; effect_id++) {

		if (!count) {
			break;
		}

		state = &entry->states[effect_id];

		if (!test_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags)) {
			continue;
		}

		count--;

		if (test_bit(DD_LG4FF_FF_EFFECT_ALLSET, &state->flags)) {
			if (state->effect.replay.length && time_after_eq(now, state->stop_at)) {
				DD_LG4FF_STOP_EFFECT(state);
				if (!--state->count) {
					entry->effects_used--;
					continue;
				}
				__set_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags);
				state->start_at = state->stop_at;
			}
		}

		dd_lg4ff_update_state(state, now);

		if (!test_bit(DD_LG4FF_FF_EFFECT_PLAYING, &state->flags)) {
			continue;
		}

		switch (state->effect.type) {
			case FF_CONSTANT:
				parameters[0].level += dd_lg4ff_calculate_constant(state);
				break;
			case FF_RAMP:
				parameters[0].level += dd_lg4ff_calculate_ramp(state);
				break;
			case FF_PERIODIC:
				parameters[0].level += dd_lg4ff_calculate_periodic(state);
				break;
			case FF_SPRING:
				if (state->slot != 0) {
					dd_lg4ff_calculate_spring(state, &parameters[state->slot]);
				}
				break;
			case FF_DAMPER:
			case FF_FRICTION:
			case FF_INERTIA:
				if (state->slot != 0) {
					dd_lg4ff_calculate_resistance(state, &parameters[state->slot]);
				}
		}
	}

	spin_unlock_irqrestore(&entry->timer_lock, flags);

	parameters[0].level = (long)parameters[0].level * gain / 0xffff;
	WRITE_ONCE(entry->ffb_output, parameters[0].level);

	ffb_level = abs(parameters[0].level);
	for (i = 1; i < 4; i++) {
		parameters[i].k1 = (long)parameters[i].k1 * gain / 0xffff;
		parameters[i].k2 = (long)parameters[i].k2 * gain / 0xffff;
		switch (entry->slots[i].effect_type) {
			case FF_SPRING:
				parameters[i].clip = parameters[i].clip * dd_lg4ff_spring_level / 100;
				break;
			case FF_DAMPER:
				parameters[i].clip = parameters[i].clip * dd_lg4ff_damper_level / 100;
				break;
			case FF_FRICTION:
				parameters[i].clip = parameters[i].clip * dd_lg4ff_friction_level / 100;
				break;
		}
		parameters[i].clip = parameters[i].clip * gain / 0xffff;
		ffb_level += parameters[i].clip * 0x7fff / 0xffff;
	}
	if (ffb_level > entry->peak_ffb_level) {
		entry->peak_ffb_level = ffb_level;
	}

	for (i = 0; i < 4; i++) {
		slot = &entry->slots[i];
		dd_lg4ff_update_slot(slot, &parameters[i]);
		if (slot->is_updated) {
			dd_lg4ff_send_cmd(entry, slot->current_cmd);
			slot->is_updated = 0;
		}
	}

	return 0;
}

/*
 * hrtimer callback wrapper, ported from new-lg4ff (hid-lg4ff.c:970-994).
 * Re-arms at the back-off period dd_lg4ff_timer() just returned, or at the
 * normal tick period while effects are still playing, or stops the timer
 * once nothing is left to play. Assigned to entry->hrtimer.function by
 * dd_lg4ff_init() below.
 */
static enum hrtimer_restart dd_lg4ff_timer_hires(struct hrtimer *t)
{
	struct dd_lg4ff_device_entry *entry = container_of(t, struct dd_lg4ff_device_entry, hrtimer);
	int delay_timer;
	int overruns;

	delay_timer = dd_lg4ff_timer(entry);

	if (delay_timer) {
		hrtimer_forward_now(&entry->hrtimer, ms_to_ktime(delay_timer));
		return HRTIMER_RESTART;
	}

	if (entry->effects_used) {
		overruns = hrtimer_forward_now(&entry->hrtimer, ms_to_ktime(dd_lg4ff_timer_msecs));
		overruns--;
		if (unlikely(overruns > 0))
			DD_LG4FF_DEBUG("Overruns: %d", overruns);
		return HRTIMER_RESTART;
	}

	DD_LG4FF_DEBUG("Stop timer.");
	return HRTIMER_NORESTART;
}

/*
 * Slot/loop-mode initializer, ported from new-lg4ff (hid-lg4ff.c:996-1019).
 * Sends the 0x0d fixed-loop-mode command, then resets and re-sends all four
 * slots empty. Called from dd_lg4ff_init() below.
 */
static void dd_lg4ff_init_slots(struct dd_lg4ff_device_entry *entry)
{
	struct dd_lg4ff_effect_parameters parameters;
	u8 cmd[8] = {0};
	int i;

	/* Set/unset fixed loop mode */
	cmd[0] = 0x0d;
	cmd[1] = dd_lg4ff_fixed_loop ? 1 : 0;
	dd_lg4ff_send_cmd(entry, cmd);

	memset(&entry->states, 0, sizeof(entry->states));
	memset(&entry->slots, 0, sizeof(entry->slots));
	memset(&parameters, 0, sizeof(parameters));

	entry->slots[0].effect_type = FF_CONSTANT;

	for (i = 0; i < 4; i++) {
		entry->slots[i].id = i;
		dd_lg4ff_update_slot(&entry->slots[i], &parameters);
		dd_lg4ff_send_cmd(entry, entry->slots[i].current_cmd);
		entry->slots[i].is_updated = 0;
	}
}

/*
 * Ported from new-lg4ff (hid-lg4ff.c:1021-1027): cmd[0]=0xf3 tells the wheel
 * to drop whatever it is currently playing. Called from dd_lg4ff_deinit()
 * below. Also zeroes ffb_output, since nothing is playing past this point;
 * the timer itself stopping (hrtimer_restart returning HRTIMER_NORESTART)
 * leaves the last computed value in place for a moment, which is fine since
 * that path always leads here shortly after.
 */
static void dd_lg4ff_stop_effects(struct dd_lg4ff_device_entry *entry)
{
	u8 cmd[7] = {0};

	cmd[0] = 0xf3;
	dd_lg4ff_send_cmd(entry, cmd);

	WRITE_ONCE(entry->ffb_output, 0);
}

/*
 * ff->upload callback, ported from new-lg4ff (hid-lg4ff.c:1029-1064). Pure
 * bookkeeping: stores the ff_effect into entry->states[id] and marks it
 * updating if it was already playing. No hardware I/O. Wired to the
 * input_dev's ff_device by dd_lg4ff_init() below.
 */
static int dd_lg4ff_upload_effect(struct input_dev *dev, struct ff_effect *effect, struct ff_effect *old)
{
	struct hid_device *hid = input_get_drvdata(dev);
	struct dd_lg4ff_device_entry *entry;
	struct dd_lg4ff_effect_state *state;
	unsigned long now = DD_LG4FF_JIFFIES2MS(jiffies);
	unsigned long flags;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return -EINVAL;
	}

	if (effect->type == FF_PERIODIC && effect->u.periodic.period == 0) {
		return -EINVAL;
	}

	state = &entry->states[effect->id];

	if (test_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags) && effect->type != state->effect.type) {
		return -EINVAL;
	}

	spin_lock_irqsave(&entry->timer_lock, flags);

	state->effect = *effect;

	if (test_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags)) {
		__set_bit(DD_LG4FF_FF_EFFECT_UPDATING, &state->flags);
		state->updated_at = now;
	}

	spin_unlock_irqrestore(&entry->timer_lock, flags);

	return 0;
}

/*
 * ff->playback callback, ported from new-lg4ff (hid-lg4ff.c:1066-1131).
 * Starts the hrtimer on the first effect and stops it when the last one
 * ends; allocates a condition slot (1-3) for SPRING/DAMPER/FRICTION/INERTIA
 * on start and frees it on stop. INERTIA and FRICTION on a wheel lacking
 * DD_LG4FF_CAP_FRICTION are cast to DAMPER, matching what the Windows driver
 * does for these toy-strength wheels. Wired to the input_dev's ff_device by
 * dd_lg4ff_init() below.
 */
static int dd_lg4ff_play_effect(struct input_dev *dev, int effect_id, int value)
{
	struct hid_device *hid = input_get_drvdata(dev);
	struct dd_lg4ff_device_entry *entry;
	struct dd_lg4ff_effect_state *state;
	unsigned long now = DD_LG4FF_JIFFIES2MS(jiffies);
	unsigned long flags;
	int i;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return -EINVAL;
	}

	state = &entry->states[effect_id];

	spin_lock_irqsave(&entry->timer_lock, flags);

	if (value > 0) {
		if (test_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags)) {
			DD_LG4FF_STOP_EFFECT(state);
		} else {
			entry->effects_used++;
			if (!hrtimer_active(&entry->hrtimer)) {
				hrtimer_start(&entry->hrtimer, ms_to_ktime(dd_lg4ff_timer_msecs), HRTIMER_MODE_REL);
				DD_LG4FF_DEBUG("Start timer.");
			}
			if ((state->effect.type == FF_SPRING || state->effect.type == FF_DAMPER
					|| state->effect.type == FF_FRICTION || state->effect.type == FF_INERTIA)
					&& state->slot == 0) {
				/* Find a free slot */
				for (i = 1; i < 4 && entry->slots[i].effect_type != 0; i++)
					;
				if (i < 4) {
					state->slot = i;
					entry->slots[i].effect_type = state->effect.type;

					/* Cast unsupported effect types to "damper": this is what the Windows
					 * driver does.
					 * This is not physically plausible, but we are working with toy-strength
					 * wheels that won't let you feel more than "big value = wheel stuck" */
					if (state->effect.type == FF_INERTIA
							|| (state->effect.type == FF_FRICTION && !(entry->wdata.capabilities & DD_LG4FF_CAP_FRICTION))) {
						entry->slots[i].effect_type = FF_DAMPER;
					}
				}
			}
		}
		__set_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags);
		state->start_at = now;
		state->count = value;
	} else {
		if (test_bit(DD_LG4FF_FF_EFFECT_STARTED, &state->flags)) {
			DD_LG4FF_STOP_EFFECT(state);
			entry->effects_used--;
			if (state->slot) {
				entry->slots[state->slot].effect_type = 0;
				state->slot = 0;
			}
		}
	}

	spin_unlock_irqrestore(&entry->timer_lock, flags);

	return 0;
}

/*
 * Per-device state initializer, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:1255-1283). Fills wdata's product/range/capabilities fields
 * from the wheel table row (dd_lg4ff_devices[]) and, for a multimode wheel,
 * layers on the alternate-mode bitmask and the real_tag/real_name pointers
 * used by mode switching. Called from dd_lg4ff_init() below.
 */
static void dd_lg4ff_init_wheel_data(struct dd_lg4ff_wheel_data * const wdata, const struct dd_lg4ff_wheel *wheel,
				  const struct dd_lg4ff_multimode_wheel *mmode_wheel,
				  const u16 real_product_id)
{
	u32 alternate_modes = 0;
	const char *real_tag = NULL;
	const char *real_name = NULL;

	if (mmode_wheel) {
		alternate_modes = mmode_wheel->alternate_modes;
		real_tag = mmode_wheel->real_tag;
		real_name = mmode_wheel->real_name;
	}

	{
		struct dd_lg4ff_wheel_data t_wdata =  { .product_id = wheel->product_id,
						     .real_product_id = real_product_id,
						     .combine = 0,
						     .min_range = wheel->min_range,
						     .max_range = wheel->max_range,
						     .set_range = wheel->set_range,
						     .alternate_modes = alternate_modes,
						     .real_tag = real_tag,
						     .real_name = real_name,
						     .capabilities = wheel->capabilities };

		memcpy(wdata, &t_wdata, sizeof(t_wdata));
	}
}

/*
 * Default autocentering command sender, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:1287-1350). Compatible with every wheel we carry (the G923
 * family); the Formula Force EX variant (hid-lg4ff.c:1353-1376) is dropped,
 * matching the trimmed device table. Wired to the input_dev's ff_device by
 * dd_lg4ff_init() below.
 */
static void dd_lg4ff_set_autocenter_default(struct input_dev *dev, u16 magnitude)
{
	struct hid_device *hid = input_get_drvdata(dev);
	u8 cmd[7];
	u32 expand_a, expand_b;
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return;
	}

	entry->wdata.autocenter = magnitude;

	/* De-activate Auto-Center */
	if (magnitude == 0) {
		cmd[0] = 0xf5;
		cmd[1] = 0x00;
		cmd[2] = 0x00;
		cmd[3] = 0x00;
		cmd[4] = 0x00;
		cmd[5] = 0x00;
		cmd[6] = 0x00;
		dd_lg4ff_send_cmd(entry, cmd);
		return;
	}

	if (magnitude <= 0xaaaa) {
		expand_a = 0x0c * magnitude;
		expand_b = 0x80 * magnitude;
	} else {
		expand_a = (0x0c * 0xaaaa) + 0x06 * (magnitude - 0xaaaa);
		expand_b = (0x80 * 0xaaaa) + 0xff * (magnitude - 0xaaaa);
	}

	/* Adjust for non-MOMO wheels */
	switch (entry->wdata.product_id) {
	case USB_DEVICE_ID_LOGITECH_MOMO_WHEEL:
	case USB_DEVICE_ID_LOGITECH_MOMO_WHEEL2:
		break;
	default:
		expand_a = expand_a >> 1;
		break;
	}

	cmd[0] = 0xfe;
	cmd[1] = 0x0d;
	cmd[2] = expand_a / 0xaaaa;
	cmd[3] = expand_a / 0xaaaa;
	cmd[4] = expand_b / 0xaaaa;
	cmd[5] = 0x00;
	cmd[6] = 0x00;
	dd_lg4ff_send_cmd(entry, cmd);

	/* Activate Auto-Center */
	cmd[0] = 0x14;
	cmd[1] = 0x00;
	cmd[2] = 0x00;
	cmd[3] = 0x00;
	cmd[4] = 0x00;
	cmd[5] = 0x00;
	cmd[6] = 0x00;
	dd_lg4ff_send_cmd(entry, cmd);
}

/*
 * Range-set command sender for the G25/G27/DFGT/G923 family, ported
 * verbatim from new-lg4ff (hid-lg4ff.c:1379-1398). The Driving Force Pro
 * variant (hid-lg4ff.c:1401-1455) is dropped: no wheel in the trimmed
 * device table needs it. Wired into dd_lg4ff_devices[]'s G923 row above.
 */
static void dd_lg4ff_set_range_g25(struct hid_device *hid, u16 range)
{
	struct dd_lg4ff_device_entry *entry;
	u8 cmd[7];

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return;
	}

	DD_LG4FF_DEBUG("G25/G27/DFGT: setting range to %u", range);

	cmd[0] = 0xf8;
	cmd[1] = 0x81;
	cmd[2] = range & 0x00ff;
	cmd[3] = (range & 0xff00) >> 8;
	cmd[4] = 0x00;
	cmd[5] = 0x00;
	cmd[6] = 0x00;
	dd_lg4ff_send_cmd(entry, cmd);
}

/*
 * ff->set_gain callback, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:1457-1468). Just stores the gain; dd_lg4ff_timer() (above)
 * is what folds it into the force math on the next tick. Wired to the
 * input_dev's ff_device by dd_lg4ff_init() below.
 */
static void dd_lg4ff_set_gain(struct input_dev *dev, u16 gain)
{
	struct hid_device *hid = input_get_drvdata(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return;
	}

	entry->wdata.gain = gain;
}

/*
 * ff->destroy callback, ported verbatim from new-lg4ff (hid-lg4ff.c:2271-
 * 2273). The classic engine keeps no ff_device-private allocation to free
 * here; entry itself is torn down separately by dd_lg4ff_deinit() below.
 */
static void dd_lg4ff_destroy(struct ff_device *ff)
{
}

/*
 * PS-mode -> native-mode switch command, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:418-421). dd_lg4ff_switch_from_ps_mode() below is the only
 * sender that knows to force this.
 */
/* 0x30 - PS mode - Understood by G923 PS */
/* Report ID must be 0x30 */
static const struct dd_lg4ff_compat_mode_switch dd_lg4ff_mode_switch_30_g923 = {
	1,
	{0xf8, 0x09, 0x07, 0x01, 0x01, 0x00, 0x00}	/* Switch mode to G923 with detach */
};

/*
 * PS-mode switch sender, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:1577-1598). Unlike dd_lg4ff_switch_compatibility_mode() (not
 * ported: only used by wheel families we don't carry), this one forces the
 * SET_REPORT's report id to 0x30, which is what the G923 PS wheel expects
 * while still in PlayStation mode.
 */
static int dd_lg4ff_switch_from_ps_mode(struct hid_device *hid, const struct dd_lg4ff_compat_mode_switch *s)
{
	struct dd_lg4ff_device_entry *entry;
	u8 cmd[7];
	u8 i;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return -EINVAL;
	}

	for (i = 0; i < s->cmd_count; i++) {
		u8 j;

		for (j = 0; j < 7; j++)
			cmd[j] = s->cmd[j + (7 * i)];

		dd_lg4ff_send_cmd_with_id(entry, cmd, 0x30);
	}
	hid_hw_wait(hid);
	return 0;
}

/*
 * Multimode wheel identification, ported from new-lg4ff (hid-lg4ff.c:2174-
 * 2207), reduced to the G923 family. new-lg4ff's dd_lg4ff_alternate_modes[0]
 * is a "native" placeholder entry (product_id 0) that its identical loop
 * skips by starting at i=1; our renumbered table (see DD_LG4FF_MODE_*_IDX
 * above) has no such placeholder, so this loop starts at i=0.
 */
static u16 dd_lg4ff_identify_multimode_wheel(struct hid_device *hid, const u16 reported_product_id, const u16 bcdDevice)
{
	u32 current_mode;
	int i;

	/* identify current mode from USB PID */
	for (i = 0; i < ARRAY_SIZE(dd_lg4ff_alternate_modes); i++) {
		DD_LG4FF_DEBUG("Testing whether PID is %X", dd_lg4ff_alternate_modes[i].product_id);
		if (reported_product_id == dd_lg4ff_alternate_modes[i].product_id)
			break;
	}

	if (i == ARRAY_SIZE(dd_lg4ff_alternate_modes))
		return 0;

	current_mode = BIT(i);

	for (i = 0; i < ARRAY_SIZE(dd_lg4ff_main_checklist); i++) {
		const u16 mask = dd_lg4ff_main_checklist[i]->mask;
		const u16 result = dd_lg4ff_main_checklist[i]->result;
		const u16 real_product_id = dd_lg4ff_main_checklist[i]->real_product_id;

		if ((current_mode & dd_lg4ff_main_checklist[i]->modes) &&
				(bcdDevice & mask) == result) {
			DD_LG4FF_DEBUG("Found wheel with real PID %X whose reported PID is %X", real_product_id, reported_product_id);
			return real_product_id;
		}
	}

	/* No match found. This is an unknown wheel; do not touch it. */
	DD_LG4FF_DEBUG("Wheel with bcdDevice %X was not recognized as multimode wheel, leaving in its current mode", bcdDevice);
	return 0;
}

/*
 * Multimode wheel handling, ported from new-lg4ff (hid-lg4ff.c:2209-2269),
 * reduced to the G923 PS -> G923 native switch. The Driving-Force auto-switch
 * branch (hid-lg4ff.c:2222-2242) is dropped: no wheel in dd_lg4ff_devices[]
 * reports as USB_DEVICE_ID_LOGITECH_WHEEL. new-lg4ff gates the G923 PS switch
 * on a module parameter (lg4ff_no_autoswitch, owned by its hid-lg.c glue);
 * this driver has no such glue and no other wheel family to keep in a
 * "compat" mode for, so the switch always runs.
 */
static int dd_lg4ff_handle_multimode_wheel(struct hid_device *hid, u16 *real_product_id, const u16 bcdDevice)
{
	const u16 reported_product_id = hid->product;
	int ret;

	*real_product_id = dd_lg4ff_identify_multimode_wheel(hid, reported_product_id, bcdDevice);
	/* Probed wheel is not a multimode wheel */
	if (!*real_product_id) {
		*real_product_id = reported_product_id;
		DD_LG4FF_DEBUG("Wheel is not a multimode wheel");
		return DD_LG4FF_MMODE_NOT_MULTIMODE;
	}

	/* Switch from "G923 PS" mode to native mode automatically. */
	if (reported_product_id == USB_DEVICE_ID_LOGITECH_G923_PS_WHEEL &&
			reported_product_id != *real_product_id) {
		ret = dd_lg4ff_switch_from_ps_mode(hid, &dd_lg4ff_mode_switch_30_g923);
		if (ret) {
			/* Wheel could not have been switched to Classic mode,
			 * leave it in "PS" mode and continue */
			hid_err(hid, "Unable to switch wheel mode, errno %d\n", ret);
			return DD_LG4FF_MMODE_IS_MULTIMODE;
		}
		return DD_LG4FF_MMODE_SWITCHED;
	}

	return DD_LG4FF_MMODE_IS_MULTIMODE;
}

/*
 * Sysfs attributes, ported from new-lg4ff (hid-lg4ff.c: combine_pedals at
 * :1725-1759, range at :1762-1801, gain at :1838-1872, autocenter at
 * :1875-1909), trimmed to these four settings. The real_id/alternate_modes,
 * LED, and per-effect-class (peak_ffb_level/spring_level/damper_level/
 * friction_level) sysfs from the same reference range are not ported here.
 *
 * These deliberately keep the classic lg4ff attribute names (range/gain/
 * autocenter/combine_pedals) rather than this driver's usual wheel_*
 * naming: the classic engine's settings have different semantics and
 * scale than the DD wheels' wheel_* attributes (a different FFB engine
 * entirely), and downstream tools such as Oversteer already know these
 * names from upstream lg4ff. The device_attribute objects still use this
 * file's dd_lg4ff_ symbol prefix; __ATTR() (rather than DEVICE_ATTR(),
 * which ties the C symbol name to the sysfs file name) is what lets the
 * symbol and file names differ.
 *
 * ffb_output, further down, is not part of that port: it is new, read-only,
 * and named without the wheel_* prefix for the same downstream-tool-naming
 * reason as the four attributes above.
 */
static ssize_t dd_lg4ff_range_show(struct device *dev, struct device_attribute *attr,
				    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	return scnprintf(buf, PAGE_SIZE, "%u\n", entry->wdata.range);
}

/* Set range to the user-specified value, clamped to the wheel's limits. */
static ssize_t dd_lg4ff_range_store(struct device *dev, struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;
	u16 range = simple_strtoul(buf, NULL, 10);

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	if (range == 0)
		range = entry->wdata.max_range;

	if (entry->wdata.set_range && range >= entry->wdata.min_range && range <= entry->wdata.max_range) {
		entry->wdata.set_range(hid, range);
		entry->wdata.range = range;
	}

	return count;
}

static struct device_attribute dd_lg4ff_attr_range =
	__ATTR(range, 0664, dd_lg4ff_range_show, dd_lg4ff_range_store);

static ssize_t dd_lg4ff_gain_show(struct device *dev, struct device_attribute *attr,
				   char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	return scnprintf(buf, PAGE_SIZE, "%u\n", entry->wdata.master_gain);
}

static ssize_t dd_lg4ff_gain_store(struct device *dev, struct device_attribute *attr,
				    const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;
	u16 gain = simple_strtoul(buf, NULL, 10);

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	entry->wdata.master_gain = gain;

	return count;
}

static struct device_attribute dd_lg4ff_attr_gain =
	__ATTR(gain, 0664, dd_lg4ff_gain_show, dd_lg4ff_gain_store);

static ssize_t dd_lg4ff_autocenter_show(struct device *dev, struct device_attribute *attr,
					 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	return scnprintf(buf, PAGE_SIZE, "%u\n", entry->wdata.autocenter);
}

/* Goes through the ff_device's set_autocenter callback, same as the FF core
 * driving FF_AUTOCENTER would, so entry->wdata.autocenter (updated inside
 * dd_lg4ff_set_autocenter_default()) stays the single source of truth.
 */
static ssize_t dd_lg4ff_autocenter_store(struct device *dev, struct device_attribute *attr,
					  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hid_input *hidinput = list_entry(hid->inputs.next, struct hid_input, list);
	struct input_dev *inputdev = hidinput->input;
	u16 autocenter = simple_strtoul(buf, NULL, 10);

	inputdev->ff->set_autocenter(inputdev, autocenter);

	return count;
}

static struct device_attribute dd_lg4ff_attr_autocenter =
	__ATTR(autocenter, 0664, dd_lg4ff_autocenter_show, dd_lg4ff_autocenter_store);

static ssize_t dd_lg4ff_combine_show(struct device *dev, struct device_attribute *attr,
				      char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	return scnprintf(buf, PAGE_SIZE, "%u\n", entry->wdata.combine);
}

static ssize_t dd_lg4ff_combine_store(struct device *dev, struct device_attribute *attr,
				       const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;
	u16 combine = simple_strtoul(buf, NULL, 10);

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	if (combine > 2)
		combine = 2;

	entry->wdata.combine = combine;
	return count;
}

/*
 * Whether the pedals are currently merged onto one axis.
 *
 * Asked by the pedal-polarity correction in hid-logitech-hidpp.c, which
 * must not touch a combined axis: dd_lg4ff_raw_event() has already
 * rewritten those bytes into a bidirectional axis centred at 0x7f, where
 * one pedal drives each direction, so turning it round would swap
 * throttle and brake rather than correct anything.
 */
bool dd_lg4ff_pedals_combined(struct hid_device *hdev)
{
	struct dd_lg4ff_device_entry *entry = dd_lg4ff_get_entry(hdev);

	return entry && entry->wdata.combine != 0;
}

/*
 * Writing to combine_pedals only records entry->wdata.combine; the byte
 * rewrite that actually makes it take effect happens in
 * dd_lg4ff_raw_event(), called for every interface-0 input report.
 */
static struct device_attribute dd_lg4ff_attr_combine_pedals =
	__ATTR(combine_pedals, 0664, dd_lg4ff_combine_show, dd_lg4ff_combine_store);

/*
 * Read-only: the classic engine's current slot-0 net force (post-gain),
 * updated once per timer tick by dd_lg4ff_timer() above. Range is roughly
 * -32768..32767, 0 meaning no force. Consumers such as a userspace
 * TrueForce streamer poll this to mirror the classic engine's force into a
 * TrueForce stream's cur field while that stream is running.
 */
static ssize_t dd_lg4ff_ffb_output_show(struct device *dev, struct device_attribute *attr,
					 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL)
		return -EINVAL;

	return scnprintf(buf, PAGE_SIZE, "%d\n", READ_ONCE(entry->ffb_output));
}

static struct device_attribute dd_lg4ff_attr_ffb_output =
	__ATTR(ffb_output, 0444, dd_lg4ff_ffb_output_show, NULL);

/*
 * Rewrite the wheel's interface-0 input report to synthesize a combined
 * pedal axis, ported from new-lg4ff's lg4ff_raw_event() (hid-lg4ff.c:
 * 1183-1253), trimmed to the G923's byte offset: the reference also
 * carries cases for the DFP/G25/G27/DFGT/G29 and older non-multimode
 * wheels, none of which this driver supports, so only the G923 case
 * survives.
 *
 * Called from hidpp_raw_event() in hid-logitech-hidpp.c for devices
 * carrying HIDPP_QUIRK_CLASS_LG4FF. Always returns 0 (rather than the
 * reference's 1 on a match) so the caller lets the modified bytes
 * continue on to normal input processing instead of having the report
 * treated as fully consumed.
 */
int dd_lg4ff_raw_event(struct hid_device *hdev, struct hid_report *report, u8 *data, int size)
{
	int offset;
	struct dd_lg4ff_device_entry *entry = dd_lg4ff_get_entry(hdev);

	if (!entry)
		return 0;

	if (entry->wdata.combine == 1) {
		switch (entry->wdata.product_id) {
		case USB_DEVICE_ID_LOGITECH_G923_WHEEL:
			offset = 6;
			break;
		default:
			return 0;
		}

		if (size < offset + 2)
			return 0;

		/* Compute a combined axis when wheel does not supply it */
		data[offset] = (0xFF + data[offset] - data[offset + 1]) >> 1;
		data[offset + 1] = 0x7F;
		return 0;
	}

	if (entry->wdata.combine == 2) {
		switch (entry->wdata.product_id) {
		case USB_DEVICE_ID_LOGITECH_G923_WHEEL:
			offset = 6;
			break;
		default:
			return 0;
		}

		if (size < offset + 3)
			return 0;

		/* Compute a combined axis when wheel does not supply it */
		data[offset] = (0xFF + data[offset] - data[offset + 2]) >> 1;
		data[offset + 2] = 0x7F;
		return 0;
	}

	return 0;
}

#ifdef CONFIG_LEDS_CLASS

/*
 * Rev-LED command sender, ported verbatim from new-lg4ff (hid-lg4ff.c:
 * 2044-2062). cmd[0]=0xf8, cmd[1]=0x12 addresses the classic wheel's LED
 * bank, the same command the G25/G27/G29 use; this is unrelated to the
 * HID++ 0x807A feature the native-mode DD wheels expose.
 */
static void dd_lg4ff_set_leds(struct hid_device *hid, u8 leds)
{
	struct dd_lg4ff_device_entry *entry;
	u8 cmd[7];

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return;
	}

	cmd[0] = 0xf8;
	cmd[1] = 0x12;
	cmd[2] = leds;
	cmd[3] = 0x00;
	cmd[4] = 0x00;
	cmd[5] = 0x00;
	cmd[6] = 0x00;
	dd_lg4ff_send_cmd(entry, cmd);
}

/*
 * led_classdev brightness callbacks, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:2064-2115). Each of the 5 registered LEDs maps to one bit
 * of entry->wdata.led_state; toggling one re-sends the whole state with
 * dd_lg4ff_set_leds(). The reference gates the hardware write on its
 * ffb_leds module param (skipping it while the FFB-level calibration
 * display owns the LEDs); that param is not ported here, so the write
 * always happens.
 */
static void dd_lg4ff_led_set_brightness(struct led_classdev *led_cdev,
			enum led_brightness value)
{
	struct device *dev = led_cdev->dev->parent;
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;
	int i, state = 0;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return;
	}

	for (i = 0; i < 5; i++) {
		if (led_cdev != entry->wdata.led[i])
			continue;
		state = (entry->wdata.led_state >> i) & 1;
		if (value == LED_OFF && state) {
			entry->wdata.led_state &= ~(1 << i);
			dd_lg4ff_set_leds(hid, entry->wdata.led_state);
		} else if (value != LED_OFF && !state) {
			entry->wdata.led_state |= 1 << i;
			dd_lg4ff_set_leds(hid, entry->wdata.led_state);
		}
		break;
	}
}

static enum led_brightness dd_lg4ff_led_get_brightness(struct led_classdev *led_cdev)
{
	struct device *dev = led_cdev->dev->parent;
	struct hid_device *hid = to_hid_device(dev);
	struct dd_lg4ff_device_entry *entry;
	int i, value = 0;

	entry = dd_lg4ff_get_entry(hid);
	if (entry == NULL) {
		return -EINVAL;
	}

	for (i = 0; i < 5; i++)
		if (led_cdev == entry->wdata.led[i]) {
			value = (entry->wdata.led_state >> i) & 1;
			break;
		}

	return value ? LED_FULL : LED_OFF;
}

/*
 * LED class device registration, ported verbatim from new-lg4ff
 * (hid-lg4ff.c:2117-2171). Registers 5 LED classdevs named
 * "<devname>::RPM1".."RPM5" and stores them in entry->wdata.led[]; a
 * registration failure for any of them unwinds and unregisters whatever
 * was already registered, then lets the driver continue without LEDs.
 * Called from dd_lg4ff_init() below, once, unconditionally, since our
 * device table (dd_lg4ff_devices[]) carries only the G923.
 */
static void dd_lg4ff_init_leds(struct hid_device *hid, struct dd_lg4ff_device_entry *entry, int i)
{
	int error, j;

	/* register led subsystem - G27/G29/G923 only */
	entry->wdata.led_state = 0;
	for (j = 0; j < 5; j++)
		entry->wdata.led[j] = NULL;

	{
		struct led_classdev *led;
		size_t name_sz;
		char *name;

		dd_lg4ff_set_leds(hid, 0);

		name_sz = strlen(dev_name(&hid->dev)) + 8;

		for (j = 0; j < 5; j++) {
			led = kzalloc(sizeof(struct led_classdev)+name_sz, GFP_KERNEL);
			if (!led) {
				hid_err(hid, "can't allocate memory for LED %d\n", j);
				goto err_leds;
			}

			name = (void *)(&led[1]);
			snprintf(name, name_sz, "%s::RPM%d", dev_name(&hid->dev), j+1);
			led->name = name;
			led->brightness = 0;
			led->max_brightness = 1;
			led->brightness_get = dd_lg4ff_led_get_brightness;
			led->brightness_set = dd_lg4ff_led_set_brightness;

			entry->wdata.led[j] = led;
			error = led_classdev_register(&hid->dev, led);

			if (error) {
				hid_err(hid, "failed to register LED %d. Aborting.\n", j);
err_leds:
				/* Deregister LEDs (if any) */
				for (j = 0; j < 5; j++) {
					led = entry->wdata.led[j];
					entry->wdata.led[j] = NULL;
					if (!led)
						continue;
					led_classdev_unregister(led);
					kfree(led);
				}
				goto out;	/* Let the driver continue without LEDs */
			}
		}
	}
out:
	return;
}
#endif

/*
 * dd_lg4ff_init() / dd_lg4ff_deinit(), ported from new-lg4ff's lg4ff_init()
 * / lg4ff_deinit() (hid-lg4ff.c:2275-2571), reduced to FFB setup plus the
 * rev-LED block (hid-lg4ff.c:2400-2409, 2545-2563; see dd_lg4ff_init_leds()
 * above). The ffb_leds sysfs attribute and its FFB-level calibration
 * display (hid-lg4ff.c:2021-2042, 2456-2462) are not ported, so the
 * sysfs-file creation/removal blocks (hid-lg4ff.c:2411-2462, 2517-2541) are
 * reduced to the four attributes defined just above.
 *
 * The entry is reached through hidpp_dd_lg4ff_slot(), a caller-owned
 * void** into struct hidpp_device::lg4ff_entry (see the comment on
 * dd_lg4ff_get_entry() above) rather than new-lg4ff's lg_drv_data->
 * device_props, since this driver has no lg_drv_data.
 *
 * Idempotency: a G923 PS wheel (c267) re-enumerates as a native G923 (c266)
 * after dd_lg4ff_handle_multimode_wheel() switches it, which means probe
 * runs this function a second time. The first (c267) pass allocates entry,
 * stores it in the slot, then hits LG4FF_MMODE_SWITCHED and must free that
 * entry and clear the slot before returning, mirroring new-lg4ff's
 * err_init path, or the second (c266) pass would leak it.
 */
int dd_lg4ff_init(struct hid_device *hdev)
{
	struct hid_input *hidinput;
	struct input_dev *dev;
	struct list_head *report_list = &hdev->report_enum[HID_OUTPUT_REPORT].report_list;
	struct hid_report *report = list_entry(report_list->next, struct hid_report, list);
	const struct usb_device_descriptor *udesc = &(hid_to_usb_dev(hdev)->descriptor);
	const u16 bcdDevice = le16_to_cpu(udesc->bcdDevice);
	const struct dd_lg4ff_multimode_wheel *mmode_wheel = NULL;
	struct dd_lg4ff_device_entry *entry;
	void **slot;
	int error, i, j;
	int mmode_ret, mmode_idx = -1;
	u16 real_product_id;
	struct ff_device *ff;

	if (list_empty(&hdev->inputs)) {
		hid_err(hdev, "no inputs found\n");
		return -ENODEV;
	}
	hidinput = list_entry(hdev->inputs.next, struct hid_input, list);
	dev = hidinput->input;

	/* Check that the report looks ok */
	if (!hid_validate_values(hdev, HID_OUTPUT_REPORT, 0, 0, 7))
		return -EINVAL;

	slot = (void **)hidpp_dd_lg4ff_slot(hdev);
	if (!slot) {
		hid_err(hdev, "Cannot add device, private driver data not allocated\n");
		return -EINVAL;
	}

	entry = kzalloc(sizeof(*entry), GFP_KERNEL);
	if (!entry)
		return -ENOMEM;

	spin_lock_init(&entry->report_lock);
	entry->hid = hdev;
	entry->report = report;
	*slot = entry;

	/* Check if a multimode wheel has been connected and
	 * handle it appropriately */
	mmode_ret = dd_lg4ff_handle_multimode_wheel(hdev, &real_product_id, bcdDevice);

	/* Wheel has been told to switch to native mode. There is no point in going on
	 * with the initialization as the wheel will do a USB reset when it switches mode
	 */
	if (mmode_ret == DD_LG4FF_MMODE_SWITCHED) {
		error = 0;
		goto err_init;
	} else if (mmode_ret < 0) {
		hid_err(hdev, "Unable to switch device mode during initialization, errno %d\n", mmode_ret);
		error = mmode_ret;
		goto err_init;
	}

	/* Check what wheel has been connected */
	for (i = 0; i < ARRAY_SIZE(dd_lg4ff_devices); i++) {
		if (hdev->product == dd_lg4ff_devices[i].product_id) {
			DD_LG4FF_DEBUG("Found compatible device, product ID %04X", dd_lg4ff_devices[i].product_id);
			break;
		}
	}

	if (i == ARRAY_SIZE(dd_lg4ff_devices)) {
		hid_err(hdev, "This device is flagged to be handled by dd-lg4ff but is not listed in dd_lg4ff_devices[]\n");
		error = -ENODEV;
		goto err_init;
	}

	if (mmode_ret == DD_LG4FF_MMODE_IS_MULTIMODE) {
		for (mmode_idx = 0; mmode_idx < ARRAY_SIZE(dd_lg4ff_multimode_wheels); mmode_idx++) {
			if (real_product_id == dd_lg4ff_multimode_wheels[mmode_idx].product_id)
				break;
		}

		if (mmode_idx == ARRAY_SIZE(dd_lg4ff_multimode_wheels)) {
			hid_err(hdev, "Device product ID %X is not listed as a multimode wheel\n", real_product_id);
			error = -ENODEV;
			goto err_init;
		}
	}

	/* Set supported force feedback capabilities */
	for (j = 0; dd_lg4ff_devices[i].ff_effects[j] >= 0; j++)
		set_bit(dd_lg4ff_devices[i].ff_effects[j], dev->ffbit);

	error = input_ff_create(dev, DD_LG4FF_MAX_EFFECTS);
	if (error)
		goto err_init;

	ff = dev->ff;
	ff->upload = dd_lg4ff_upload_effect;
	ff->playback = dd_lg4ff_play_effect;
	ff->set_gain = dd_lg4ff_set_gain;
	ff->destroy = dd_lg4ff_destroy;

	/* Initialize device properties */
	if (mmode_ret == DD_LG4FF_MMODE_IS_MULTIMODE) {
		BUG_ON(mmode_idx == -1);
		mmode_wheel = &dd_lg4ff_multimode_wheels[mmode_idx];
	}
	dd_lg4ff_init_wheel_data(&entry->wdata, &dd_lg4ff_devices[i], mmode_wheel, real_product_id);

	set_bit(FF_GAIN, dev->ffbit);

	/* Check if autocentering is available and
	 * set the centering force to zero by default */
	if (test_bit(FF_AUTOCENTER, dev->ffbit)) {
		dev->ff->set_autocenter = dd_lg4ff_set_autocenter_default;
		dev->ff->set_autocenter(dev, 0);
	}

#ifdef CONFIG_LEDS_CLASS
	entry->has_leds = 1;
	dd_lg4ff_init_leds(hdev, entry, i);
#endif

	/* Create sysfs interface */
	error = device_create_file(&hdev->dev, &dd_lg4ff_attr_combine_pedals);
	if (error)
		hid_warn(hdev, "Unable to create sysfs interface for \"combine_pedals\", errno %d\n", error);
	error = device_create_file(&hdev->dev, &dd_lg4ff_attr_range);
	if (error)
		hid_warn(hdev, "Unable to create sysfs interface for \"range\", errno %d\n", error);
	error = device_create_file(&hdev->dev, &dd_lg4ff_attr_ffb_output);
	if (error)
		hid_warn(hdev, "Unable to create sysfs interface for \"ffb_output\", errno %d\n", error);
	if (test_bit(FF_CONSTANT, dev->ffbit)) {
		error = device_create_file(&hdev->dev, &dd_lg4ff_attr_gain);
		if (error)
			hid_warn(hdev, "Unable to create sysfs interface for \"gain\", errno %d\n", error);
		if (test_bit(FF_AUTOCENTER, dev->ffbit)) {
			error = device_create_file(&hdev->dev, &dd_lg4ff_attr_autocenter);
			if (error)
				hid_warn(hdev, "Unable to create sysfs interface for \"autocenter\", errno %d\n", error);
		}
	}

	/* Set the maximum range to start with */
	entry->wdata.range = entry->wdata.max_range;
	if (entry->wdata.set_range)
		entry->wdata.set_range(hdev, entry->wdata.range);

	dd_lg4ff_init_slots(entry);

	entry->effects_used = 0;
	entry->wdata.master_gain = 0xffff;
	entry->wdata.gain = 0xffff;

	spin_lock_init(&entry->timer_lock);

#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 15, 0)
	hrtimer_init(&entry->hrtimer, CLOCK_MONOTONIC, HRTIMER_MODE_REL);
	entry->hrtimer.function = dd_lg4ff_timer_hires;
#else
	hrtimer_setup(&entry->hrtimer, dd_lg4ff_timer_hires, CLOCK_MONOTONIC, HRTIMER_MODE_REL);
#endif

	hid_info(hdev, "Force feedback support for G923 (classic mode)\n");
	hid_info(hdev, "Hires timer: period = %d ms", dd_lg4ff_timer_msecs);

	return 0;

err_init:
	*slot = NULL;
	kfree(entry);
	return error;
}

void dd_lg4ff_deinit(struct hid_device *hdev)
{
	struct hid_input *hidinput;
	struct input_dev *dev;
	struct dd_lg4ff_device_entry *entry;
	void **slot;

	slot = (void **)hidpp_dd_lg4ff_slot(hdev);
	if (!slot) {
		hid_err(hdev, "Error while deinitializing device, no private driver data.\n");
		return;
	}

	entry = (struct dd_lg4ff_device_entry *)*slot;
	if (!entry)
		return; /* Nothing more to do */

	/*
	 * Only interface 0 ever gets a real entry, and only it registers a
	 * hid_input (dd_lg4ff_init runs after HID_CONNECT_HIDINPUT there).
	 * Interfaces 1 and 2 always bail out above via the entry-NULL check,
	 * so it is safe to assume hdev->inputs is non-empty from here on;
	 * list_entry() on an empty list would read past the list head.
	 */
	hidinput = list_entry(hdev->inputs.next, struct hid_input, list);
	dev = hidinput->input;

	device_remove_file(&hdev->dev, &dd_lg4ff_attr_combine_pedals);
	device_remove_file(&hdev->dev, &dd_lg4ff_attr_range);
	device_remove_file(&hdev->dev, &dd_lg4ff_attr_ffb_output);
	if (test_bit(FF_CONSTANT, dev->ffbit)) {
		device_remove_file(&hdev->dev, &dd_lg4ff_attr_gain);
		if (test_bit(FF_AUTOCENTER, dev->ffbit))
			device_remove_file(&hdev->dev, &dd_lg4ff_attr_autocenter);
	}

	hrtimer_cancel(&entry->hrtimer);

	/* Belt and suspenders: dd_lg4ff_stop_effects() below also zeroes
	 * ffb_output, but a poller reading it between hrtimer_cancel() and
	 * that call would otherwise still see the last live value.
	 */
	WRITE_ONCE(entry->ffb_output, 0);

	dd_lg4ff_stop_effects(entry);

#ifdef CONFIG_LEDS_CLASS
	if (entry->has_leds) {
		int j;
		struct led_classdev *led;

		/* Deregister LEDs (if any) */
		for (j = 0; j < 5; j++) {
			led = entry->wdata.led[j];
			entry->wdata.led[j] = NULL;
			if (!led)
				continue;
			led_classdev_unregister(led);
			kfree(led);
		}
	}
#endif

	*slot = NULL;
	kfree(entry);
}
