// SPDX-License-Identifier: GPL-2.0-only
/*
 *  HIDPP protocol for Logitech receivers
 *
 *  Copyright (c) 2011 Logitech (c)
 *  Copyright (c) 2012-2013 Google (c)
 *  Copyright (c) 2013-2014 Red Hat Inc.
 */


#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/delay.h>
#include <linux/device.h>
#include <linux/input.h>
#include <linux/usb.h>
#include <linux/hid.h>
#include <linux/hidraw.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/sched.h>
#include <linux/sched/clock.h>
#include <linux/kfifo.h>
#include <linux/input/mt.h>
#include <linux/workqueue.h>
#include <linux/atomic.h>
#include <linux/fixp-arith.h>
#include <linux/version.h>
/*
 * linux/unaligned.h was introduced in kernel 6.12, older kernels use asm/unaligned.h
 * Note: Using LINUX_VERSION_CODE instead of __has_include() for sparse compatibility
 */
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 12, 0)
#include <linux/unaligned.h>
#else
#include <asm/unaligned.h>
#endif
#include <linux/math.h>
#include <linux/math64.h>

/*
 * Kernel compatibility macros
 */

/* usb_set_wireless_status was added in kernel 6.0 */
#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 0, 0)
#define usb_set_wireless_status(intf, status) do { } while (0)
#define USB_WIRELESS_STATUS_CONNECTED 0
#define USB_WIRELESS_STATUS_DISCONNECTED 1
#endif

/* report_fixup callback signature changed from u8* to const u8* in 6.12 */
#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 12, 0)
#define HIDPP_REPORT_FIXUP_RETURN_TYPE u8 *
#else
#define HIDPP_REPORT_FIXUP_RETURN_TYPE const u8 *
#endif
/*
 * Upstream in-tree drivers include "usbhid/usbhid.h" to get
 * hid_to_usb_dev(). The header is not exported by kernel-devel on
 * several distributions (Fedora, CachyOS, Arch family), which broke
 * out-of-tree builds on those hosts even though the symbol itself is
 * trivial. Inline the one macro we use so the driver builds anywhere
 * linux/usb.h (which provides to_usb_device) is available.
 */
#ifndef hid_to_usb_dev
#define hid_to_usb_dev(hid_dev) \
	to_usb_device((hid_dev)->dev.parent->parent)
#endif
#include "hid-ids.h"

/*
 * Model tag for kernel log messages, resolved from the bound identity.
 *
 * Every wheel of the direct-drive (DD) family shares one code path, so
 * a hardcoded model name in log strings would lie on half the hardware.
 * The RS50 in G PRO compatibility mode spoofs the G PRO product ID but
 * keeps its own USB product string ("RS50 Base for PlayStation/PC",
 * verified live), while a real G PRO reports "PRO Racing Wheel"
 * (verified from contributor captures on real hardware), so the
 * substring check below separates the two reliably.
 */
/*
 * The single identity heuristic every consumer must share: an RS50
 * borrowing the G PRO product ID keeps its own USB product string
 * ("RS50 Base for PlayStation/PC", verified live), while a real G PRO
 * reports "PRO Racing Wheel" (verified from contributor captures).
 * dd_wheel_name() (log tags) and dd_is_real_gpro() (LED-surface
 * gating) both build on this ONE strstr so a firmware string change
 * or new compat PID can never make logs and sysfs disagree about
 * which wheel this is.
 */
static bool dd_product_claims_rs50(struct hid_device *hdev)
{
	return strstr(hdev->name, "RS50") != NULL;
}

static const char *dd_wheel_name(struct hid_device *hdev)
{
	switch (hdev->product) {
	case USB_DEVICE_ID_LOGITECH_RS50:
		return "RS50 (native)";
	case USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL:
	case USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL:
		if (dd_product_claims_rs50(hdev))
			return "RS50 (G PRO compatibility mode)";
		return "G PRO";
	default:
		return "DD wheel";
	}
}

/*
 * Real G PRO Racing Wheel: is this bound identity an actual G PRO, as
 * opposed to an RS50 spoofing the G PRO product ID in compatibility
 * mode? The RS50 keeps its own USB product string under the borrowed
 * PID (verified live), while a real G PRO reports "PRO Racing Wheel"
 * (verified from contributor captures). Used to gate the LED surface:
 * the two rims have entirely different LED hardware and protocols.
 */
static bool dd_is_real_gpro(struct hid_device *hdev)
{
	return (hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
		hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL) &&
	       !dd_product_claims_rs50(hdev);
}

#define dd_info(hdev, fmt, ...) \
	hid_info(hdev, "%s: " fmt, dd_wheel_name(hdev), ##__VA_ARGS__)
#define dd_warn(hdev, fmt, ...) \
	hid_warn(hdev, "%s: " fmt, dd_wheel_name(hdev), ##__VA_ARGS__)
#define dd_err(hdev, fmt, ...) \
	hid_err(hdev, "%s: " fmt, dd_wheel_name(hdev), ##__VA_ARGS__)
#define dd_dbg(hdev, fmt, ...) \
	hid_dbg(hdev, "%s: " fmt, dd_wheel_name(hdev), ##__VA_ARGS__)
#include "hidpp_dd_tf_init.h"

/*
 * Build-time identifier supplied by Kbuild (-DHIDPP_DD_GIT_HASH=...). Falls
 * back to "unknown" so the module still builds when the source dir is
 * neither a git checkout nor stamped by tools/dkms-update.sh (e.g.
 * tarball install).
 */
#ifndef HIDPP_DD_GIT_HASH
#define HIDPP_DD_GIT_HASH "unknown"
#endif

MODULE_DESCRIPTION("Support for Logitech devices relying on the HID++ specification");
MODULE_LICENSE("GPL");
MODULE_VERSION(HIDPP_DD_GIT_HASH);
MODULE_AUTHOR("Benjamin Tissoires <benjamin.tissoires@gmail.com>");
MODULE_AUTHOR("Nestor Lopez Casado <nlopezcasad@logitech.com>");
MODULE_AUTHOR("Bastien Nocera <hadess@hadess.net>");

static bool disable_tap_to_click;
module_param(disable_tap_to_click, bool, 0644);
MODULE_PARM_DESC(disable_tap_to_click,
	"Disable Tap-To-Click mode reporting for touchpads (only on the K400 currently).");

/*
 * inject_pid: append a USB HID PID (Physical Input Device, Usage Page 0x0F)
 * output collection to interface 0's report descriptor on RS50 / G Pro
 * wheels, and route PID output reports written by userspace (Wine's
 * hid_joystick over /dev/hidraw) to the wheel's real FFB path on
 * interface 2. Needed for Proton's default hidraw-backed dinput
 * (no PROTON_ENABLE_HIDRAW required).
 *
 *   0 = off (default; no descriptor change, no override installed)
 *   1 = dry-run: inject descriptor, install override, LOG every PID output
 *       report we receive, but do NOT drive the wheel. Lets us observe
 *       what Wine actually writes before we trust our translations.
 *   2 = actuate: full translation, calls hidpp_dd_ff_upload/playback to drive
 *       the wheel via interface 2.
 *
 * Dry-run exists specifically so we can bring this up on a live wheel
 * without risking a slam from a mis-translated effect.
 */
static uint inject_pid;
module_param(inject_pid, uint, 0644);
MODULE_PARM_DESC(inject_pid,
	"PID injection on interface 0 of direct-drive (RS50/G PRO) wheels: 0=off (default), 1=dry-run (log only), 2=actuate (drive the wheel).");

/*
 * HID++ software-id OR'd into every request's funcindex_clientid.
 *
 * Upstream hid-logitech-hidpp uses 0x01. The RS50 / G PRO PEDAL unit, however,
 * is a separate MCU bridged by the wheel base at HID++ device index 0x02, and
 * its firmware SILENTLY DROPS every request carrying software-id 0x01 - it
 * sends no answer at all, not even a HID++ error. Verified live by sweeping the
 * id against the pedal's getFeature(0x80A4): 0x02, 0x05, 0x0a and 0x0f are all
 * answered, only 0x01 gets total silence; meanwhile the base (0xff) and motor
 * (0x05) answer any id including 0x01. Our old 0x01 therefore made every pedal
 * query (e.g. the 0x80A4 response-curve probe in discover_settings_features)
 * time out, and the retry loop turned that into a 15-20 s init stall (issue
 * #30). G HUB uses 0x0a; we match it so the pedal answers on the first try.
 * The base and motor are unaffected by the change.
 */
#define LINUX_KERNEL_SW_ID			0x0a

#define REPORT_ID_HIDPP_SHORT			0x10
#define REPORT_ID_HIDPP_LONG			0x11
#define REPORT_ID_HIDPP_VERY_LONG		0x12

#define HIDPP_REPORT_SHORT_LENGTH		7
#define HIDPP_REPORT_LONG_LENGTH		20
#define HIDPP_REPORT_VERY_LONG_MAX_LENGTH	64

#define HIDPP_REPORT_SHORT_SUPPORTED		BIT(0)
#define HIDPP_REPORT_LONG_SUPPORTED		BIT(1)
#define HIDPP_REPORT_VERY_LONG_SUPPORTED	BIT(2)

#define HIDPP_SUB_ID_CONSUMER_VENDOR_KEYS	0x03
#define HIDPP_SUB_ID_ROLLER			0x05
#define HIDPP_SUB_ID_MOUSE_EXTRA_BTNS		0x06
#define HIDPP_SUB_ID_USER_IFACE_EVENT		0x08
#define HIDPP_USER_IFACE_EVENT_ENCRYPTION_KEY_LOST	BIT(5)

#define HIDPP_QUIRK_CLASS_WTP			BIT(0)
#define HIDPP_QUIRK_CLASS_M560			BIT(1)
#define HIDPP_QUIRK_CLASS_K400			BIT(2)
#define HIDPP_QUIRK_CLASS_G920			BIT(3)
#define HIDPP_QUIRK_CLASS_K750			BIT(4)

/* bits 2..20 are reserved for classes */
/* #define HIDPP_QUIRK_CONNECT_EVENTS		BIT(21) disabled */
#define HIDPP_QUIRK_WTP_PHYSICAL_BUTTONS	BIT(22)
#define HIDPP_QUIRK_DELAYED_INIT		BIT(23)
#define HIDPP_QUIRK_FORCE_OUTPUT_REPORTS	BIT(24)
#define HIDPP_QUIRK_HIDPP_WHEELS		BIT(25)
#define HIDPP_QUIRK_HIDPP_EXTRA_MOUSE_BTNS	BIT(26)
#define HIDPP_QUIRK_HIDPP_CONSUMER_VENDOR_KEYS	BIT(27)
#define HIDPP_QUIRK_HI_RES_SCROLL_1P0		BIT(28)
#define HIDPP_QUIRK_WIRELESS_STATUS		BIT(29)
#define HIDPP_QUIRK_RESET_HI_RES_SCROLL		BIT(30)
#define HIDPP_QUIRK_DD_FFB			BIT(31)

/* These are just aliases for now */
#define HIDPP_QUIRK_KBD_SCROLL_WHEEL HIDPP_QUIRK_HIDPP_WHEELS
#define HIDPP_QUIRK_KBD_ZOOM_WHEEL   HIDPP_QUIRK_HIDPP_WHEELS

/* Convenience constant to check for any high-res support. */
#define HIDPP_CAPABILITY_HI_RES_SCROLL	(HIDPP_CAPABILITY_HIDPP10_FAST_SCROLL | \
					 HIDPP_CAPABILITY_HIDPP20_HI_RES_SCROLL | \
					 HIDPP_CAPABILITY_HIDPP20_HI_RES_WHEEL)

#define HIDPP_CAPABILITY_HIDPP10_BATTERY	BIT(0)
#define HIDPP_CAPABILITY_HIDPP20_BATTERY	BIT(1)
#define HIDPP_CAPABILITY_BATTERY_MILEAGE	BIT(2)
#define HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS	BIT(3)
#define HIDPP_CAPABILITY_BATTERY_VOLTAGE	BIT(4)
#define HIDPP_CAPABILITY_BATTERY_PERCENTAGE	BIT(5)
#define HIDPP_CAPABILITY_UNIFIED_BATTERY	BIT(6)
#define HIDPP_CAPABILITY_HIDPP20_HI_RES_WHEEL	BIT(7)
#define HIDPP_CAPABILITY_HIDPP20_HI_RES_SCROLL	BIT(8)
#define HIDPP_CAPABILITY_HIDPP10_FAST_SCROLL	BIT(9)
#define HIDPP_CAPABILITY_ADC_MEASUREMENT	BIT(10)

#define lg_map_key_clear(c)  hid_map_usage_clear(hi, usage, bit, max, EV_KEY, (c))

/*
 * There are two hidpp protocols in use, the first version hidpp10 is known
 * as register access protocol or RAP, the second version hidpp20 is known as
 * feature access protocol or FAP
 *
 * Most older devices (including the Unifying usb receiver) use the RAP protocol
 * where as most newer devices use the FAP protocol. Both protocols are
 * compatible with the underlying transport, which could be usb, Unifiying, or
 * bluetooth. The message lengths are defined by the hid vendor specific report
 * descriptor for the HIDPP_SHORT report type (total message lenth 7 bytes) and
 * the HIDPP_LONG report type (total message length 20 bytes)
 *
 * The RAP protocol uses both report types, whereas the FAP only uses HIDPP_LONG
 * messages. The Unifying receiver itself responds to RAP messages (device index
 * is 0xFF for the receiver), and all messages (short or long) with a device
 * index between 1 and 6 are passed untouched to the corresponding paired
 * Unifying device.
 *
 * The paired device can be RAP or FAP, it will receive the message untouched
 * from the Unifiying receiver.
 */

struct fap {
	u8 feature_index;
	u8 funcindex_clientid;
	u8 params[HIDPP_REPORT_VERY_LONG_MAX_LENGTH - 4U];
};

struct rap {
	u8 sub_id;
	u8 reg_address;
	u8 params[HIDPP_REPORT_VERY_LONG_MAX_LENGTH - 4U];
};

struct hidpp_report {
	u8 report_id;
	u8 device_index;
	union {
		struct fap fap;
		struct rap rap;
		u8 rawbytes[sizeof(struct fap)];
	};
} __packed;

struct hidpp_battery {
	u8 feature_index;
	u8 solar_feature_index;
	u8 voltage_feature_index;
	u8 adc_measurement_feature_index;
	struct power_supply_desc desc;
	struct power_supply *ps;
	char name[64];
	int status;
	int capacity;
	int level;
	int voltage;
	int charge_type;
	bool online;
	u8 supported_levels_1004;
};

/**
 * struct hidpp_scroll_counter - Utility class for processing high-resolution
 *                             scroll events.
 * @dev: the input device for which events should be reported.
 * @wheel_multiplier: the scalar multiplier to be applied to each wheel event
 * @remainder: counts the number of high-resolution units moved since the last
 *             low-resolution event (REL_WHEEL or REL_HWHEEL) was sent. Should
 *             only be used by class methods.
 * @direction: direction of last movement (1 or -1)
 * @last_time: last event time, used to reset remainder after inactivity
 */
struct hidpp_scroll_counter {
	int wheel_multiplier;
	int remainder;
	int direction;
	unsigned long long last_time;
};

struct hidpp_dd_pid_state;	/* defined later, PID injection translator */

struct hidpp_device {
	struct hid_device *hid_dev;
	struct input_dev *input;
	struct mutex send_mutex;
	void *send_receive_buf;
	char *name;		/* will never be NULL and should not be freed */
	wait_queue_head_t wait;
	int very_long_report_length;
	bool answer_available;
	u8 protocol_major;
	u8 protocol_minor;

	void *private_data;

	struct work_struct work;
	struct work_struct reset_hi_res_work;
	struct delayed_work ff_retry_work;
	int ff_retries;
	struct kfifo delayed_work_fifo;
	struct input_dev *delayed_input;

	unsigned long quirks;
	unsigned long capabilities;
	u8 supported_reports;

	struct hidpp_battery battery;
	struct hidpp_scroll_counter vertical_wheel_counter;

	u8 wireless_feature_index;

	bool connected_once;

	/*
	 * Set on a direct-drive wheel's interface 0, which we claim only to
	 * track the steering position. That interface's input report carries
	 * no report ID, so its first byte is joystick DATA (hat + buttons 1-4)
	 * and can collide with REPORT_ID_HIDPP_*. hidpp_raw_event must not
	 * parse those frames as HID++ - see the comment there.
	 */
	bool no_hidpp_reports;

	/*
	 * Scratch buffer for the PID-injected interface-0 descriptor. Filled
	 * in hidpp_report_fixup when inject_pid=1; devm-allocated on hdev so
	 * it lives as long as hdev does. NULL means no injection happened
	 * on this device. See hidpp_dd_pid_rdesc.
	 */
	u8 *pid_fixup_buf;

	/*
	 * Per-device PID translator state, kept here (rather than via
	 * private_data) because interface 0 of RS50-in-compat-mode also
	 * has HIDPP_QUIRK_DD_FFB set and that quirk's existing teardown
	 * path assumes private_data points at hidpp_dd_ff_data. Using a
	 * dedicated field keeps the two concerns independent. devm-
	 * allocated on hdev; cleared by hidpp_dd_pid_uninstall on teardown.
	 */
	struct hidpp_dd_pid_state *pid_state;
};

/* HID++ 1.0 error codes */
#define HIDPP_ERROR				0x8f
#define HIDPP_ERROR_SUCCESS			0x00
#define HIDPP_ERROR_INVALID_SUBID		0x01
#define HIDPP_ERROR_INVALID_ADRESS		0x02
#define HIDPP_ERROR_INVALID_VALUE		0x03
#define HIDPP_ERROR_CONNECT_FAIL		0x04
#define HIDPP_ERROR_TOO_MANY_DEVICES		0x05
#define HIDPP_ERROR_ALREADY_EXISTS		0x06
#define HIDPP_ERROR_BUSY			0x07
#define HIDPP_ERROR_UNKNOWN_DEVICE		0x08
#define HIDPP_ERROR_RESOURCE_ERROR		0x09
#define HIDPP_ERROR_REQUEST_UNAVAILABLE		0x0a
#define HIDPP_ERROR_INVALID_PARAM_VALUE		0x0b
#define HIDPP_ERROR_WRONG_PIN_CODE		0x0c
/* HID++ 2.0 error codes */
#define HIDPP20_ERROR_NO_ERROR			0x00
#define HIDPP20_ERROR_UNKNOWN			0x01
#define HIDPP20_ERROR_INVALID_ARGS		0x02
#define HIDPP20_ERROR_OUT_OF_RANGE		0x03
#define HIDPP20_ERROR_HW_ERROR			0x04
#define HIDPP20_ERROR_NOT_ALLOWED		0x05
#define HIDPP20_ERROR_INVALID_FEATURE_INDEX	0x06
#define HIDPP20_ERROR_INVALID_FUNCTION_ID	0x07
#define HIDPP20_ERROR_BUSY			0x08
#define HIDPP20_ERROR_UNSUPPORTED		0x09
#define HIDPP20_ERROR				0xff

static int __hidpp_send_report(struct hid_device *hdev,
				struct hidpp_report *hidpp_report)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	int fields_count, ret;

	switch (hidpp_report->report_id) {
	case REPORT_ID_HIDPP_SHORT:
		fields_count = HIDPP_REPORT_SHORT_LENGTH;
		break;
	case REPORT_ID_HIDPP_LONG:
		fields_count = HIDPP_REPORT_LONG_LENGTH;
		break;
	case REPORT_ID_HIDPP_VERY_LONG:
		fields_count = hidpp->very_long_report_length;
		break;
	default:
		return -ENODEV;
	}

	/*
	 * Default device_index to the receiver (0xff) unless the caller
	 * has already set a specific sub-device index (e.g. 0x05 for the
	 * G Pro calibration engine). Sub-device targeting is needed for
	 * features that only respond via one of the wheel's auxiliary
	 * HID++ devices rather than the root.
	 */
	if (hidpp_report->device_index == 0)
		hidpp_report->device_index = 0xff;

	if (hidpp->quirks & HIDPP_QUIRK_FORCE_OUTPUT_REPORTS) {
		ret = hid_hw_output_report(hdev, (u8 *)hidpp_report, fields_count);
		/*
		 * RS50 in G Pro compatibility mode (PID c272/c268 with the
		 * HIDPP_DD_FFB quirk promoted) inherits the FORCE_OUTPUT_REPORTS
		 * quirk from the G Pro id-table entry but has no interrupt
		 * OUT endpoint on interface 1, so usbhid_output_report
		 * returns -ENOSYS. Mirror the fallback hidraw_write does in
		 * the same situation: drop down to a SET_REPORT control
		 * transfer instead. This is a no-op on real G Pro / G920
		 * which DO have the OUT endpoint and complete on the first
		 * call.
		 */
		if (ret == -ENOSYS)
			ret = hid_hw_raw_request(hdev,
				hidpp_report->report_id,
				(u8 *)hidpp_report, fields_count,
				HID_OUTPUT_REPORT, HID_REQ_SET_REPORT);
	} else {
		ret = hid_hw_raw_request(hdev, hidpp_report->report_id,
			(u8 *)hidpp_report, fields_count, HID_OUTPUT_REPORT,
			HID_REQ_SET_REPORT);
	}

	return ret == fields_count ? 0 : -1;
}

/*
 * Effectively send the message to the device, waiting for its answer.
 *
 * Must be called with hidpp->send_mutex locked
 *
 * Same return protocol than hidpp_send_message_sync():
 * - success on 0
 * - negative error means transport error
 * - positive value means protocol error
 */
static int __do_hidpp_send_message_sync(struct hidpp_device *hidpp,
	struct hidpp_report *message,
	struct hidpp_report *response)
{
	int ret;

	__must_hold(&hidpp->send_mutex);

	hidpp->send_receive_buf = response;
	hidpp->answer_available = false;

	/*
	 * Default the device index BEFORE the question is copied for
	 * answer matching. __hidpp_send_report() applies the same
	 * default in-place at send time; if the copy happened first,
	 * the question would keep 0x00 while the wire (and therefore
	 * the answer) carries 0xff, hidpp_match_answer() would reject
	 * every first-attempt answer on the device-index check, and
	 * each sync call would eat a full 5 s timeout before the retry
	 * accidentally succeeded with the mutated message (observed
	 * live: every settings GET/SET stalling ~5 s).
	 */
	if (message->device_index == 0)
		message->device_index = 0xff;

	/*
	 * So that we can later validate the answer when it arrives
	 * in hidpp_raw_event
	 */
	*response = *message;

	ret = __hidpp_send_report(hidpp->hid_dev, message);
	if (ret) {
		dbg_hid("__hidpp_send_report returned err: %d\n", ret);
		memset(response, 0, sizeof(struct hidpp_report));
		return ret;
	}

	if (!wait_event_timeout(hidpp->wait, hidpp->answer_available,
				5*HZ)) {
		dbg_hid("%s:timeout waiting for response\n", __func__);
		memset(response, 0, sizeof(struct hidpp_report));
		return -ETIMEDOUT;
	}

	if (response->report_id == REPORT_ID_HIDPP_SHORT &&
	    response->rap.sub_id == HIDPP_ERROR) {
		ret = response->rap.params[1];
		dbg_hid("%s:got hidpp error %02X\n", __func__, ret);
		return ret;
	}

	if ((response->report_id == REPORT_ID_HIDPP_LONG ||
	     response->report_id == REPORT_ID_HIDPP_VERY_LONG) &&
	    response->fap.feature_index == HIDPP20_ERROR) {
		ret = response->fap.params[1];
		dbg_hid("%s:got hidpp 2.0 error %02X\n", __func__, ret);
		return ret;
	}

	return 0;
}

/*
 * hidpp_send_message_sync() returns 0 in case of success, and something else
 * in case of a failure.
 *
 * See __do_hidpp_send_message_sync() for a detailed explanation of the returned
 * value.
 */
static int hidpp_send_message_sync(struct hidpp_device *hidpp,
	struct hidpp_report *message,
	struct hidpp_report *response)
{
	int ret;
	int max_retries = 3;

	mutex_lock(&hidpp->send_mutex);

	do {
		ret = __do_hidpp_send_message_sync(hidpp, message, response);
		/*
		 * A transport failure (ret < 0, e.g. -ETIMEDOUT: the device
		 * sent no answer) memsets `response`, so the report_id-keyed
		 * BUSY checks below would never break and we would retry a dead
		 * query up to 3x - each a full 5s timeout, all while holding
		 * send_mutex. That is what turned one silent query into a
		 * 15-20s init stall and blocked every other HID++ user behind
		 * it. Retrying only helps for an explicit HID++ BUSY error;
		 * bail out immediately on any transport error.
		 */
		if (ret < 0)
			break;
		if (response->report_id == REPORT_ID_HIDPP_SHORT &&
		    ret != HIDPP_ERROR_BUSY)
			break;
		if ((response->report_id == REPORT_ID_HIDPP_LONG ||
		     response->report_id == REPORT_ID_HIDPP_VERY_LONG) &&
		    ret != HIDPP20_ERROR_BUSY)
			break;

		dbg_hid("%s:got busy hidpp error %02X, retrying\n", __func__, ret);
	} while (--max_retries);

	mutex_unlock(&hidpp->send_mutex);
	return ret;

}

/*
 * Payload convention for RS50 / G Pro HID++ SET commands.
 *
 * All of the settings SETs on these wheels use an HID++ short message
 * (report id 0x10) with a 3-byte payload. For scalar settings (range,
 * strength, damping, TRUEFORCE, brake force, FFB filter, centre
 * calibration) the first two bytes carry a big-endian u16 value and
 * the third byte is set to 0x00. Every G Hub capture on both wheels
 * across every settings sweep we have shows byte 2 as 0x00: it is
 * padding to the minimum short-message payload length, not a
 * semantically meaningful "reserved" byte the device inspects. Do not
 * rely on any specific value except 0x00.
 *
 * A handful of exceptions carry a real value in byte 2:
 *   - FFB filter SET writes auto / explicit flags in byte 0 and the
 *     filter level in byte 2 (see hidpp_dd_ff_write_filter).
 *   - LIGHTSYNC SETs pack type / direction / commit markers into the
 *     tail of the short message (see hidpp_dd_lightsync_apply_slot).
 * Those sites assign byte 2 explicitly and document the value inline.
 *
 * Canonical HID++ error handler for the RS50 / G Pro settings paths.
 *
 * hidpp_send_fap_command_sync() (and its to_device variant) signal three
 * states via one int: 0 success, ret < 0 transport error (e.g. -ETIMEDOUT
 * or -EPIPE from the URB layer), ret > 0 the HID++ error byte the device
 * returned. Callers need to log the right message and translate to an
 * errno. This helper does both: pass the raw ret and a short verb-phrase
 * ("set range", "set LED brightness", "apply LIGHTSYNC slot 3"), get back
 * 0 on success, a negative errno on failure. Positive rets become -EIO.
 */
static int hidpp_errno(struct hid_device *hid, int ret, const char *op)
{
	if (ret == 0)
		return 0;
	if (ret > 0) {
		dd_err(hid, "HID++ error 0x%02x on %s\n", ret, op);
		return -EIO;
	}
	dd_err(hid, "Failed to %s: %d\n", op, ret);
	return ret;
}

/*
 * hidpp_send_fap_command_sync() returns 0 in case of success, and something else
 * in case of a failure.
 *
 * See __do_hidpp_send_message_sync() for a detailed explanation of the returned
 * value.
 */
static int hidpp_send_fap_command_sync(struct hidpp_device *hidpp,
	u8 feat_index, u8 funcindex_clientid, u8 *params, int param_count,
	struct hidpp_report *response)
{
	struct hidpp_report *message;
	int ret;

	if (param_count > sizeof(message->fap.params)) {
		hid_dbg(hidpp->hid_dev,
			"Invalid number of parameters passed to command (%d != %llu)\n",
			param_count,
			(unsigned long long) sizeof(message->fap.params));
		return -EINVAL;
	}

	message = kzalloc(sizeof(struct hidpp_report), GFP_KERNEL);
	if (!message)
		return -ENOMEM;

	/*
	 * Direct-drive wheels require SHORT reports (0x10) for HID++ commands.
	 * Unlike most FAP devices that use LONG (0x11), they ignore LONG
	 * reports and only responds to SHORT. It always responds with VERY_LONG
	 * (0x12) regardless of input report type. Use SHORT when possible.
	 */
	if ((hidpp->quirks & HIDPP_QUIRK_DD_FFB) &&
	    param_count <= (HIDPP_REPORT_SHORT_LENGTH - 4))
		message->report_id = REPORT_ID_HIDPP_SHORT;
	else if (param_count > (HIDPP_REPORT_LONG_LENGTH - 4))
		message->report_id = REPORT_ID_HIDPP_VERY_LONG;
	else
		message->report_id = REPORT_ID_HIDPP_LONG;
	message->fap.feature_index = feat_index;
	message->fap.funcindex_clientid = funcindex_clientid | LINUX_KERNEL_SW_ID;
	memcpy(&message->fap.params, params, param_count);

	ret = hidpp_send_message_sync(hidpp, message, response);
	kfree(message);
	return ret;
}

/*
 * Same as hidpp_send_fap_command_sync() but addresses a specific sub-device
 * index rather than the root (0xff). Used for features that only live on a
 * sub-device (e.g. G Pro centre calibration on sub-device 0x05).
 */
static int hidpp_send_fap_to_device_sync(struct hidpp_device *hidpp,
	u8 device_index, u8 feat_index, u8 funcindex_clientid,
	u8 *params, int param_count, struct hidpp_report *response)
{
	struct hidpp_report *message;
	int ret;

	if (param_count > sizeof(message->fap.params))
		return -EINVAL;

	message = kzalloc(sizeof(struct hidpp_report), GFP_KERNEL);
	if (!message)
		return -ENOMEM;

	/*
	 * Only the direct-drive wheels currently use sub-device-addressed FAPs,
	 * and they require SHORT reports for small-param sends. The middle
	 * LONG case covers payloads that outgrow SHORT's 3 bytes but fit LONG
	 * (e.g. the 13-byte fn4 response-curve chunks the pedal-unit uploader
	 * sends to sub-device 0x02); VERY_LONG covers anything larger. All
	 * three thresholds mirror the SHORT-first path
	 * hidpp_send_fap_command_sync takes for the same quirk, so a given
	 * param_count picks the same report type through either function.
	 */
	if (param_count > (HIDPP_REPORT_LONG_LENGTH - 4))
		message->report_id = REPORT_ID_HIDPP_VERY_LONG;
	else if (param_count > (HIDPP_REPORT_SHORT_LENGTH - 4))
		message->report_id = REPORT_ID_HIDPP_LONG;
	else
		message->report_id = REPORT_ID_HIDPP_SHORT;
	message->device_index = device_index;
	message->fap.feature_index = feat_index;
	message->fap.funcindex_clientid = funcindex_clientid | LINUX_KERNEL_SW_ID;
	memcpy(&message->fap.params, params, param_count);

	ret = hidpp_send_message_sync(hidpp, message, response);
	kfree(message);
	return ret;
}

/*
 * hidpp_send_rap_command_sync() returns 0 in case of success, and something else
 * in case of a failure.
 *
 * See __do_hidpp_send_message_sync() for a detailed explanation of the returned
 * value.
 */
static int hidpp_send_rap_command_sync(struct hidpp_device *hidpp_dev,
	u8 report_id, u8 sub_id, u8 reg_address, u8 *params, int param_count,
	struct hidpp_report *response)
{
	struct hidpp_report *message;
	int ret, max_count;

	/* Send as long report if short reports are not supported. */
	if (report_id == REPORT_ID_HIDPP_SHORT &&
	    !(hidpp_dev->supported_reports & HIDPP_REPORT_SHORT_SUPPORTED))
		report_id = REPORT_ID_HIDPP_LONG;

	switch (report_id) {
	case REPORT_ID_HIDPP_SHORT:
		max_count = HIDPP_REPORT_SHORT_LENGTH - 4;
		break;
	case REPORT_ID_HIDPP_LONG:
		max_count = HIDPP_REPORT_LONG_LENGTH - 4;
		break;
	case REPORT_ID_HIDPP_VERY_LONG:
		max_count = hidpp_dev->very_long_report_length - 4;
		break;
	default:
		return -EINVAL;
	}

	if (param_count > max_count)
		return -EINVAL;

	message = kzalloc(sizeof(struct hidpp_report), GFP_KERNEL);
	if (!message)
		return -ENOMEM;
	message->report_id = report_id;
	message->rap.sub_id = sub_id;
	message->rap.reg_address = reg_address;
	memcpy(&message->rap.params, params, param_count);

	ret = hidpp_send_message_sync(hidpp_dev, message, response);
	kfree(message);
	return ret;
}

static inline bool hidpp_match_answer(struct hidpp_device *hidpp,
		struct hidpp_report *question, struct hidpp_report *answer)
{
	/*
	 * On the direct-drive wheels, answers always echo the device index
	 * of the question, and without this check a question addressed to
	 * the base device (0xff) can be "answered" by a late or unsolicited
	 * report from a sub-device (RS50: 0x01 display / 0x02 pedal base /
	 * 0x05 motor) that happens to share the feature index and function
	 * nibble.
	 *
	 * The check MUST stay gated on the DD quirk: for devices paired
	 * through a Unifying/Lightspeed receiver, hid-logitech-dj's
	 * ll_raw_request rewrites wire byte 1 to the paired slot index
	 * (1..7) AFTER our question snapshot was taken, so the answer
	 * comes back with a device index the snapshot (0xff) can never
	 * match - an unconditional check makes every sync command on
	 * every receiver-paired mouse/keyboard eat the full timeout.
	 * Upstream ignores the index entirely; we only need it where the
	 * sub-device collision exists, and the DD wheels are always
	 * direct USB where the wire index is exactly what we sent.
	 */
	if ((hidpp->quirks & HIDPP_QUIRK_DD_FFB) &&
	    answer->device_index != question->device_index)
		return false;

	/*
	 * The direct-drive wheels don't echo back the software ID - they return
	 * only the function index in the upper nibble, leaving the lower nibble
	 * 0 - so for them we compare the function nibble only when the answer's
	 * SW_ID is 0.
	 *
	 * Gate this leniency on the DD quirk, exactly like the device-index
	 * check above. Only the DD wheels zero the SW_ID; for any other
	 * Logitech device an unsolicited SW_ID-0 event/broadcast that happens
	 * to share a pending question's feature index and function nibble would
	 * otherwise be mistaken for its answer. Others keep upstream-strict
	 * matching (full funcindex_clientid compare below).
	 */
	if ((hidpp->quirks & HIDPP_QUIRK_DD_FFB) &&
	    (answer->fap.funcindex_clientid & 0x0f) == 0) {
		/* Device didn't echo SW_ID - compare function ID only */
		return (answer->fap.feature_index == question->fap.feature_index) &&
		       ((answer->fap.funcindex_clientid & 0xf0) ==
			(question->fap.funcindex_clientid & 0xf0));
	}

	return (answer->fap.feature_index == question->fap.feature_index) &&
	   (answer->fap.funcindex_clientid == question->fap.funcindex_clientid);
}

static inline bool hidpp_match_error(struct hidpp_device *hidpp,
		struct hidpp_report *question, struct hidpp_report *answer)
{
	/* Same DD-only device-index gate as hidpp_match_answer() above. */
	if ((hidpp->quirks & HIDPP_QUIRK_DD_FFB) &&
	    answer->device_index != question->device_index)
		return false;

	return ((answer->rap.sub_id == HIDPP_ERROR) ||
	    (answer->fap.feature_index == HIDPP20_ERROR)) &&
	    (answer->fap.funcindex_clientid == question->fap.feature_index) &&
	    (answer->fap.params[0] == question->fap.funcindex_clientid);
}

static inline bool hidpp_report_is_connect_event(struct hidpp_device *hidpp,
		struct hidpp_report *report)
{
	return (hidpp->wireless_feature_index &&
		(report->fap.feature_index == hidpp->wireless_feature_index)) ||
		((report->report_id == REPORT_ID_HIDPP_SHORT) &&
		(report->rap.sub_id == 0x41));
}

/*
 * hidpp_prefix_name() prefixes the current given name with "Logitech ".
 */
static void hidpp_prefix_name(char **name, int name_length)
{
#define PREFIX_LENGTH 9 /* "Logitech " */

	int new_length;
	char *new_name;

	if (name_length > PREFIX_LENGTH &&
	    strncmp(*name, "Logitech ", PREFIX_LENGTH) == 0)
		/* The prefix has is already in the name */
		return;

	new_length = PREFIX_LENGTH + name_length;
	new_name = kzalloc(new_length, GFP_KERNEL);
	if (!new_name)
		return;

	snprintf(new_name, new_length, "Logitech %s", *name);

	kfree(*name);

	*name = new_name;
}

/*
 * Updates the USB wireless_status based on whether the headset
 * is turned on and reachable.
 */
static void hidpp_update_usb_wireless_status(struct hidpp_device *hidpp)
{
	struct hid_device *hdev = hidpp->hid_dev;
	struct usb_interface *intf;

	if (!(hidpp->quirks & HIDPP_QUIRK_WIRELESS_STATUS))
		return;
	if (!hid_is_usb(hdev))
		return;

	intf = to_usb_interface(hdev->dev.parent);
	usb_set_wireless_status(intf, hidpp->battery.online ?
				USB_WIRELESS_STATUS_CONNECTED :
				USB_WIRELESS_STATUS_DISCONNECTED);
}

/**
 * hidpp_scroll_counter_handle_scroll() - Send high- and low-resolution scroll
 *                                        events given a high-resolution wheel
 *                                        movement.
 * @input_dev: Pointer to the input device
 * @counter: a hid_scroll_counter struct describing the wheel.
 * @hi_res_value: the movement of the wheel, in the mouse's high-resolution
 *                units.
 *
 * Given a high-resolution movement, this function converts the movement into
 * fractions of 120 and emits high-resolution scroll events for the input
 * device. It also uses the multiplier from &struct hid_scroll_counter to
 * emit low-resolution scroll events when appropriate for
 * backwards-compatibility with userspace input libraries.
 */
static void hidpp_scroll_counter_handle_scroll(struct input_dev *input_dev,
					       struct hidpp_scroll_counter *counter,
					       int hi_res_value)
{
	int low_res_value, remainder, direction;
	unsigned long long now, previous;

	hi_res_value = hi_res_value * 120/counter->wheel_multiplier;
	input_report_rel(input_dev, REL_WHEEL_HI_RES, hi_res_value);

	remainder = counter->remainder;
	direction = hi_res_value > 0 ? 1 : -1;

	now = sched_clock();
	previous = counter->last_time;
	counter->last_time = now;
	/*
	 * Reset the remainder after a period of inactivity or when the
	 * direction changes. This prevents the REL_WHEEL emulation point
	 * from sliding for devices that don't always provide the same
	 * number of movements per detent.
	 */
	if (now - previous > 1000000000 || direction != counter->direction)
		remainder = 0;

	counter->direction = direction;
	remainder += hi_res_value;

	/* Some wheels will rest 7/8ths of a detent from the previous detent
	 * after slow movement, so we want the threshold for low-res events to
	 * be in the middle between two detents (e.g. after 4/8ths) as
	 * opposed to on the detents themselves (8/8ths).
	 */
	if (abs(remainder) >= 60) {
		/* Add (or subtract) 1 because we want to trigger when the wheel
		 * is half-way to the next detent (i.e. scroll 1 detent after a
		 * 1/2 detent movement, 2 detents after a 1 1/2 detent movement,
		 * etc.).
		 */
		low_res_value = remainder / 120;
		if (low_res_value == 0)
			low_res_value = (hi_res_value > 0 ? 1 : -1);
		input_report_rel(input_dev, REL_WHEEL, low_res_value);
		remainder -= low_res_value * 120;
	}
	counter->remainder = remainder;
}

/* -------------------------------------------------------------------------- */
/* HIDP++ 1.0 commands                                                        */
/* -------------------------------------------------------------------------- */

#define HIDPP_SET_REGISTER				0x80
#define HIDPP_GET_REGISTER				0x81
#define HIDPP_SET_LONG_REGISTER				0x82
#define HIDPP_GET_LONG_REGISTER				0x83

/**
 * hidpp10_set_register - Modify a HID++ 1.0 register.
 * @hidpp_dev: the device to set the register on.
 * @register_address: the address of the register to modify.
 * @byte: the byte of the register to modify. Should be less than 3.
 * @mask: mask of the bits to modify
 * @value: new values for the bits in mask
 * Return: 0 if successful, otherwise a negative error code.
 */
static int hidpp10_set_register(struct hidpp_device *hidpp_dev,
	u8 register_address, u8 byte, u8 mask, u8 value)
{
	struct hidpp_report response;
	int ret;
	u8 params[3] = { 0 };

	ret = hidpp_send_rap_command_sync(hidpp_dev,
					  REPORT_ID_HIDPP_SHORT,
					  HIDPP_GET_REGISTER,
					  register_address,
					  NULL, 0, &response);
	if (ret)
		return ret;

	memcpy(params, response.rap.params, 3);

	params[byte] &= ~mask;
	params[byte] |= value & mask;

	return hidpp_send_rap_command_sync(hidpp_dev,
					   REPORT_ID_HIDPP_SHORT,
					   HIDPP_SET_REGISTER,
					   register_address,
					   params, 3, &response);
}

#define HIDPP_REG_ENABLE_REPORTS			0x00
#define HIDPP_ENABLE_CONSUMER_REPORT			BIT(0)
#define HIDPP_ENABLE_WHEEL_REPORT			BIT(2)
#define HIDPP_ENABLE_MOUSE_EXTRA_BTN_REPORT		BIT(3)
#define HIDPP_ENABLE_BAT_REPORT				BIT(4)
#define HIDPP_ENABLE_HWHEEL_REPORT			BIT(5)

static int hidpp10_enable_battery_reporting(struct hidpp_device *hidpp_dev)
{
	return hidpp10_set_register(hidpp_dev, HIDPP_REG_ENABLE_REPORTS, 0,
			  HIDPP_ENABLE_BAT_REPORT, HIDPP_ENABLE_BAT_REPORT);
}

#define HIDPP_REG_FEATURES				0x01
#define HIDPP_ENABLE_SPECIAL_BUTTON_FUNC		BIT(1)
#define HIDPP_ENABLE_FAST_SCROLL			BIT(6)

/* On HID++ 1.0 devices, high-res scroll was called "scrolling acceleration". */
static int hidpp10_enable_scrolling_acceleration(struct hidpp_device *hidpp_dev)
{
	return hidpp10_set_register(hidpp_dev, HIDPP_REG_FEATURES, 0,
			  HIDPP_ENABLE_FAST_SCROLL, HIDPP_ENABLE_FAST_SCROLL);
}

#define HIDPP_REG_BATTERY_STATUS			0x07

static int hidpp10_battery_status_map_level(u8 param)
{
	int level;

	switch (param) {
	case 1 ... 2:
		level = POWER_SUPPLY_CAPACITY_LEVEL_CRITICAL;
		break;
	case 3 ... 4:
		level = POWER_SUPPLY_CAPACITY_LEVEL_LOW;
		break;
	case 5 ... 6:
		level = POWER_SUPPLY_CAPACITY_LEVEL_NORMAL;
		break;
	case 7:
		level = POWER_SUPPLY_CAPACITY_LEVEL_HIGH;
		break;
	default:
		level = POWER_SUPPLY_CAPACITY_LEVEL_UNKNOWN;
	}

	return level;
}

static int hidpp10_battery_status_map_status(u8 param)
{
	int status;

	switch (param) {
	case 0x00:
		/* discharging (in use) */
		status = POWER_SUPPLY_STATUS_DISCHARGING;
		break;
	case 0x21: /* (standard) charging */
	case 0x24: /* fast charging */
	case 0x25: /* slow charging */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 0x26: /* topping charge */
	case 0x22: /* charge complete */
		status = POWER_SUPPLY_STATUS_FULL;
		break;
	case 0x20: /* unknown */
		status = POWER_SUPPLY_STATUS_UNKNOWN;
		break;
	/*
	 * 0x01...0x1F = reserved (not charging)
	 * 0x23 = charging error
	 * 0x27..0xff = reserved
	 */
	default:
		status = POWER_SUPPLY_STATUS_NOT_CHARGING;
		break;
	}

	return status;
}

static int hidpp10_query_battery_status(struct hidpp_device *hidpp)
{
	struct hidpp_report response;
	int ret, status;

	ret = hidpp_send_rap_command_sync(hidpp,
					REPORT_ID_HIDPP_SHORT,
					HIDPP_GET_REGISTER,
					HIDPP_REG_BATTERY_STATUS,
					NULL, 0, &response);
	if (ret)
		return ret;

	hidpp->battery.level =
		hidpp10_battery_status_map_level(response.rap.params[0]);
	status = hidpp10_battery_status_map_status(response.rap.params[1]);
	hidpp->battery.status = status;
	/* the capacity is only available when discharging or full */
	hidpp->battery.online = status == POWER_SUPPLY_STATUS_DISCHARGING ||
				status == POWER_SUPPLY_STATUS_FULL;

	return 0;
}

#define HIDPP_REG_BATTERY_MILEAGE			0x0D

static int hidpp10_battery_mileage_map_status(u8 param)
{
	int status;

	switch (param >> 6) {
	case 0x00:
		/* discharging (in use) */
		status = POWER_SUPPLY_STATUS_DISCHARGING;
		break;
	case 0x01: /* charging */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 0x02: /* charge complete */
		status = POWER_SUPPLY_STATUS_FULL;
		break;
	/*
	 * 0x03 = charging error
	 */
	default:
		status = POWER_SUPPLY_STATUS_NOT_CHARGING;
		break;
	}

	return status;
}

static int hidpp10_query_battery_mileage(struct hidpp_device *hidpp)
{
	struct hidpp_report response;
	int ret, status;

	ret = hidpp_send_rap_command_sync(hidpp,
					REPORT_ID_HIDPP_SHORT,
					HIDPP_GET_REGISTER,
					HIDPP_REG_BATTERY_MILEAGE,
					NULL, 0, &response);
	if (ret)
		return ret;

	hidpp->battery.capacity = response.rap.params[0];
	status = hidpp10_battery_mileage_map_status(response.rap.params[2]);
	hidpp->battery.status = status;
	/* the capacity is only available when discharging or full */
	hidpp->battery.online = status == POWER_SUPPLY_STATUS_DISCHARGING ||
				status == POWER_SUPPLY_STATUS_FULL;

	return 0;
}

static int hidpp10_battery_event(struct hidpp_device *hidpp, u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	int status, capacity, level;
	bool changed;

	if (report->report_id != REPORT_ID_HIDPP_SHORT)
		return 0;

	switch (report->rap.sub_id) {
	case HIDPP_REG_BATTERY_STATUS:
		capacity = hidpp->battery.capacity;
		level = hidpp10_battery_status_map_level(report->rawbytes[1]);
		status = hidpp10_battery_status_map_status(report->rawbytes[2]);
		break;
	case HIDPP_REG_BATTERY_MILEAGE:
		capacity = report->rap.params[0];
		level = hidpp->battery.level;
		status = hidpp10_battery_mileage_map_status(report->rawbytes[3]);
		break;
	default:
		return 0;
	}

	changed = capacity != hidpp->battery.capacity ||
		  level != hidpp->battery.level ||
		  status != hidpp->battery.status;

	/* the capacity is only available when discharging or full */
	hidpp->battery.online = status == POWER_SUPPLY_STATUS_DISCHARGING ||
				status == POWER_SUPPLY_STATUS_FULL;

	if (changed) {
		hidpp->battery.level = level;
		hidpp->battery.status = status;
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
	}

	return 0;
}

#define HIDPP_REG_PAIRING_INFORMATION			0xB5
#define HIDPP_EXTENDED_PAIRING				0x30
#define HIDPP_DEVICE_NAME				0x40

static char *hidpp_unifying_get_name(struct hidpp_device *hidpp_dev)
{
	struct hidpp_report response;
	int ret;
	u8 params[1] = { HIDPP_DEVICE_NAME };
	char *name;
	int len;

	ret = hidpp_send_rap_command_sync(hidpp_dev,
					REPORT_ID_HIDPP_SHORT,
					HIDPP_GET_LONG_REGISTER,
					HIDPP_REG_PAIRING_INFORMATION,
					params, 1, &response);
	if (ret)
		return NULL;

	len = response.rap.params[1];

	if (2 + len > sizeof(response.rap.params))
		return NULL;

	if (len < 4) /* logitech devices are usually at least Xddd */
		return NULL;

	name = kzalloc(len + 1, GFP_KERNEL);
	if (!name)
		return NULL;

	memcpy(name, &response.rap.params[2], len);

	/* include the terminating '\0' */
	hidpp_prefix_name(&name, len + 1);

	return name;
}

static int hidpp_unifying_get_serial(struct hidpp_device *hidpp, u32 *serial)
{
	struct hidpp_report response;
	int ret;
	u8 params[1] = { HIDPP_EXTENDED_PAIRING };

	ret = hidpp_send_rap_command_sync(hidpp,
					REPORT_ID_HIDPP_SHORT,
					HIDPP_GET_LONG_REGISTER,
					HIDPP_REG_PAIRING_INFORMATION,
					params, 1, &response);
	if (ret)
		return ret;

	/*
	 * We don't care about LE or BE, we will output it as a string
	 * with %4phD, so we need to keep the order.
	 */
	*serial = *((u32 *)&response.rap.params[1]);
	return 0;
}

static int hidpp_unifying_init(struct hidpp_device *hidpp)
{
	struct hid_device *hdev = hidpp->hid_dev;
	const char *name;
	u32 serial;
	int ret;

	ret = hidpp_unifying_get_serial(hidpp, &serial);
	if (ret)
		return ret;

	snprintf(hdev->uniq, sizeof(hdev->uniq), "%4phD", &serial);
	dbg_hid("HID++ Unifying: Got serial: %s\n", hdev->uniq);

	name = hidpp_unifying_get_name(hidpp);
	if (!name)
		return -EIO;

	snprintf(hdev->name, sizeof(hdev->name), "%s", name);
	dbg_hid("HID++ Unifying: Got name: %s\n", name);

	kfree(name);
	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x0000: Root                                                               */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_ROOT					0x0000
#define HIDPP_PAGE_ROOT_IDX				0x00

#define CMD_ROOT_GET_FEATURE				0x00
#define CMD_ROOT_GET_PROTOCOL_VERSION			0x10

static int hidpp_root_get_feature(struct hidpp_device *hidpp, u16 feature,
	u8 *feature_index)
{
	struct hidpp_report response;
	int ret;
	u8 params[2] = { feature >> 8, feature & 0x00FF };

	ret = hidpp_send_fap_command_sync(hidpp,
			HIDPP_PAGE_ROOT_IDX,
			CMD_ROOT_GET_FEATURE,
			params, 2, &response);
	if (ret)
		return ret;

	if (response.fap.params[0] == 0)
		return -ENOENT;

	*feature_index = response.fap.params[0];

	return ret;
}

/*
 * Discover a feature page's index on a specific sub-device. Analogous to
 * hidpp_root_get_feature() but sends the ROOT GetFeature query to a
 * sub-device address instead of 0xff. Needed for features that only
 * respond on an auxiliary HID++ device (e.g. the G Pro's centre
 * calibration engine lives on sub-device 0x05).
 */
static int hidpp_root_get_feature_on_device(struct hidpp_device *hidpp,
	u8 device_index, u16 feature, u8 *feature_index)
{
	struct hidpp_report response;
	int ret;
	u8 params[2] = { feature >> 8, feature & 0x00FF };

	ret = hidpp_send_fap_to_device_sync(hidpp, device_index,
			HIDPP_PAGE_ROOT_IDX, CMD_ROOT_GET_FEATURE,
			params, 2, &response);
	if (ret)
		return ret;

	if (response.fap.params[0] == 0)
		return -ENOENT;

	*feature_index = response.fap.params[0];
	return 0;
}

static int hidpp_root_get_protocol_version(struct hidpp_device *hidpp)
{
	const u8 ping_byte = 0x5a;
	u8 ping_data[3] = { 0, 0, ping_byte };
	struct hidpp_report response;
	int ret;

	ret = hidpp_send_rap_command_sync(hidpp,
			REPORT_ID_HIDPP_SHORT,
			HIDPP_PAGE_ROOT_IDX,
			CMD_ROOT_GET_PROTOCOL_VERSION | LINUX_KERNEL_SW_ID,
			ping_data, sizeof(ping_data), &response);

	if (ret == HIDPP_ERROR_INVALID_SUBID) {
		hidpp->protocol_major = 1;
		hidpp->protocol_minor = 0;
		goto print_version;
	}

	/* the device might not be connected */
	if (ret == HIDPP_ERROR_RESOURCE_ERROR ||
	    ret == HIDPP_ERROR_UNKNOWN_DEVICE)
		return -EIO;

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	if (response.rap.params[2] != ping_byte) {
		hid_err(hidpp->hid_dev, "%s: ping mismatch 0x%02x != 0x%02x\n",
			__func__, response.rap.params[2], ping_byte);
		return -EPROTO;
	}

	hidpp->protocol_major = response.rap.params[0];
	hidpp->protocol_minor = response.rap.params[1];

print_version:
	if (!hidpp->connected_once) {
		hid_info(hidpp->hid_dev, "HID++ %u.%u device connected.\n",
			 hidpp->protocol_major, hidpp->protocol_minor);
		hidpp->connected_once = true;
	} else
		hid_dbg(hidpp->hid_dev, "HID++ %u.%u device connected.\n",
			 hidpp->protocol_major, hidpp->protocol_minor);
	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x0003: Device Information                                                 */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_DEVICE_INFORMATION			0x0003

#define CMD_GET_DEVICE_INFO				0x00

static int hidpp_get_serial(struct hidpp_device *hidpp, u32 *serial)
{
	struct hidpp_report response;
	u8 feature_index;
	int ret;

	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_DEVICE_INFORMATION,
				     &feature_index);
	if (ret)
		return ret;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_GET_DEVICE_INFO,
					  NULL, 0, &response);
	if (ret)
		return ret;

	/* See hidpp_unifying_get_serial() */
	*serial = *((u32 *)&response.rap.params[1]);
	return 0;
}

static int hidpp_serial_init(struct hidpp_device *hidpp)
{
	struct hid_device *hdev = hidpp->hid_dev;
	u32 serial;
	int ret;

	ret = hidpp_get_serial(hidpp, &serial);
	if (ret)
		return ret;

	snprintf(hdev->uniq, sizeof(hdev->uniq), "%4phD", &serial);
	dbg_hid("HID++ DeviceInformation: Got serial: %s\n", hdev->uniq);

	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x0005: GetDeviceNameType                                                  */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_GET_DEVICE_NAME_TYPE			0x0005

#define CMD_GET_DEVICE_NAME_TYPE_GET_COUNT		0x00
#define CMD_GET_DEVICE_NAME_TYPE_GET_DEVICE_NAME	0x10
#define CMD_GET_DEVICE_NAME_TYPE_GET_TYPE		0x20

static int hidpp_devicenametype_get_count(struct hidpp_device *hidpp,
	u8 feature_index, u8 *nameLength)
{
	struct hidpp_report response;
	int ret;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
		CMD_GET_DEVICE_NAME_TYPE_GET_COUNT, NULL, 0, &response);

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	*nameLength = response.fap.params[0];

	return ret;
}

static int hidpp_devicenametype_get_device_name(struct hidpp_device *hidpp,
	u8 feature_index, u8 char_index, char *device_name, int len_buf)
{
	struct hidpp_report response;
	int ret, i;
	int count;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
		CMD_GET_DEVICE_NAME_TYPE_GET_DEVICE_NAME, &char_index, 1,
		&response);

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	switch (response.report_id) {
	case REPORT_ID_HIDPP_VERY_LONG:
		count = hidpp->very_long_report_length - 4;
		break;
	case REPORT_ID_HIDPP_LONG:
		count = HIDPP_REPORT_LONG_LENGTH - 4;
		break;
	case REPORT_ID_HIDPP_SHORT:
		count = HIDPP_REPORT_SHORT_LENGTH - 4;
		break;
	default:
		return -EPROTO;
	}

	if (len_buf < count)
		count = len_buf;

	for (i = 0; i < count; i++)
		device_name[i] = response.fap.params[i];

	return count;
}

static char *hidpp_get_device_name(struct hidpp_device *hidpp)
{
	u8 feature_index;
	u8 __name_length;
	char *name;
	unsigned index = 0;
	int ret;

	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_GET_DEVICE_NAME_TYPE,
		&feature_index);
	if (ret)
		return NULL;

	ret = hidpp_devicenametype_get_count(hidpp, feature_index,
		&__name_length);
	if (ret)
		return NULL;

	name = kzalloc(__name_length + 1, GFP_KERNEL);
	if (!name)
		return NULL;

	while (index < __name_length) {
		ret = hidpp_devicenametype_get_device_name(hidpp,
			feature_index, index, name + index,
			__name_length - index);
		if (ret <= 0) {
			kfree(name);
			return NULL;
		}
		index += ret;
	}

	/* include the terminating '\0' */
	hidpp_prefix_name(&name, __name_length + 1);

	return name;
}

/* -------------------------------------------------------------------------- */
/* 0x1000: Battery level status                                               */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_BATTERY_LEVEL_STATUS				0x1000

#define CMD_BATTERY_LEVEL_STATUS_GET_BATTERY_LEVEL_STATUS	0x00
#define CMD_BATTERY_LEVEL_STATUS_GET_BATTERY_CAPABILITY		0x10

#define EVENT_BATTERY_LEVEL_STATUS_BROADCAST			0x00

#define FLAG_BATTERY_LEVEL_DISABLE_OSD				BIT(0)
#define FLAG_BATTERY_LEVEL_MILEAGE				BIT(1)
#define FLAG_BATTERY_LEVEL_RECHARGEABLE				BIT(2)

static int hidpp_map_battery_level(int capacity)
{
	if (capacity < 11)
		return POWER_SUPPLY_CAPACITY_LEVEL_CRITICAL;
	/*
	 * The spec says this should be < 31 but some devices report 30
	 * with brand new batteries and Windows reports 30 as "Good".
	 */
	else if (capacity < 30)
		return POWER_SUPPLY_CAPACITY_LEVEL_LOW;
	else if (capacity < 81)
		return POWER_SUPPLY_CAPACITY_LEVEL_NORMAL;
	return POWER_SUPPLY_CAPACITY_LEVEL_FULL;
}

static int hidpp20_batterylevel_map_status_capacity(u8 data[3], int *capacity,
						    int *next_capacity,
						    int *level)
{
	int status;

	*capacity = data[0];
	*next_capacity = data[1];
	*level = POWER_SUPPLY_CAPACITY_LEVEL_UNKNOWN;

	/* When discharging, we can rely on the device reported capacity.
	 * For all other states the device reports 0 (unknown).
	 */
	switch (data[2]) {
	case 0: /* discharging (in use) */
		status = POWER_SUPPLY_STATUS_DISCHARGING;
		*level = hidpp_map_battery_level(*capacity);
		break;
	case 1: /* recharging */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 2: /* charge in final stage */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 3: /* charge complete */
		status = POWER_SUPPLY_STATUS_FULL;
		*level = POWER_SUPPLY_CAPACITY_LEVEL_FULL;
		*capacity = 100;
		break;
	case 4: /* recharging below optimal speed */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	/*
	 * 5 = invalid battery type
	 * 6 = thermal error
	 * 7 = other charging error
	 */
	default:
		status = POWER_SUPPLY_STATUS_NOT_CHARGING;
		break;
	}

	return status;
}

static int hidpp20_batterylevel_get_battery_capacity(struct hidpp_device *hidpp,
						     u8 feature_index,
						     int *status,
						     int *capacity,
						     int *next_capacity,
						     int *level)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_BATTERY_LEVEL_STATUS_GET_BATTERY_LEVEL_STATUS,
					  NULL, 0, &response);
	/* Ignore these intermittent errors */
	if (ret == HIDPP_ERROR_RESOURCE_ERROR)
		return -EIO;
	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	*status = hidpp20_batterylevel_map_status_capacity(params, capacity,
							   next_capacity,
							   level);

	return 0;
}

static int hidpp20_batterylevel_get_battery_info(struct hidpp_device *hidpp,
						  u8 feature_index)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;
	unsigned int level_count, flags;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_BATTERY_LEVEL_STATUS_GET_BATTERY_CAPABILITY,
					  NULL, 0, &response);
	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	level_count = params[0];
	flags = params[1];

	if (level_count < 10 || !(flags & FLAG_BATTERY_LEVEL_MILEAGE))
		hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS;
	else
		hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_MILEAGE;

	return 0;
}

static int hidpp20_query_battery_info_1000(struct hidpp_device *hidpp)
{
	int ret;
	int status, capacity, next_capacity, level;

	if (hidpp->battery.feature_index == 0xff) {
		ret = hidpp_root_get_feature(hidpp,
					     HIDPP_PAGE_BATTERY_LEVEL_STATUS,
					     &hidpp->battery.feature_index);
		if (ret)
			return ret;
	}

	ret = hidpp20_batterylevel_get_battery_capacity(hidpp,
						hidpp->battery.feature_index,
						&status, &capacity,
						&next_capacity, &level);
	if (ret)
		return ret;

	ret = hidpp20_batterylevel_get_battery_info(hidpp,
						hidpp->battery.feature_index);
	if (ret)
		return ret;

	hidpp->battery.status = status;
	hidpp->battery.capacity = capacity;
	hidpp->battery.level = level;
	/* the capacity is only available when discharging or full */
	hidpp->battery.online = status == POWER_SUPPLY_STATUS_DISCHARGING ||
				status == POWER_SUPPLY_STATUS_FULL;

	return 0;
}

static int hidpp20_battery_event_1000(struct hidpp_device *hidpp,
				 u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	int status, capacity, next_capacity, level;
	bool changed;

	if (report->fap.feature_index != hidpp->battery.feature_index ||
	    report->fap.funcindex_clientid != EVENT_BATTERY_LEVEL_STATUS_BROADCAST)
		return 0;

	status = hidpp20_batterylevel_map_status_capacity(report->fap.params,
							  &capacity,
							  &next_capacity,
							  &level);

	/* the capacity is only available when discharging or full */
	hidpp->battery.online = status == POWER_SUPPLY_STATUS_DISCHARGING ||
				status == POWER_SUPPLY_STATUS_FULL;

	changed = capacity != hidpp->battery.capacity ||
		  level != hidpp->battery.level ||
		  status != hidpp->battery.status;

	if (changed) {
		hidpp->battery.level = level;
		hidpp->battery.capacity = capacity;
		hidpp->battery.status = status;
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
	}

	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x1001: Battery voltage                                                    */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_BATTERY_VOLTAGE 0x1001

#define CMD_BATTERY_VOLTAGE_GET_BATTERY_VOLTAGE 0x00

#define EVENT_BATTERY_VOLTAGE_STATUS_BROADCAST 0x00

static int hidpp20_battery_map_status_voltage(u8 data[3], int *voltage,
						int *level, int *charge_type)
{
	int status;

	long flags = (long) data[2];
	*level = POWER_SUPPLY_CAPACITY_LEVEL_UNKNOWN;

	if (flags & 0x80)
		switch (flags & 0x07) {
		case 0:
			status = POWER_SUPPLY_STATUS_CHARGING;
			break;
		case 1:
			status = POWER_SUPPLY_STATUS_FULL;
			*level = POWER_SUPPLY_CAPACITY_LEVEL_FULL;
			break;
		case 2:
			status = POWER_SUPPLY_STATUS_NOT_CHARGING;
			break;
		default:
			status = POWER_SUPPLY_STATUS_UNKNOWN;
			break;
		}
	else
		status = POWER_SUPPLY_STATUS_DISCHARGING;

	*charge_type = POWER_SUPPLY_CHARGE_TYPE_STANDARD;
	if (test_bit(3, &flags)) {
		*charge_type = POWER_SUPPLY_CHARGE_TYPE_FAST;
	}
	if (test_bit(4, &flags)) {
		*charge_type = POWER_SUPPLY_CHARGE_TYPE_TRICKLE;
	}
	if (test_bit(5, &flags)) {
		*level = POWER_SUPPLY_CAPACITY_LEVEL_CRITICAL;
	}

	*voltage = get_unaligned_be16(data);

	return status;
}

static int hidpp20_battery_get_battery_voltage(struct hidpp_device *hidpp,
						 u8 feature_index,
						 int *status, int *voltage,
						 int *level, int *charge_type)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_BATTERY_VOLTAGE_GET_BATTERY_VOLTAGE,
					  NULL, 0, &response);

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_VOLTAGE;

	*status = hidpp20_battery_map_status_voltage(params, voltage,
						     level, charge_type);

	return 0;
}

static int hidpp20_map_battery_capacity(struct hid_device *hid_dev, int voltage)
{
	/* NB: This voltage curve doesn't necessarily map perfectly to all
	 * devices that implement the BATTERY_VOLTAGE feature. This is because
	 * there are a few devices that use different battery technology.
	 */

	static const int voltages[100] = {
		4186, 4156, 4143, 4133, 4122, 4113, 4103, 4094, 4086, 4075,
		4067, 4059, 4051, 4043, 4035, 4027, 4019, 4011, 4003, 3997,
		3989, 3983, 3976, 3969, 3961, 3955, 3949, 3942, 3935, 3929,
		3922, 3916, 3909, 3902, 3896, 3890, 3883, 3877, 3870, 3865,
		3859, 3853, 3848, 3842, 3837, 3833, 3828, 3824, 3819, 3815,
		3811, 3808, 3804, 3800, 3797, 3793, 3790, 3787, 3784, 3781,
		3778, 3775, 3772, 3770, 3767, 3764, 3762, 3759, 3757, 3754,
		3751, 3748, 3744, 3741, 3737, 3734, 3730, 3726, 3724, 3720,
		3717, 3714, 3710, 3706, 3702, 3697, 3693, 3688, 3683, 3677,
		3671, 3666, 3662, 3658, 3654, 3646, 3633, 3612, 3579, 3537
	};

	int i;

	if (unlikely(voltage < 3500 || voltage >= 5000))
		hid_warn_once(hid_dev,
			      "%s: possibly using the wrong voltage curve\n",
			      __func__);

	for (i = 0; i < ARRAY_SIZE(voltages); i++) {
		if (voltage >= voltages[i])
			return ARRAY_SIZE(voltages) - i;
	}

	return 0;
}

static int hidpp20_query_battery_voltage_info(struct hidpp_device *hidpp)
{
	int ret;
	int status, voltage, level, charge_type;

	if (hidpp->battery.voltage_feature_index == 0xff) {
		ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_BATTERY_VOLTAGE,
					     &hidpp->battery.voltage_feature_index);
		if (ret)
			return ret;
	}

	ret = hidpp20_battery_get_battery_voltage(hidpp,
						  hidpp->battery.voltage_feature_index,
						  &status, &voltage, &level, &charge_type);

	if (ret)
		return ret;

	hidpp->battery.status = status;
	hidpp->battery.voltage = voltage;
	hidpp->battery.capacity = hidpp20_map_battery_capacity(hidpp->hid_dev,
							       voltage);
	hidpp->battery.level = level;
	hidpp->battery.charge_type = charge_type;
	hidpp->battery.online = status != POWER_SUPPLY_STATUS_NOT_CHARGING;

	return 0;
}

static int hidpp20_battery_voltage_event(struct hidpp_device *hidpp,
					    u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	int status, voltage, level, charge_type;

	if (report->fap.feature_index != hidpp->battery.voltage_feature_index ||
		report->fap.funcindex_clientid != EVENT_BATTERY_VOLTAGE_STATUS_BROADCAST)
		return 0;

	status = hidpp20_battery_map_status_voltage(report->fap.params, &voltage,
						    &level, &charge_type);

	hidpp->battery.online = status != POWER_SUPPLY_STATUS_NOT_CHARGING;

	if (voltage != hidpp->battery.voltage || status != hidpp->battery.status) {
		hidpp->battery.voltage = voltage;
		hidpp->battery.capacity = hidpp20_map_battery_capacity(hidpp->hid_dev,
								       voltage);
		hidpp->battery.status = status;
		hidpp->battery.level = level;
		hidpp->battery.charge_type = charge_type;
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
	}
	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x1004: Unified battery                                                    */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_UNIFIED_BATTERY				0x1004

#define CMD_UNIFIED_BATTERY_GET_CAPABILITIES			0x00
#define CMD_UNIFIED_BATTERY_GET_STATUS				0x10

#define EVENT_UNIFIED_BATTERY_STATUS_EVENT			0x00

#define FLAG_UNIFIED_BATTERY_LEVEL_CRITICAL			BIT(0)
#define FLAG_UNIFIED_BATTERY_LEVEL_LOW				BIT(1)
#define FLAG_UNIFIED_BATTERY_LEVEL_GOOD				BIT(2)
#define FLAG_UNIFIED_BATTERY_LEVEL_FULL				BIT(3)

#define FLAG_UNIFIED_BATTERY_FLAGS_RECHARGEABLE			BIT(0)
#define FLAG_UNIFIED_BATTERY_FLAGS_STATE_OF_CHARGE		BIT(1)

static int hidpp20_unifiedbattery_get_capabilities(struct hidpp_device *hidpp,
						   u8 feature_index)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS ||
	    hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_PERCENTAGE) {
		/* we have already set the device capabilities, so let's skip */
		return 0;
	}

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_UNIFIED_BATTERY_GET_CAPABILITIES,
					  NULL, 0, &response);
	/* Ignore these intermittent errors */
	if (ret == HIDPP_ERROR_RESOURCE_ERROR)
		return -EIO;
	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	/*
	 * If the device supports state of charge (battery percentage) we won't
	 * export the battery level information. there are 4 possible battery
	 * levels and they all are optional, this means that the device might
	 * not support any of them, we are just better off with the battery
	 * percentage.
	 */
	if (params[1] & FLAG_UNIFIED_BATTERY_FLAGS_STATE_OF_CHARGE) {
		hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_PERCENTAGE;
		hidpp->battery.supported_levels_1004 = 0;
	} else {
		hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS;
		hidpp->battery.supported_levels_1004 = params[0];
	}

	return 0;
}

static int hidpp20_unifiedbattery_map_status(struct hidpp_device *hidpp,
					     u8 charging_status,
					     u8 external_power_status)
{
	int status;

	switch (charging_status) {
	case 0: /* discharging */
		status = POWER_SUPPLY_STATUS_DISCHARGING;
		break;
	case 1: /* charging */
	case 2: /* charging slow */
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 3: /* complete */
		status = POWER_SUPPLY_STATUS_FULL;
		break;
	case 4: /* error */
		status = POWER_SUPPLY_STATUS_NOT_CHARGING;
		hid_info(hidpp->hid_dev, "%s: charging error",
			 hidpp->name);
		break;
	default:
		status = POWER_SUPPLY_STATUS_NOT_CHARGING;
		break;
	}

	return status;
}

static int hidpp20_unifiedbattery_map_level(struct hidpp_device *hidpp,
					    u8 battery_level)
{
	/* cler unsupported level bits */
	battery_level &= hidpp->battery.supported_levels_1004;

	if (battery_level & FLAG_UNIFIED_BATTERY_LEVEL_FULL)
		return POWER_SUPPLY_CAPACITY_LEVEL_FULL;
	else if (battery_level & FLAG_UNIFIED_BATTERY_LEVEL_GOOD)
		return POWER_SUPPLY_CAPACITY_LEVEL_NORMAL;
	else if (battery_level & FLAG_UNIFIED_BATTERY_LEVEL_LOW)
		return POWER_SUPPLY_CAPACITY_LEVEL_LOW;
	else if (battery_level & FLAG_UNIFIED_BATTERY_LEVEL_CRITICAL)
		return POWER_SUPPLY_CAPACITY_LEVEL_CRITICAL;

	return POWER_SUPPLY_CAPACITY_LEVEL_UNKNOWN;
}

static int hidpp20_unifiedbattery_get_status(struct hidpp_device *hidpp,
					     u8 feature_index,
					     u8 *state_of_charge,
					     int *status,
					     int *level)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_UNIFIED_BATTERY_GET_STATUS,
					  NULL, 0, &response);
	/* Ignore these intermittent errors */
	if (ret == HIDPP_ERROR_RESOURCE_ERROR)
		return -EIO;
	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	*state_of_charge = params[0];
	*status = hidpp20_unifiedbattery_map_status(hidpp, params[2], params[3]);
	*level = hidpp20_unifiedbattery_map_level(hidpp, params[1]);

	return 0;
}

static int hidpp20_query_battery_info_1004(struct hidpp_device *hidpp)
{
	int ret;
	u8 state_of_charge;
	int status, level;

	if (hidpp->battery.feature_index == 0xff) {
		ret = hidpp_root_get_feature(hidpp,
					     HIDPP_PAGE_UNIFIED_BATTERY,
					     &hidpp->battery.feature_index);
		if (ret)
			return ret;
	}

	ret = hidpp20_unifiedbattery_get_capabilities(hidpp,
					hidpp->battery.feature_index);
	if (ret)
		return ret;

	ret = hidpp20_unifiedbattery_get_status(hidpp,
						hidpp->battery.feature_index,
						&state_of_charge,
						&status,
						&level);
	if (ret)
		return ret;

	hidpp->capabilities |= HIDPP_CAPABILITY_UNIFIED_BATTERY;
	hidpp->battery.capacity = state_of_charge;
	hidpp->battery.status = status;
	hidpp->battery.level = level;
	hidpp->battery.online = true;

	return 0;
}

static int hidpp20_battery_event_1004(struct hidpp_device *hidpp,
				 u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	u8 *params = (u8 *)report->fap.params;
	int state_of_charge, status, level;
	bool changed;

	if (report->fap.feature_index != hidpp->battery.feature_index ||
	    report->fap.funcindex_clientid != EVENT_UNIFIED_BATTERY_STATUS_EVENT)
		return 0;

	state_of_charge = params[0];
	status = hidpp20_unifiedbattery_map_status(hidpp, params[2], params[3]);
	level = hidpp20_unifiedbattery_map_level(hidpp, params[1]);

	changed = status != hidpp->battery.status ||
		  (state_of_charge != hidpp->battery.capacity &&
		   hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_PERCENTAGE) ||
		  (level != hidpp->battery.level &&
		   hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS);

	if (changed) {
		hidpp->battery.capacity = state_of_charge;
		hidpp->battery.status = status;
		hidpp->battery.level = level;
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
	}

	return 0;
}

/* -------------------------------------------------------------------------- */
/* Battery feature helpers                                                    */
/* -------------------------------------------------------------------------- */

static enum power_supply_property hidpp_battery_props[] = {
	POWER_SUPPLY_PROP_ONLINE,
	POWER_SUPPLY_PROP_STATUS,
	POWER_SUPPLY_PROP_SCOPE,
	POWER_SUPPLY_PROP_MODEL_NAME,
	POWER_SUPPLY_PROP_MANUFACTURER,
	POWER_SUPPLY_PROP_SERIAL_NUMBER,
	0, /* placeholder for POWER_SUPPLY_PROP_CAPACITY, */
	0, /* placeholder for POWER_SUPPLY_PROP_CAPACITY_LEVEL, */
	0, /* placeholder for POWER_SUPPLY_PROP_VOLTAGE_NOW, */
};

static int hidpp_battery_get_property(struct power_supply *psy,
				      enum power_supply_property psp,
				      union power_supply_propval *val)
{
	struct hidpp_device *hidpp = power_supply_get_drvdata(psy);
	int ret = 0;

	switch (psp) {
	case POWER_SUPPLY_PROP_STATUS:
		val->intval = hidpp->battery.status;
		break;
	case POWER_SUPPLY_PROP_CAPACITY:
		val->intval = hidpp->battery.capacity;
		break;
	case POWER_SUPPLY_PROP_CAPACITY_LEVEL:
		val->intval = hidpp->battery.level;
		break;
	case POWER_SUPPLY_PROP_SCOPE:
		val->intval = POWER_SUPPLY_SCOPE_DEVICE;
		break;
	case POWER_SUPPLY_PROP_ONLINE:
		val->intval = hidpp->battery.online;
		break;
	case POWER_SUPPLY_PROP_MODEL_NAME:
		if (!strncmp(hidpp->name, "Logitech ", 9))
			val->strval = hidpp->name + 9;
		else
			val->strval = hidpp->name;
		break;
	case POWER_SUPPLY_PROP_MANUFACTURER:
		val->strval = "Logitech";
		break;
	case POWER_SUPPLY_PROP_SERIAL_NUMBER:
		val->strval = hidpp->hid_dev->uniq;
		break;
	case POWER_SUPPLY_PROP_VOLTAGE_NOW:
		/* hardware reports voltage in mV. sysfs expects uV */
		val->intval = hidpp->battery.voltage * 1000;
		break;
	case POWER_SUPPLY_PROP_CHARGE_TYPE:
		val->intval = hidpp->battery.charge_type;
		break;
	default:
		ret = -EINVAL;
		break;
	}

	return ret;
}

/* -------------------------------------------------------------------------- */
/* 0x1d4b: Wireless device status                                             */
/* -------------------------------------------------------------------------- */
#define HIDPP_PAGE_WIRELESS_DEVICE_STATUS			0x1d4b

static int hidpp_get_wireless_feature_index(struct hidpp_device *hidpp, u8 *feature_index)
{
	return hidpp_root_get_feature(hidpp,
				      HIDPP_PAGE_WIRELESS_DEVICE_STATUS,
				      feature_index);
}

/* -------------------------------------------------------------------------- */
/* 0x1f20: ADC measurement                                                    */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_ADC_MEASUREMENT 0x1f20

#define CMD_ADC_MEASUREMENT_GET_ADC_MEASUREMENT 0x00

#define EVENT_ADC_MEASUREMENT_STATUS_BROADCAST 0x00

static int hidpp20_map_adc_measurement_1f20_capacity(struct hid_device *hid_dev, int voltage)
{
	/* NB: This voltage curve doesn't necessarily map perfectly to all
	 * devices that implement the ADC_MEASUREMENT feature. This is because
	 * there are a few devices that use different battery technology.
	 *
	 * Adapted from:
	 * https://github.com/Sapd/HeadsetControl/blob/acd972be0468e039b93aae81221f20a54d2d60f7/src/devices/logitech_g633_g933_935.c#L44-L52
	 */
	static const int voltages[100] = {
		4030, 4024, 4018, 4011, 4003, 3994, 3985, 3975, 3963, 3951,
		3937, 3922, 3907, 3893, 3880, 3868, 3857, 3846, 3837, 3828,
		3820, 3812, 3805, 3798, 3791, 3785, 3779, 3773, 3768, 3762,
		3757, 3752, 3747, 3742, 3738, 3733, 3729, 3724, 3720, 3716,
		3712, 3708, 3704, 3700, 3696, 3692, 3688, 3685, 3681, 3677,
		3674, 3670, 3667, 3663, 3660, 3657, 3653, 3650, 3646, 3643,
		3640, 3637, 3633, 3630, 3627, 3624, 3620, 3617, 3614, 3611,
		3608, 3604, 3601, 3598, 3595, 3592, 3589, 3585, 3582, 3579,
		3576, 3573, 3569, 3566, 3563, 3560, 3556, 3553, 3550, 3546,
		3543, 3539, 3536, 3532, 3529, 3525, 3499, 3466, 3433, 3399,
	};

	int i;

	if (voltage == 0)
		return 0;

	if (unlikely(voltage < 3400 || voltage >= 5000))
		hid_warn_once(hid_dev,
			      "%s: possibly using the wrong voltage curve\n",
			      __func__);

	for (i = 0; i < ARRAY_SIZE(voltages); i++) {
		if (voltage >= voltages[i])
			return ARRAY_SIZE(voltages) - i;
	}

	return 0;
}

static int hidpp20_map_adc_measurement_1f20(u8 data[3], int *voltage)
{
	int status;
	u8 flags;

	flags = data[2];

	switch (flags) {
	case 0x01:
		status = POWER_SUPPLY_STATUS_DISCHARGING;
		break;
	case 0x03:
		status = POWER_SUPPLY_STATUS_CHARGING;
		break;
	case 0x07:
		status = POWER_SUPPLY_STATUS_FULL;
		break;
	case 0x0F:
	default:
		status = POWER_SUPPLY_STATUS_UNKNOWN;
		break;
	}

	*voltage = get_unaligned_be16(data);

	dbg_hid("Parsed 1f20 data as flag 0x%02x voltage %dmV\n",
		flags, *voltage);

	return status;
}

/* Return value is whether the device is online */
static bool hidpp20_get_adc_measurement_1f20(struct hidpp_device *hidpp,
						 u8 feature_index,
						 int *status, int *voltage)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	*status = POWER_SUPPLY_STATUS_UNKNOWN;
	*voltage = 0;
	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_ADC_MEASUREMENT_GET_ADC_MEASUREMENT,
					  NULL, 0, &response);

	if (ret > 0) {
		hid_dbg(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return false;
	}

	*status = hidpp20_map_adc_measurement_1f20(params, voltage);
	return true;
}

static int hidpp20_query_adc_measurement_info_1f20(struct hidpp_device *hidpp)
{
	if (hidpp->battery.adc_measurement_feature_index == 0xff) {
		int ret;

		ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_ADC_MEASUREMENT,
					     &hidpp->battery.adc_measurement_feature_index);
		if (ret)
			return ret;

		hidpp->capabilities |= HIDPP_CAPABILITY_ADC_MEASUREMENT;
	}

	hidpp->battery.online = hidpp20_get_adc_measurement_1f20(hidpp,
								 hidpp->battery.adc_measurement_feature_index,
								 &hidpp->battery.status,
								 &hidpp->battery.voltage);
	hidpp->battery.capacity = hidpp20_map_adc_measurement_1f20_capacity(hidpp->hid_dev,
									    hidpp->battery.voltage);
	hidpp_update_usb_wireless_status(hidpp);

	return 0;
}

static int hidpp20_adc_measurement_event_1f20(struct hidpp_device *hidpp,
					    u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	int status, voltage;

	if (report->fap.feature_index != hidpp->battery.adc_measurement_feature_index ||
		report->fap.funcindex_clientid != EVENT_ADC_MEASUREMENT_STATUS_BROADCAST)
		return 0;

	status = hidpp20_map_adc_measurement_1f20(report->fap.params, &voltage);

	hidpp->battery.online = status != POWER_SUPPLY_STATUS_UNKNOWN;

	if (voltage != hidpp->battery.voltage || status != hidpp->battery.status) {
		hidpp->battery.status = status;
		hidpp->battery.voltage = voltage;
		hidpp->battery.capacity = hidpp20_map_adc_measurement_1f20_capacity(hidpp->hid_dev, voltage);
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
		hidpp_update_usb_wireless_status(hidpp);
	}
	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x2120: Hi-resolution scrolling                                            */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_HI_RESOLUTION_SCROLLING			0x2120

#define CMD_HI_RESOLUTION_SCROLLING_SET_HIGHRES_SCROLLING_MODE	0x10

static int hidpp_hrs_set_highres_scrolling_mode(struct hidpp_device *hidpp,
	bool enabled, u8 *multiplier)
{
	u8 feature_index;
	int ret;
	u8 params[1];
	struct hidpp_report response;

	ret = hidpp_root_get_feature(hidpp,
				     HIDPP_PAGE_HI_RESOLUTION_SCROLLING,
				     &feature_index);
	if (ret)
		return ret;

	params[0] = enabled ? BIT(0) : 0;
	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_HI_RESOLUTION_SCROLLING_SET_HIGHRES_SCROLLING_MODE,
					  params, sizeof(params), &response);
	if (ret)
		return ret;
	*multiplier = response.fap.params[1];
	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x2121: HiRes Wheel                                                        */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_HIRES_WHEEL		0x2121

#define CMD_HIRES_WHEEL_GET_WHEEL_CAPABILITY	0x00
#define CMD_HIRES_WHEEL_SET_WHEEL_MODE		0x20

static int hidpp_hrw_get_wheel_capability(struct hidpp_device *hidpp,
	u8 *multiplier)
{
	u8 feature_index;
	int ret;
	struct hidpp_report response;

	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_HIRES_WHEEL,
				     &feature_index);
	if (ret)
		goto return_default;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
					  CMD_HIRES_WHEEL_GET_WHEEL_CAPABILITY,
					  NULL, 0, &response);
	if (ret)
		goto return_default;

	*multiplier = response.fap.params[0];
	return 0;
return_default:
	hid_warn(hidpp->hid_dev,
		 "Couldn't get wheel multiplier (error %d)\n", ret);
	return ret;
}

static int hidpp_hrw_set_wheel_mode(struct hidpp_device *hidpp, bool invert,
	bool high_resolution, bool use_hidpp)
{
	u8 feature_index;
	int ret;
	u8 params[1];
	struct hidpp_report response;

	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_HIRES_WHEEL,
				     &feature_index);
	if (ret)
		return ret;

	params[0] = (invert          ? BIT(2) : 0) |
		    (high_resolution ? BIT(1) : 0) |
		    (use_hidpp       ? BIT(0) : 0);

	return hidpp_send_fap_command_sync(hidpp, feature_index,
					   CMD_HIRES_WHEEL_SET_WHEEL_MODE,
					   params, sizeof(params), &response);
}

/* -------------------------------------------------------------------------- */
/* 0x4301: Solar Keyboard                                                     */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_SOLAR_KEYBOARD			0x4301

#define CMD_SOLAR_SET_LIGHT_MEASURE			0x00

#define EVENT_SOLAR_BATTERY_BROADCAST			0x00
#define EVENT_SOLAR_BATTERY_LIGHT_MEASURE		0x10
#define EVENT_SOLAR_CHECK_LIGHT_BUTTON			0x20

static int hidpp_solar_request_battery_event(struct hidpp_device *hidpp)
{
	struct hidpp_report response;
	u8 params[2] = { 1, 1 };
	int ret;

	if (hidpp->battery.feature_index == 0xff) {
		ret = hidpp_root_get_feature(hidpp,
					     HIDPP_PAGE_SOLAR_KEYBOARD,
					     &hidpp->battery.solar_feature_index);
		if (ret)
			return ret;
	}

	ret = hidpp_send_fap_command_sync(hidpp,
					  hidpp->battery.solar_feature_index,
					  CMD_SOLAR_SET_LIGHT_MEASURE,
					  params, 2, &response);
	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_MILEAGE;

	return 0;
}

static int hidpp_solar_battery_event(struct hidpp_device *hidpp,
				     u8 *data, int size)
{
	struct hidpp_report *report = (struct hidpp_report *)data;
	int capacity, lux, status;
	u8 function;

	function = report->fap.funcindex_clientid;


	if (report->fap.feature_index != hidpp->battery.solar_feature_index ||
	    !(function == EVENT_SOLAR_BATTERY_BROADCAST ||
	      function == EVENT_SOLAR_BATTERY_LIGHT_MEASURE ||
	      function == EVENT_SOLAR_CHECK_LIGHT_BUTTON))
		return 0;

	capacity = report->fap.params[0];

	switch (function) {
	case EVENT_SOLAR_BATTERY_LIGHT_MEASURE:
		lux = (report->fap.params[1] << 8) | report->fap.params[2];
		if (lux > 200)
			status = POWER_SUPPLY_STATUS_CHARGING;
		else
			status = POWER_SUPPLY_STATUS_DISCHARGING;
		break;
	case EVENT_SOLAR_CHECK_LIGHT_BUTTON:
	default:
		if (capacity < hidpp->battery.capacity)
			status = POWER_SUPPLY_STATUS_DISCHARGING;
		else
			status = POWER_SUPPLY_STATUS_CHARGING;

	}

	if (capacity == 100)
		status = POWER_SUPPLY_STATUS_FULL;

	hidpp->battery.online = true;
	if (capacity != hidpp->battery.capacity ||
	    status != hidpp->battery.status) {
		hidpp->battery.capacity = capacity;
		hidpp->battery.status = status;
		if (hidpp->battery.ps)
			power_supply_changed(hidpp->battery.ps);
	}

	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x6010: Touchpad FW items                                                  */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_TOUCHPAD_FW_ITEMS			0x6010

#define CMD_TOUCHPAD_FW_ITEMS_SET			0x10

struct hidpp_touchpad_fw_items {
	uint8_t presence;
	uint8_t desired_state;
	uint8_t state;
	uint8_t persistent;
};

/*
 * send a set state command to the device by reading the current items->state
 * field. items is then filled with the current state.
 */
static int hidpp_touchpad_fw_items_set(struct hidpp_device *hidpp,
				       u8 feature_index,
				       struct hidpp_touchpad_fw_items *items)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
		CMD_TOUCHPAD_FW_ITEMS_SET, &items->state, 1, &response);

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	items->presence = params[0];
	items->desired_state = params[1];
	items->state = params[2];
	items->persistent = params[3];

	return 0;
}

/* -------------------------------------------------------------------------- */
/* 0x6100: TouchPadRawXY                                                      */
/* -------------------------------------------------------------------------- */

#define HIDPP_PAGE_TOUCHPAD_RAW_XY			0x6100

#define CMD_TOUCHPAD_GET_RAW_INFO			0x00
#define CMD_TOUCHPAD_SET_RAW_REPORT_STATE		0x20

#define EVENT_TOUCHPAD_RAW_XY				0x00

#define TOUCHPAD_RAW_XY_ORIGIN_LOWER_LEFT		0x01
#define TOUCHPAD_RAW_XY_ORIGIN_UPPER_LEFT		0x03

struct hidpp_touchpad_raw_info {
	u16 x_size;
	u16 y_size;
	u8 z_range;
	u8 area_range;
	u8 timestamp_unit;
	u8 maxcontacts;
	u8 origin;
	u16 res;
};

struct hidpp_touchpad_raw_xy_finger {
	u8 contact_type;
	u8 contact_status;
	u16 x;
	u16 y;
	u8 z;
	u8 area;
	u8 finger_id;
};

struct hidpp_touchpad_raw_xy {
	u16 timestamp;
	struct hidpp_touchpad_raw_xy_finger fingers[2];
	u8 spurious_flag;
	u8 end_of_frame;
	u8 finger_count;
	u8 button;
};

static int hidpp_touchpad_get_raw_info(struct hidpp_device *hidpp,
	u8 feature_index, struct hidpp_touchpad_raw_info *raw_info)
{
	struct hidpp_report response;
	int ret;
	u8 *params = (u8 *)response.fap.params;

	ret = hidpp_send_fap_command_sync(hidpp, feature_index,
		CMD_TOUCHPAD_GET_RAW_INFO, NULL, 0, &response);

	if (ret > 0) {
		hid_err(hidpp->hid_dev, "%s: received protocol error 0x%02x\n",
			__func__, ret);
		return -EPROTO;
	}
	if (ret)
		return ret;

	raw_info->x_size = get_unaligned_be16(&params[0]);
	raw_info->y_size = get_unaligned_be16(&params[2]);
	raw_info->z_range = params[4];
	raw_info->area_range = params[5];
	raw_info->maxcontacts = params[7];
	raw_info->origin = params[8];
	/* res is given in unit per inch */
	raw_info->res = get_unaligned_be16(&params[13]) * 2 / 51;

	return ret;
}

static int hidpp_touchpad_set_raw_report_state(struct hidpp_device *hidpp_dev,
		u8 feature_index, bool send_raw_reports,
		bool sensor_enhanced_settings)
{
	struct hidpp_report response;

	/*
	 * Params:
	 *   bit 0 - enable raw
	 *   bit 1 - 16bit Z, no area
	 *   bit 2 - enhanced sensitivity
	 *   bit 3 - width, height (4 bits each) instead of area
	 *   bit 4 - send raw + gestures (degrades smoothness)
	 *   remaining bits - reserved
	 */
	u8 params = send_raw_reports | (sensor_enhanced_settings << 2);

	return hidpp_send_fap_command_sync(hidpp_dev, feature_index,
		CMD_TOUCHPAD_SET_RAW_REPORT_STATE, &params, 1, &response);
}

static void hidpp_touchpad_touch_event(u8 *data,
	struct hidpp_touchpad_raw_xy_finger *finger)
{
	u8 x_m = data[0] << 2;
	u8 y_m = data[2] << 2;

	finger->x = x_m << 6 | data[1];
	finger->y = y_m << 6 | data[3];

	finger->contact_type = data[0] >> 6;
	finger->contact_status = data[2] >> 6;

	finger->z = data[4];
	finger->area = data[5];
	finger->finger_id = data[6] >> 4;
}

static void hidpp_touchpad_raw_xy_event(struct hidpp_device *hidpp_dev,
		u8 *data, struct hidpp_touchpad_raw_xy *raw_xy)
{
	memset(raw_xy, 0, sizeof(struct hidpp_touchpad_raw_xy));
	raw_xy->end_of_frame = data[8] & 0x01;
	raw_xy->spurious_flag = (data[8] >> 1) & 0x01;
	raw_xy->finger_count = data[15] & 0x0f;
	raw_xy->button = (data[8] >> 2) & 0x01;

	if (raw_xy->finger_count) {
		hidpp_touchpad_touch_event(&data[2], &raw_xy->fingers[0]);
		hidpp_touchpad_touch_event(&data[9], &raw_xy->fingers[1]);
	}
}

/* -------------------------------------------------------------------------- */
/* 0x8123: Force feedback support                                             */
/* -------------------------------------------------------------------------- */

#define HIDPP_FF_GET_INFO		0x01
#define HIDPP_FF_RESET_ALL		0x11
#define HIDPP_FF_DOWNLOAD_EFFECT	0x21
#define HIDPP_FF_SET_EFFECT_STATE	0x31
#define HIDPP_FF_DESTROY_EFFECT		0x41
#define HIDPP_FF_GET_APERTURE		0x51
#define HIDPP_FF_SET_APERTURE		0x61
#define HIDPP_FF_GET_GLOBAL_GAINS	0x71
#define HIDPP_FF_SET_GLOBAL_GAINS	0x81

#define HIDPP_FF_EFFECT_STATE_GET	0x00
#define HIDPP_FF_EFFECT_STATE_STOP	0x01
#define HIDPP_FF_EFFECT_STATE_PLAY	0x02
#define HIDPP_FF_EFFECT_STATE_PAUSE	0x03

#define HIDPP_FF_EFFECT_CONSTANT	0x00
#define HIDPP_FF_EFFECT_PERIODIC_SINE		0x01
#define HIDPP_FF_EFFECT_PERIODIC_SQUARE		0x02
#define HIDPP_FF_EFFECT_PERIODIC_TRIANGLE	0x03
#define HIDPP_FF_EFFECT_PERIODIC_SAWTOOTHUP	0x04
#define HIDPP_FF_EFFECT_PERIODIC_SAWTOOTHDOWN	0x05
#define HIDPP_FF_EFFECT_SPRING		0x06
#define HIDPP_FF_EFFECT_DAMPER		0x07
#define HIDPP_FF_EFFECT_FRICTION	0x08
#define HIDPP_FF_EFFECT_INERTIA		0x09
#define HIDPP_FF_EFFECT_RAMP		0x0A

#define HIDPP_FF_EFFECT_AUTOSTART	0x80

#define HIDPP_FF_EFFECTID_NONE		-1
#define HIDPP_FF_EFFECTID_AUTOCENTER	-2
#define HIDPP_AUTOCENTER_PARAMS_LENGTH	18

#define HIDPP_FF_MAX_PARAMS	20
#define HIDPP_FF_RESERVED_SLOTS	1

struct hidpp_ff_private_data {
	struct hidpp_device *hidpp;
	u8 feature_index;
	u8 version;
	u16 gain;
	s16 range;
	u8 slot_autocenter;
	u8 num_effects;
	int *effect_ids;
	struct workqueue_struct *wq;
	struct work_struct work;	/* single drain worker for the pending list */
	struct list_head pending;	/* FIFO of hidpp_ff_work_data */
	spinlock_t lock;		/* guards pending + queue_len; taken from atomic ctx */
	int queue_len;
};

struct hidpp_ff_work_data {
	struct list_head node;
	int effect_id;
	u8 command;
	u8 params[HIDPP_FF_MAX_PARAMS];
	u8 size;
};

static const signed short hidpp_ff_effects[] = {
	FF_CONSTANT,
	FF_PERIODIC,
	FF_SINE,
	FF_SQUARE,
	FF_SAW_UP,
	FF_SAW_DOWN,
	FF_TRIANGLE,
	FF_SPRING,
	FF_DAMPER,
	FF_AUTOCENTER,
	FF_GAIN,
	-1
};

static const signed short hidpp_ff_effects_v2[] = {
	FF_RAMP,
	FF_FRICTION,
	FF_INERTIA,
	-1
};

static const u8 HIDPP_FF_CONDITION_CMDS[] = {
	HIDPP_FF_EFFECT_SPRING,
	HIDPP_FF_EFFECT_FRICTION,
	HIDPP_FF_EFFECT_DAMPER,
	HIDPP_FF_EFFECT_INERTIA
};

static const char *HIDPP_FF_CONDITION_NAMES[] = {
	"spring",
	"friction",
	"damper",
	"inertia"
};


static u8 hidpp_ff_find_effect(struct hidpp_ff_private_data *data, int effect_id)
{
	int i;

	for (i = 0; i < data->num_effects; i++)
		if (data->effect_ids[i] == effect_id)
			return i+1;

	return 0;
}

/* Send one queued command and apply its response. Runs only in the worker. */
static void hidpp_ff_send_one(struct hidpp_ff_private_data *data,
			      struct hidpp_ff_work_data *wd)
{
	struct hidpp_report response;
	u8 slot;
	int ret;

	/* add slot number if needed */
	switch (wd->effect_id) {
	case HIDPP_FF_EFFECTID_AUTOCENTER:
		wd->params[0] = data->slot_autocenter;
		break;
	case HIDPP_FF_EFFECTID_NONE:
		/* leave slot as zero */
		break;
	default:
		/* find current slot for effect */
		wd->params[0] = hidpp_ff_find_effect(data, wd->effect_id);
		break;
	}

	/* send command and wait for reply */
	ret = hidpp_send_fap_command_sync(data->hidpp, data->feature_index,
		wd->command, wd->params, wd->size, &response);

	if (ret) {
		hid_err(data->hidpp->hid_dev, "Failed to send command to device!\n");
		return;
	}

	/* parse return data */
	switch (wd->command) {
	case HIDPP_FF_DOWNLOAD_EFFECT:
		slot = response.fap.params[0];
		if (slot > 0 && slot <= data->num_effects) {
			if (wd->effect_id >= 0)
				/* regular effect uploaded */
				data->effect_ids[slot-1] = wd->effect_id;
			else if (wd->effect_id >= HIDPP_FF_EFFECTID_AUTOCENTER)
				/* autocenter spring uploaded */
				data->slot_autocenter = slot;
		}
		break;
	case HIDPP_FF_DESTROY_EFFECT:
		if (wd->effect_id >= 0)
			/* regular effect destroyed */
			data->effect_ids[wd->params[0]-1] = -1;
		else if (wd->effect_id >= HIDPP_FF_EFFECTID_AUTOCENTER)
			/* autocenter spring destroyed */
			data->slot_autocenter = 0;
		break;
	case HIDPP_FF_SET_GLOBAL_GAINS:
		data->gain = (wd->params[0] << 8) + wd->params[1];
		break;
	case HIDPP_FF_SET_APERTURE:
		data->range = (wd->params[0] << 8) + wd->params[1];
		break;
	default:
		/* no action needed */
		break;
	}
}

/*
 * Drain the pending FIFO. A single worker processes commands one at a time;
 * the device serialises HID++ at ~3 ms/command, so parallelism would not
 * help and would reorder commands. effect_ids/slot_autocenter are touched
 * only here, so they need no extra locking; data->lock guards only the list.
 */
static void hidpp_ff_work_handler(struct work_struct *w)
{
	struct hidpp_ff_private_data *data =
		container_of(w, struct hidpp_ff_private_data, work);
	struct hidpp_ff_work_data *wd;
	unsigned long flags;

	for (;;) {
		spin_lock_irqsave(&data->lock, flags);
		if (list_empty(&data->pending)) {
			spin_unlock_irqrestore(&data->lock, flags);
			return;
		}
		wd = list_first_entry(&data->pending,
				      struct hidpp_ff_work_data, node);
		list_del(&wd->node);
		data->queue_len--;
		spin_unlock_irqrestore(&data->lock, flags);

		hidpp_ff_send_one(data, wd);
		kfree(wd);
	}
}

/*
 * Queue an FFB command, coalescing a run of identical-key updates.
 *
 * A game replaying a constant force re-uploads (and re-plays) the same effect
 * far faster than the device's ~300 command/s HID++ drain rate, so without
 * coalescing the FIFO grows without bound (issue #8). When the newest command
 * shares its (effect_id, command) with the item already at the tail of the
 * pending list, we overwrite that item's payload instead of appending: the
 * device only ever needs the latest state of a given effect, and the tail is
 * by definition not yet in flight (the worker removes items before sending),
 * so this never reorders distinct commands. DESTROY is never coalesced.
 *
 * Reachable from the atomic playback path (input core's event_lock), hence
 * GFP_ATOMIC and a spinlock taken with irqsave.
 */
static int hidpp_ff_queue_work(struct hidpp_ff_private_data *data, int effect_id, u8 command, u8 *params, u8 size)
{
	struct hidpp_ff_work_data *wd, *tail;
	unsigned long flags;
	bool coalescible = command == HIDPP_FF_DOWNLOAD_EFFECT ||
			   command == HIDPP_FF_SET_EFFECT_STATE ||
			   command == HIDPP_FF_SET_GLOBAL_GAINS ||
			   command == HIDPP_FF_SET_APERTURE;
	int s;

	wd = kzalloc(sizeof(*wd), GFP_ATOMIC);
	if (!wd)
		return -ENOMEM;

	wd->effect_id = effect_id;
	wd->command = command;
	wd->size = size;
	memcpy(wd->params, params, size);

	spin_lock_irqsave(&data->lock, flags);

	if (coalescible && !list_empty(&data->pending)) {
		tail = list_last_entry(&data->pending,
				       struct hidpp_ff_work_data, node);
		if (tail->command == command && tail->effect_id == effect_id) {
			memcpy(tail->params, params, size);
			tail->size = size;
			spin_unlock_irqrestore(&data->lock, flags);
			kfree(wd);
			return 0;
		}
	}

	list_add_tail(&wd->node, &data->pending);
	s = ++data->queue_len;
	spin_unlock_irqrestore(&data->lock, flags);

	queue_work(data->wq, &data->work);

	/* warn about excessive queue size */
	if (s >= 20 && s % 20 == 0)
		hid_warn(data->hidpp->hid_dev, "Force feedback command queue contains %d commands, causing substantial delays!", s);

	return 0;
}

static int hidpp_ff_upload_effect(struct input_dev *dev, struct ff_effect *effect, struct ff_effect *old)
{
	struct hidpp_ff_private_data *data = dev->ff->private;
	u8 params[20];
	u8 size;
	int force;

	/* set common parameters */
	params[2] = effect->replay.length >> 8;
	params[3] = effect->replay.length & 255;
	params[4] = effect->replay.delay >> 8;
	params[5] = effect->replay.delay & 255;

	switch (effect->type) {
	case FF_CONSTANT:
		force = (effect->u.constant.level * fixp_sin16((effect->direction * 360) >> 16)) >> 15;
		params[1] = HIDPP_FF_EFFECT_CONSTANT;
		params[6] = force >> 8;
		params[7] = force & 255;
		params[8] = effect->u.constant.envelope.attack_level >> 7;
		params[9] = effect->u.constant.envelope.attack_length >> 8;
		params[10] = effect->u.constant.envelope.attack_length & 255;
		params[11] = effect->u.constant.envelope.fade_level >> 7;
		params[12] = effect->u.constant.envelope.fade_length >> 8;
		params[13] = effect->u.constant.envelope.fade_length & 255;
		size = 14;
		dbg_hid("Uploading constant force level=%d in dir %d = %d\n",
				effect->u.constant.level,
				effect->direction, force);
		dbg_hid("          envelope attack=(%d, %d ms) fade=(%d, %d ms)\n",
				effect->u.constant.envelope.attack_level,
				effect->u.constant.envelope.attack_length,
				effect->u.constant.envelope.fade_level,
				effect->u.constant.envelope.fade_length);
		break;
	case FF_PERIODIC:
	{
		switch (effect->u.periodic.waveform) {
		case FF_SINE:
			params[1] = HIDPP_FF_EFFECT_PERIODIC_SINE;
			break;
		case FF_SQUARE:
			params[1] = HIDPP_FF_EFFECT_PERIODIC_SQUARE;
			break;
		case FF_SAW_UP:
			params[1] = HIDPP_FF_EFFECT_PERIODIC_SAWTOOTHUP;
			break;
		case FF_SAW_DOWN:
			params[1] = HIDPP_FF_EFFECT_PERIODIC_SAWTOOTHDOWN;
			break;
		case FF_TRIANGLE:
			params[1] = HIDPP_FF_EFFECT_PERIODIC_TRIANGLE;
			break;
		default:
			hid_err(data->hidpp->hid_dev, "Unexpected periodic waveform type %i!\n", effect->u.periodic.waveform);
			return -EINVAL;
		}
		force = (effect->u.periodic.magnitude * fixp_sin16((effect->direction * 360) >> 16)) >> 15;
		params[6] = effect->u.periodic.magnitude >> 8;
		params[7] = effect->u.periodic.magnitude & 255;
		params[8] = effect->u.periodic.offset >> 8;
		params[9] = effect->u.periodic.offset & 255;
		params[10] = effect->u.periodic.period >> 8;
		params[11] = effect->u.periodic.period & 255;
		params[12] = effect->u.periodic.phase >> 8;
		params[13] = effect->u.periodic.phase & 255;
		params[14] = effect->u.periodic.envelope.attack_level >> 7;
		params[15] = effect->u.periodic.envelope.attack_length >> 8;
		params[16] = effect->u.periodic.envelope.attack_length & 255;
		params[17] = effect->u.periodic.envelope.fade_level >> 7;
		params[18] = effect->u.periodic.envelope.fade_length >> 8;
		params[19] = effect->u.periodic.envelope.fade_length & 255;
		size = 20;
		dbg_hid("Uploading periodic force mag=%d/dir=%d, offset=%d, period=%d ms, phase=%d\n",
				effect->u.periodic.magnitude, effect->direction,
				effect->u.periodic.offset,
				effect->u.periodic.period,
				effect->u.periodic.phase);
		dbg_hid("          envelope attack=(%d, %d ms) fade=(%d, %d ms)\n",
				effect->u.periodic.envelope.attack_level,
				effect->u.periodic.envelope.attack_length,
				effect->u.periodic.envelope.fade_level,
				effect->u.periodic.envelope.fade_length);
		break;
	}
	case FF_RAMP:
		params[1] = HIDPP_FF_EFFECT_RAMP;
		force = (effect->u.ramp.start_level * fixp_sin16((effect->direction * 360) >> 16)) >> 15;
		params[6] = force >> 8;
		params[7] = force & 255;
		force = (effect->u.ramp.end_level * fixp_sin16((effect->direction * 360) >> 16)) >> 15;
		params[8] = force >> 8;
		params[9] = force & 255;
		params[10] = effect->u.ramp.envelope.attack_level >> 7;
		params[11] = effect->u.ramp.envelope.attack_length >> 8;
		params[12] = effect->u.ramp.envelope.attack_length & 255;
		params[13] = effect->u.ramp.envelope.fade_level >> 7;
		params[14] = effect->u.ramp.envelope.fade_length >> 8;
		params[15] = effect->u.ramp.envelope.fade_length & 255;
		size = 16;
		dbg_hid("Uploading ramp force level=%d -> %d in dir %d = %d\n",
				effect->u.ramp.start_level,
				effect->u.ramp.end_level,
				effect->direction, force);
		dbg_hid("          envelope attack=(%d, %d ms) fade=(%d, %d ms)\n",
				effect->u.ramp.envelope.attack_level,
				effect->u.ramp.envelope.attack_length,
				effect->u.ramp.envelope.fade_level,
				effect->u.ramp.envelope.fade_length);
		break;
	case FF_FRICTION:
	case FF_INERTIA:
	case FF_SPRING:
	case FF_DAMPER:
		params[1] = HIDPP_FF_CONDITION_CMDS[effect->type - FF_SPRING];
		params[6] = effect->u.condition[0].left_saturation >> 9;
		params[7] = (effect->u.condition[0].left_saturation >> 1) & 255;
		params[8] = effect->u.condition[0].left_coeff >> 8;
		params[9] = effect->u.condition[0].left_coeff & 255;
		params[10] = effect->u.condition[0].deadband >> 9;
		params[11] = (effect->u.condition[0].deadband >> 1) & 255;
		params[12] = effect->u.condition[0].center >> 8;
		params[13] = effect->u.condition[0].center & 255;
		params[14] = effect->u.condition[0].right_coeff >> 8;
		params[15] = effect->u.condition[0].right_coeff & 255;
		params[16] = effect->u.condition[0].right_saturation >> 9;
		params[17] = (effect->u.condition[0].right_saturation >> 1) & 255;
		size = 18;
		dbg_hid("Uploading %s force left coeff=%d, left sat=%d, right coeff=%d, right sat=%d\n",
				HIDPP_FF_CONDITION_NAMES[effect->type - FF_SPRING],
				effect->u.condition[0].left_coeff,
				effect->u.condition[0].left_saturation,
				effect->u.condition[0].right_coeff,
				effect->u.condition[0].right_saturation);
		dbg_hid("          deadband=%d, center=%d\n",
				effect->u.condition[0].deadband,
				effect->u.condition[0].center);
		break;
	default:
		hid_err(data->hidpp->hid_dev, "Unexpected force type %i!\n", effect->type);
		return -EINVAL;
	}

	return hidpp_ff_queue_work(data, effect->id, HIDPP_FF_DOWNLOAD_EFFECT, params, size);
}

static int hidpp_ff_playback(struct input_dev *dev, int effect_id, int value)
{
	struct hidpp_ff_private_data *data = dev->ff->private;
	u8 params[2];

	params[1] = value ? HIDPP_FF_EFFECT_STATE_PLAY : HIDPP_FF_EFFECT_STATE_STOP;

	dbg_hid("St%sing playback of effect %d.\n", value?"art":"opp", effect_id);

	return hidpp_ff_queue_work(data, effect_id, HIDPP_FF_SET_EFFECT_STATE, params, ARRAY_SIZE(params));
}

static int hidpp_ff_erase_effect(struct input_dev *dev, int effect_id)
{
	struct hidpp_ff_private_data *data = dev->ff->private;
	u8 slot = 0;

	dbg_hid("Erasing effect %d.\n", effect_id);

	return hidpp_ff_queue_work(data, effect_id, HIDPP_FF_DESTROY_EFFECT, &slot, 1);
}

static void hidpp_ff_set_autocenter(struct input_dev *dev, u16 magnitude)
{
	struct hidpp_ff_private_data *data = dev->ff->private;
	u8 params[HIDPP_AUTOCENTER_PARAMS_LENGTH];

	dbg_hid("Setting autocenter to %d.\n", magnitude);

	/* start a standard spring effect */
	params[1] = HIDPP_FF_EFFECT_SPRING | HIDPP_FF_EFFECT_AUTOSTART;
	/* zero delay and duration */
	params[2] = params[3] = params[4] = params[5] = 0;
	/* set coeff to 25% of saturation */
	params[8] = params[14] = magnitude >> 11;
	params[9] = params[15] = (magnitude >> 3) & 255;
	params[6] = params[16] = magnitude >> 9;
	params[7] = params[17] = (magnitude >> 1) & 255;
	/* zero deadband and center */
	params[10] = params[11] = params[12] = params[13] = 0;

	hidpp_ff_queue_work(data, HIDPP_FF_EFFECTID_AUTOCENTER, HIDPP_FF_DOWNLOAD_EFFECT, params, ARRAY_SIZE(params));
}

static void hidpp_ff_set_gain(struct input_dev *dev, u16 gain)
{
	struct hidpp_ff_private_data *data = dev->ff->private;
	u8 params[4];

	dbg_hid("Setting gain to %d.\n", gain);

	params[0] = gain >> 8;
	params[1] = gain & 255;
	params[2] = 0; /* no boost */
	params[3] = 0;

	hidpp_ff_queue_work(data, HIDPP_FF_EFFECTID_NONE, HIDPP_FF_SET_GLOBAL_GAINS, params, ARRAY_SIZE(params));
}

static ssize_t hidpp_ff_range_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hid_input *hidinput;
	struct input_dev *idev;
	struct hidpp_ff_private_data *data;
	struct usb_interface *iface;

	/* Handle cross-interface case: range sysfs is on interface 1, inputs on 0 */
	if (hid_is_usb(hid)) {
		iface = to_usb_interface(hid->dev.parent);
		if (iface->cur_altsetting->desc.bInterfaceNumber != 0) {
			struct hid_device *hid0;
			hid0 = usb_get_intfdata(usb_ifnum_to_if(hid_to_usb_dev(hid), 0));
			if (hid0)
				hid = hid0;
		}
	}

	if (list_empty(&hid->inputs))
		return -ENODEV;

	hidinput = list_entry(hid->inputs.next, struct hid_input, list);
	idev = hidinput->input;
	if (!idev || !idev->ff)
		return -ENODEV;

	data = idev->ff->private;
	return sysfs_emit(buf, "%u\n", data->range);
}

static ssize_t hidpp_ff_range_store(struct device *dev, struct device_attribute *attr, const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hid_input *hidinput;
	struct input_dev *idev;
	struct hidpp_ff_private_data *data;
	struct usb_interface *iface;
	u8 params[2];
	int range;
	int ret;
	__u16 product = hid->product;

	ret = kstrtoint(buf, 10, &range);
	if (ret)
		return ret;

	/* Handle cross-interface case: range sysfs is on interface 1, inputs on 0 */
	if (hid_is_usb(hid)) {
		iface = to_usb_interface(hid->dev.parent);
		if (iface->cur_altsetting->desc.bInterfaceNumber != 0) {
			struct hid_device *hid0;
			hid0 = usb_get_intfdata(usb_ifnum_to_if(hid_to_usb_dev(hid), 0));
			if (hid0)
				hid = hid0;
		}
	}

	if (list_empty(&hid->inputs))
		return -ENODEV;

	hidinput = list_entry(hid->inputs.next, struct hid_input, list);
	idev = hidinput->input;
	if (!idev || !idev->ff)
		return -ENODEV;

	data = idev->ff->private;

	/* Direct-drive wheels (RS50, G Pro) support up to 1080 degrees rotation */
	if (product == USB_DEVICE_ID_LOGITECH_RS50 ||
	    product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
	    product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
		range = clamp(range, 180, 1080);
	else
		range = clamp(range, 180, 900);

	params[0] = range >> 8;
	params[1] = range & 0x00FF;

	hidpp_ff_queue_work(data, -1, HIDPP_FF_SET_APERTURE, params, ARRAY_SIZE(params));

	return count;
}

static DEVICE_ATTR(range, S_IRUSR | S_IWUSR | S_IRGRP | S_IWGRP | S_IROTH, hidpp_ff_range_show, hidpp_ff_range_store);

static void hidpp_ff_destroy(struct ff_device *ff)
{
	struct hidpp_ff_private_data *data = ff->private;
	struct hid_device *hid = data->hidpp->hid_dev;

	hid_info(hid, "Unloading HID++ force feedback.\n");

	device_remove_file(&hid->dev, &dev_attr_range);
	/* drains and waits for the worker, leaving the pending list empty */
	destroy_workqueue(data->wq);

	/* defensive: free anything the drain left behind (should be none) */
	while (!list_empty(&data->pending)) {
		struct hidpp_ff_work_data *wd =
			list_first_entry(&data->pending,
					 struct hidpp_ff_work_data, node);
		list_del(&wd->node);
		kfree(wd);
	}

	kfree(data->effect_ids);
}

static int hidpp_ff_init(struct hidpp_device *hidpp,
			 struct hidpp_ff_private_data *data)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hid_input *hidinput;
	struct input_dev *dev;
	struct usb_device_descriptor *udesc;
	u16 bcdDevice;
	struct ff_device *ff;
	int error, j, num_slots = data->num_effects;
	u8 version;
	struct usb_interface *iface;

	if (!hid_is_usb(hid)) {
		hid_err(hid, "device is not USB\n");
		return -ENODEV;
	}

	/*
	 * Direct-drive wheels (RS50, G Pro) have HID++ on interface 1 but
	 * input on interface 0. If we're on a non-zero interface, walk
	 * sibling interfaces looking for one whose hid_device has non-
	 * empty inputs. This avoids hardcoding interface 0 in case a
	 * future firmware rev reshuffles them.
	 */
	iface = to_usb_interface(hid->dev.parent);
	if ((hidpp->quirks & HIDPP_QUIRK_CLASS_G920) &&
	    iface->cur_altsetting->desc.bInterfaceNumber != 0) {
		struct usb_device *udev = hid_to_usb_dev(hid);
		struct hid_device *hid_input = NULL;
		int found_ifnum = -1;
		int i;

		hid_info(hid,
			 "G920 FFB init: on interface %d, walking siblings for one with inputs\n",
			 iface->cur_altsetting->desc.bInterfaceNumber);

		for (i = 0; i < USB_MAXINTERFACES; i++) {
			struct usb_interface *sibling = usb_ifnum_to_if(udev, i);
			struct hid_device *sib;

			if (!sibling)
				continue;
			sib = usb_get_intfdata(sibling);
			if (!sib)
				continue;
			if (!list_empty(&sib->inputs)) {
				hid_input = sib;
				found_ifnum = i;
				break;
			}
		}
		if (hid_input) {
			hid_info(hid,
				 "G920 FFB init: attaching to interface %d input dev\n",
				 found_ifnum);
			hid = hid_input;
		} else {
			hid_err(hid,
				"G920 FFB init: no sibling interface has inputs; FFB will not register\n");
		}
	}

	if (!hid || list_empty(&hid->inputs)) {
		hid_err(hid, "G920 FFB init: no inputs on target hid_device\n");
		return -ENODEV;
	}
	hidinput = list_entry(hid->inputs.next, struct hid_input, list);
	dev = hidinput->input;

	if (!dev) {
		hid_err(hid, "Struct input_dev not set!\n");
		return -EINVAL;
	}

	/* Get firmware release */
	udesc = &(hid_to_usb_dev(hid)->descriptor);
	bcdDevice = le16_to_cpu(udesc->bcdDevice);
	version = bcdDevice & 255;

	/* Set supported force feedback capabilities */
	for (j = 0; hidpp_ff_effects[j] >= 0; j++)
		set_bit(hidpp_ff_effects[j], dev->ffbit);
	if (version > 1)
		for (j = 0; hidpp_ff_effects_v2[j] >= 0; j++)
			set_bit(hidpp_ff_effects_v2[j], dev->ffbit);

	error = input_ff_create(dev, num_slots);

	if (error) {
		hid_err(dev, "Failed to create FF device!\n");
		return error;
	}
	/*
	 * Create a copy of passed data, so we can transfer memory
	 * ownership to FF core
	 */
	data = kmemdup(data, sizeof(*data), GFP_KERNEL);
	if (!data)
		return -ENOMEM;
	data->effect_ids = kcalloc(num_slots, sizeof(int), GFP_KERNEL);
	if (!data->effect_ids) {
		kfree(data);
		return -ENOMEM;
	}
	data->wq = create_singlethread_workqueue("hidpp-ff-sendqueue");
	if (!data->wq) {
		kfree(data->effect_ids);
		kfree(data);
		return -ENOMEM;
	}

	data->hidpp = hidpp;
	data->version = version;
	for (j = 0; j < num_slots; j++)
		data->effect_ids[j] = -1;

	ff = dev->ff;
	ff->private = data;

	ff->upload = hidpp_ff_upload_effect;
	ff->erase = hidpp_ff_erase_effect;
	ff->playback = hidpp_ff_playback;
	ff->set_gain = hidpp_ff_set_gain;
	ff->set_autocenter = hidpp_ff_set_autocenter;
	ff->destroy = hidpp_ff_destroy;

	/* Create sysfs interface */
	error = device_create_file(&(hidpp->hid_dev->dev), &dev_attr_range);
	if (error)
		hid_warn(hidpp->hid_dev, "Unable to create sysfs interface for \"range\", errno %d!\n", error);

	/* init the hardware command queue */
	INIT_LIST_HEAD(&data->pending);
	spin_lock_init(&data->lock);
	INIT_WORK(&data->work, hidpp_ff_work_handler);
	data->queue_len = 0;

	hid_info(hid, "Force feedback support loaded (firmware release %d).\n",
		 version);

	return 0;
}

/* ************************************************************************** */
/*                                                                            */
/* Device Support                                                             */
/*                                                                            */
/* ************************************************************************** */

/* -------------------------------------------------------------------------- */
/* Touchpad HID++ devices                                                     */
/* -------------------------------------------------------------------------- */

#define WTP_MANUAL_RESOLUTION				39

struct wtp_data {
	u16 x_size, y_size;
	u8 finger_count;
	u8 mt_feature_index;
	u8 button_feature_index;
	u8 maxcontacts;
	bool flip_y;
	unsigned int resolution;
};

static int wtp_input_mapping(struct hid_device *hdev, struct hid_input *hi,
		struct hid_field *field, struct hid_usage *usage,
		unsigned long **bit, int *max)
{
	return -1;
}

static void wtp_populate_input(struct hidpp_device *hidpp,
			       struct input_dev *input_dev)
{
	struct wtp_data *wd = hidpp->private_data;

	__set_bit(EV_ABS, input_dev->evbit);
	__set_bit(EV_KEY, input_dev->evbit);
	__clear_bit(EV_REL, input_dev->evbit);
	__clear_bit(EV_LED, input_dev->evbit);

	input_set_abs_params(input_dev, ABS_MT_POSITION_X, 0, wd->x_size, 0, 0);
	input_abs_set_res(input_dev, ABS_MT_POSITION_X, wd->resolution);
	input_set_abs_params(input_dev, ABS_MT_POSITION_Y, 0, wd->y_size, 0, 0);
	input_abs_set_res(input_dev, ABS_MT_POSITION_Y, wd->resolution);

	/* Max pressure is not given by the devices, pick one */
	input_set_abs_params(input_dev, ABS_MT_PRESSURE, 0, 50, 0, 0);

	input_set_capability(input_dev, EV_KEY, BTN_LEFT);

	if (hidpp->quirks & HIDPP_QUIRK_WTP_PHYSICAL_BUTTONS)
		input_set_capability(input_dev, EV_KEY, BTN_RIGHT);
	else
		__set_bit(INPUT_PROP_BUTTONPAD, input_dev->propbit);

	input_mt_init_slots(input_dev, wd->maxcontacts, INPUT_MT_POINTER |
		INPUT_MT_DROP_UNUSED);
}

static void wtp_touch_event(struct hidpp_device *hidpp,
	struct hidpp_touchpad_raw_xy_finger *touch_report)
{
	struct wtp_data *wd = hidpp->private_data;
	int slot;

	if (!touch_report->finger_id || touch_report->contact_type)
		/* no actual data */
		return;

	slot = input_mt_get_slot_by_key(hidpp->input, touch_report->finger_id);

	input_mt_slot(hidpp->input, slot);
	input_mt_report_slot_state(hidpp->input, MT_TOOL_FINGER,
					touch_report->contact_status);
	if (touch_report->contact_status) {
		input_event(hidpp->input, EV_ABS, ABS_MT_POSITION_X,
				touch_report->x);
		input_event(hidpp->input, EV_ABS, ABS_MT_POSITION_Y,
				wd->flip_y ? wd->y_size - touch_report->y :
					     touch_report->y);
		input_event(hidpp->input, EV_ABS, ABS_MT_PRESSURE,
				touch_report->area);
	}
}

static void wtp_send_raw_xy_event(struct hidpp_device *hidpp,
		struct hidpp_touchpad_raw_xy *raw)
{
	int i;

	for (i = 0; i < 2; i++)
		wtp_touch_event(hidpp, &(raw->fingers[i]));

	if (raw->end_of_frame &&
	    !(hidpp->quirks & HIDPP_QUIRK_WTP_PHYSICAL_BUTTONS))
		input_event(hidpp->input, EV_KEY, BTN_LEFT, raw->button);

	if (raw->end_of_frame || raw->finger_count <= 2) {
		input_mt_sync_frame(hidpp->input);
		input_sync(hidpp->input);
	}
}

static int wtp_mouse_raw_xy_event(struct hidpp_device *hidpp, u8 *data)
{
	struct wtp_data *wd = hidpp->private_data;
	u8 c1_area = ((data[7] & 0xf) * (data[7] & 0xf) +
		      (data[7] >> 4) * (data[7] >> 4)) / 2;
	u8 c2_area = ((data[13] & 0xf) * (data[13] & 0xf) +
		      (data[13] >> 4) * (data[13] >> 4)) / 2;
	struct hidpp_touchpad_raw_xy raw = {
		.timestamp = data[1],
		.fingers = {
			{
				.contact_type = 0,
				.contact_status = !!data[7],
				.x = get_unaligned_le16(&data[3]),
				.y = get_unaligned_le16(&data[5]),
				.z = c1_area,
				.area = c1_area,
				.finger_id = data[2],
			}, {
				.contact_type = 0,
				.contact_status = !!data[13],
				.x = get_unaligned_le16(&data[9]),
				.y = get_unaligned_le16(&data[11]),
				.z = c2_area,
				.area = c2_area,
				.finger_id = data[8],
			}
		},
		.finger_count = wd->maxcontacts,
		.spurious_flag = 0,
		.end_of_frame = (data[0] >> 7) == 0,
		.button = data[0] & 0x01,
	};

	wtp_send_raw_xy_event(hidpp, &raw);

	return 1;
}

static int wtp_raw_event(struct hid_device *hdev, u8 *data, int size)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct wtp_data *wd = hidpp->private_data;
	struct hidpp_report *report = (struct hidpp_report *)data;
	struct hidpp_touchpad_raw_xy raw;

	if (!wd || !hidpp->input)
		return 1;

	switch (data[0]) {
	case 0x02:
		if (size < 2) {
			hid_err(hdev, "Received HID report of bad size (%d)",
				size);
			return 1;
		}
		if (hidpp->quirks & HIDPP_QUIRK_WTP_PHYSICAL_BUTTONS) {
			input_event(hidpp->input, EV_KEY, BTN_LEFT,
					!!(data[1] & 0x01));
			input_event(hidpp->input, EV_KEY, BTN_RIGHT,
					!!(data[1] & 0x02));
			input_sync(hidpp->input);
			return 0;
		} else {
			if (size < 21)
				return 1;
			return wtp_mouse_raw_xy_event(hidpp, &data[7]);
		}
	case REPORT_ID_HIDPP_LONG:
		/* size is already checked in hidpp_raw_event. */
		if ((report->fap.feature_index != wd->mt_feature_index) ||
		    (report->fap.funcindex_clientid != EVENT_TOUCHPAD_RAW_XY))
			return 1;
		hidpp_touchpad_raw_xy_event(hidpp, data + 4, &raw);

		wtp_send_raw_xy_event(hidpp, &raw);
		return 0;
	}

	return 0;
}

static int wtp_get_config(struct hidpp_device *hidpp)
{
	struct wtp_data *wd = hidpp->private_data;
	struct hidpp_touchpad_raw_info raw_info = {0};
	int ret;

	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_TOUCHPAD_RAW_XY,
		&wd->mt_feature_index);
	if (ret)
		/* means that the device is not powered up */
		return ret;

	ret = hidpp_touchpad_get_raw_info(hidpp, wd->mt_feature_index,
		&raw_info);
	if (ret)
		return ret;

	wd->x_size = raw_info.x_size;
	wd->y_size = raw_info.y_size;
	wd->maxcontacts = raw_info.maxcontacts;
	wd->flip_y = raw_info.origin == TOUCHPAD_RAW_XY_ORIGIN_LOWER_LEFT;
	wd->resolution = raw_info.res;
	if (!wd->resolution)
		wd->resolution = WTP_MANUAL_RESOLUTION;

	return 0;
}

static int wtp_allocate(struct hid_device *hdev, const struct hid_device_id *id)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct wtp_data *wd;

	wd = devm_kzalloc(&hdev->dev, sizeof(struct wtp_data),
			GFP_KERNEL);
	if (!wd)
		return -ENOMEM;

	hidpp->private_data = wd;

	return 0;
};

static int wtp_connect(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct wtp_data *wd = hidpp->private_data;
	int ret;

	if (!wd->x_size) {
		ret = wtp_get_config(hidpp);
		if (ret) {
			hid_err(hdev, "Can not get wtp config: %d\n", ret);
			return ret;
		}
	}

	return hidpp_touchpad_set_raw_report_state(hidpp, wd->mt_feature_index,
			true, true);
}

/* ------------------------------------------------------------------------- */
/* Logitech M560 devices                                                     */
/* ------------------------------------------------------------------------- */

/*
 * Logitech M560 protocol overview
 *
 * The Logitech M560 mouse, is designed for windows 8. When the middle and/or
 * the sides buttons are pressed, it sends some keyboard keys events
 * instead of buttons ones.
 * To complicate things further, the middle button keys sequence
 * is different from the odd press and the even press.
 *
 * forward button -> Super_R
 * backward button -> Super_L+'d' (press only)
 * middle button -> 1st time: Alt_L+SuperL+XF86TouchpadOff (press only)
 *                  2nd time: left-click (press only)
 * NB: press-only means that when the button is pressed, the
 * KeyPress/ButtonPress and KeyRelease/ButtonRelease events are generated
 * together sequentially; instead when the button is released, no event is
 * generated !
 *
 * With the command
 *	10<xx>0a 3500af03 (where <xx> is the mouse id),
 * the mouse reacts differently:
 * - it never sends a keyboard key event
 * - for the three mouse button it sends:
 *	middle button               press   11<xx>0a 3500af00...
 *	side 1 button (forward)     press   11<xx>0a 3500b000...
 *	side 2 button (backward)    press   11<xx>0a 3500ae00...
 *	middle/side1/side2 button   release 11<xx>0a 35000000...
 */

static const u8 m560_config_parameter[] = {0x00, 0xaf, 0x03};

/* how buttons are mapped in the report */
#define M560_MOUSE_BTN_LEFT		0x01
#define M560_MOUSE_BTN_RIGHT		0x02
#define M560_MOUSE_BTN_WHEEL_LEFT	0x08
#define M560_MOUSE_BTN_WHEEL_RIGHT	0x10

#define M560_SUB_ID			0x0a
#define M560_BUTTON_MODE_REGISTER	0x35

static int m560_send_config_command(struct hid_device *hdev)
{
	struct hidpp_report response;
	struct hidpp_device *hidpp_dev;

	hidpp_dev = hid_get_drvdata(hdev);

	return hidpp_send_rap_command_sync(
		hidpp_dev,
		REPORT_ID_HIDPP_SHORT,
		M560_SUB_ID,
		M560_BUTTON_MODE_REGISTER,
		(u8 *)m560_config_parameter,
		sizeof(m560_config_parameter),
		&response
	);
}

static int m560_raw_event(struct hid_device *hdev, u8 *data, int size)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	/* sanity check */
	if (!hidpp->input) {
		hid_err(hdev, "error in parameter\n");
		return -EINVAL;
	}

	if (size < 7) {
		hid_err(hdev, "error in report\n");
		return 0;
	}

	if (data[0] == REPORT_ID_HIDPP_LONG &&
	    data[2] == M560_SUB_ID && data[6] == 0x00) {
		/*
		 * m560 mouse report for middle, forward and backward button
		 *
		 * data[0] = 0x11
		 * data[1] = device-id
		 * data[2] = 0x0a
		 * data[5] = 0xaf -> middle
		 *	     0xb0 -> forward
		 *	     0xae -> backward
		 *	     0x00 -> release all
		 * data[6] = 0x00
		 */

		switch (data[5]) {
		case 0xaf:
			input_report_key(hidpp->input, BTN_MIDDLE, 1);
			break;
		case 0xb0:
			input_report_key(hidpp->input, BTN_FORWARD, 1);
			break;
		case 0xae:
			input_report_key(hidpp->input, BTN_BACK, 1);
			break;
		case 0x00:
			input_report_key(hidpp->input, BTN_BACK, 0);
			input_report_key(hidpp->input, BTN_FORWARD, 0);
			input_report_key(hidpp->input, BTN_MIDDLE, 0);
			break;
		default:
			hid_err(hdev, "error in report\n");
			return 0;
		}
		input_sync(hidpp->input);

	} else if (data[0] == 0x02) {
		/*
		 * Logitech M560 mouse report
		 *
		 * data[0] = type (0x02)
		 * data[1..2] = buttons
		 * data[3..5] = xy
		 * data[6] = wheel
		 */

		int v;

		input_report_key(hidpp->input, BTN_LEFT,
			!!(data[1] & M560_MOUSE_BTN_LEFT));
		input_report_key(hidpp->input, BTN_RIGHT,
			!!(data[1] & M560_MOUSE_BTN_RIGHT));

		if (data[1] & M560_MOUSE_BTN_WHEEL_LEFT) {
			input_report_rel(hidpp->input, REL_HWHEEL, -1);
			input_report_rel(hidpp->input, REL_HWHEEL_HI_RES,
					 -120);
		} else if (data[1] & M560_MOUSE_BTN_WHEEL_RIGHT) {
			input_report_rel(hidpp->input, REL_HWHEEL, 1);
			input_report_rel(hidpp->input, REL_HWHEEL_HI_RES,
					 120);
		}

		v = sign_extend32(hid_field_extract(hdev, data + 3, 0, 12), 11);
		input_report_rel(hidpp->input, REL_X, v);

		v = sign_extend32(hid_field_extract(hdev, data + 3, 12, 12), 11);
		input_report_rel(hidpp->input, REL_Y, v);

		v = sign_extend32(data[6], 7);
		if (v != 0)
			hidpp_scroll_counter_handle_scroll(hidpp->input,
					&hidpp->vertical_wheel_counter, v);

		input_sync(hidpp->input);
	}

	return 1;
}

static void m560_populate_input(struct hidpp_device *hidpp,
				struct input_dev *input_dev)
{
	__set_bit(EV_KEY, input_dev->evbit);
	__set_bit(BTN_MIDDLE, input_dev->keybit);
	__set_bit(BTN_RIGHT, input_dev->keybit);
	__set_bit(BTN_LEFT, input_dev->keybit);
	__set_bit(BTN_BACK, input_dev->keybit);
	__set_bit(BTN_FORWARD, input_dev->keybit);

	__set_bit(EV_REL, input_dev->evbit);
	__set_bit(REL_X, input_dev->relbit);
	__set_bit(REL_Y, input_dev->relbit);
	__set_bit(REL_WHEEL, input_dev->relbit);
	__set_bit(REL_HWHEEL, input_dev->relbit);
	__set_bit(REL_WHEEL_HI_RES, input_dev->relbit);
	__set_bit(REL_HWHEEL_HI_RES, input_dev->relbit);
}

static int m560_input_mapping(struct hid_device *hdev, struct hid_input *hi,
		struct hid_field *field, struct hid_usage *usage,
		unsigned long **bit, int *max)
{
	return -1;
}

/* ------------------------------------------------------------------------- */
/* Logitech K400 devices                                                     */
/* ------------------------------------------------------------------------- */

/*
 * The Logitech K400 keyboard has an embedded touchpad which is seen
 * as a mouse from the OS point of view. There is a hardware shortcut to disable
 * tap-to-click but the setting is not remembered accross reset, annoying some
 * users.
 *
 * We can toggle this feature from the host by using the feature 0x6010:
 * Touchpad FW items
 */

struct k400_private_data {
	u8 feature_index;
};

static int k400_disable_tap_to_click(struct hidpp_device *hidpp)
{
	struct k400_private_data *k400 = hidpp->private_data;
	struct hidpp_touchpad_fw_items items = {};
	int ret;

	if (!k400->feature_index) {
		ret = hidpp_root_get_feature(hidpp,
			HIDPP_PAGE_TOUCHPAD_FW_ITEMS,
			&k400->feature_index);
		if (ret)
			/* means that the device is not powered up */
			return ret;
	}

	ret = hidpp_touchpad_fw_items_set(hidpp, k400->feature_index, &items);
	if (ret)
		return ret;

	return 0;
}

static int k400_allocate(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct k400_private_data *k400;

	k400 = devm_kzalloc(&hdev->dev, sizeof(struct k400_private_data),
			    GFP_KERNEL);
	if (!k400)
		return -ENOMEM;

	hidpp->private_data = k400;

	return 0;
};

static int k400_connect(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	if (!disable_tap_to_click)
		return 0;

	return k400_disable_tap_to_click(hidpp);
}

/* ------------------------------------------------------------------------- */
/* Logitech G920 Driving Force Racing Wheel for Xbox One                     */
/* ------------------------------------------------------------------------- */

#define HIDPP_PAGE_G920_FORCE_FEEDBACK			0x8123

static int g920_ff_set_autocenter(struct hidpp_device *hidpp,
				  struct hidpp_ff_private_data *data)
{
	struct hidpp_report response;
	u8 params[HIDPP_AUTOCENTER_PARAMS_LENGTH] = {
		[1] = HIDPP_FF_EFFECT_SPRING | HIDPP_FF_EFFECT_AUTOSTART,
	};
	int ret;

	/* initialize with zero autocenter to get wheel in usable state */

	dbg_hid("Setting autocenter to 0.\n");
	ret = hidpp_send_fap_command_sync(hidpp, data->feature_index,
					  HIDPP_FF_DOWNLOAD_EFFECT,
					  params, ARRAY_SIZE(params),
					  &response);
	if (ret)
		hid_warn(hidpp->hid_dev, "Failed to autocenter device!\n");
	else
		data->slot_autocenter = response.fap.params[0];

	return ret;
}

static int g920_get_config(struct hidpp_device *hidpp,
			   struct hidpp_ff_private_data *data)
{
	struct hidpp_report response;
	int ret;

	memset(data, 0, sizeof(*data));

	/* Find feature and store for later use */
	ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_G920_FORCE_FEEDBACK,
				     &data->feature_index);
	if (ret)
		return ret;

	/* Read number of slots available in device */
	ret = hidpp_send_fap_command_sync(hidpp, data->feature_index,
					  HIDPP_FF_GET_INFO,
					  NULL, 0,
					  &response);
	if (ret) {
		if (ret < 0)
			return ret;
		hid_err(hidpp->hid_dev,
			"%s: received protocol error 0x%02x\n", __func__, ret);
		return -EPROTO;
	}

	data->num_effects = response.fap.params[0] - HIDPP_FF_RESERVED_SLOTS;

	/* reset all forces */
	ret = hidpp_send_fap_command_sync(hidpp, data->feature_index,
					  HIDPP_FF_RESET_ALL,
					  NULL, 0,
					  &response);
	if (ret)
		hid_warn(hidpp->hid_dev, "Failed to reset all forces!\n");

	ret = hidpp_send_fap_command_sync(hidpp, data->feature_index,
					  HIDPP_FF_GET_APERTURE,
					  NULL, 0,
					  &response);
	if (ret) {
		hid_warn(hidpp->hid_dev,
			 "Failed to read range from device!\n");
	}
	/* Direct-drive wheels default to 1080, belt-driven to 900 */
	if (ret) {
		if (hidpp->hid_dev->product == USB_DEVICE_ID_LOGITECH_RS50 ||
		    hidpp->hid_dev->product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
		    hidpp->hid_dev->product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
			data->range = 1080;
		else
			data->range = 900;
	} else {
		data->range = get_unaligned_be16(&response.fap.params[0]);
	}

	/* Read the current gain values */
	ret = hidpp_send_fap_command_sync(hidpp, data->feature_index,
					  HIDPP_FF_GET_GLOBAL_GAINS,
					  NULL, 0,
					  &response);
	if (ret)
		hid_warn(hidpp->hid_dev,
			 "Failed to read gain values from device!\n");
	data->gain = ret ?
		0xffff : get_unaligned_be16(&response.fap.params[0]);

	/* ignore boost value at response.fap.params[2] */

	return g920_ff_set_autocenter(hidpp, data);
}

/*
 * Multi-interface direct-drive wheels (G Pro) can hit a race where
 * interface 1 (HID++) finishes probing before interface 0's hid_connect
 * populates its inputs list, which happens on kernels with async USB
 * interface probing. hidpp_ff_init's sibling walk then sees interface 0
 * with no inputs and returns -ENODEV, leaving the joystick evdev without
 * FF registered. Retry a handful of times with a short delay; by the
 * second or third tick the sibling's inputs are invariably populated.
 *
 * Cap so we don't spin forever on a genuine no-inputs device.
 */
#define HIDPP_FF_MAX_INIT_RETRIES	20
#define HIDPP_FF_INIT_RETRY_MS		50

static void hidpp_ff_retry_work(struct work_struct *work)
{
	struct hidpp_device *hidpp =
		container_of(to_delayed_work(work), struct hidpp_device,
			     ff_retry_work);
	struct hidpp_ff_private_data data;
	int ret;

	ret = g920_get_config(hidpp, &data);
	if (ret) {
		hid_warn(hidpp->hid_dev,
			 "FF retry %d: g920_get_config failed: %d (giving up)\n",
			 hidpp->ff_retries, ret);
		return;
	}

	ret = hidpp_ff_init(hidpp, &data);
	if (ret == -ENODEV &&
	    hidpp->ff_retries++ < HIDPP_FF_MAX_INIT_RETRIES) {
		queue_delayed_work(system_long_wq, &hidpp->ff_retry_work,
				   msecs_to_jiffies(HIDPP_FF_INIT_RETRY_MS));
		return;
	}
	if (ret)
		hid_warn(hidpp->hid_dev,
			 "FF retry: giving up after %d attempts (errno %d)\n",
			 hidpp->ff_retries, ret);
	else
		hid_info(hidpp->hid_dev,
			 "FF retry: succeeded after %d attempts\n",
			 hidpp->ff_retries);
}

/* -------------------------------------------------------------------------- */
/* Logitech direct-drive wheels: FFB + settings (RS50, G PRO)                 */
/* -------------------------------------------------------------------------- */

/*
 * The direct-drive wheels (RS50, G Pro) use a completely different FFB architecture than G920/G923.
 * Instead of HID++ feature 0x8123, it uses dedicated endpoint 0x03 OUT
 * with raw 64-byte output reports for real-time force feedback.
 *
 * FFB commands are sent via a workqueue to avoid blocking in callback context.
 */

#define HIDPP_DD_FF_REPORT_ID		0x01
#define HIDPP_DD_FF_EFFECT_CONSTANT		0x01
#define HIDPP_DD_FF_REPORT_SIZE		64
#define HIDPP_DD_INPUT_REPORT_SIZE		30	/* Interface 0 joystick report */

/* Direct-drive FFB refresh command (sent periodically to maintain FFB state) */
#define HIDPP_DD_FF_REFRESH_ID		0x05
#define HIDPP_DD_FF_REFRESH_CMD		0x07
#define HIDPP_DD_FF_REFRESH_INTERVAL_MS	20000	/* 20 seconds */

/*
 * In-kernel TrueForce texture channel (KF/TF separation, issue #8).
 *
 * The wheel's interface-2 type-0x01 report carries BOTH force channels
 * in one packet: the bytes 6-9 preamble ("cur") is the held motor
 * torque target, and byte 10 ("new samples this packet") announces
 * additive audio content:
 *   - byte10 = 0: plain constant-force (KF) update - torque in cur,
 *     no audio. This is what struct hidpp_dd_ff_report has always sent.
 *   - byte10 = 4: unified stream packet - torque in cur PLUS bytes
 *     12..63 carrying a 13-slot rolling window of haptic samples (each
 *     u16 offset binary, duplicated L/R), advanced 4 slots per packet.
 *     The driver emits these at its 500 Hz effect tick (2 kHz slot
 *     rate; each 1 kHz synthesized sample fills two slots); games
 *     stream the same shape anywhere from 250 to ~1000 pkt/s, so slot
 *     consumption is packet-paced. See docs/TRUEFORCE_PROTOCOL.md.
 *
 * While texture is active the driver sends ONLY unified packets (the
 * separate force packet is skipped for that tick - one packet per tick
 * either way). Routing vibration-class evdev effects (FF_RUMBLE,
 * high-frequency FF_PERIODIC) through the window instead of summing
 * them into the steering force replicates the Windows KF/TF split:
 * texture rides the wheel's audio-haptic path and no longer modulates
 * ("grits") the steering axis. Continuous unified streaming is also
 * how AC EVO drives the wheel for entire sessions, which is the
 * evidence that a byte10=4 packet satisfies the firmware's host-alive
 * watchdog exactly like a KF packet (issue #16 unwind behaviour).
 *
 * The TF session needs a one-time init: the 68-packet sequence in
 * hidpp_dd_tf_init.h sent twice (G Hub behaviour). We run it lazily from a
 * workqueue the first time a texture effect actually plays, so wheels
 * that never see texture effects never see TF traffic either.
 */
#define HIDPP_DD_TF_CMD_STREAM		0x01	/* audio window packet */
#define HIDPP_DD_TF_CMD_START		0x03	/* start / play */
#define HIDPP_DD_TF_CMD_STOP		0x04	/* stop / clear */
#define HIDPP_DD_TF_WINDOW			13	/* rolling window slots */
#define HIDPP_DD_TF_NEW_SAMPLES		4	/* new samples per packet */
#define HIDPP_DD_TF_FLAG_BYTE		0x0d	/* constant per captures */
/*
 * Texture amplitude ceiling (post gain/strength scaling), half of full
 * scale. The wheel's DSP plays window samples as vibration below
 * roughly 0.5-0.7 of full scale but crosses into pulling the steering
 * axis above it (TF4ALL project measurements on G PRO; consistent with
 * our 49%-amplitude test buzz sitting at the audible-but-not-steering
 * boundary). Real SDK games stream tiny amplitudes (ACC median 123 of
 * 32767) so this cap never touches game-shaped content - it only stops
 * a synthetic full-scale FF_RUMBLE from hijacking steering torque.
 */
#define HIDPP_DD_TF_MAX_AMPLITUDE		16383
/*
 * Crossover between "steering-shaping" and "texture" periodics: period
 * at or below this (>= 20 Hz) is texture and routes to TF; slower
 * periodic effects keep contributing to the steering force. FF_RUMBLE
 * is always texture.
 */
#define HIDPP_DD_TF_CROSSOVER_PERIOD_MS	50

/* texture_route values */
#define HIDPP_DD_TEXTURE_ROUTE_KF		0	/* legacy: sum into steering */
#define HIDPP_DD_TEXTURE_ROUTE_TF		1	/* stream via TrueForce */

/* Hard-failure cap for the lazy TF session init before giving up. */
#define HIDPP_DD_TF_INIT_MAX_ATTEMPTS	3

/*
 * Direct-drive wheel HID++ feature PAGE IDs for wheel settings.
 * These are used with hidpp_root_get_feature() to discover the actual
 * feature indices, which vary per device. Never use hardcoded indices!
 */
#define HIDPP_DD_PAGE_BRIGHTNESS		0x8040	/* LED Brightness Control */
/*
 * 0x807A speaks two per-model dialects: the RS50 rim's per-LED RGB
 * LIGHTSYNC protocol (constants below, section 9 of the spec) and the
 * real G PRO rim's level-based rev lights (HIDPP_DD_REV_* next to
 * wheel_rev_level_store). Changes to either side must be checked
 * against the other - they share the feature page and index.
 */
#define HIDPP_DD_PAGE_LIGHTSYNC		0x807A	/* LIGHTSYNC LED Effects */
#define HIDPP_DD_PAGE_RGB_CONFIG		0x807B	/* RGB Zone Config (LED color data) */
#define HIDPP_DD_PAGE_PROFILE_NOTIFY	0x80D0	/* Emits profile-change broadcast event */
#define HIDPP_DD_PAGE_DAMPING		0x8133	/* Wheel Damping */
#define HIDPP_DD_PAGE_BRAKEFORCE		0x8134	/* Brake Force Threshold */
#define HIDPP_DD_PAGE_STRENGTH		0x8136	/* FFB Strength */
#define HIDPP_DD_PAGE_PROFILE		0x8137	/* Profile Switching */
#define HIDPP_DD_PAGE_RANGE			0x8138	/* Rotation Range (emits rotation-change broadcast event) */
#define HIDPP_DD_PAGE_TRUEFORCE		0x8139	/* TRUEFORCE Bass Shaker */
#define HIDPP_DD_PAGE_FILTER		0x8140	/* FFB Filter */
#define HIDPP_DD_PAGE_RESPONSE_CURVE	0x80A4	/* Per-axis 64-point response curves */
#define HIDPP_DD_PEDAL_DEV_IDX		0x02	/* HID++ sub-device index of the pedal unit (0x80A4 axis curves) */
#define HIDPP_DD_PAGE_CALIBRATE		0x812C	/* Centre calibration (G Pro sub-device 0x05) */
#define HIDPP_DD_PAGE_SYNC			0x1BC0	/* Unknown sync/prepare feature */

/*
 * RS50 HID descriptor declares buttons 1-92 but only ~20 are physically present.
 * Buttons >= 81 (0x51) overflow past valid Linux input codes (max 767), causing
 * "Invalid code" kernel messages. We ignore these phantom buttons during input
 * mapping.
 */
#define HIDPP_DD_MAX_BUTTON_USAGE	0x50	/* Accept buttons 1-80, ignore 81+ */

/* Direct-drive wheel HID++ function IDs for settings */
#define HIDPP_DD_HIDPP_FN_GET_INFO		0x00	/* Function 0: get capabilities/limits */
#define HIDPP_DD_HIDPP_FN_GET		0x10	/* Function 1: get current value */
#define HIDPP_DD_HIDPP_FN_SET		0x20	/* Function 2: set value */

/*
 * RS50 LIGHTSYNC LED Effects (feature page 0x807A)
 * Supports 5 custom slots with 10 individually addressable RGB LEDs each.
 * Each slot has a direction (animation style) and per-LED color config.
 */
#define HIDPP_DD_LIGHTSYNC_NUM_LEDS		10	/* Physical LEDs on the wheel */
#define HIDPP_DD_LIGHTSYNC_NUM_SLOTS	5	/* Custom slots (CUSTOM 1-5) */

/*
 * LIGHTSYNC function codes.
 * Note: Same function numbers have different meanings on features 0x0B vs 0x0C!
 *
 * Feature 0x0B (LIGHTSYNC effect control):
 *   - Fn 3 (0x3C): Set effect mode (1-5)
 *   - Fn 6 (0x6C): Enable/disable LED subsystem
 *
 * Feature 0x0C (RGB Zone Config):
 *   - Fn 1 (0x1C): GetConfig (read slot data)
 *   - Fn 2 (0x2C): SetConfig (write RGB colors)
 *   - Fn 3 (0x3C): Activate slot
 *   - Fn 4 (0x4C): Set slot name
 */
/*
 * Feature 0x0B (LIGHTSYNC) functions.
 * Values are function_number << 4, with sw_id added by hidpp_send_fap_command_sync.
 * G Hub coldstart queries fn0/fn1/fn2 before fn4/fn7 - device may need this init.
 */
#define HIDPP_DD_LIGHTSYNC_FN_GET_INFO	0x00	/* fn0: Get feature info */
#define HIDPP_DD_LIGHTSYNC_FN_GET_CAPS	0x10	/* fn1: Get capabilities */
#define HIDPP_DD_LIGHTSYNC_FN_GET_STATE	0x20	/* fn2: Get current state */
#define HIDPP_DD_LIGHTSYNC_FN_SET_EFFECT	0x30	/* fn3: Set effect mode */
#define HIDPP_DD_LIGHTSYNC_FN_SET_LEDS	0x40	/* fn4: Set LED count/config */
#define HIDPP_DD_LIGHTSYNC_FN_SET_CONFIG	0x60	/* fn6: Set effect config (LONG report) */
#define HIDPP_DD_LIGHTSYNC_FN_ENABLE	0x70	/* fn7: Enable LED display/preview */

/*
 * Feature 0x0C (RGB Config) functions.
 * Values are function_number << 4, with sw_id added by hidpp_send_fap_command_sync.
 */
#define HIDPP_DD_RGB_FN_GET_CONFIG		0x10	/* fn1: Get slot config */
#define HIDPP_DD_RGB_FN_SET_CONFIG		0x20	/* fn2: Set RGB colors (VERY_LONG) */
#define HIDPP_DD_RGB_FN_GET_NAME		0x30	/* fn3: Get slot name (also activates) */
#define HIDPP_DD_RGB_FN_ACTIVATE		0x30	/* fn3: Activate slot (same as GET_NAME) */
#define HIDPP_DD_RGB_FN_SET_NAME		0x40	/* fn4: Set slot name */
#define HIDPP_DD_RGB_FN_PRE_CONFIG		0x60	/* fn6: Pre-config before RGB data */
#define HIDPP_DD_RGB_FN_COMMIT		0x70	/* fn7: Commit after RGB data */

/* LIGHTSYNC direction values (animation effect direction) */
#define HIDPP_DD_LIGHTSYNC_DIR_LEFT_RIGHT	0	/* Left to Right sweep */
#define HIDPP_DD_LIGHTSYNC_DIR_RIGHT_LEFT	1	/* Right to Left sweep */
#define HIDPP_DD_LIGHTSYNC_DIR_INSIDE_OUT	2	/* Center outward (expand) */
#define HIDPP_DD_LIGHTSYNC_DIR_OUTSIDE_IN	3	/* Edges inward (contract) */

/* LIGHTSYNC per-slot configuration */
#define HIDPP_DD_SLOT_NAME_MAX_LEN	8	/* Max slot name length (from device info) */

/*
 * Slot name SET uses fn4 on 0x0C with payload { slot, len, name[len] }.
 * That's 2 + HIDPP_DD_SLOT_NAME_MAX_LEN = 10 bytes, which must fit in the
 * params[16] buffer used by the store handler. Keep the assertion near
 * the define so growing the name cap without widening the buffer trips
 * at build time.
 */
static_assert(2 + HIDPP_DD_SLOT_NAME_MAX_LEN <= 16,
	      "slot-name wire payload must fit in the 16-byte params buffer");

struct hidpp_dd_lightsync_slot {
	u8 direction;			/* Direction/animation style (0-3) */
	u8 brightness;			/* Per-slot brightness (0-100) */
	char name[HIDPP_DD_SLOT_NAME_MAX_LEN + 1];	/* Slot name + null terminator */
	u8 colors[HIDPP_DD_LIGHTSYNC_NUM_LEDS * 3]; /* RGB for each LED (30 bytes) */
};

/* Marker for features that weren't discovered (not supported by device) */
#define HIDPP_DD_FEATURE_NOT_FOUND		0xFF

/* Direct-drive FFB constants */
/*
 * Maximum simultaneous FFB effect slots advertised to userspace via
 * input_ff_create(). The kernel input ff-core uses this to size its
 * effect_owners[] table; once full, EVIOCSFF returns -ENOSPC and
 * userspace can't upload more effects until it explicitly erases or
 * closes the fd.
 *
 * The value is purely a software limit on this driver's side - the
 * wheel firmware does not have a concept of effect slots in our
 * dedicated-endpoint FFB protocol, all effects are mixed in software
 * in our timer callback. We pick 63 to match what the upstream
 * G920/G923 path advertises (so userspace tools that probe
 * num_effects see the same number on both code paths) and to give
 * pumper-style test programs (ffmvforce, which uploads a fresh
 * effect per click without erasing) more headroom before they
 * exhaust slots and stop working.
 */
#define HIDPP_DD_FF_MAX_EFFECTS		63
#define HIDPP_DD_FF_TIMER_INTERVAL_MS	2	/* 500 Hz update rate */

/*
 * FRICTION stick-zone half-width, in encoder counts per timer tick.
 * Inside +/- this velocity the emulated friction force ramps linearly
 * instead of stepping to full scale (Karnopp model; see the FF_FRICTION
 * case in hidpp_dd_ff_effect_tick). 8 counts/tick ~= 22 deg/s at the default
 * 900-degree range - comfortably above encoder noise, well below any
 * deliberate steering motion.
 */
#define HIDPP_DD_FF_FRICTION_RAMP_COUNTS	8

/*
 * Default wheel_spring_damping percent (see the spring_damping field).
 * 25% of the spring's own coefficient is a conservative stabilising
 * ratio: enough to damp the latency-driven ringing observed with stiff
 * game-uploaded centring springs, small enough not to make springs feel
 * syrupy. Tunable at runtime; 0 disables (pre-2026-07 behaviour).
 */
#define HIDPP_DD_FF_SPRING_DAMPING_DEFAULT	25

/* Effect state tracking */
struct hidpp_dd_ff_effect {
	struct ff_effect effect;
	bool uploaded;
	bool playing;
	/*
	 * Wall-clock start of the current playback window (jiffies). Used by
	 * time-dependent effects (constant-with-envelope, ramp, periodic,
	 * replay duration). Set on hidpp_dd_ff_playback(value != 0), frozen on
	 * stop, irrelevant for condition effects (they read live wheel state).
	 */
	unsigned long play_start;
	/* Replay count remaining after the current window; 0 == one-shot. */
	int replays_left;
	/*
	 * Channel assignment for this playback: true = TrueForce texture
	 * stream, false = steering-force sum. Decided ONCE in
	 * hidpp_dd_ff_playback when the effect starts (route enabled, TF
	 * session ready, effect texture-class) and held stable for the
	 * whole play cycle, so neither the lazy TF init completing nor a
	 * mid-play SetParameters across the texture crossover can yank a
	 * live effect between channels (which stepped the steering force
	 * by the effect's amplitude in one 2 ms tick).
	 */
	bool use_tf;
};

/* Direct-drive FFB output report structure (64 bytes to endpoint 0x03) */
struct hidpp_dd_ff_report {
	u8 report_id;		/* 0x01 */
	u8 reserved[3];		/* 0x00, 0x00, 0x00 */
	u8 effect_type;		/* 0x01 = constant force */
	u8 sequence;		/* incrementing counter (single byte, wraps at 255) */
	__le16 force;		/* 0x0000=left, 0x8000=center, 0xFFFF=right */
	__le16 force_dup;	/* duplicate of force value */
	u8 padding[54];		/* zeros */
} __packed;

static_assert(sizeof(struct hidpp_dd_ff_report) == HIDPP_DD_FF_REPORT_SIZE,
	      "DD FFB report structure size mismatch");

/* Direct-drive FFB work item for async USB transfers */
struct hidpp_dd_ff_work {
	struct work_struct work;
	struct hidpp_dd_ff_data *ff_data;
	u16 force;
	/*
	 * When set, report_buf was pre-built by the queuer (TrueForce
	 * stream/control packets) and is sent verbatim; when clear, the
	 * handler builds the classic constant-force report from `force`.
	 */
	bool raw;
	/*
	 * Per-work DMA-safe buffer for USB transfer.
	 * This avoids race conditions where hid_hw_output_report() returns
	 * before the USB transfer completes, and another work item could
	 * overwrite a shared buffer while it's still being DMA'd.
	 */
	u8 report_buf[HIDPP_DD_FF_REPORT_SIZE];
};

/* Direct-drive FFB private data */
struct hidpp_dd_ff_data {
	struct hidpp_device *hidpp;
	struct hidpp_device *owner_hidpp;/* hidpp that allocated this ff_data */
	struct hid_device *ff_hdev;	/* hid_device for interface 2 (FFB) */
	struct input_dev *input;
	struct workqueue_struct *wq;	/* Workqueue for async USB transfers */
	struct delayed_work init_work;	/* Deferred initialization */
	int init_retries;		/* Init retry counter */
	struct delayed_work refresh_work; /* Periodic FFB refresh (05 07 cmd) */
	struct work_struct settings_refresh_work; /* Re-query device settings after profile change */
	struct timer_list effect_timer;	/* Timer for continuous FFB updates */
	atomic_t sequence;
	atomic_t pending_work;		/* Number of pending work items */
	atomic_t stopping;		/* Set when driver is shutting down */
	atomic_t initialized;		/* FFB fully initialized */
	unsigned long last_err_log;	/* Timestamp of last error log */
	int err_count;			/* Error count for throttling */

	/*
	 * HID++ feature indices - discovered via hidpp_root_get_feature().
	 * Set to HIDPP_DD_FEATURE_NOT_FOUND (0xFF) if feature not supported.
	 */
	u8 idx_range;			/* Feature index for rotation range */
	u8 idx_strength;		/* Feature index for FFB strength */
	u8 idx_damping;			/* Feature index for damping */
	u8 idx_trueforce;		/* Feature index for TRUEFORCE */
	u8 idx_brakeforce;		/* Feature index for brake force */
	u8 idx_filter;			/* Feature index for FFB filter */
	u8 idx_response_curve;		/* Feature index for 0x80A4 axis response curves (steering, base dev 0xff) */
	u8 idx_pedal_curve;		/* Feature index for 0x80A4 curves on the pedal unit (dev 0x02, axes 0-2) */
	u8 idx_brightness;		/* Feature index for LED brightness */
	u8 idx_lightsync;		/* Feature index for LIGHTSYNC effects */
	u8 idx_rgb_config;		/* Feature index for RGB Zone Config */
	/*
	 * Real-G-PRO rev-light state (level-based 0x807A protocol; see
	 * wheel_rev_level_store). Serialised by rev_lock: sysfs stores queue
	 * rev_work, which owns every send; the arm burst must run exactly
	 * once. rev_target is the newest requested level (latest-value-wins);
	 * the worker always flushes the latest, never stale intermediates.
	 */
	struct mutex rev_lock;
	struct delayed_work rev_work;	/* coalescing flush; runs on system_unbound_wq */
	bool rev_armed;			/* one-time arm burst sent (rev_lock) */
	bool rev_err_logged;		/* worker: send-fail warned once this streak (rev_lock) */
	u8 rev_level;			/* last successfully commanded level 0-10 (reported by _show) */
	u8 rev_target;			/* newest requested level, WRITE_ONCE/READ_ONCE */
	unsigned long rev_last_write;	/* jiffies of last level-pair attempt (rev_lock) */
	u8 idx_profile;			/* Feature index for Profile switching */
	u8 idx_profile_notify;		/* Feature index for profile-change broadcasts (0x80D0) */
	u8 idx_sync;			/* Feature index for sync/prepare (0x1BC0) */
	u8 idx_calibrate;		/* Feature index for centre calibration (G Pro sub-device 0x05, page 0x812C) */
	u8 calibrate_dev_idx;		/* HID++ device index used for calibrate sends (0x05 on G Pro) */
	u8 idx_compat_angle;		/* Compat-mode steering angle (HID++ feature 0x8138). Discovered lazily by hidpp_dd_compat_set_range. */
	u8 idx_compat_strength;		/* Compat-mode FFB strength (HID++ feature 0x8136). Discovered lazily by hidpp_dd_compat_set_strength. */
	u8 idx_compat_trueforce;	/* Compat-mode TRUEFORCE strength (HID++ feature 0x8139, fn 3). Discovered lazily by hidpp_dd_compat_set_trueforce. */
	u8 idx_compat_damping;		/* Compat-mode damping (HID++ feature 0x8133, fn 1; verified at fallback idx 0x14). Discovered lazily by hidpp_dd_compat_set_damping. */
	u8 idx_compat_filter;		/* Compat-mode FFB filter (HID++ feature 0x8140, fn 2; verified at fallback idx 0x1a). Discovered lazily by hidpp_dd_compat_set_filter. */

	/*
	 * Per-feature SET function numbers.
	 * The RS50 uses fn=2 (0x20) for all SET operations, but the G Pro
	 * uses different function numbers per feature (e.g. fn=1 for damping,
	 * fn=3 for TRUEFORCE). Defaults set in hidpp_dd_ff_init(); device-specific
	 * overrides applied during feature discovery.
	 */
	u8 fn_set_range;
	u8 fn_set_strength;
	u8 fn_set_damping;
	u8 fn_set_trueforce;
	u8 fn_set_brakeforce;
	u8 fn_set_filter;
	u8 fn_set_brightness;		/* feature 0x8040 SET fn; default HIDPP_DD_HIDPP_FN_SET */

	/* Mode and profile state (Feature 0x8137) */
	u8 current_mode;		/* 0=desktop, 1=onboard */
	u8 current_profile;		/* 0=desktop, 1-5=onboard profiles */
	bool mode_known;		/* true once hidpp_dd_get_current_mode succeeded at least once; false means current_mode/current_profile are the safe-desktop default not a fresh query */
	u8 sensitivity;			/* Last sensitivity written via sysfs (0-100, 50=linear); uploaded as a 0x80A4 steering curve, not readable back from the wheel */
	/*
	 * Pedal shaping generator inputs, indexed 0=throttle 1=brake 2=clutch.
	 * Like steering sensitivity, the wheel stores only the resulting 0x80A4
	 * curve, not these inputs, so the last value written is cached here for
	 * readback. pedal_sens defaults to 50 (linear); pedal_deadzone to 0/0.
	 */
	u8 pedal_sens[3];
	u8 pedal_deadzone[3][2];		/* [axis][0]=lower %, [axis][1]=upper % */

	/* Wheel settings (sysfs configurable) */
	u16 range;			/* rotation range in degrees */
	u16 strength;			/* FFB strength (0-65535) */
	u16 damping;			/* damping level (0-65535) */
	u16 trueforce;			/* TRUEFORCE level (0-65535) */
	u16 brake_force;		/* Brake Force threshold (0-65535) */
	u8 ffb_filter;			/* FFB filter level (1-15) */
	u8 ffb_filter_auto;		/* Auto FFB filter (0=off, 1=on) */
	u8 led_brightness;		/* LED brightness (0-100) */
	u8 brightness_caps;		/* x8040 getInfo capabilities byte */
	bool brightness_info_read;	/* fn0 getInfo probed once */

	/* Device identity (DeviceInfo 0x0003; read once at init) */
	char serial[13];		/* 12-char Base34 serial + NUL */
	char fw_main[16];		/* base firmware, e.g. "U1 65.03.B0038" */
	char fw_motor[16];		/* motor firmware (sub-device 0x05) */
	u8 led_effect;			/* LED effect mode (1-5, 5=custom) */

	/* LIGHTSYNC per-slot configuration (full RGB control) */
	u8 led_active_slot;		/* Currently selected slot (0-4) */
	struct hidpp_dd_lightsync_slot led_slots[HIDPP_DD_LIGHTSYNC_NUM_SLOTS];
	u8 ls_num_slots;          /* latched from 0x0C fn0; clamped <= NUM_SLOTS */
	u8 ls_num_leds;           /* latched from 0x0C fn0; clamped <= NUM_LEDS */

	/* Oversteer compatibility - stored locally, no hardware effect */
	/*
	 * Emulated autocenter: a driver-side centring spring summed into
	 * the effect timer's force whenever nonzero. Raw 0-65535 scale
	 * (the evdev FF_AUTOCENTER and Oversteer `autocenter` file
	 * convention). Replaces the earlier store-only stub.
	 */
	u16 autocenter;
	/*
	 * Per-effect-class output scales, 0-100 percent, default 100
	 * (the new-lg4ff / Oversteer convention: spring_level,
	 * damper_level, friction_level files). Applied to the emulated
	 * SPRING/DAMPER/FRICTION outputs in the effect tick.
	 */
	u8 spring_level;
	u8 damper_level;
	u8 friction_level;
	/*
	 * True once interface 0 has delivered at least one input report.
	 * Until then ff->wheel_pos is its kzalloc 0 ("hard left"), and
	 * anything position-fed (the autocenter spring) must stay quiet
	 * or it would yank an untouched wheel.
	 */
	bool wheel_pos_seen;

	/* FFB effects tracking */
	struct hidpp_dd_ff_effect effects[HIDPP_DD_FF_MAX_EFFECTS];
	spinlock_t effects_lock;	/* Protects effects array */
	s32 last_force;			/* Last force sent; used by playback() to know whether a release-to-zero packet is needed when all effects stop. */
	s32 constant_force;		/* Cached sum of currently-playing FF_CONSTANT contributions; condition/periodic/ramp effects are computed per-tick inside the timer callback. */

	/*
	 * Live wheel state used by condition-effect emulation (SPRING,
	 * DAMPER, FRICTION, INERTIA). Updated from the interface-0 raw
	 * input report handler at the wheel's native poll rate (roughly
	 * 500 Hz for these wheels). The timer callback reads these lock-free
	 * via READ_ONCE; writers use WRITE_ONCE. wheel_pos is raw encoder
	 * 0..65535 (0x8000 == centre). wheel_vel and wheel_accel are
	 * signed derivatives in encoder-counts per input sample, computed
	 * inside the FFB timer tick from successive wheel_pos samples.
	 */
	u16 wheel_pos;			/* latest raw encoder position, 0..65535 */
	u16 wheel_pos_prev;		/* previous sample (timer-local) */
	s32 wheel_vel;			/* encoder delta between consecutive timer ticks */
	s32 wheel_vel_prev;
	s32 wheel_accel;
	bool wheel_state_primed;	/* false until the timer has seen two samples */
	/*
	 * "any effect is currently playing" short-circuit. When false the
	 * timer stops rescheduling itself and the wheel stays idle. When
	 * true (set under effects_lock whenever an effect transitions to
	 * playing) the timer keeps firing at HIDPP_DD_FF_TIMER_INTERVAL_MS so
	 * condition effects get a live wheel-state sample each tick, even
	 * if the instantaneous force they compute happens to be zero.
	 */
	bool any_effect_playing;
	/*
	 * Sign toggle for the FF_CONSTANT level before we send it to the
	 * wheel. 1 (default) = invert, which matches the sign Wine/Proton
	 * produces for DirectInput games like ACC. 0 = pass-through,
	 * which matches Linux's documented evdev convention (direction
	 * 0x4000 east + positive level = rightward force) and is correct
	 * for native-evdev apps. Lockless via READ_ONCE / WRITE_ONCE;
	 * exposed to userspace as wheel_ffb_constant_sign (0 / 1).
	 */
	bool ffb_constant_sign;
	/*
	 * Synthetic damping for emulated SPRING effects, in percent (0-100)
	 * of a DAMPER running the spring's own coefficient. Our spring is a
	 * pure proportional controller closed over the timer -> workqueue ->
	 * USB path; that loop latency on a low-friction direct-drive motor
	 * makes a stiff undamped spring ring (grow-and-diverge oscillation
	 * until the wheel's over-torque failsafe cuts power - observed live
	 * with AC EVO map-load centring, 2026-06-30). Real wheels damp the
	 * spring inside the firmware servo loop; this term restores that
	 * behaviour. Lockless via READ_ONCE/WRITE_ONCE; exposed as
	 * wheel_spring_damping.
	 */
	u8 spring_damping;
	/*
	 * In-kernel TrueForce texture channel (see the HIDPP_DD_TF_* block for
	 * the design). texture_route selects where vibration-class effects
	 * go; the tf_* runtime state below is touched only from the effect
	 * timer callback (single-threaded) except tf_ready/tf_init_queued,
	 * which the lazy init work handler sets (READ_ONCE/WRITE_ONCE).
	 */
	u8 texture_route;		/* HIDPP_DD_TEXTURE_ROUTE_KF / _TF */
	bool tf_ready;			/* two-pass session init completed */
	bool tf_init_queued;		/* init work queued/running; cleared for retry on failure */
	bool tf_streaming;		/* between START and STOP */
	bool tf_recentre_sent;		/* wind-down recentre packet already out */
	u8 tf_seq;			/* TF stream sequence counter */
	u8 tf_init_attempts;		/* hard init failures so far (cap: HIDPP_DD_TF_INIT_MAX_ATTEMPTS) */
	u16 tf_window[HIDPP_DD_TF_WINDOW];	/* rolling window, offset binary */
	struct work_struct tf_init_work; /* runs the 2x68-packet init (system_unbound_wq) */
	/*
	 * Honest-range poll: re-reads the physical rotation range every
	 * HIDPP_DD_FF_REFRESH_INTERVAL_MS on system_unbound_wq, decoupled from
	 * the force-stream workqueue so its synchronous HID++ GET can
	 * never stall force delivery. Skipped while effects play.
	 */
	struct delayed_work range_poll_work;
	/*
	 * Auto-restore of externally-reset ranges (see
	 * hidpp_dd_ff_range_maybe_restore). Default on; strike counter caps
	 * restores at 3 per session and is reset by an explicit
	 * wheel_range write.
	 */
	bool range_restore;
	u8 range_restore_attempts;
	/*
	 * Nonzero = a restore is owed: the range the wheel had before an
	 * external reset to 90. Set at detection, retried on every poll
	 * tick until it succeeds / strikes out / becomes moot (poll-work
	 * context only). Cleared by an explicit wheel_range write.
	 */
	u16 restore_want;
	u16 gain;			/* Global FF_GAIN multiplier (0..0xFFFF = 0..100%); lockless, READ_ONCE/WRITE_ONCE */

	/* Track whether we opened HID device for runtime HID++ communication */
	bool hid_open;
	bool ff_hdev_open;	/* Track whether interface 2 is open for FFB I/O */

#ifdef CONFIG_HID_LOGITECH_HIDPP_DEBUG
	/* Debug interface state (per-device, not global) */
	u8 debug_last_response[16];
	int debug_last_ret;
	u8 debug_last_feature;
	u8 debug_last_function;
#endif
};

/* Maximum pending work items to prevent memory exhaustion */
#define HIDPP_DD_FF_MAX_PENDING_WORK	8

/* FFB initialization timing - event-based with retry */
#define HIDPP_DD_FF_INIT_DELAY_MS		100	/* Initial delay - allows USB enumeration to settle */
#define HIDPP_DD_FF_INIT_RETRY_MS		25	/* Retry interval if interfaces not ready */
#define HIDPP_DD_FF_MAX_INIT_RETRIES	36	/* Max retries (100 + 25×36 = 1s total fallback) */

/* Forward declarations */
static void hidpp_dd_ff_work_handler(struct work_struct *work);
static void hidpp_dd_ff_send_force(struct hidpp_dd_ff_data *ff, s32 force);
static bool hidpp_dd_ff_effect_is_texture(const struct ff_effect *eff);
static bool hidpp_dd_tf_tick(struct hidpp_dd_ff_data *ff, bool any_texture,
			 const s32 *samples, s32 force);
static void hidpp_dd_tf_init_work_handler(struct work_struct *work);
static void hidpp_dd_query_device_identity(struct hidpp_dd_ff_data *ff);
static int hidpp_dd_set_range_hw(struct hidpp_dd_ff_data *ff, int range);
static void hidpp_dd_ff_effect_timer_callback(struct timer_list *t);
static void hidpp_dd_track_wheel_pos(struct hidpp_device *hidpp, u8 *data, int size);
static struct hidpp_dd_ff_data *hidpp_dd_find_ff_data(struct hid_device *hdev);
static int hidpp_dd_response_curve_upload(struct hidpp_device *hidpp,
					  struct hidpp_dd_ff_data *ff,
					  u8 dev_idx, u8 axis, u8 idx,
					  const char *buf, size_t count);
static int hidpp_dd_response_curve_revert(struct hidpp_device *hidpp,
					  u8 dev_idx, u8 axis, u8 idx);

/*
 * Project a FF_CONSTANT effect's signed level onto the wheel's X axis.
 *
 * Direction 0x4000 (East)  = sin(90)  = +1 = full right
 * Direction 0xC000 (West)  = sin(270) = -1 = full left
 * Direction 0 (South)      = sin(0)   =  0 = no X force
 * Direction 0x8000 (North) = sin(180) =  0 = no X force
 *
 * Games using direction=0 with signed levels get zero force from this
 * formula. This is correct: well-behaved apps use direction=0x4000 for
 * right-pushing constant force. Wine's DirectInput translation handles
 * this mapping.
 */
static s32 hidpp_dd_project_constant(const struct ff_effect *effect)
{
	s32 level = effect->u.constant.level;

	return (level * fixp_sin16((effect->direction * 360) >> 16)) >> 15;
}

/*
 * Recompute constant_force as the sum of all playing FF_CONSTANT slots.
 * Must be called under effects_lock. Single source of truth for
 * ff->constant_force: avoids per-slot assignment asymmetries during
 * upload/playback-start/playback-stop/erase. The "any effect is playing"
 * short-circuit is also refreshed here so condition effects (which never
 * touch constant_force) still drive the timer alive.
 */
static void hidpp_dd_ff_recompute_constant_force_locked(struct hidpp_dd_ff_data *ff)
{
	s32 force = 0;
	bool any = false;
	int i;

	for (i = 0; i < HIDPP_DD_FF_MAX_EFFECTS; i++) {
		const struct hidpp_dd_ff_effect *e = &ff->effects[i];

		if (!e->uploaded || !e->playing)
			continue;
		any = true;
		if (e->effect.type == FF_CONSTANT)
			force += hidpp_dd_project_constant(&e->effect);
	}
	/*
	 * Writer runs under effects_lock; timer callback reads lock-free
	 * via READ_ONCE. WRITE_ONCE keeps the stores atomic relative to
	 * those reads.
	 */
	WRITE_ONCE(ff->constant_force, force);
	WRITE_ONCE(ff->any_effect_playing, any);
}

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
 * Compute one effect's instantaneous contribution to the net wheel force,
 * in the same [-S16_MAX, S16_MAX] signed domain hidpp_dd_ff_send_force takes.
 * Returns 0 for types we don't yet emulate (RAMP, PERIODIC) so those
 * uploads are accepted but don't produce force until we finish wiring
 * them up. Caller iterates and sums; caller holds effects_lock.
 *
 * CONSTANT: envelope-shaped magnitude projected onto the X axis (existing
 * hidpp_dd_project_constant semantics), preserving the sign convention the
 * earlier capture validation pinned down.
 *
 * SPRING:   condition formula fed by wheel_pos - 0x8000 (signed centred
 *           position). Pulls wheel back to centre.
 * DAMPER:   condition formula fed by wheel_vel (signed encoder-counts per
 *           sample). Opposes motion, proportional to speed.
 * FRICTION: condition formula fed by a saturated unit velocity
 *           (±S16_MAX for any non-zero velocity, 0 otherwise). Produces
 *           constant friction opposing motion direction.
 * INERTIA:  condition formula fed by wheel_accel. Opposes acceleration.
 */
static s32 hidpp_dd_ff_effect_tick(const struct hidpp_dd_ff_data *ff_state,
			       const struct hidpp_dd_ff_effect *e,
			       u32 elapsed_ms,
			       s32 wheel_pos_signed,
			       s32 wheel_vel, s32 wheel_accel)
{
	const struct ff_effect *eff = &e->effect;
	const struct ff_condition_effect *c;
	s32 f;
	u32 duration = eff->replay.length;

	switch (eff->type) {
	case FF_CONSTANT:
		/*
		 * FF_CONSTANT sign handling.
		 *
		 * Linux evdev's documented convention (direction=0x4000
		 * is east; level>0 with that direction means "force
		 * pointing east"/right for our single-axis wheel) is what
		 * native-evdev apps send, and our direct-evdev constant-
		 * force test (uploading via EVIOCSFF straight to the
		 * event node) behaves exactly that way.
		 *
		 * Games routed through Wine/Proton's DirectInput path
		 * (verified on Assetto Corsa Competizione) arrive at our
		 * driver with the sign inverted relative to that
		 * convention: the physics model's centring force lands
		 * as level>0 when the wheel is right-of-centre, which
		 * amplifies displacement instead of damping it. The flip
		 * appears to happen in Wine's PID-over-evdev translation
		 * bridge; we have not fully pinned down where.
		 *
		 * Expose the sign via ff->ffb_constant_sign so userspace
		 * can pick per-app: 1 (flipped, default) works for ACC
		 * and other Wine/Proton titles, 0 (pass-through) works
		 * for native-evdev apps. Toggle via the
		 * wheel_ffb_constant_sign sysfs attribute.
		 */
		f = hidpp_dd_project_constant(eff);
		if (READ_ONCE(ff_state->ffb_constant_sign))
			f = -f;
		return hidpp_dd_apply_envelope(&eff->u.constant.envelope, f,
					   elapsed_ms, duration);
	case FF_SPRING: {
		/*
		 * Restoring spring force, plus synthetic damping (see the
		 * spring_damping field comment). The damping term is the
		 * DAMPER formula run with the spring's own coefficient and
		 * scaled by spring_damping percent, so stiffer springs get
		 * proportionally stronger damping - the ratio, not the
		 * absolute damping, is what sets loop stability. Velocity
		 * uses the same x256 metric scaling as FF_DAMPER below.
		 */
		s32 fs;
		u8 damping;

		c = &eff->u.condition[0];
		fs = hidpp_dd_condition_force(c, wheel_pos_signed);
		damping = READ_ONCE(ff_state->spring_damping);
		if (damping) {
			s32 coeff = max(abs((s32)c->right_coeff),
					abs((s32)c->left_coeff));
			s32 vel_metric = clamp(wheel_vel * 256,
					       (s32)S16_MIN, (s32)S16_MAX);
			/*
			 * The game's saturation caps bound the WHOLE spring
			 * output, damping included: without this clamp the
			 * damping term bypassed the per-effect saturation
			 * that hidpp_dd_condition_force applies, so a spring the
			 * game deliberately capped gentle could deliver up
			 * to 25% of full scale in velocity resistance.
			 */
			s32 sat = max_t(s32, c->right_saturation,
					c->left_saturation);

			fs -= ((coeff * vel_metric) >> 15) * damping / 100;
			fs = clamp(fs, -sat, sat);
		}
		/* Global per-class scale (Oversteer spring_level). */
		return fs * READ_ONCE(ff_state->spring_level) / 100;
	}
	case FF_DAMPER:
		/*
		 * Scale the raw wheel velocity up so that realistic motion
		 * fills a useful fraction of the s16 metric range that
		 * hidpp_dd_condition_force expects. The wheel's encoder emits
		 * 65536 counts per full rotation of the range (default 900
		 * degrees). Derived velocity therefore sits around 2..100
		 * counts per 2 ms tick in normal driving and saturates into
		 * the few-hundreds during hand-shakes. Left raw, a typical
		 * 16000-ish right_coeff from the game multiplied by a
		 * vel of 10 and shifted by 15 produces about 4 units of
		 * force out of 32767, which is why DAMPER felt invisible.
		 *
		 * Multiply by 256 so 128 counts/tick maps onto S16_MAX,
		 * giving meaningful force at ordinary speeds with natural
		 * saturation for fast motion. Avoid the signed left shift
		 * (`wheel_vel << 8`) which is UB for negative wheel_vel.
		 */
		c = &eff->u.condition[0];
		/* Global per-class scale (Oversteer damper_level). */
		return hidpp_dd_condition_force(c,
			clamp(wheel_vel * 256, (s32)S16_MIN, (s32)S16_MAX)) *
			READ_ONCE(ff_state->damper_level) / 100;
	case FF_FRICTION: {
		/*
		 * Karnopp-style friction: full-scale opposing force above a
		 * small velocity window, linear ramp inside it. The previous
		 * bang-bang version (any non-zero velocity -> +/-S16_MAX
		 * metric) chattered: at slow turning speeds the per-tick
		 * encoder delta hovers around 0..2 counts where quantisation
		 * noise flips the sign every few ticks, so the friction
		 * force slammed full-magnitude left/right at up to 500 Hz -
		 * felt as gritty/notchy steering, worst near idle (issue #8).
		 * Real friction models (and wheel firmware) ramp through a
		 * stick zone instead of stepping.
		 */
		s32 vel = wheel_vel;

		c = &eff->u.condition[0];
		if (vel == 0)
			return 0;
		if (vel >= HIDPP_DD_FF_FRICTION_RAMP_COUNTS)
			vel = S16_MAX;
		else if (vel <= -HIDPP_DD_FF_FRICTION_RAMP_COUNTS)
			vel = -S16_MAX;
		else
			vel *= S16_MAX / HIDPP_DD_FF_FRICTION_RAMP_COUNTS;
		/* Global per-class scale (Oversteer friction_level). */
		return hidpp_dd_condition_force(c, vel) *
			READ_ONCE(ff_state->friction_level) / 100;
	}
	case FF_INERTIA:
		/*
		 * Acceleration is even smaller than velocity. Scale by
		 * 4096 so a quick hand-shake reaches saturation. INERTIA
		 * is rare in games; this is a reasonable default. Same
		 * multiplication-not-shift rule as DAMPER above.
		 */
		c = &eff->u.condition[0];
		return hidpp_dd_condition_force(c,
			clamp(wheel_accel * 4096, (s32)S16_MIN, (s32)S16_MAX));
	case FF_RAMP: {
		/*
		 * Linear interpolation from start_level to end_level over
		 * replay.length. Games use this for gear-shift ramps and
		 * brief haptic cues; length 0 degenerates into the start
		 * level held indefinitely.
		 */
		s32 start = eff->u.ramp.start_level;
		s32 end = eff->u.ramp.end_level;
		s32 val;

		if (duration == 0) {
			val = start;
		} else {
			u32 t = elapsed_ms;

			if (t > duration)
				t = duration;
			val = start + (s32)(((end - start) * (s64)t) / duration);
		}
		return hidpp_dd_apply_envelope(&eff->u.ramp.envelope, val,
					   elapsed_ms, duration);
	}
	case FF_PERIODIC: {
		/*
		 * Periodic waveform generator for the five standard shapes.
		 * Semantics per Linux Documentation/input/ff.rst and the
		 * USB HID PID spec:
		 *   out(t) = offset + magnitude * wave(phase_at_t)
		 * where wave() is in [-1, +1], magnitude is s16, offset is
		 * s16, and phase advances at 2pi / period per ms. We then
		 * apply the envelope on the magnitude contribution (games
		 * expect attack/fade to shape the oscillation envelope, not
		 * the DC offset).
		 *
		 * fixp_sin16 takes degrees in [0, 360) and returns a
		 * fixed-point sin in q15 format (-0x8000..+0x7fff). Our own
		 * scaling keeps the whole pipeline in signed 16-bit, matching
		 * the rest of hidpp_dd_ff_effect_tick's output domain.
		 */
		u16 period = eff->u.periodic.period;
		s16 magnitude = eff->u.periodic.magnitude;
		s16 offset = eff->u.periodic.offset;
		u16 phase = eff->u.periodic.phase;
		u32 angle_deg;
		s32 wave_q15;
		s32 scaled_magnitude;
		s32 out;

		if (period == 0)
			return offset;

		/*
		 * angle_deg in [0, 360). Compute `elapsed_ms % period`
		 * first so the multiplication by 360 can't overflow u32
		 * even for very long-running effects (without the modulo,
		 * elapsed_ms * 360 overflows around 11.9 million ms ~=
		 * 3.3 hours). Phase is a u16 where 0xFFFF equals one full
		 * wavelength.
		 */
		{
			u32 cycle_ms = elapsed_ms % (u32)period;

			angle_deg = ((cycle_ms * 360U) / period +
				     ((u32)phase * 360U) / 0xFFFF) % 360U;
		}

		switch (eff->u.periodic.waveform) {
		case FF_SINE:
			wave_q15 = fixp_sin16(angle_deg);
			break;
		case FF_SQUARE:
			wave_q15 = angle_deg < 180 ? S16_MAX : -S16_MAX;
			break;
		case FF_TRIANGLE:
			/*
			 * Linear 0..180..360 -> +max..-max..+max, peaking
			 * at 90deg and troughing at 270deg.
			 */
			if (angle_deg < 90)
				wave_q15 = (s32)angle_deg * S16_MAX / 90;
			else if (angle_deg < 270)
				wave_q15 = S16_MAX -
					((s32)(angle_deg - 90) * 2 * S16_MAX) / 180;
			else
				wave_q15 = -S16_MAX +
					((s32)(angle_deg - 270) * S16_MAX) / 90;
			break;
		case FF_SAW_UP:
			wave_q15 = -S16_MAX +
				((s32)angle_deg * 2 * S16_MAX) / 360;
			break;
		case FF_SAW_DOWN:
			wave_q15 = S16_MAX -
				((s32)angle_deg * 2 * S16_MAX) / 360;
			break;
		default:
			wave_q15 = 0;
			break;
		}

		scaled_magnitude = hidpp_dd_apply_envelope(
			&eff->u.periodic.envelope, magnitude, elapsed_ms,
			duration);
		out = offset + ((scaled_magnitude * wave_q15) >> 15);
		return out;
	}
	case FF_RUMBLE: {
		/*
		 * Gamepad-style dual-motor rumble, approximated on our
		 * single-motor direct-drive wheel as a low-frequency
		 * square-ish oscillation. strong_magnitude drives a
		 * slow shake (~25 Hz), weak_magnitude drives a faster
		 * buzz (~100 Hz); the two get alternated by period so
		 * the wheel wobbles noticeably during collisions and
		 * other gamepad-target rumble triggers.
		 *
		 * Not a perfect mapping (a real dual-rotor gamepad has
		 * two separate asymmetric masses; we have one motor),
		 * but games that send FF_RUMBLE to a wheel generally
		 * want "something shaky happened" feedback rather than
		 * precise haptic timing. Mirrors what ff-memless does
		 * when a device advertises only FF_PERIODIC; here we
		 * do the inverse since our forward (motor) path is a
		 * single constant force over time.
		 */
		u16 strong = eff->u.rumble.strong_magnitude;
		u16 weak = eff->u.rumble.weak_magnitude;
		s32 strong_force = 0;
		s32 weak_force = 0;

		if (strong) {
			/* 25 Hz strong shake, period = 40 ms. */
			u32 phase = elapsed_ms % 40U;
			s32 sign = phase < 20 ? 1 : -1;
			strong_force = sign * (s32)(strong >> 1);
		}
		if (weak) {
			/* 100 Hz weak buzz, period = 10 ms. */
			u32 phase = elapsed_ms % 10U;
			s32 sign = phase < 5 ? 1 : -1;
			weak_force = sign * (s32)(weak >> 2);
		}
		return clamp(strong_force + weak_force,
			     (s32)S16_MIN, (s32)S16_MAX);
	}
	default:
		return 0;
	}
}

/*
 * Convert a signed force (game-space) to offset binary (wire format).
 * 0x8000 = center, 0x0000 = full left, 0xFFFF = full right. Clamps to
 * s16 first: without the clamp, a strong right force summed across
 * multiple FF_CONSTANT effects overflows and wraps into a left force.
 */
static u16 hidpp_dd_force_to_offset_binary(s32 force)
{
	force = clamp(force, (s32)S16_MIN, (s32)S16_MAX);
	return (u16)(force + 0x8000);
}

/*
 * Timer callback - sends continuous force updates to the wheel.
 * Direct-drive wheels require periodic force commands to maintain FFB effect.
 */
static void hidpp_dd_ff_effect_timer_callback(struct timer_list *t)
{
	struct hidpp_dd_ff_data *ff = container_of(t, struct hidpp_dd_ff_data, effect_timer);
	s32 force = 0;
	s32 tf_sample[2] = { 0, 0 };
	s32 wheel_pos_signed, wheel_vel, wheel_accel;
	u16 cur_pos;
	unsigned long flags, now;
	bool any_playing;
	bool any_texture = false;
	bool route_tf;
	int i;

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	route_tf = READ_ONCE(ff->texture_route) == HIDPP_DD_TEXTURE_ROUTE_TF;

	/*
	 * Refresh derived wheel state. wheel_pos is updated lock-free by
	 * the interface-0 raw-event path (hidpp_dd_track_wheel_pos); we derive
	 * velocity and acceleration here at the fixed timer cadence so
	 * the derivatives are stable. Two-sample priming avoids bogus
	 * first-tick velocity spikes.
	 */
	cur_pos = READ_ONCE(ff->wheel_pos);
	if (!ff->wheel_state_primed) {
		ff->wheel_pos_prev = cur_pos;
		ff->wheel_vel = 0;
		ff->wheel_vel_prev = 0;
		ff->wheel_accel = 0;
		ff->wheel_state_primed = true;
	} else {
		s32 new_vel = (s32)(s16)(cur_pos - ff->wheel_pos_prev);

		ff->wheel_accel = new_vel - ff->wheel_vel;
		ff->wheel_vel_prev = ff->wheel_vel;
		ff->wheel_vel = new_vel;
		ff->wheel_pos_prev = cur_pos;
	}
	wheel_pos_signed = (s32)cur_pos - 0x8000;
	wheel_vel = ff->wheel_vel;
	wheel_accel = ff->wheel_accel;

	now = jiffies;
	any_playing = false;

	spin_lock_irqsave(&ff->effects_lock, flags);
	for (i = 0; i < HIDPP_DD_FF_MAX_EFFECTS; i++) {
		struct hidpp_dd_ff_effect *e = &ff->effects[i];
		unsigned long elapsed_ms_long;
		u32 elapsed_ms;

		if (!e->uploaded || !e->playing)
			continue;

		/*
		 * Effects with a non-zero replay.delay sit in a pre-start
		 * window: play_start was set to (playback_moment + delay)
		 * so `now - play_start` is negative (as unsigned, a huge
		 * number) until delay elapses. Without this guard that
		 * underflow was interpreted as "replay.length exceeded"
		 * below and we stopped the effect before it started; that
		 * is what kept fftest's periodic sine (delay=1000ms) from
		 * producing any motion. While delayed, keep the effect
		 * alive (any_playing = true) but contribute nothing.
		 */
		if (time_before(now, e->play_start)) {
			any_playing = true;
			continue;
		}

		/*
		 * Handle replay.length timeouts for effects with bounded
		 * duration. Two values mean "no timeout": 0 (per the kernel
		 * input ff API) and 0xFFFF (the conventional max-u16
		 * sentinel used by ffmvforce and many SDL FFB tools as
		 * "play indefinitely"; without this, perpetual effects
		 * would silently die at 65535 ms - issue #16).
		 */
		elapsed_ms_long = jiffies_to_msecs(now - e->play_start);
		if (e->effect.replay.length && e->effect.replay.length != 0xFFFF &&
		    elapsed_ms_long >= (unsigned long)e->effect.replay.length) {
			if (e->replays_left > 0) {
				e->replays_left--;
				e->play_start = now;
				elapsed_ms_long = 0;
			} else {
				e->playing = false;
				/* constant_force cache will be refreshed below. */
				continue;
			}
		}
		elapsed_ms = (u32)elapsed_ms_long;
		any_playing = true;

		/*
		 * Lazy TF bring-up trigger: a texture-class effect is
		 * playing but the session isn't ready. This playback keeps
		 * riding the steering channel (route was decided at
		 * playback start); once the session is up, the NEXT
		 * playback moves to the TF stream - no mid-play channel
		 * migration. The init work runs on system_unbound_wq so
		 * its 2x68 blocking sends never head-of-line-block the
		 * force stream on ff->wq.
		 */
		if (route_tf && !smp_load_acquire(&ff->tf_ready) &&
		    !READ_ONCE(ff->tf_init_queued) &&
		    hidpp_dd_ff_effect_is_texture(&e->effect)) {
			WRITE_ONCE(ff->tf_init_queued, true);
			queue_work(system_unbound_wq, &ff->tf_init_work);
		}

		if (e->use_tf) {
			/*
			 * Texture effect on the TrueForce channel: generate
			 * this tick's two 1 kHz samples (1 ms apart inside
			 * the 2 ms tick). A fast periodic's DC offset is a
			 * steering component, not texture - the TF audio
			 * path cannot hold a sustained torque - so the
			 * offset stays on the steering sum and only the AC
			 * part streams.
			 */
			s32 dc = e->effect.type == FF_PERIODIC ?
				 e->effect.u.periodic.offset : 0;

			any_texture = true;
			tf_sample[0] += hidpp_dd_ff_effect_tick(ff, e,
					elapsed_ms, wheel_pos_signed,
					wheel_vel, wheel_accel) - dc;
			tf_sample[1] += hidpp_dd_ff_effect_tick(ff, e,
					elapsed_ms + 1,
					wheel_pos_signed,
					wheel_vel, wheel_accel) - dc;
			force += dc;
			continue;
		}

		force += hidpp_dd_ff_effect_tick(ff, e, elapsed_ms,
					     wheel_pos_signed,
					     wheel_vel, wheel_accel);
	}

	/*
	 * Refresh the cached FF_CONSTANT-only sum and the any_playing
	 * short-circuit. Condition effects (spring/damper/...) are NOT
	 * cached; they're recomputed from live wheel state every tick.
	 */
	{
		s32 const_only = 0;

		for (i = 0; i < HIDPP_DD_FF_MAX_EFFECTS; i++) {
			const struct hidpp_dd_ff_effect *e = &ff->effects[i];

			if (!e->uploaded || !e->playing)
				continue;
			if (e->effect.type == FF_CONSTANT)
				const_only += hidpp_dd_project_constant(&e->effect);
		}
		WRITE_ONCE(ff->constant_force, const_only);
		WRITE_ONCE(ff->any_effect_playing, any_playing);
	}
	spin_unlock_irqrestore(&ff->effects_lock, flags);

	/*
	 * Apply FF_GAIN to the game-effect sum HERE (it used to live in
	 * hidpp_dd_ff_send_force) so the autocenter term below stays
	 * gain-independent: hardware autocenter on other wheels is not
	 * scaled by the game's gain, and a game that exits leaving
	 * FF_GAIN low must not silently disable the user's centring
	 * spring.
	 */
	{
		u16 gain = READ_ONCE(ff->gain);

		if (gain != 0xFFFF)
			force = (s32)(((s64)force * gain) / 0xFFFF);
	}

	/*
	 * Emulated autocenter: a centring spring summed on top of any
	 * game effects, active while the sysfs/evdev autocenter value is
	 * nonzero (raw 0-65535; the evdev FF_AUTOCENTER scale). Gated on
	 * wheel_pos_seen: before the first input report wheel_pos still
	 * reads 0 ("hard left") and an ungated spring would yank an
	 * untouched wheel. Damped with the same coefficient-proportional
	 * term as emulated FF_SPRING so it cannot ring the direct-drive
	 * motor. Deliberately added AFTER the gain scaling above.
	 */
	{
		u16 ac = READ_ONCE(ff->autocenter);

		if (ac && READ_ONCE(ff->wheel_pos_seen)) {
			s32 k = ac >> 1;	/* 0-65535 -> 0-32767 coeff */
			/*
			 * Steepen the spring so it reaches full authority
			 * within ~1/8 of the axis (about +/-56 degrees at a
			 * 900-degree range) instead of only at full lock -
			 * a linear-over-full-range spring at moderate level
			 * computes to ~1% force for hand-sized deflections
			 * and is imperceptible (feel-verified 2026-07-03).
			 * This matches how hardware autocenter behaves on
			 * other wheels: firm within a narrow window.
			 */
			s32 pos_metric = clamp(wheel_pos_signed * 8,
					       (s32)S16_MIN, (s32)S16_MAX);
			s32 vel_metric = clamp(wheel_vel * 256,
					       (s32)S16_MIN, (s32)S16_MAX);

			force += -((k * pos_metric) >> 15) -
				 ((k * vel_metric) >> 15) *
					 READ_ONCE(ff->spring_damping) / 100;
		}
	}

	/*
	 * Drive the TrueForce texture channel. Also runs while a stream
	 * is still open with no texture playing so the STOP gets sent
	 * (and re-sent if it was dropped) instead of the wheel looping
	 * the stale window. When a unified stream packet went out it
	 * already carries this tick's force in its cur preamble, so the
	 * separate force packet below is skipped for that tick - one
	 * 64-byte packet per tick either way.
	 */
	{
		bool force_sent = false;

		if (any_texture || ff->tf_streaming)
			force_sent = hidpp_dd_tf_tick(ff, any_texture,
						      tf_sample, force);

		/*
		 * Push the current force on each timer tick (unless the
		 * unified TF packet above already did). The wheel firmware
		 * treats a gap in commands as "host idle" and runs an
		 * unwind-to-soft-stop / recenter safety routine, so
		 * coalescing identical-force ticks made any held constant
		 * force evaporate within a couple of seconds (issue #16,
		 * ffmvforce repro). At 500 Hz x 64 bytes the USB cost is
		 * ~32 KB/s, negligible.
		 */
		if (!force_sent)
			hidpp_dd_ff_send_force(ff, force);
	}
	ff->last_force = force;

	/*
	 * Keep the timer alive as long as any effect is playing, even if
	 * the instantaneous net force is zero (e.g. a DAMPER at rest, or
	 * a SPRING at exact centre). Without this, the wheel would stop
	 * sampling and the condition effects would never fire. Also stay
	 * alive while a TF stream is open so a dropped STOP can retry,
	 * and while autocenter is set so the centring spring keeps
	 * tracking the wheel.
	 */
	if ((any_playing || ff->tf_streaming || READ_ONCE(ff->autocenter)) &&
	    !atomic_read_acquire(&ff->stopping) &&
	    atomic_read(&ff->initialized))
		mod_timer(&ff->effect_timer,
			  jiffies + msecs_to_jiffies(HIDPP_DD_FF_TIMER_INTERVAL_MS));
}

/*
 * Send a force value to the wheel (non-blocking, queues work).
 */
static void hidpp_dd_ff_send_force(struct hidpp_dd_ff_data *ff, s32 force)
{
	struct hidpp_dd_ff_work *ff_work;
	int pending;

	if (!ff || atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	pending = atomic_read(&ff->pending_work);
	if (pending >= HIDPP_DD_FF_MAX_PENDING_WORK) {
		/*
		 * Queue saturated - the USB link is likely stalled. Count the
		 * drop like the kmalloc-failure path below (this is the more
		 * likely drop under a stalled link), otherwise sustained
		 * saturation silently evaporates forces with no dmesg trace.
		 */
		ff->err_count++;
		return;
	}

	ff_work = kmalloc(sizeof(*ff_work), GFP_ATOMIC);
	if (!ff_work) {
		/*
		 * Dropping a 500 Hz FFB sample is normally invisible, so
		 * count the drops and let the shared last_err_log rate
		 * limiter surface the count next time it fires. err_count
		 * is also bumped from hidpp_dd_ff_work_handler on USB errors;
		 * both paths feed into the same "how bad was the last
		 * window" metric.
		 */
		ff->err_count++;
		return;
	}

	/*
	 * FF_GAIN is applied by the caller (the effect timer scales the
	 * game-effect sum before adding the gain-independent autocenter
	 * term); this function sends the force as given.
	 */
	ff_work->force = hidpp_dd_force_to_offset_binary(force);
	ff_work->ff_data = ff;
	ff_work->raw = false;
	INIT_WORK(&ff_work->work, hidpp_dd_ff_work_handler);

	atomic_inc(&ff->pending_work);
	queue_work(ff->wq, &ff_work->work);
}

/*
 * In-kernel TrueForce texture channel. See the HIDPP_DD_TF_* define block
 * for the protocol/design rationale. All hidpp_dd_tf_* runtime state is
 * timer-callback-private except tf_ready (published by the init work
 * with store-release, consumed with load-acquire).
 */

/*
 * Vibration-class effects ride the TF audio stream when texture_route
 * selects it; everything else keeps shaping the steering force.
 */
static bool hidpp_dd_ff_effect_is_texture(const struct ff_effect *eff)
{
	switch (eff->type) {
	case FF_RUMBLE:
		return true;
	case FF_PERIODIC:
		return eff->u.periodic.period > 0 &&
		       eff->u.periodic.period <= HIDPP_DD_TF_CROSSOVER_PERIOD_MS;
	default:
		return false;
	}
}

/*
 * Queue a pre-built 64-byte interface-2 packet for sending. Safe from
 * atomic (timer) context; mirrors hidpp_dd_ff_send_force's guards. Returns
 * false when the packet was dropped (queue pressure, allocation
 * failure, teardown) so callers can keep their stream state honest.
 *
 * TF packets keep two slots of the shared pending budget free for the
 * steering-force stream: KF is also the firmware's host-alive signal,
 * so under queue pressure texture is the stream to shed first.
 */
static bool hidpp_dd_tf_queue_raw(struct hidpp_dd_ff_data *ff, const u8 *pkt)
{
	struct hidpp_dd_ff_work *ff_work;

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return false;
	if (atomic_read(&ff->pending_work) >= HIDPP_DD_FF_MAX_PENDING_WORK - 2) {
		ff->err_count++;
		return false;
	}

	ff_work = kmalloc(sizeof(*ff_work), GFP_ATOMIC);
	if (!ff_work) {
		ff->err_count++;
		return false;
	}

	ff_work->ff_data = ff;
	ff_work->raw = true;
	memcpy(ff_work->report_buf, pkt, HIDPP_DD_FF_REPORT_SIZE);
	INIT_WORK(&ff_work->work, hidpp_dd_ff_work_handler);

	atomic_inc(&ff->pending_work);
	queue_work(ff->wq, &ff_work->work);
	return true;
}

/*
 * Queue a TF control packet (START/STOP). Timer context only (tf_seq).
 * tf_seq only advances when the packet was actually queued, so drops
 * do not leave gaps in the wire sequence.
 */
static bool hidpp_dd_tf_queue_ctrl(struct hidpp_dd_ff_data *ff, u8 cmd)
{
	u8 pkt[HIDPP_DD_FF_REPORT_SIZE] = { 0 };

	pkt[0] = HIDPP_DD_FF_REPORT_ID;
	pkt[4] = cmd;
	pkt[5] = ff->tf_seq;
	if (!hidpp_dd_tf_queue_raw(ff, pkt))
		return false;
	ff->tf_seq++;
	return true;
}

/*
 * Queue one unified TF stream packet: the steering-force sum rides in
 * the bytes 6-9 preamble ("cur") and the rolling window carries the
 * texture audio on top.
 *
 * Bytes 6-9 are the wheel's MOTOR TORQUE TARGET while a TF session is
 * active, with the window played additively over it - established by
 * the TF4ALL project's Windows-side captures (AC EVO streams its game
 * FFB in cur and audio in the window of the same packet) and consistent
 * with our own KF packet, which is this exact layout with zero new
 * samples. Earlier revisions duplicated the NEWEST AUDIO SAMPLE into
 * 6-9, which commanded the motor to follow the texture amplitude
 * whenever a stream packet interleaved with the 500 Hz force packets.
 *
 * cur is deliberately NOT scaled by wheel_strength here: it is the same
 * bytes-6-9 field the KF packet carries, and KF forces are verified to
 * be strength-scaled by the wheel firmware itself (unlike the window
 * samples, which play at face value and are host-scaled before they
 * reach this function). Windows corroborates: G Hub's strength setting
 * audibly works in SDK titles whose entire force path is cur.
 *
 * `quartet` is the tick's HIDPP_DD_TF_NEW_SAMPLES new window slots, or
 * NULL to resend the current window unchanged (STOP-path recentre).
 * The rolling window is only advanced AFTER the packet actually
 * queued: committing first and dropping the packet under queue
 * pressure would silently skip 2 ms of texture the wheel never saw
 * while the next packet still claims 4 new samples.
 *
 * Layout per docs/TRUEFORCE_PROTOCOL.md: cur duplicated at 6-9, sample
 * count and the 0x0d flag at 10/11, then the 13 window slots
 * oldest-first, each u16 duplicated L/R. Timer context only (tf_seq,
 * tf_window).
 */
static bool hidpp_dd_tf_queue_stream(struct hidpp_dd_ff_data *ff, s32 force,
				     const u16 *quartet)
{
	u8 pkt[HIDPP_DD_FF_REPORT_SIZE] = { 0 };
	u16 cur = hidpp_dd_force_to_offset_binary(force);
	int shifted = HIDPP_DD_TF_WINDOW - HIDPP_DD_TF_NEW_SAMPLES;
	int i;

	pkt[0] = HIDPP_DD_FF_REPORT_ID;
	pkt[4] = HIDPP_DD_TF_CMD_STREAM;
	pkt[5] = ff->tf_seq;
	put_unaligned_le16(cur, &pkt[6]);
	put_unaligned_le16(cur, &pkt[8]);
	pkt[10] = HIDPP_DD_TF_NEW_SAMPLES;
	pkt[11] = HIDPP_DD_TF_FLAG_BYTE;
	for (i = 0; i < HIDPP_DD_TF_WINDOW; i++) {
		u16 v;

		if (!quartet)
			v = ff->tf_window[i];
		else if (i < shifted)
			v = ff->tf_window[i + HIDPP_DD_TF_NEW_SAMPLES];
		else
			v = quartet[i - shifted];
		put_unaligned_le16(v, &pkt[12 + i * 4]);
		put_unaligned_le16(v, &pkt[14 + i * 4]);
	}
	if (!hidpp_dd_tf_queue_raw(ff, pkt))
		return false;
	ff->tf_seq++;
	if (quartet) {
		memmove(&ff->tf_window[0],
			&ff->tf_window[HIDPP_DD_TF_NEW_SAMPLES],
			shifted * sizeof(ff->tf_window[0]));
		memcpy(&ff->tf_window[shifted], quartet,
		       HIDPP_DD_TF_NEW_SAMPLES * sizeof(ff->tf_window[0]));
	}
	return true;
}

/*
 * Lazy TF session bring-up: replay the captured 68-packet init sequence
 * twice (G Hub behaviour; the sequence byte restarts at 1 each pass and
 * the live stream continues counting from where init left off). Runs in
 * workqueue context the first time a texture effect plays. On failure,
 * tf_ready stays false and texture effects keep summing into the
 * steering force - degraded feel, never lost effects.
 */
static void hidpp_dd_tf_init_work_handler(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
					       tf_init_work);
	struct hid_device *hdev;
	u8 *pkt;
	int pass, i, ret = 0;

	BUILD_BUG_ON(HIDPP_DD_TF_INIT_PACKET_LEN != HIDPP_DD_FF_REPORT_SIZE);

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	pkt = kmalloc(HIDPP_DD_FF_REPORT_SIZE, GFP_KERNEL);
	if (!pkt) {
		/* Retryable: let a later texture playback try again. */
		WRITE_ONCE(ff->tf_init_queued, false);
		return;
	}

	for (pass = 0; pass < 2 && ret >= 0; pass++) {
		u8 seq = 1;

		for (i = 0; i < HIDPP_DD_TF_INIT_PACKET_COUNT; i++) {
			if (atomic_read_acquire(&ff->stopping)) {
				kfree(pkt);
				return;
			}
			/*
			 * Re-read the interface-2 device every packet: its
			 * remove path clears ff_hdev (and cancels this work,
			 * but a racing clear must not leave us sending to a
			 * stopped device for the rest of a 136-packet loop).
			 */
			hdev = READ_ONCE(ff->ff_hdev);
			if (!hdev) {
				kfree(pkt);
				return;
			}
			memcpy(pkt, hidpp_dd_tf_init_packets[i],
			       HIDPP_DD_TF_INIT_PACKET_LEN);
			pkt[HIDPP_DD_TF_INIT_SEQ_OFFSET] = seq++;
			ret = hid_hw_output_report(hdev, pkt,
						   HIDPP_DD_FF_REPORT_SIZE);
			/* Only -ENOSYS means ->output_report is unimplemented;
			 * fall back to raw_request then. Real transport errors
			 * must surface, not get re-sent as a control transfer. */
			if (ret == -ENOSYS)
				ret = hid_hw_raw_request(hdev,
						HIDPP_DD_FF_REPORT_ID, pkt,
						HIDPP_DD_FF_REPORT_SIZE,
						HID_OUTPUT_REPORT,
						HID_REQ_SET_REPORT);
			if (ret < 0)
				break;
		}
	}
	kfree(pkt);

	if (ret < 0) {
		/*
		 * Bounded retry: a transient USB error should not pin
		 * texture effects to the steering channel for the whole
		 * session (the pre-retry behaviour). Clearing
		 * tf_init_queued lets the next texture playback re-queue
		 * this work; after HIDPP_DD_TF_INIT_MAX_ATTEMPTS hard failures
		 * the flag stays set and the session runs degraded.
		 */
		ff->tf_init_attempts++;
		if (ff->tf_init_attempts < HIDPP_DD_TF_INIT_MAX_ATTEMPTS) {
			dd_warn(ff->hidpp->hid_dev,
				 "TrueForce texture channel init failed (%d), attempt %u/%u; will retry on the next texture effect\n",
				 ret, ff->tf_init_attempts,
				 HIDPP_DD_TF_INIT_MAX_ATTEMPTS);
			WRITE_ONCE(ff->tf_init_queued, false);
		} else {
			dd_warn(ff->hidpp->hid_dev,
				 "TrueForce texture channel init failed (%d); giving up for this session, texture effects stay on the steering channel\n",
				 ret);
		}
		return;
	}

	ff->tf_seq = HIDPP_DD_TF_INIT_PACKET_COUNT + 1;
	/* Publish tf_seq (and the init itself) before tf_ready. */
	smp_store_release(&ff->tf_ready, true);
	dd_info(ff->hidpp->hid_dev,
		 "TrueForce texture channel ready (vibration effects ride the TF stream)\n");
}

/*
 * Per-tick TF driver, called from the effect timer after the effect sum.
 * `samples` holds this tick's two 1 kHz texture samples (signed force
 * domain, pre-gain); `force` is the final steering-force sum for this
 * tick (post-gain, autocenter included).
 *
 * Emits ONE unified stream packet per tick (500 Hz): the steering force
 * in the cur preamble and four new window slots - each 1 kHz texture
 * sample duplicated once, which is time-correct at the resulting 2 kHz
 * window-slot rate. This replaces the earlier two-stream interleave
 * (500 Hz force packets + 250 Hz audio packets whose preamble wrongly
 * carried audio).
 *
 * When the last texture effect stops, re-centres the window (once) and
 * sends STOP so the wheel's DSP returns to silence instead of looping
 * the stale window.
 *
 * Return value is the force-carrier contract: true means a stream
 * packet carrying `force` in its cur preamble was queued THIS tick and
 * the caller must not send a second force packet; false means the
 * caller owns the tick's force send. Every path in this function
 * returns exactly that truth - keep it that way, since a wrong true
 * starves the firmware's host-alive watchdog and a wrong false doubles
 * the torque command.
 */
static bool hidpp_dd_tf_tick(struct hidpp_dd_ff_data *ff, bool any_texture,
			 const s32 *samples, s32 force)
{
	u16 gain, strength;
	u16 quartet[HIDPP_DD_TF_NEW_SAMPLES];
	int i;

	if (!any_texture) {
		if (ff->tf_streaming) {
			bool sent = false;

			/*
			 * Recentre and stop. The recentre packet is sent at
			 * most ONCE per session wind-down (tf_recentre_sent):
			 * repeating it on every STOP retry under queue
			 * pressure would keep the queue at the very
			 * threshold that is dropping the STOP. It carries
			 * the live force in cur, so the tick it goes out it
			 * IS the force carrier and the caller must not send
			 * a second one.
			 *
			 * Only mark the stream stopped once the STOP
			 * actually queued: a dropped STOP would leave the
			 * wheel's DSP looping the last window while the
			 * driver believes the stream is down. The timer
			 * keeps ticking while tf_streaming is set, so a
			 * failed STOP retries on the next tick (with the
			 * caller's plain force packet as that tick's
			 * carrier).
			 */
			if (!ff->tf_recentre_sent) {
				memset16(ff->tf_window, 0x8000,
					 HIDPP_DD_TF_WINDOW);
				sent = hidpp_dd_tf_queue_stream(ff, force,
								NULL);
				ff->tf_recentre_sent = sent;
			}
			if (hidpp_dd_tf_queue_ctrl(ff, HIDPP_DD_TF_CMD_STOP))
				ff->tf_streaming = false;
			return sent;
		}
		return false;
	}

	if (!ff->tf_streaming) {
		/*
		 * START must land before stream packets mean anything; if
		 * it was dropped, skip this tick's samples and retry.
		 */
		if (!hidpp_dd_tf_queue_ctrl(ff, HIDPP_DD_TF_CMD_START))
			return false;
		ff->tf_streaming = true;
		ff->tf_recentre_sent = false;
	}

	gain = READ_ONCE(ff->gain);
	strength = READ_ONCE(ff->strength);
	/* Each 1 kHz sample fills two of the packet's four new slots. */
	for (i = 0; i < HIDPP_DD_TF_NEW_SAMPLES / 2; i++) {
		s32 s = samples[i];

		if (gain != 0xFFFF)
			s = (s32)(((s64)s * gain) / 0xFFFF);
		/*
		 * Scale by the user's wheel strength. The wheel firmware
		 * applies the 0x8136 strength setting to steering (KF)
		 * forces itself but plays TF audio samples at face value
		 * (verified live 2026-07-02: full-volume buzz at 20%
		 * strength), so without this a texture effect blasts at
		 * full amplitude on a wheel the user dialled down.
		 */
		if (strength != 0xFFFF)
			s = (s32)(((s64)s * strength) / 0xFFFF);
		/*
		 * Keep texture in the DSP's vibration regime; above the
		 * cap the window content starts steering the motor (see
		 * HIDPP_DD_TF_MAX_AMPLITUDE).
		 */
		s = clamp(s, -(s32)HIDPP_DD_TF_MAX_AMPLITUDE,
			  (s32)HIDPP_DD_TF_MAX_AMPLITUDE);
		/*
		 * Duplicate each 1 kHz sample into two adjacent window
		 * slots: at one packet per 2 ms tick the wheel consumes
		 * window slots at 2 kHz, so the pair plays for the 1 ms
		 * the sample represents.
		 */
		quartet[i * 2] = hidpp_dd_force_to_offset_binary(s);
		quartet[i * 2 + 1] = quartet[i * 2];
	}

	/* Window advance is committed inside queue_stream, only on success. */
	return hidpp_dd_tf_queue_stream(ff, force, quartet);
}

/*
 * FF effect upload callback - stores effect for later playback.
 */
static int hidpp_dd_ff_upload(struct input_dev *dev, struct ff_effect *effect,
			  struct ff_effect *old)
{
	struct hidpp_dd_ff_data *ff = dev->ff->private;
	int id = effect->id;
	unsigned long flags;
	bool recompute = false;

	if (!ff || id < 0 || id >= HIDPP_DD_FF_MAX_EFFECTS)
		return -EINVAL;

	spin_lock_irqsave(&ff->effects_lock, flags);
	ff->effects[id].effect = *effect;
	ff->effects[id].uploaded = true;
	if (!old)
		ff->effects[id].playing = false;

	/*
	 * If an already-playing effect is being re-parametrised (level
	 * tweaks during playback from DirectInput SetParameters), the
	 * cached FF_CONSTANT sum needs to be refreshed now; the timer
	 * would pick it up on its next tick regardless, but recomputing
	 * here keeps the live behaviour tight for userspace tools like
	 * ffcfstress that continuously restream the level.
	 */
	if (ff->effects[id].playing && effect->type == FF_CONSTANT) {
		hidpp_dd_ff_recompute_constant_force_locked(ff);
		recompute = true;
	}
	spin_unlock_irqrestore(&ff->effects_lock, flags);

	if (recompute && !atomic_read_acquire(&ff->stopping))
		mod_timer(&ff->effect_timer,
			  jiffies + msecs_to_jiffies(HIDPP_DD_FF_TIMER_INTERVAL_MS));

	/*
	 * Log full effect parameters, not just the type: root-causing FFB
	 * feel/stability issues (e.g. the AC EVO map-load ringing) needs to
	 * know exactly what the game uploaded. Enable at runtime with
	 * dynamic debug: echo 'format "Upload effect" +p' > .../control
	 */
	switch (effect->type) {
	case FF_CONSTANT:
		dd_dbg(ff->hidpp->hid_dev,
			"Upload effect %d type=%d CONSTANT level=%d dir=0x%04x len=%u\n",
			id, effect->type, effect->u.constant.level,
			effect->direction, effect->replay.length);
		break;
	case FF_SPRING:
	case FF_DAMPER:
	case FF_FRICTION:
	case FF_INERTIA:
		dd_dbg(ff->hidpp->hid_dev,
			"Upload effect %d type=%d CONDITION rc=%d lc=%d rs=%u ls=%u db=%u ctr=%d len=%u\n",
			id, effect->type,
			effect->u.condition[0].right_coeff,
			effect->u.condition[0].left_coeff,
			effect->u.condition[0].right_saturation,
			effect->u.condition[0].left_saturation,
			effect->u.condition[0].deadband,
			effect->u.condition[0].center,
			effect->replay.length);
		break;
	case FF_PERIODIC:
		dd_dbg(ff->hidpp->hid_dev,
			"Upload effect %d type=%d PERIODIC wave=%d period=%u mag=%d off=%d len=%u\n",
			id, effect->type, effect->u.periodic.waveform,
			effect->u.periodic.period, effect->u.periodic.magnitude,
			effect->u.periodic.offset, effect->replay.length);
		break;
	case FF_RUMBLE:
		dd_dbg(ff->hidpp->hid_dev,
			"Upload effect %d type=%d RUMBLE strong=%u weak=%u len=%u\n",
			id, effect->type, effect->u.rumble.strong_magnitude,
			effect->u.rumble.weak_magnitude, effect->replay.length);
		break;
	case FF_RAMP:
		dd_dbg(ff->hidpp->hid_dev,
			"Upload effect %d type=%d RAMP start=%d end=%d len=%u\n",
			id, effect->type, effect->u.ramp.start_level,
			effect->u.ramp.end_level, effect->replay.length);
		break;
	default:
		dd_dbg(ff->hidpp->hid_dev, "Upload effect %d type=%d\n",
			id, effect->type);
		break;
	}
	return 0;
}

/*
 * FF effect erase callback - removes effect.
 */
static int hidpp_dd_ff_erase(struct input_dev *dev, int id)
{
	struct hidpp_dd_ff_data *ff = dev->ff->private;
	unsigned long flags;

	if (!ff || id < 0 || id >= HIDPP_DD_FF_MAX_EFFECTS)
		return -EINVAL;

	spin_lock_irqsave(&ff->effects_lock, flags);
	ff->effects[id].uploaded = false;
	ff->effects[id].playing = false;
	memset(&ff->effects[id].effect, 0, sizeof(struct ff_effect));
	hidpp_dd_ff_recompute_constant_force_locked(ff);
	spin_unlock_irqrestore(&ff->effects_lock, flags);

	dd_dbg(ff->hidpp->hid_dev, "Erased effect %d\n", id);
	return 0;
}

/*
 * FF playback callback - starts or stops an effect.
 */
static int hidpp_dd_ff_playback(struct input_dev *dev, int id, int value)
{
	struct hidpp_dd_ff_data *ff = dev->ff->private;
	unsigned long flags;
	bool any_playing;

	if (!ff || id < 0 || id >= HIDPP_DD_FF_MAX_EFFECTS)
		return -EINVAL;

	spin_lock_irqsave(&ff->effects_lock, flags);

	if (!ff->effects[id].uploaded) {
		spin_unlock_irqrestore(&ff->effects_lock, flags);
		return -EINVAL;
	}

	if (value) {
		/*
		 * Start window: record the absolute playback start so the
		 * timer can track envelopes, ramps, and replay timeouts.
		 * replay.delay is honoured by starting the effect-playing
		 * flag immediately and having the tick callback treat the
		 * window before (play_start + delay) as "not yet".
		 */
		ff->effects[id].play_start = jiffies +
			msecs_to_jiffies(ff->effects[id].effect.replay.delay);
		ff->effects[id].replays_left = value > 0 ? value - 1 : 0;
		/*
		 * Channel decision for this play cycle (see the use_tf
		 * field comment): TF only when the route selects it AND
		 * the session is already up. If the session is still
		 * initialising, this playback stays on the steering
		 * channel for its whole duration.
		 */
		ff->effects[id].use_tf =
			READ_ONCE(ff->texture_route) == HIDPP_DD_TEXTURE_ROUTE_TF &&
			smp_load_acquire(&ff->tf_ready) &&
			hidpp_dd_ff_effect_is_texture(&ff->effects[id].effect);
	}
	ff->effects[id].playing = (value != 0);

	hidpp_dd_ff_recompute_constant_force_locked(ff);
	any_playing = READ_ONCE(ff->any_effect_playing);

	spin_unlock_irqrestore(&ff->effects_lock, flags);

	dd_dbg(ff->hidpp->hid_dev,
		"FFB playback id=%d type=%d value=%d any_playing=%d\n",
		id, ff->effects[id].effect.type, value, any_playing);

	/*
	 * Any transition that leaves effects playing needs the timer
	 * running. The transition to "nothing playing" fires the timer
	 * immediately to emit a single zero-force ("return to idle")
	 * packet and let the callback stop rescheduling itself.
	 */
	if (atomic_read_acquire(&ff->stopping))
		return 0;
	if (any_playing)
		mod_timer(&ff->effect_timer,
			  jiffies + msecs_to_jiffies(HIDPP_DD_FF_TIMER_INTERVAL_MS));
	else if (ff->last_force != 0)
		mod_timer(&ff->effect_timer, jiffies);

	return 0;
}

/*
 * Set FF gain (global force multiplier).
 *
 * Applied at send time in hidpp_dd_ff_send_force; independent of the wheel's
 * own strength setting (which the user controls via sysfs).
 */
static void hidpp_dd_ff_set_gain(struct input_dev *dev, u16 gain)
{
	struct hidpp_dd_ff_data *ff = dev->ff->private;

	if (!ff)
		return;
	WRITE_ONCE(ff->gain, gain);
	dd_dbg(ff->hidpp->hid_dev, "FF_GAIN set to %u (%u%%)\n",
		gain, ((u32)gain * 100) / 0xFFFF);
}

/*
 * Emulated autocenter (evdev FF_AUTOCENTER): stores the magnitude and
 * makes sure the effect timer runs so the centring spring in the tick
 * takes effect even with no game effects playing. Games writing 0
 * before taking over FFB disable it for their session, as on other
 * wheels.
 */
static void hidpp_dd_ff_set_autocenter(struct input_dev *dev, u16 magnitude)
{
	struct hidpp_dd_ff_data *ff = dev->ff->private;

	if (!ff)
		return;
	WRITE_ONCE(ff->autocenter, magnitude);
	if (magnitude && !atomic_read_acquire(&ff->stopping) &&
	    atomic_read(&ff->initialized))
		mod_timer(&ff->effect_timer, jiffies +
			  msecs_to_jiffies(HIDPP_DD_FF_TIMER_INTERVAL_MS));
	dd_dbg(ff->hidpp->hid_dev, "FF_AUTOCENTER set to %u\n",
		magnitude);
}

/* Work handler - runs in workqueue context where blocking calls are safe */
static void hidpp_dd_ff_work_handler(struct work_struct *work)
{
	struct hidpp_dd_ff_work *ff_work = container_of(work, struct hidpp_dd_ff_work, work);
	struct hidpp_dd_ff_data *ff = ff_work->ff_data;
	struct hidpp_dd_ff_report *report;
	struct hid_device *hdev;
	int ret;

	/* Safety check: abort if driver is shutting down or data is invalid */
	if (!ff) {
		kfree(ff_work);
		return;
	}
	if (atomic_read_acquire(&ff->stopping)) {
		atomic_dec(&ff->pending_work);
		kfree(ff_work);
		return;
	}

	/*
	 * Cache ff_hdev locally using READ_ONCE to prevent TOCTOU race.
	 * Destroy may set ff_hdev to NULL between our check and use.
	 */
	hdev = READ_ONCE(ff->ff_hdev);
	if (!hdev) {
		atomic_dec(&ff->pending_work);
		kfree(ff_work);
		return;
	}

	/*
	 * Use the per-work buffer to avoid race conditions where
	 * hid_hw_output_report() returns before DMA completes.
	 *
	 * Raw work items (TrueForce stream/control packets) arrive with
	 * report_buf already built by the queuer, sequence included; only
	 * the classic constant-force report is built here.
	 */
	if (!ff_work->raw) {
		report = (struct hidpp_dd_ff_report *)ff_work->report_buf;
		memset(report, 0, HIDPP_DD_FF_REPORT_SIZE);
		report->report_id = HIDPP_DD_FF_REPORT_ID;
		report->effect_type = HIDPP_DD_FF_EFFECT_CONSTANT;
		report->sequence = atomic_inc_return(&ff->sequence) & 0xFF;
		report->force = cpu_to_le16(ff_work->force);
		report->force_dup = report->force;
	}

	/*
	 * Send FFB via interface 2's HID output report mechanism.
	 * Try hid_hw_output_report first (uses interrupt OUT if available),
	 * fall back to hid_hw_raw_request (uses SET_REPORT control transfer).
	 * This mirrors what hidraw does in hidraw_write().
	 */
	ret = hid_hw_output_report(hdev, ff_work->report_buf, HIDPP_DD_FF_REPORT_SIZE);
	/*
	 * Only -ENOSYS means the transport has no ->output_report; fall back to
	 * raw_request (SET_REPORT) then. Every other value is a real transport
	 * failure - do NOT re-send it as a control transfer (that masked
	 * -EPIPE/-ETIMEDOUT and could put the packet on the wire twice).
	 */
	if (ret == -ENOSYS) {
		ret = hid_hw_raw_request(hdev, HIDPP_DD_FF_REPORT_ID,
					 ff_work->report_buf, HIDPP_DD_FF_REPORT_SIZE,
					 HID_OUTPUT_REPORT, HID_REQ_SET_REPORT);
	}

	if (ret < 0) {
		/*
		 * At 500 Hz this error path would flood dmesg on a persistent
		 * USB fault. Rate-limit to one message per minute; the shared
		 * last_err_log timestamp coordinates with the refresh handler
		 * so a failing device produces a single steady trickle.
		 */
		if (time_after(jiffies, ff->last_err_log + HZ * 60)) {
			dd_err(hdev,
				"Force feedback command failed (error %d, %d errors since last log)\n",
				ret, ff->err_count + 1);
			ff->last_err_log = jiffies;
			ff->err_count = 0;
		} else {
			ff->err_count++;
		}
	}

	/*
	 * Decrement pending work counter AFTER all ff field accesses.
	 * This prevents use-after-free if destroy() runs between the
	 * decrement and subsequent ff access.
	 */
	atomic_dec(&ff->pending_work);
	kfree(ff_work);
}

/*
 * Rotation-range read-back: keep the cached (and sysfs-reported) range
 * honest against external changes.
 *
 * Some game launches (observed with AC EVO under Proton, 2026-06-29/30)
 * reset the wheel's physical rotation range to 90 degrees WITHOUT any
 * HID++ rotation-change broadcast, so hidpp_dd_ff_raw_hidpp_event never sees
 * it and the cache silently goes stale: sysfs keeps claiming 900 degrees
 * while the rim is physically locked at 90. That mismatch is what
 * confuses users ("I set 900, why does it stop at 90?").
 *
 * Deliberate design: DETECT and REPORT, never fight. Automatically
 * writing the range back was tried and abandoned - re-applying range or
 * mode while a game holds active FFB desyncs the centre on a direct-
 * drive wheel and ends in a violent swing. Instead, re-read the true
 * value on the existing 20 s keepalive cadence; on an external change,
 * update the cache, log it, and sysfs_notify() poll()ers on wheel_range
 * so userspace tools can surface or handle it.
 */
/*
 * Gated auto-restore for externally-reset ranges.
 *
 * Root cause (usbmon, 2026-07-02): some games' SDK sessions push an
 * operating range once at session start via a TrueForce type-0x0e
 * packet on interface 2 (AC EVO pushes 90.0), invisible to HID++. The
 * push is one-shot and a HID++ re-apply afterwards sticks - verified
 * through full laps - so restoring the previous range is safe and is
 * what the user expects ("I set 900; why is it suddenly 90?").
 *
 * Every gate below exists because of a real incident:
 * - desktop mode only, and NEVER an automatic mode switch (mode churn
 *   under active FFB caused violent centre desync, twice);
 * - wheel near centre and stationary (a range change while the wheel
 *   is deflected/held is what desyncs the centre) - if not, skip
 *   without consuming a strike and let the next poll retry;
 * - at most 3 restore attempts per session (a persistent external
 *   writer wins; we log and stop rather than fight);
 * - runs only from the sleepable range poll, which itself pauses
 *   while evdev effects play.
 *
 * Opt out via wheel_range_restore=0.
 */
/*
 * Read the wheel's raw encoder position over HID++ (calibration
 * feature, sub-device 0x05, fn=1 GET - the same read
 * wheel_calibrate_here uses). Sleepable context. 0x8000 = centre.
 */
static int hidpp_dd_ff_read_encoder(struct hidpp_dd_ff_data *ff, u16 *pos)
{
	struct hidpp_report response;
	u8 params[3] = { 0, 0, 0 };
	int ret;

	if (ff->idx_calibrate == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;
	ret = hidpp_send_fap_to_device_sync(ff->hidpp, ff->calibrate_dev_idx,
					    ff->idx_calibrate,
					    0x10 /* fn=1 */,
					    params, 3, &response);
	if (ret)
		return ret;
	*pos = (response.fap.params[0] << 8) | response.fap.params[1];
	return 0;
}

static void hidpp_dd_ff_range_maybe_restore(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	u16 want = ff->restore_want;
	u16 p1, p2;

	if (!want)
		return;
	if (atomic_read_acquire(&ff->stopping))
		return;
	if (!READ_ONCE(ff->range_restore)) {
		dd_dbg(hidpp->hid_dev, "range restore skipped (disabled)\n");
		return;
	}
	/*
	 * Moot: the range moved off 90 by other means (the game applied
	 * its own configured value, the user wrote one, ...). Drop the
	 * pending restore rather than overriding whatever won.
	 */
	if (READ_ONCE(ff->range) != 90) {
		dd_dbg(hidpp->hid_dev, "range restore moot (range now %u)\n",
			READ_ONCE(ff->range));
		ff->restore_want = 0;
		return;
	}
	if (!ff->mode_known || ff->current_mode != 0) {
		dd_dbg(hidpp->hid_dev, "range restore skipped (mode_known=%d mode=%d)\n",
			ff->mode_known, ff->current_mode);
		return;
	}
	if (ff->range_restore_attempts >= 3) {
		ff->restore_want = 0;
		return;
	}

	/*
	 * The wheel must be stationary: never move the soft stops while
	 * the user is actively turning. Stillness is measured with two
	 * on-demand HID++ encoder reads 50 ms apart. The reads return
	 * RAW absolute encoder values (centre is wherever calibration
	 * put it, NOT 0x8000 - an earlier centred-ness check compared
	 * against 0x8000 and deferred forever), so only the delta is
	 * meaningful; the cached ff->wheel_pos is unusable here as it
	 * only updates when the wheel emits input reports.
	 *
	 * No centred-ness requirement is needed: restores only ever
	 * WIDEN the range (90 -> the pre-reset value), and a position
	 * within the old +/-45 degrees is by definition inside any wider
	 * range's stops, so a widening write cannot snap the wheel.
	 */
	if (hidpp_dd_ff_read_encoder(ff, &p1)) {
		dd_dbg(hidpp->hid_dev, "range restore skipped (encoder read failed)\n");
		return;
	}
	msleep(50);
	/*
	 * Teardown may have started during the sleep; each further sync
	 * send would then ride its full timeout against a dead device and
	 * stall the workqueue flush in hidpp_dd_ff_destroy.
	 */
	if (atomic_read_acquire(&ff->stopping))
		return;
	if (hidpp_dd_ff_read_encoder(ff, &p2))
		return;
	if (abs((int)p2 - (int)p1) > 200) {
		dd_dbg(hidpp->hid_dev,
			"range restore deferred (wheel moving)\n");
		return;
	}

	/*
	 * Re-validate after the stillness window: an explicit wheel_range
	 * write during the ~100 ms of encoder reads clears restore_want
	 * (and may have moved the range off 90). Honour it rather than
	 * overwriting the user's fresh intent with the stale snapshot.
	 */
	if (ff->restore_want != want || READ_ONCE(ff->range) != 90)
		return;

	ff->range_restore_attempts++;
	if (hidpp_dd_set_range_hw(ff, want) == 0) {
		ff->restore_want = 0;
		dd_info(hidpp->hid_dev,
			 "rotation range auto-restored to %u degrees (attempt %u/3; disable via wheel_range_restore)\n",
			 want, ff->range_restore_attempts);
		sysfs_notify(&hidpp->hid_dev->dev.kobj, NULL, "wheel_range");
	} else {
		dd_warn(hidpp->hid_dev,
			 "rotation range auto-restore to %u degrees failed\n",
			 want);
	}
	if (ff->range_restore_attempts == 3) {
		ff->restore_want = 0;
		dd_warn(hidpp->hid_dev,
			 "an external writer keeps changing the rotation range; giving up on auto-restore for this session\n");
	}
}

static void hidpp_dd_ff_range_readback(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hidpp_report response;
	u8 params[3] = {0, 0, 0};
	u16 hw_range, cached;
	int ret;

	if (ff->idx_range == HIDPP_DD_FEATURE_NOT_FOUND)
		return;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_range,
					  HIDPP_DD_HIDPP_FN_GET, params, 0,
					  &response);
	if (ret)
		return;

	hw_range = (response.fap.params[0] << 8) | response.fap.params[1];
	if (hw_range < 90 || hw_range > 2700)
		return;

	cached = READ_ONCE(ff->range);
	if (hw_range == cached)
		return;

	WRITE_ONCE(ff->range, hw_range);
	dd_info(hidpp->hid_dev,
		 "rotation range changed externally: %u -> %u degrees (not set via this driver; typically a game launch). wheel_range now reports the real value\n",
		 cached, hw_range);
	sysfs_notify(&hidpp->hid_dev->dev.kobj, NULL, "wheel_range");

	/*
	 * Only the known pathology earns a pending restore: an external
	 * reset landing exactly on 90 (the SDK session-init push). Any
	 * other externally-set value is a game applying its configured
	 * steering lock - legitimate intent, respected as-is.
	 */
	if (hw_range == 90 && cached != 90)
		ff->restore_want = cached;
}

/*
 * Self-arming range poll, on system_unbound_wq: the synchronous HID++
 * GET above can block for seconds if the wheel stops answering, so it
 * must never share a queue with the 500 Hz force stream. Skipped while
 * effects play - the silent range reset this poll hunts happens at
 * game launch (FFB idle), and a stale reading during a race is
 * corrected within one interval of the effects stopping.
 */
static void hidpp_dd_ff_range_poll_work(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
					       range_poll_work.work);

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	if (!READ_ONCE(ff->any_effect_playing)) {
		hidpp_dd_ff_range_readback(ff);
		/* Retry any owed restore until it lands or strikes out. */
		hidpp_dd_ff_range_maybe_restore(ff);
	}

	if (!atomic_read_acquire(&ff->stopping) && atomic_read(&ff->initialized))
		queue_delayed_work(system_unbound_wq, &ff->range_poll_work,
				   msecs_to_jiffies(HIDPP_DD_FF_REFRESH_INTERVAL_MS));
}

/*
 * Periodic FFB refresh handler - sends the 05 07 command to maintain FFB state.
 * Our cadence is HIDPP_DD_FF_REFRESH_INTERVAL_MS (20 s); G Hub runs a similar
 * keepalive to prevent FFB timeout during idle periods.
 */
static void hidpp_dd_ff_refresh_work(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
					       refresh_work.work);
	struct hid_device *hdev;
	u8 *refresh_cmd;
	int ret;

	/* Abort if shutting down or not initialized (container_of guarantees ff valid) */
	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	/*
	 * Cache ff_hdev locally using READ_ONCE to prevent TOCTOU race.
	 * Destroy may set ff_hdev to NULL between our check and use.
	 */
	hdev = READ_ONCE(ff->ff_hdev);
	if (!hdev)
		return;

	/*
	 * Allocate DMA-safe buffer for USB transfer.
	 * Stack buffers are NOT DMA-safe on many architectures (ARM, VMAP_STACK).
	 */
	refresh_cmd = kzalloc(HIDPP_DD_FF_REPORT_SIZE, GFP_KERNEL);
	if (!refresh_cmd)
		return;

	/* Build the 05 07 refresh command */
	refresh_cmd[0] = HIDPP_DD_FF_REFRESH_ID;	/* 0x05 */
	refresh_cmd[1] = HIDPP_DD_FF_REFRESH_CMD;	/* 0x07 */
	refresh_cmd[7] = 0xFF;
	refresh_cmd[8] = 0xFF;

	/* Send the refresh command */
	ret = hid_hw_output_report(hdev, refresh_cmd, HIDPP_DD_FF_REPORT_SIZE);
	if (ret < 0 && ret != -EIO && ret != -ENODEV) {
		/* output_report not available, try raw_request instead */
		ret = hid_hw_raw_request(hdev, HIDPP_DD_FF_REFRESH_ID,
					 refresh_cmd, HIDPP_DD_FF_REPORT_SIZE,
					 HID_OUTPUT_REPORT, HID_REQ_SET_REPORT);
	}

	kfree(refresh_cmd);

	if (ret < 0) {
		/* Only log occasional errors to avoid flooding */
		if (time_after(jiffies, ff->last_err_log + HZ * 60)) {
			dd_warn(hdev, "FFB keepalive failed (error %d) - force feedback may stop working\n", ret);
			ff->last_err_log = jiffies;
		}
	}

	/* Reschedule if still running - use dedicated workqueue for consistency */
	if (!atomic_read_acquire(&ff->stopping) && atomic_read(&ff->initialized)) {
		queue_delayed_work(ff->wq, &ff->refresh_work,
				   msecs_to_jiffies(HIDPP_DD_FF_REFRESH_INTERVAL_MS));
	}
}

/* Forward declaration */
static void hidpp_dd_ff_init_work(struct work_struct *work);
static void hidpp_dd_ff_query_settings(struct hidpp_dd_ff_data *ff);

/*
 * Re-query device settings after a profile change so sysfs reflects the
 * new profile's range/strength/damping/etc. Triggered from the
 * profile-change broadcast handler (user picked a profile from the
 * wheel-base Settings menu) or from hidpp_dd_set_mode after a successful
 * sysfs-driven switch.
 */
static void hidpp_dd_ff_settings_refresh_work(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
					       settings_refresh_work);

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;
	hidpp_dd_ff_query_settings(ff);
}

/*
 * Handle device-pushed broadcasts from interface 1.
 *
 * Profile-changed event: feature 0x80D0 emits `<rep> <dev_idx>
 * <idx_profile_notify> 0x10 <new_profile> 0x01 ...`. Caused by the
 * user picking a profile via the wheel-base Settings menu.
 *
 * Rotation-changed event: feature 0x8138 emits `<rep> <dev_idx>
 * <idx_range> 0x00 <range_hi> <range_lo> ...`. Firmware pushes this
 * whenever the active range changes (typically as a side effect of
 * profile switch, but also hardware-driven adjustments).
 *
 * Both cases update the local cache immediately and schedule a full
 * re-query so dependent settings (strength, damping, etc.) follow.
 *
 * Runs from hidpp_raw_event in softirq context: no sync HID++ calls.
 * Returns 1 to swallow the event, 0 to let further processing continue.
 */
static int hidpp_dd_ff_raw_hidpp_event(struct hidpp_device *hidpp, u8 *data,
				   int size)
{
	struct hidpp_dd_ff_data *ff = READ_ONCE(hidpp->private_data);
	bool is_long;

	if (!ff || !(hidpp->quirks & HIDPP_QUIRK_DD_FFB))
		return 0;
	/*
	 * Only gate on `stopping`. The broadcast cache updates below are
	 * pure field writes (current_profile, mode, range); they do not
	 * need the FFB runtime (effect timer, workqueue, input FF device)
	 * to be ready - a broadcast can legitimately arrive during the
	 * deferred-init window before `initialized` is set, and gating on
	 * that flag here would silently discard the profile- and
	 * rotation-change broadcast until the user manually re-queried
	 * via sysfs.
	 */
	if (atomic_read_acquire(&ff->stopping))
		return 0;

	/*
	 * Broadcasts arrive on interface 1 as LONG or VERY_LONG reports
	 * depending on device firmware; accept either. SHORT reports
	 * aren't used by these events on these wheels.
	 */
	if (size < 5)
		return 0;
	is_long = data[0] == REPORT_ID_HIDPP_LONG ||
		  data[0] == REPORT_ID_HIDPP_VERY_LONG;
	if (!is_long)
		return 0;

	/*
	 * HID++ feature indices are per-device-index tables, so a feature
	 * index only means what we think it means on the wheel BASE
	 * (device index 0xff). We actively talk to sub-devices 0x01/0x02/
	 * 0x05 (pedal base, motor unit), which have their own feature
	 * tables; an unsolicited event from one of those could carry a
	 * feature index that happens to collide with idx_brightness /
	 * idx_lightsync / idx_profile_notify / idx_range on the base and
	 * be misparsed. Gate every handler below on the base index
	 * (0xff = the corded wheel itself, as seen on every base GET
	 * response and broadcast in the captures).
	 */
	if (data[1] != 0xff)
		return 0;

	/*
	 * Profile-changed: <rep> <dev> <idx_profile_notify> <fn|sw> <new> ...
	 *
	 * Earlier analysis on RS50 expected fn=1, but fresh G Pro captures
	 * (issue #15, 2026-04-19) show fn=0 for every profile broadcast.
	 * The discriminator is really sw_id == 0 (unsolicited), not the
	 * function number: our own requests always carry sw_id=1 and G Hub
	 * uses 0xa/0xb, so any sw_id==0 packet on this feature is a device
	 * broadcast.
	 */
	if (ff->idx_profile_notify != HIDPP_DD_FEATURE_NOT_FOUND &&
	    data[2] == ff->idx_profile_notify &&
	    (data[3] & 0x0F) == 0x00) {
		u8 profile = data[4];

		if (profile <= 5) {
			WRITE_ONCE(ff->current_profile, profile);
			WRITE_ONCE(ff->current_mode, (profile == 0) ? 0 : 1);
			/* An unsolicited broadcast is authoritative: the wheel
			 * just told us the live profile. Safe to trust for
			 * mode-dependent caching decisions afterwards.
			 */
			WRITE_ONCE(ff->mode_known, true);
			dd_info(hidpp->hid_dev,
				 "Profile change broadcast -> %s (profile %u)\n",
				 profile ? "onboard" : "desktop", profile);
			/* Re-query profile-dependent settings. */
			queue_work(ff->wq, &ff->settings_refresh_work);
		}
		return 1;
	}

	/*
	 * OLED-side settings edit: on the RS50 the wheel signals a profile or
	 * settings change made at its own OLED by broadcasting on the Profile
	 * feature (0x8137) itself - fn0, sw_id 0 (e.g. `12 ff 17 00 01 01 ...`)
	 * - NOT on 0x80D0 (which the G Pro uses for profile broadcasts). Without
	 * this, cached settings (strength/damping/filter/TF edited at the OLED)
	 * go stale until the next profile switch. Our own 0x8137 GET/getInfo
	 * responses are matched and consumed by the sync path before reaching
	 * here, so this only fires on genuine unsolicited broadcasts.
	 */
	if (ff->idx_profile != HIDPP_DD_FEATURE_NOT_FOUND &&
	    data[2] == ff->idx_profile &&
	    data[3] == 0x00) {
		dd_info(hidpp->hid_dev,
			 "OLED settings-edit broadcast -> re-querying settings\n");
		queue_work(ff->wq, &ff->settings_refresh_work);
		return 1;
	}

	/*
	 * Rotation-changed: <rep> <dev> <idx_range> <fn|sw=0> <hi> <lo> ...
	 *
	 * Same sw_id==0 unsolicited-broadcast gate as the profile handler
	 * above: discriminates device-originated notifications from GET
	 * responses to our own requests (which carry sw_id=1).
	 */
	if (size >= 6 &&
	    ff->idx_range != HIDPP_DD_FEATURE_NOT_FOUND &&
	    data[2] == ff->idx_range &&
	    (data[3] & 0x0F) == 0x00) {
		u16 range = ((u16)data[4] << 8) | data[5];

		if (range > 0 && range <= 2700) {
			WRITE_ONCE(ff->range, range);
			dd_info(hidpp->hid_dev,
				 "Rotation change broadcast -> %u degrees\n",
				 range);
		}
		return 1;
	}

	/*
	 * BrightnessControl events (x8040 official spec): event 0 is
	 * brightnessChangeEvent with a BE16 brightness, fired on
	 * user-initiated changes (the wheel's OLED menu) and after a
	 * rounded setBrightness; event 1 is illuminationChangeEvent.
	 * Without this handler the led_brightness cache went stale
	 * whenever brightness was changed on the wheel itself. This is
	 * LED brightness only: the old model that 0x8040 doubled as
	 * desktop sensitivity was disproved on hardware (writes only dim
	 * the LEDs; G Hub's Sensitivity slider is a 0x80A4 curve upload).
	 * Same sw_id==0 unsolicited-broadcast gate as the handlers above.
	 */
	if (ff->idx_brightness != HIDPP_DD_FEATURE_NOT_FOUND &&
	    data[2] == ff->idx_brightness &&
	    (data[3] & 0x0F) == 0x00) {
		u8 evt = data[3] >> 4;

		if (evt == 0 && size >= 6) {
			u16 raw = ((u16)data[4] << 8) | data[5];
			u8 val = min_t(u16, raw, 100);

			WRITE_ONCE(ff->led_brightness, val);
			dd_info(hidpp->hid_dev,
				 "Brightness change broadcast -> %u%%\n",
				 val);
			sysfs_notify(&hidpp->hid_dev->dev.kobj, NULL,
				     "wheel_led_brightness");
		} else if (evt == 1) {
			dd_dbg(hidpp->hid_dev,
				"Illumination change broadcast -> %u\n",
				data[4]);
		} else {
			return 0;	/* unknown event: let others look */
		}
		return 1;
	}

	/*
	 * LIGHTSYNC effect-change broadcast: `12ff<idx>00 <effect>` fires
	 * whenever the active LED effect changes (G Hub writes, and
	 * presumably the wheel's own UI). Confirmed across seven captures
	 * (2026-01-26_lightsync, 2026-01-30_onboard_led_effect, ...) with
	 * effect values 1-9 - the same ID space the fn1 supported-effect
	 * list advertises. Keeps the led_effect cache honest.
	 */
	if (ff->idx_lightsync != HIDPP_DD_FEATURE_NOT_FOUND &&
	    data[2] == ff->idx_lightsync &&
	    data[3] == 0x00 && size >= 5) {
		u8 effect = data[4];

		if (effect >= 1 && effect <= 9) {
			WRITE_ONCE(ff->led_effect, effect);
			dd_info(hidpp->hid_dev,
				 "LED effect change broadcast -> %u\n",
				 effect);
			sysfs_notify(&hidpp->hid_dev->dev.kobj, NULL,
				     "wheel_led_effect");
		}
		return 1;
	}

	return 0;
}

/*
 * Discover HID++ feature indices for the "settings" surface: per-wheel
 * tuning exposed as wheel_* sysfs attributes, plus profile / mode /
 * calibrate. These features are shared between RS50 and G Pro (though
 * the G Pro init path has its own inline discovery currently).
 * Per-feature failures are non-fatal; the idx stays HIDPP_DD_FEATURE_NOT_FOUND
 * and dependent sysfs handlers return -EOPNOTSUPP.
 */
static void hidpp_dd_discover_settings_features(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	int ret;

	ff->idx_range = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_strength = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_damping = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_trueforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_brakeforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_filter = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_response_curve = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_pedal_curve = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->pedal_sens[0] = ff->pedal_sens[1] = ff->pedal_sens[2] = 50;
	ff->idx_brightness = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_profile = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_profile_notify = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_calibrate = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_angle = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_strength = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_trueforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_damping = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_filter = HIDPP_DD_FEATURE_NOT_FOUND;

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_RANGE, &ff->idx_range);
	if (ret == 0)
		dd_dbg(hid, "Range feature at index 0x%02x\n", ff->idx_range);
	else if (ret != -ENOENT)
		dd_dbg(hid, "Range feature lookup failed: %d\n", ret);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_STRENGTH, &ff->idx_strength);
	if (ret == 0)
		dd_dbg(hid, "Strength feature at index 0x%02x\n", ff->idx_strength);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_DAMPING, &ff->idx_damping);
	if (ret == 0)
		dd_dbg(hid, "Damping feature at index 0x%02x\n", ff->idx_damping);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_TRUEFORCE, &ff->idx_trueforce);
	if (ret == 0)
		dd_dbg(hid, "TRUEFORCE feature at index 0x%02x\n", ff->idx_trueforce);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_BRAKEFORCE, &ff->idx_brakeforce);
	if (ret == 0)
		dd_dbg(hid, "Brake force feature at index 0x%02x\n", ff->idx_brakeforce);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_FILTER, &ff->idx_filter);
	if (ret == 0)
		dd_dbg(hid, "FFB filter feature at index 0x%02x\n", ff->idx_filter);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_RESPONSE_CURVE,
				     &ff->idx_response_curve);
	if (ret == 0)
		dd_dbg(hid, "Response curve feature at index 0x%02x\n",
		       ff->idx_response_curve);

	/*
	 * The pedal unit is a separate MCU (device index 0x02) that exposes the
	 * same 0x80A4 feature at its own index, covering three axes (0=throttle,
	 * 1=brake, 2=clutch). Hardware-verified 2026-07-16: the pedal MCU applies
	 * an uploaded curve to its PC HID output, so these are real shaping
	 * controls, not just onboard/console storage.
	 */
	ret = hidpp_root_get_feature_on_device(hidpp, HIDPP_DD_PEDAL_DEV_IDX,
					       HIDPP_DD_PAGE_RESPONSE_CURVE,
					       &ff->idx_pedal_curve);
	if (ret == 0)
		dd_dbg(hid, "Pedal response curve feature at index 0x%02x (dev 0x%02x)\n",
		       ff->idx_pedal_curve, HIDPP_DD_PEDAL_DEV_IDX);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_BRIGHTNESS, &ff->idx_brightness);
	if (ret == 0)
		dd_dbg(hid, "LED brightness feature at index 0x%02x\n", ff->idx_brightness);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_PROFILE, &ff->idx_profile);
	if (ret == 0)
		dd_dbg(hid, "Profile feature at index 0x%02x\n", ff->idx_profile);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_PROFILE_NOTIFY, &ff->idx_profile_notify);
	if (ret == 0)
		dd_dbg(hid, "Profile notify feature at index 0x%02x\n", ff->idx_profile_notify);

	/*
	 * Centre calibration lives on sub-device 0x05, matching the G Pro.
	 * RS50 captures (2026-04-22_re_calibrate.pcapng) show G Hub issuing
	 *   10 05 <idx> 1a 00 00 00   (fn=1 GET current encoder)
	 *   11 05 <idx> 1a <hi> <lo>  (device returns raw position)
	 *   10 05 <idx> 3a <hi> <lo>  (fn=3 SET centre to that value)
	 * where <idx> was 0x0f for the captured wheel. Root feature 0x0001
	 * on the 0x05 sub-device gives us the correct index at runtime.
	 */
	ret = hidpp_root_get_feature_on_device(hidpp, ff->calibrate_dev_idx,
					       HIDPP_DD_PAGE_CALIBRATE,
					       &ff->idx_calibrate);
	if (ret == 0)
		dd_dbg(hid, "Calibrate feature at dev 0x%02x index 0x%02x\n",
			ff->calibrate_dev_idx, ff->idx_calibrate);

	hidpp_dd_query_device_identity(ff);
}

/*
 * Format one DeviceInfo getFwInfo response (official x0003 layout:
 * type, 3-char ASCII prefix, BCD number, BCD revision, BE16 BCD build)
 * as e.g. "U1 65.03.B0038". Non-printable prefix bytes are skipped
 * (the wheel pads short names with NULs).
 */
static void hidpp_dd_format_fw_entity(const u8 *p, char *out, size_t len)
{
	char name[4];
	int i, n = 0;

	for (i = 1; i <= 3; i++)
		if (p[i] >= 0x20 && p[i] < 0x7f)
			name[n++] = p[i];
	name[n] = '\0';
	scnprintf(out, len, "%s %02x.%02x.B%02x%02x",
		  name, p[4], p[5], p[6], p[7]);
}

/*
 * Read the wheel's identity from DeviceInfo (feature 0x0003): the real
 * 12-character serial number (fn2, gated on the capabilities bit;
 * live-verified identical to the USB iSerial descriptor) and the
 * active main-firmware version, plus the motor unit's own firmware
 * from sub-device 0x05's DeviceInfo (entity type 0 = active FW; the
 * base reports e.g. "U1 65.03.B0038", the motor "SC 02.01.B0042").
 * Logged once at init - invaluable for correlating firmware-dependent
 * behaviour in issue reports - and exposed via the wheel_serial /
 * wheel_firmware attributes. All reads; failures leave fields empty.
 */
static void hidpp_dd_query_device_identity(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3] = { 0, 0, 0 };
	u8 idx, entities;
	int ret, i;

	ret = hidpp_root_get_feature(hidpp, 0x0003, &idx);
	if (ret)
		return;

	ret = hidpp_send_fap_command_sync(hidpp, idx, HIDPP_DD_HIDPP_FN_GET_INFO,
					  params, 0, &response);
	if (ret)
		return;
	entities = response.fap.params[0];

	/* capabilities byte 14, bit 0 = serialNumber (fn2) supported */
	if (response.fap.params[14] & 0x01) {
		ret = hidpp_send_fap_command_sync(hidpp, idx,
						  HIDPP_DD_HIDPP_FN_SET /* fn2 getDeviceSerialNumber */,
						  params, 0, &response);
		if (ret == 0) {
			for (i = 0; i < 12; i++) {
				u8 c = response.fap.params[i];

				if (c < 0x20 || c >= 0x7f)
					break;
				ff->serial[i] = c;
			}
			ff->serial[i] = '\0';
		}
	}

	for (i = 0; i < min_t(int, entities, 4); i++) {
		params[0] = i;
		ret = hidpp_send_fap_command_sync(hidpp, idx,
						  HIDPP_DD_HIDPP_FN_GET,
						  params, 1, &response);
		if (ret)
			continue;
		if (response.fap.params[0] == 0x00)	/* main application FW */
			hidpp_dd_format_fw_entity(response.fap.params,
					      ff->fw_main, sizeof(ff->fw_main));
	}

	/* Motor unit firmware: sub-device 0x05 has its own DeviceInfo. */
	if (hidpp_root_get_feature_on_device(hidpp, 0x05, 0x0003, &idx) == 0) {
		for (i = 0; i < 4; i++) {
			params[0] = i;
			ret = hidpp_send_fap_to_device_sync(hidpp, 0x05, idx,
							    HIDPP_DD_HIDPP_FN_GET,
							    params, 1, &response);
			if (ret)
				break;
			if (response.fap.params[0] == 0x00) {
				hidpp_dd_format_fw_entity(response.fap.params,
						      ff->fw_motor,
						      sizeof(ff->fw_motor));
				break;
			}
		}
	}

	dd_info(hid, "serial %s, base FW %s, motor FW %s\n",
		 ff->serial[0] ? ff->serial : "?",
		 ff->fw_main[0] ? ff->fw_main : "?",
		 ff->fw_motor[0] ? ff->fw_motor : "?");
}

/*
 * Discover HID++ feature indices for the RS50's custom LIGHTSYNC LED
 * system. These features are RS50-specific in current driver scope
 * (the G Pro's LIGHTSYNC wiring has not been byte-verified yet).
 * Per-feature failures are non-fatal; a wheel that lacks any of these
 * simply cannot drive its RGB ring via this driver.
 */
static void hidpp_dd_discover_lightsync_features(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	int ret;

	ff->idx_lightsync = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_rgb_config = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_sync = HIDPP_DD_FEATURE_NOT_FOUND;

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_LIGHTSYNC, &ff->idx_lightsync);
	if (ret == 0)
		dd_dbg(hid, "Lightsync feature at index 0x%02x\n", ff->idx_lightsync);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_RGB_CONFIG, &ff->idx_rgb_config);
	if (ret == 0)
		dd_dbg(hid, "RGB config feature at index 0x%02x\n", ff->idx_rgb_config);

	ret = hidpp_root_get_feature(hidpp, HIDPP_DD_PAGE_SYNC, &ff->idx_sync);
	if (ret == 0)
		dd_dbg(hid, "Sync feature at index 0x%02x\n", ff->idx_sync);
}

/*
 * Top-level discovery entry point. Runs both halves; call sites that
 * only need settings (no LIGHTSYNC ring) can call the split functions
 * directly.
 */
static void hidpp_dd_ff_discover_features(struct hidpp_dd_ff_data *ff)
{
	struct hid_device *hid = ff->hidpp->hid_dev;

	dd_dbg(hid, "Discovering HID++ features\n");
	hidpp_dd_discover_settings_features(ff);
	hidpp_dd_discover_lightsync_features(ff);
	dd_dbg(hid, "Feature discovery completed\n");
}

/*
 * Query current mode/profile from device.
 * Feature 0x8137 fn1 returns: [profile] [mode?] ...
 * Profile 0 = Desktop mode, Profiles 1-5 = Onboard mode
 */
static int hidpp_dd_get_current_mode(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3] = {0, 0, 0};
	int ret;

	if (ff->idx_profile == HIDPP_DD_FEATURE_NOT_FOUND) {
		dd_dbg(hid, "Profile feature not found, defaulting to desktop mode\n");
		ff->current_profile = 0;
		ff->current_mode = 0;
		ff->mode_known = false;
		return 0;
	}

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_profile,
					  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
	if (ret) {
		dd_warn(hid, "Failed to query mode: %d, assuming desktop\n",
			 ret);
		ff->current_profile = 0;
		ff->current_mode = 0;
		ff->mode_known = false;
		return ret;
	}

	/*
	 * fn=1 GET response layout, settled live 2026-07-02 against the
	 * wheel's own OLED: params[0] = profile index (0 = desktop,
	 * 1..5 = onboard slot), params[1] = mode flag. An earlier
	 * capture note misread this as [mode_class, slot] and a decode
	 * based on it reported "profile 1" while the wheel sat on slot 2;
	 * the plain params[0] read (matching the SET encoding and the
	 * native spec) is correct on both native and compat.
	 */
	ff->current_profile = response.fap.params[0];
	ff->current_mode = (ff->current_profile == 0) ? 0 : 1;
	ff->mode_known = true;

	dd_info(hid, "Current mode: %s (profile %d)\n",
		 ff->current_mode ? "onboard" : "desktop", ff->current_profile);

	return 0;
}

/*
 * Set mode/profile on device.
 * Profile 0 = Desktop mode
 * Profiles 1-5 = Onboard profiles
 */
static int hidpp_dd_set_mode(struct hidpp_dd_ff_data *ff, u8 profile)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3];
	int ret;

	if (ff->idx_profile == HIDPP_DD_FEATURE_NOT_FOUND) {
		dd_warn(hid, "Profile feature not found\n");
		return -ENODEV;
	}

	if (profile > 5) {
		dd_warn(hid, "Invalid profile %d (must be 0-5)\n", profile);
		return -EINVAL;
	}

	/*
	 * Feature 0x8137 fn=2 wire format, settled live 2026-07-02
	 * against the wheel's OLED and the raw G Hub packets: the SET
	 * takes the plain profile number in params[0] (`10ff172d 03` =
	 * slot 3, empty/0 = desktop) - same on native and compat, and
	 * symmetric with the fn=1 GET ([profile][mode], see
	 * hidpp_dd_get_current_mode). An earlier revision briefly encoded
	 * the SET as [0x02, slot, 0] after a capture-note misparse; the
	 * wheel reads that params[0]=2 as "profile 2" (verified: the
	 * OLED landed on slot 2's name).
	 */
	params[0] = profile;
	params[1] = 0;
	params[2] = 0;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_profile,
					  HIDPP_DD_HIDPP_FN_SET, params, 3, &response);
	if (ret) {
		dd_warn(hid, "Failed to set profile %d: %d\n", profile, ret);
		return ret;
	}

	ff->current_profile = profile;
	ff->current_mode = (profile == 0) ? 0 : 1;

	dd_info(hid, "Switched to %s mode (profile %d)\n",
		 ff->current_mode ? "onboard" : "desktop", profile);

	/*
	 * Dependent settings (range, strength, damping, ...) can change
	 * with the profile. Schedule a full re-query so sysfs doesn't hold
	 * stale values. The device usually emits a rotation broadcast too,
	 * but the settings we read via HID++ GETs don't trigger their own
	 * events.
	 */
	queue_work(ff->wq, &ff->settings_refresh_work);

	return 0;
}

/* Forward declarations for LIGHTSYNC functions */
static int hidpp_dd_lightsync_enable(struct hidpp_device *hidpp, struct hidpp_dd_ff_data *ff);
static void hidpp_dd_lightsync_query_slot_names(struct hidpp_device *hidpp,
					    struct hidpp_dd_ff_data *ff);
static void hidpp_dd_lightsync_query_slot_configs(struct hidpp_device *hidpp,
					      struct hidpp_dd_ff_data *ff);
static int hidpp_dd_lightsync_apply_slot(struct hidpp_device *hidpp,
				     struct hidpp_dd_ff_data *ff, u8 slot,
				     bool set_effect);

/*
 * Query current device settings using discovered feature indices.
 */
/*
 * Query the device for its current values of the common settings
 * (range, strength, damping, trueforce, brakeforce, ffb_filter,
 * brightness) and populate the ff cache. Each feature is
 * independent; a missing or failing query leaves the pre-populated
 * default alone. Shared by the RS50 and G Pro settings init paths so
 * they cannot drift on which settings get queried (SYS.F15).
 */
static void hidpp_dd_ff_query_common_settings(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3] = {0, 0, 0};
	int ret;
	u16 value;

	if (ff->idx_range != HIDPP_DD_FEATURE_NOT_FOUND) {
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_range,
						  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
		if (ret == 0) {
			value = (response.fap.params[0] << 8) | response.fap.params[1];
			if (value >= 90 && value <= 2700) {
				WRITE_ONCE(ff->range, value);
				hid_dbg(hid, "Wheel: range = %d degrees\n", value);
			}
		}
	}

	if (ff->idx_strength != HIDPP_DD_FEATURE_NOT_FOUND) {
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_strength,
						  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
		if (ret == 0) {
			value = (response.fap.params[0] << 8) | response.fap.params[1];
			ff->strength = value;
			hid_dbg(hid, "Wheel: strength = %d%%\n",
				DIV_ROUND_CLOSEST(value * 100, 65535));
		}
	}

	if (ff->idx_damping != HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * Damping is GET on fn0, not fn1. Unlike range/strength/brake
		 * (fn1 = GET, fn2 = SET), the damping feature uses fn0 = GET and
		 * fn1 = SET, and an empty-payload fn1 is "set damping = 0". This
		 * code used to read via fn1, which ZEROED the wheel's damping on
		 * every probe and every profile/mode switch and then cached 0.
		 * Verified live: fn0 reads the current value; the old fn1 read
		 * emitted a damping-changed-to-0 event.
		 */
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_damping,
						  HIDPP_DD_HIDPP_FN_GET_INFO, params, 0, &response);
		if (ret == 0) {
			value = (response.fap.params[0] << 8) | response.fap.params[1];
			ff->damping = value;
			hid_dbg(hid, "Wheel: damping = %d%%\n",
				DIV_ROUND_CLOSEST(value * 100, 65535));
		}
	}

	if (ff->idx_trueforce != HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * TrueForce current value is GET on fn2 (HIDPP_DD_HIDPP_FN_SET's
		 * 0x20 numbering, not a set here). fn0 returns a constant max
		 * (0xffff) and fn1 is the change-event slot that answers a
		 * solicited read with 0, so the old fn1 read cached a bogus 0%.
		 * Verified live and against G HUB's read function.
		 */
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_trueforce,
						  HIDPP_DD_HIDPP_FN_SET, params, 0, &response);
		if (ret == 0) {
			value = (response.fap.params[0] << 8) | response.fap.params[1];
			ff->trueforce = value;
			hid_dbg(hid, "Wheel: TRUEFORCE = %d%%\n",
				DIV_ROUND_CLOSEST(value * 100, 65535));
		}
	}

	if (ff->idx_brakeforce != HIDPP_DD_FEATURE_NOT_FOUND) {
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_brakeforce,
						  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
		if (ret == 0) {
			value = (response.fap.params[0] << 8) | response.fap.params[1];
			ff->brake_force = value;
			hid_dbg(hid, "Wheel: brake force = %d%%\n",
				DIV_ROUND_CLOSEST(value * 100, 65535));
		}
	}

	if (ff->idx_filter != HIDPP_DD_FEATURE_NOT_FOUND) {
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_filter,
						  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
		if (ret == 0) {
			ff->ffb_filter_auto = (response.fap.params[0] == 0x05) ? 1 : 0;
			ff->ffb_filter = response.fap.params[2];
			hid_dbg(hid, "Wheel: FFB filter = %d, auto = %d\n",
				ff->ffb_filter, ff->ffb_filter_auto);
		}
	}

	/*
	 * Feature 0x8040 is x8040 BrightnessControl: LED brightness only.
	 * The driver used to treat it as desktop-mode sensitivity too;
	 * that was disproved on hardware (writes only dim the LEDs) and
	 * against the 2026-01-30 desktop_sensitivity capture, where G
	 * Hub's Sensitivity slider is a 0x80A4 response-curve upload on
	 * the steering axis, so the read here must never touch
	 * ff->sensitivity. Values are 16-bit big-endian per the official
	 * spec - decode both bytes, not just the LSB.
	 */
	if (ff->idx_brightness != HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * One-time getInfo probe (fn0): official layout is
		 * maxBrightness (BE16), steps LSB, capabilities,
		 * minBrightness (BE16). Validates the driver's 0-100
		 * assumption instead of hardcoding it, and captures the
		 * capability bits (events / illumination / transient).
		 */
		if (!ff->brightness_info_read) {
			ret = hidpp_send_fap_command_sync(hidpp,
					ff->idx_brightness,
					HIDPP_DD_HIDPP_FN_GET_INFO, params, 0,
					&response);
			if (ret == 0) {
				u16 max = (response.fap.params[0] << 8) |
					  response.fap.params[1];

				ff->brightness_caps = response.fap.params[3];
				ff->brightness_info_read = true;
				hid_dbg(hid,
					"Wheel: BrightnessControl max=%u caps=0x%02x\n",
					max, ff->brightness_caps);
				if (max != 100)
					hid_warn(hid,
						 "Wheel: BrightnessControl maxBrightness=%u (driver assumes 100)\n",
						 max);
			}
		}

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_brightness,
						  HIDPP_DD_HIDPP_FN_GET, params, 0, &response);
		if (ret == 0) {
			u16 raw = (response.fap.params[0] << 8) |
				  response.fap.params[1];
			u8 val = min_t(u16, raw, 100);

			ff->led_brightness = val;
			hid_dbg(hid, "Wheel: LED brightness = %d%%\n", val);
		}
	}
}

static void hidpp_dd_ff_query_settings(struct hidpp_dd_ff_data *ff)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid;
	int ret;

	if (!hidpp)
		return;
	if (atomic_read_acquire(&ff->stopping))
		return;

	hid = hidpp->hid_dev;
	dd_dbg(hid, "Querying device settings\n");

	/* Query mode/profile first - this affects which settings are available */
	hidpp_dd_get_current_mode(ff);

	hidpp_dd_ff_query_common_settings(ff);

	dd_dbg(hid, "Settings query completed\n");

	/*
	 * Enable LIGHTSYNC LED subsystem - required before LED commands
	 * work. Gated off on a real G PRO: its rim speaks a LEVEL-based
	 * rev-light dialect on the same 0x807A feature page, and running
	 * the RS50-shaped enable/query/apply sequence against it would be
	 * exactly the wrong-protocol traffic the per-model sysfs gating
	 * exists to prevent (and could disturb the arm state
	 * wheel_rev_level depends on).
	 */
	if (ff->idx_lightsync != HIDPP_DD_FEATURE_NOT_FOUND &&
	    !dd_is_real_gpro(hid)) {
		ret = hidpp_dd_lightsync_enable(hidpp, ff);
		if (ret) {
			dd_warn(hid, "Failed to enable LIGHTSYNC: %d\n", ret);
		} else {
			/* Query slot names and RGB configs from the device
			 * so the in-driver cache reflects device state before
			 * the first apply. Without query_slot_configs the cache
			 * holds driver default white and the apply below would
			 * overwrite any G Hub-saved colors (PROBE.F4).
			 */
			hidpp_dd_lightsync_query_slot_names(hidpp, ff);
			hidpp_dd_lightsync_query_slot_configs(hidpp, ff);

			/*
			 * After enabling, send initial configuration to the device.
			 * Without this, LEDs are enabled but have no config, staying dark.
			 * The sequence must be: enable (0x6C) -> set config (0x2C) -> activate (0x3C)
			 */
			dd_dbg(hid, "Sending initial LED configuration\n");
			/*
		 * set_effect=false: apply the slot's colours but do NOT force
		 * effect mode 5, so an animated effect the wheel restored from
		 * its profile survives load (issue #29's sibling for effect).
		 */
		ret = hidpp_dd_lightsync_apply_slot(hidpp, ff, ff->led_active_slot,
						    false);
			if (ret)
				dd_warn(hid, "Failed to apply initial LED config: %d\n", ret);
			else
				dd_dbg(hid, "Initial LED configuration applied\n");
		}
	}
}

/*
 * Deferred FFB initialization - waits for all USB interfaces to be ready.
 * Uses event-based retry logic instead of fixed delay.
 */
static void hidpp_dd_ff_init_work(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
					       init_work.work);
	struct hidpp_device *hidpp = ff->hidpp;
	struct hid_device *hid = hidpp->hid_dev;
	struct usb_interface *iface0, *iface2;
	struct hid_device *ff_hdev;
	struct hid_device *input_hdev;
	struct hid_input *hidinput;
	struct input_dev *input;
	int ret;
	int total_wait_ms;

	dd_dbg(hid, "FFB init attempt %d/%d\n",
		ff->init_retries + 1, HIDPP_DD_FF_MAX_INIT_RETRIES);

	/* Check if we're being shut down */
	if (atomic_read_acquire(&ff->stopping)) {
		dd_dbg(hid, "FFB init aborted - driver shutting down\n");
		return;
	}

	/*
	 * Check if FFB endpoint (interface 2) is ready.
	 * This interface handles force feedback USB transfers.
	 */
	iface2 = usb_ifnum_to_if(hid_to_usb_dev(hid), 2);
	if (!iface2) {
		dd_err(hid, "FFB init failed - USB device structure invalid\n");
		return;
	}

	ff_hdev = usb_get_intfdata(iface2);
	if (!ff_hdev) {
		if (ff->init_retries++ < HIDPP_DD_FF_MAX_INIT_RETRIES) {
			queue_delayed_work(ff->wq, &ff->init_work,
					   msecs_to_jiffies(HIDPP_DD_FF_INIT_RETRY_MS));
			return;
		}
		total_wait_ms = HIDPP_DD_FF_INIT_DELAY_MS +
				(HIDPP_DD_FF_MAX_INIT_RETRIES * HIDPP_DD_FF_INIT_RETRY_MS);
		dd_err(hid, "Force feedback unavailable - FFB endpoint did not initialize after %dms\n",
			total_wait_ms);
		return;
	}

	/*
	 * Check if wheel input device (interface 0) is ready.
	 * This interface provides the joystick/wheel input we attach FFB to.
	 */
	iface0 = usb_ifnum_to_if(hid_to_usb_dev(hid), 0);
	if (!iface0) {
		dd_err(hid, "FFB init failed - USB device structure invalid\n");
		return;
	}

	input_hdev = usb_get_intfdata(iface0);
	if (!input_hdev) {
		if (ff->init_retries++ < HIDPP_DD_FF_MAX_INIT_RETRIES) {
			queue_delayed_work(ff->wq, &ff->init_work,
					   msecs_to_jiffies(HIDPP_DD_FF_INIT_RETRY_MS));
			return;
		}
		total_wait_ms = HIDPP_DD_FF_INIT_DELAY_MS +
				(HIDPP_DD_FF_MAX_INIT_RETRIES * HIDPP_DD_FF_INIT_RETRY_MS);
		dd_err(hid, "Force feedback unavailable - wheel input device did not initialize after %dms\n",
			total_wait_ms);
		return;
	}

	/* Check if input device has been registered */
	if (list_empty(&input_hdev->inputs)) {
		if (ff->init_retries++ < HIDPP_DD_FF_MAX_INIT_RETRIES) {
			queue_delayed_work(ff->wq, &ff->init_work,
					   msecs_to_jiffies(HIDPP_DD_FF_INIT_RETRY_MS));
			return;
		}
		total_wait_ms = HIDPP_DD_FF_INIT_DELAY_MS +
				(HIDPP_DD_FF_MAX_INIT_RETRIES * HIDPP_DD_FF_INIT_RETRY_MS);
		dd_err(hid, "Force feedback unavailable - wheel not registered as input device after %dms\n",
			total_wait_ms);
		return;
	}

	hidinput = list_entry(input_hdev->inputs.next, struct hid_input, list);
	input = hidinput->input;
	if (!input) {
		dd_err(hid, "Force feedback unavailable - input device structure is invalid\n");
		return;
	}

	/* Success - log how long initialization took */
	if (ff->init_retries > 0) {
		dd_info(hid, "Device ready after %d retries (%dms)\n",
			 ff->init_retries,
			 HIDPP_DD_FF_INIT_DELAY_MS + (ff->init_retries * HIDPP_DD_FF_INIT_RETRY_MS));
	}

	/* Store references */
	ff->ff_hdev = ff_hdev;
	ff->input = input;

	dd_dbg(hid, "Setting FF capability bits\n");

	/*
	 * Advertised effect surface. Set these BEFORE input_ff_create so
	 * the kernel's ff-core can copy dev->ffbit into its own ff->ffbit
	 * bitmap (drivers/input/ff-core.c line 322-324). If the bits are
	 * only set on dev->ffbit after input_ff_create, ff->ffbit stays
	 * empty, which for most effect types still works because the
	 * compat_effect() default branch passes them through; but for
	 * FF_RUMBLE specifically compat_effect tries to convert it to
	 * FF_PERIODIC and verifies FF_PERIODIC is set in ff->ffbit, so
	 * the upload fails with -EINVAL. Setting all bits first avoids
	 * that whole class of rejection.
	 *
	 * All the condition effects (SPRING, DAMPER, FRICTION, INERTIA)
	 * are emulated in software against the live wheel state read from
	 * interface 0 input reports; the direct-drive wheel firmware itself only
	 * understands raw constant forces on interface 2 endpoint 0x03.
	 * FF_CONSTANT is the fundamental one; everything else layers on
	 * top at the hidpp_dd_ff_effect_tick level.
	 *
	 * FF_RUMBLE is a gamepad effect (strong + weak motor pair); not
	 * native to a direct-drive wheel. We approximate it as a slow
	 * square-wave shake on the single motor so games that trigger
	 * rumble on impact / low-rev cues still produce something felt.
	 * fftest's effects #4 and #5 exercise exactly this path.
	 */
	set_bit(FF_CONSTANT, input->ffbit);
	set_bit(FF_SPRING, input->ffbit);
	set_bit(FF_DAMPER, input->ffbit);
	set_bit(FF_FRICTION, input->ffbit);
	set_bit(FF_INERTIA, input->ffbit);
	set_bit(FF_RAMP, input->ffbit);
	set_bit(FF_PERIODIC, input->ffbit);
	set_bit(FF_SINE, input->ffbit);
	set_bit(FF_SQUARE, input->ffbit);
	set_bit(FF_TRIANGLE, input->ffbit);
	set_bit(FF_SAW_UP, input->ffbit);
	set_bit(FF_SAW_DOWN, input->ffbit);
	set_bit(FF_RUMBLE, input->ffbit);
	/* Gain control */
	set_bit(FF_GAIN, input->ffbit);
	/*
	 * Emulated autocenter (driver-side centring spring). Advertising
	 * the bit matters beyond the feature itself: games conventionally
	 * write FF_AUTOCENTER 0 before taking over FFB, which now
	 * correctly disables a user-set centring spring for the session.
	 */
	set_bit(FF_AUTOCENTER, input->ffbit);

	/* Create FF device with our custom handlers */
	ret = input_ff_create(input, HIDPP_DD_FF_MAX_EFFECTS);
	if (ret) {
		dd_err(hid, "Force feedback unavailable - kernel FF subsystem error %d\n", ret);
		return;
	}

	input->ff->private = ff;
	input->ff->upload = hidpp_dd_ff_upload;
	input->ff->erase = hidpp_dd_ff_erase;
	input->ff->playback = hidpp_dd_ff_playback;
	input->ff->set_gain = hidpp_dd_ff_set_gain;
	input->ff->set_autocenter = hidpp_dd_ff_set_autocenter;

	/*
	 * Open interface 2's HID device for FFB I/O.
	 * This enables hid_hw_output_report() to send FFB commands.
	 * Without this, output reports to interface 2 will fail silently.
	 */
	ret = hid_hw_open(ff_hdev);
	if (ret) {
		dd_err(hid, "Cannot open FFB interface (error %d) - FFB disabled\n", ret);
		input_ff_destroy(input);
		return;
	}
	ff->ff_hdev_open = true;

	/* Mark as fully initialized - timer was already set up in hidpp_dd_ff_init() */
	atomic_set(&ff->initialized, 1);

	/*
	 * Start the periodic FFB refresh timer (05 07 command).
	 * Runs every HIDPP_DD_FF_REFRESH_INTERVAL_MS (20 s) during playback.
	 *
	 * Note: The refresh command uses Report ID 0x05, which is not declared
	 * in interface 2's HID descriptor (only Report ID 0x01 is declared).
	 * However, the device does accept this command - USB captures from
	 * Windows G Hub confirm it's sent successfully. The Linux HID layer
	 * should pass through undeclared report IDs without issue.
	 */
	/*
	 * The wheel needs NO periodic 05-07 keepalive. G HUB never sends 05 07
	 * to the wheel - in the captures those are 32-byte DualShock-4 lightbar
	 * packets to a controller that was plugged in at the time - and G HUB's
	 * interface-2 endpoint is silent at idle. Host-alive is carried by the
	 * type-01 FFB stream. refresh_work (a 64-byte 05-07 every 20 s) is
	 * therefore no longer queued; it is left dormant (not deleted) pending
	 * feel-test confirmation that FFB persists across idle without it,
	 * after which the work item and its plumbing can be removed outright.
	 *
	 * range_poll_work below is unrelated (it detects a game SDK's
	 * launch-time rotation-range reset) and stays.
	 */
	queue_delayed_work(system_unbound_wq, &ff->range_poll_work,
			   msecs_to_jiffies(HIDPP_DD_FF_REFRESH_INTERVAL_MS));

	/*
	 * Effect timer is started on-demand when effects play.
	 * The wheel requires continuous FFB commands to maintain force.
	 * Timer will be started by playback callback when needed.
	 */
	dd_info(hid, "Effect timer ready (interval=%dms, starts on effect play)\n", HIDPP_DD_FF_TIMER_INTERVAL_MS);

	/*
	 * Re-open the HID device for IO before sending HID++ commands.
	 * hidpp_probe() calls hid_hw_close() after completing, which stops
	 * the interrupt IN endpoint. We need it active to receive responses.
	 *
	 * IMPORTANT: We do NOT close it here - we keep it open for runtime
	 * HID++ communication via sysfs. It will be closed in hidpp_dd_ff_destroy().
	 */
	ret = hid_hw_open(hid);
	if (ret) {
		dd_err(hid, "Cannot read wheel settings (error %d) - using defaults\n", ret);
		goto skip_hidpp;
	}
	ff->hid_open = true;

	/* Discover HID++ feature indices before querying settings */
	hidpp_dd_ff_discover_features(ff);

	/* Query device settings to sync our cached values */
	hidpp_dd_ff_query_settings(ff);

skip_hidpp:

	dd_info(hid, "Force feedback initialized (full effect palette; conditions emulated host-side, textures via TrueForce)\n");
	dd_dbg(hid, "Init work completed successfully\n");
}

/*
 * Direct-drive wheel sysfs attributes for wheel settings (shared by RS50 and G Pro).
 * These use HID++ protocol via interface 1 to configure the wheel.
 */

static ssize_t wheel_range_show(struct device *dev, struct device_attribute *attr,
			       char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/*
	 * ff->range is written from the rotation-change broadcast handler
	 * (hidpp_raw_hidpp_event, which runs under the HID IRQ context)
	 * with WRITE_ONCE; pair the read here so the IRQ-side update is
	 * observed cleanly on weakly ordered architectures. Other scalar
	 * settings (strength, damping, ...) don't have a broadcast writer
	 * and are fine with plain accesses.
	 */
	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->range));
}

/*
 * G Pro compat-mode steering angle path. Used when the wheel is enumerated
 * as G Pro Xbox / PS (PID c272/c268) but is actually an RS50 in
 * compatibility mode - in that mode the standard 0x812F-class range
 * feature is not advertised, but the wheel exposes a different feature
 * pair that GHUB drives instead:
 *
 *   HID++ feature 0x8138 fn 2 - set live steering angle
 *       params: [angle_hi, angle_lo, 0x00] (16-bit big-endian degrees)
 *   HID++ feature 0x8137 fn 2 - switch profile / mode (already wired
 *       as ff->idx_profile by hidpp_dd_discover_settings_features)
 *       params: [0x00, 0x00, 0x00] = desktop mode (verified)
 *               [0x02, slot, 0x00] = onboard slot 1..5 (encoding for
 *                                    N>0 not yet fully verified)
 *
 * Compat-mode feature IDs were derived from USBPcap captures of GHUB
 * driving this wheel firmware (dev/captures/2026-04-26_compat_*.pcapng).
 * They are looked up via ROOT.GetFeature so the driver still works if
 * a future firmware revision reorders the feature table; the resulting
 * indices are cached on hidpp_dd_ff_data so we only pay the discovery cost
 * once per wheel session.
 *
 * The wheel must be in desktop mode for the live angle command to
 * take effect (an onboard profile loaded into the active slot pins
 * its own stored angle). On Linux the user enters desktop mode by
 * writing 0 to wheel_profile, which sends feature 0x8137 fn=2 with
 * params [0, 0, 0]; the wheel honours this in compat mode just as
 * it does in native mode. The compat-mode mode-switch was decoded
 * 2026-04-26 from a take-control USBPcap capture and verified end-
 * to-end against the live wheel.
 */
/*
 * Feature IDs and known-working indices, both empirically derived. We try
 * ROOT.GetFeature first (portable across hypothetical firmware revisions)
 * and fall back to the hardcoded indices we observed working on the
 * 2026-04-26 capture wheel.
 */
/* Per-setting feature IDs and fallback indices, all derived from
 * USBPcap captures of GHUB driving a 2026-04-26 wheel firmware.
 * Fallback indices are what we observed; ROOT.GetFeature is tried
 * first so the driver still works if a future firmware revision
 * reorders the table. Feature IDs reuse the canonical native RS50
 * IDs; whether compat firmware advertises them is firmware-
 * dependent, hence the hardcoded fallback indices.
 *
 * Mode switch in compat mode goes through feature 0x8137 (Profile,
 * already wired by hidpp_dd_discover_settings_features as ff->idx_profile)
 * with fn=2 and params [profile, 0, 0]: 0 = desktop, 1..5 = onboard
 * slot (live-verified against the OLED 2026-07-02; an interim
 * [mode_class, slot] reading of the captures was wrong).
 * The wheel boots in onboard mode in compat,
 * onboard ignores the live host SETs below, so userspace must
 * write 0 to wheel_profile first to enter desktop mode and have
 * these SETs take effect on the motor.
 *
 * An earlier draft of this file shipped a "force_desktop_mode"
 * helper that wrote 10ff1a2d 00 00 0b to feature 0x1a; the
 * dedicated filter-only capture proved that was actually setting
 * the FFB filter level to 11, not switching modes. Removed and
 * replaced with the wheel_profile path above.
 */
#define HIDPP_DD_COMPAT_FEATURE_ID_ANGLE		0x8138
#define HIDPP_DD_COMPAT_FALLBACK_IDX_ANGLE		0x18
#define HIDPP_DD_COMPAT_FN_ANGLE			(2 << 4)

#define HIDPP_DD_COMPAT_FEATURE_ID_STRENGTH		0x8136
#define HIDPP_DD_COMPAT_FALLBACK_IDX_STRENGTH	0x16
#define HIDPP_DD_COMPAT_FN_STRENGTH			(2 << 4)

#define HIDPP_DD_COMPAT_FEATURE_ID_TRUEFORCE	0x8139
#define HIDPP_DD_COMPAT_FALLBACK_IDX_TRUEFORCE	0x19
#define HIDPP_DD_COMPAT_FN_TRUEFORCE		(3 << 4)

/*
 * Damping verified at idx 0x14 fn=1 from the isolated damping-only
 * capture: GHUB's slider sweep emitted 10ff141d <BE16 0..0xFFFF> 00
 * across 0/20/50/80/100%. The earlier guess of idx 0x15 fn=2 was
 * wrong - GHUB never sends 10ff152d (sw_id=d) anywhere in any
 * capture, only 10ff152c (read) and 10ff152a (other subsystem).
 * Feature ID 0x8133 matches the canonical native damping page.
 */
#define HIDPP_DD_COMPAT_FEATURE_ID_DAMPING		0x8133
#define HIDPP_DD_COMPAT_FALLBACK_IDX_DAMPING	0x14
#define HIDPP_DD_COMPAT_FN_DAMPING			(1 << 4)

/*
 * FFB filter verified at idx 0x1a fn=2 from the isolated filter-only
 * capture: 10ff1a2d 00 00 <level> across slider values 0/3/7/10/15.
 * Compat-mode parameter format is simpler than native (no flags
 * byte): bytes 0-1 zero, byte 2 carries the 1..15 level.
 */
#define HIDPP_DD_COMPAT_FEATURE_ID_FILTER		0x8140
#define HIDPP_DD_COMPAT_FALLBACK_IDX_FILTER		0x1A
#define HIDPP_DD_COMPAT_FN_FILTER			(2 << 4)

static u8 hidpp_dd_compat_lookup(struct hidpp_device *hidpp, u16 feature_id,
			     u8 fallback_idx, const char *what)
{
	struct hid_device *hid = hidpp->hid_dev;
	u8 idx = 0;
	int ret;

	ret = hidpp_root_get_feature(hidpp, feature_id, &idx);
	if (ret == 0) {
		/*
		 * The wheel answered ROOT.GetFeature. A non-zero index means the
		 * feature is present there; index 0 is HID++ for "feature not
		 * present", so do NOT fall back to a hardcoded guess in that
		 * case - a wrong guess once set the FFB filter while trying to
		 * set something else. Report it absent; the caller returns
		 * -EOPNOTSUPP rather than poking a bystander feature.
		 */
		if (idx != 0)
			return idx;
		dd_dbg(hid, "compat: wheel reports %s (0x%04x) not present\n",
		       what, feature_id);
		return HIDPP_DD_FEATURE_NOT_FOUND;
	}
	/*
	 * Transport error, not a "not present" answer: old compat firmware may
	 * not answer ROOT.GetFeature at all. Only then fall back to the
	 * historically-verified index for this feature.
	 *
	 * BUT these fallback indices are the RS50's catalog, and PID c272/c268
	 * is shared by two very different wheels: an RS50 spoofing the G PRO
	 * PID in compatibility mode (RS50 catalog - the fallbacks are correct)
	 * and a REAL G PRO, whose catalog is shifted down by 2 (verified from
	 * the issue #8 G HUB captures: e.g. idx 0x18 is the FFB FILTER on a
	 * real G PRO, not rotation range). Applying an RS50 fallback index on a
	 * real G PRO would cross-wire a setting into a bystander feature - the
	 * same class of bug the ROOT.GetFeature guard above already prevents. A
	 * real G PRO only reaches this path if native resolution also failed,
	 * so there is no reliable index to use: report the feature absent
	 * rather than guess.
	 */
	if (dd_is_real_gpro(hid)) {
		dd_warn(hid,
			"compat: ROOT.GetFeature(0x%04x) for %s failed (%d) on a real G PRO; RS50 fallback idx 0x%02x is wrong here, reporting absent\n",
			feature_id, what, ret, fallback_idx);
		return HIDPP_DD_FEATURE_NOT_FOUND;
	}
	dd_warn(hid,
		"compat: ROOT.GetFeature(0x%04x) for %s failed (%d); using verified fallback index 0x%02x\n",
		feature_id, what, ret, fallback_idx);
	return fallback_idx;
}

/*
 * Generic 16-bit-BE compat-mode setter. Takes a feature ID, a fallback
 * index, the SET function nibble (already shifted), and a 16-bit value.
 * Caches the discovered feature index in *cached_idx so subsequent
 * calls skip the discovery round-trip. Onboard mode silently ignores
 * live SETs; userspace must write 0 to wheel_profile first to enter
 * desktop mode (feature 0x8137 fn=2 with [0,0,0]) before these writes
 * take physical effect on the motor. The compat-mode mode-switch was
 * decoded 2026-04-26 and verified end-to-end against the live wheel.
 */
static int hidpp_dd_compat_set_u16(struct hidpp_device *hidpp,
			       struct hidpp_dd_ff_data *ff,
			       u8 *cached_idx, u16 feature_id, u8 fallback_idx,
			       u8 fn, u16 value, const char *what)
{
	struct hidpp_report response;
	u8 params[3];
	int ret;

	if (*cached_idx == HIDPP_DD_FEATURE_NOT_FOUND)
		*cached_idx = hidpp_dd_compat_lookup(hidpp, feature_id,
						 fallback_idx, what);
	if (*cached_idx == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;	/* wheel says the feature isn't there */

	params[0] = (value >> 8) & 0xFF;
	params[1] = value & 0xFF;
	params[2] = 0;
	ret = hidpp_send_fap_command_sync(hidpp, *cached_idx, fn,
					  params, 3, &response);
	return hidpp_errno(hidpp->hid_dev, ret, what);
}

/*
 * Compat-mode FFB filter setter. Distinct from hidpp_dd_compat_set_u16
 * because the wire format puts the level in params[2], not as a
 * BE16 in params[0..1].
 */
static int hidpp_dd_compat_set_filter(struct hidpp_device *hidpp,
				  struct hidpp_dd_ff_data *ff, u8 level)
{
	struct hidpp_report response;
	u8 params[3];
	int ret;

	if (ff->idx_compat_filter == HIDPP_DD_FEATURE_NOT_FOUND)
		ff->idx_compat_filter = hidpp_dd_compat_lookup(hidpp,
			HIDPP_DD_COMPAT_FEATURE_ID_FILTER,
			HIDPP_DD_COMPAT_FALLBACK_IDX_FILTER, "compat set filter");
	if (ff->idx_compat_filter == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;	/* wheel says the filter feature isn't there */
	params[0] = 0x00;
	params[1] = 0x00;
	params[2] = level;
	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_compat_filter,
					  HIDPP_DD_COMPAT_FN_FILTER,
					  params, 3, &response);
	return hidpp_errno(hidpp->hid_dev, ret, "compat set filter");
}

static int hidpp_dd_compat_set_range(struct hidpp_device *hidpp,
				 struct hidpp_dd_ff_data *ff, int range)
{
	return hidpp_dd_compat_set_u16(hidpp, ff, &ff->idx_compat_angle,
		HIDPP_DD_COMPAT_FEATURE_ID_ANGLE,
		HIDPP_DD_COMPAT_FALLBACK_IDX_ANGLE,
		HIDPP_DD_COMPAT_FN_ANGLE, (u16)range, "compat set range");
}

static int hidpp_dd_compat_set_strength(struct hidpp_device *hidpp,
				    struct hidpp_dd_ff_data *ff, u16 value)
{
	return hidpp_dd_compat_set_u16(hidpp, ff, &ff->idx_compat_strength,
		HIDPP_DD_COMPAT_FEATURE_ID_STRENGTH,
		HIDPP_DD_COMPAT_FALLBACK_IDX_STRENGTH,
		HIDPP_DD_COMPAT_FN_STRENGTH, value, "compat set strength");
}

static int hidpp_dd_compat_set_trueforce(struct hidpp_device *hidpp,
				     struct hidpp_dd_ff_data *ff, u16 value)
{
	return hidpp_dd_compat_set_u16(hidpp, ff, &ff->idx_compat_trueforce,
		HIDPP_DD_COMPAT_FEATURE_ID_TRUEFORCE,
		HIDPP_DD_COMPAT_FALLBACK_IDX_TRUEFORCE,
		HIDPP_DD_COMPAT_FN_TRUEFORCE, value, "compat set trueforce");
}

static int hidpp_dd_compat_set_damping(struct hidpp_device *hidpp,
				   struct hidpp_dd_ff_data *ff, u16 value)
{
	return hidpp_dd_compat_set_u16(hidpp, ff, &ff->idx_compat_damping,
		HIDPP_DD_COMPAT_FEATURE_ID_DAMPING,
		HIDPP_DD_COMPAT_FALLBACK_IDX_DAMPING,
		HIDPP_DD_COMPAT_FN_DAMPING, value, "compat set damping");
}

/*
 * Send a rotation range to the wheel (native or compat path) and update
 * the cache on success. Shared by wheel_range_store and the range
 * auto-restore. Sleepable context.
 */
static int hidpp_dd_set_range_hw(struct hidpp_dd_ff_data *ff, int range)
{
	struct hidpp_device *hidpp = ff->hidpp;
	struct hidpp_report response;
	u8 params[3];
	int ret;

	if (ff->idx_range == HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * Compat-mode fallback: the standard 0x812F-style range
		 * feature is not advertised when the RS50 enumerates as a
		 * G Pro, but a different feature index pair (0x18 / 0x1a)
		 * accepts the same range as a live host-pushed value. See
		 * hidpp_dd_compat_set_range() for the protocol notes.
		 */
		ret = hidpp_dd_compat_set_range(hidpp, ff, range);
		if (ret)
			return ret;
	} else {
		params[0] = (range >> 8) & 0xFF;	/* High byte */
		params[1] = range & 0xFF;	/* Low byte */
		params[2] = 0;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_range,
						  ff->fn_set_range, params, 3,
						  &response);
		ret = hidpp_errno(hidpp->hid_dev, ret, "set range");
		if (ret)
			return ret;
	}

	/*
	 * Pair with the READ_ONCE in wheel_range_show and the WRITE_ONCE
	 * in the rotation-change broadcast handler.
	 */
	WRITE_ONCE(ff->range, range);
	return 0;
}

static ssize_t wheel_range_store(struct device *dev, struct device_attribute *attr,
				const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	int range, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &range);
	if (ret)
		return ret;

	/*
	 * Numeric range attrs clamp to the supported interval; enum /
	 * mode attrs reject out-of-range values with -EINVAL (see e.g.
	 * wheel_texture_route_store). Clamping is the convention for
	 * percentages, angles and filter levels across the driver.
	 */
	range = clamp(range, 90, 2700);

	ret = hidpp_dd_set_range_hw(ff, range);
	if (ret)
		return ret;

	/* A fresh explicit intent supersedes any owed auto-restore. */
	ff->range_restore_attempts = 0;
	ff->restore_want = 0;
	dd_info(hid, "Rotation range set to %d degrees\n", range);
	return count;
}

static DEVICE_ATTR(wheel_range, 0664,
		   wheel_range_show, wheel_range_store);

/*
 * Oversteer-compatible 'range' attribute - same functionality as wheel_range.
 * Named differently internally to avoid conflict with hidpp_ff's dev_attr_range.
 */
static struct device_attribute dev_attr_wheel_compat_range =
	__ATTR(range, 0664, wheel_range_show, wheel_range_store);

static ssize_t wheel_strength_show(struct device *dev, struct device_attribute *attr,
				  char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/* Convert from 0-65535 range to 0-100 percentage (rounded) */
	return sysfs_emit(buf, "%u\n", DIV_ROUND_CLOSEST(ff->strength * 100, 65535));
}

static ssize_t wheel_strength_store(struct device *dev, struct device_attribute *attr,
				   const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int strength, ret;
	u16 value;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &strength);
	if (ret)
		return ret;

	/* Clamp to 0-100% */
	strength = clamp(strength, 0, 100);

	/* Convert percentage to 0-65535 range */
	value = (strength * 65535) / 100;

	if (ff->idx_strength == HIDPP_DD_FEATURE_NOT_FOUND) {
		/* Compat-mode fallback: same encoding as native (Nm * 8192
		 * scale, capped at u16 max), different feature index. See
		 * docs/HIDPP_DD_PROTOCOL_SPECIFICATION.md section 5.1. */
		ret = hidpp_dd_compat_set_strength(hidpp, ff, value);
		if (ret)
			return ret;
	} else {
		params[0] = (value >> 8) & 0xFF;	/* High byte */
		params[1] = value & 0xFF;	/* Low byte */
		params[2] = 0;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_strength,
						  ff->fn_set_strength,
						  params, 3, &response);
		ret = hidpp_errno(hid, ret, "set strength");
		if (ret)
			return ret;
	}

	ff->strength = value;
	dd_info(hid, "FFB strength set to %d%%\n", strength);
	return count;
}

static DEVICE_ATTR(wheel_strength, 0664,
		   wheel_strength_show, wheel_strength_store);

/*
 * Oversteer-compatible 'gain' attribute. The FILE speaks the raw
 * 0-65535 scale that Oversteer (and the new-lg4ff convention) expects
 * - Oversteer converts to percent in its UI. Internally it drives the
 * same wheel strength setting as wheel_strength (which keeps its
 * human-friendly 0-100 percent scale). An earlier revision aliased
 * this file directly to wheel_strength's percent handlers, which made
 * Oversteer read 65 as "0% gain".
 */
static ssize_t wheel_compat_gain_show(struct device *dev,
				      struct device_attribute *attr, char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	return sysfs_emit(buf, "%u\n", ff->strength);
}

static ssize_t wheel_compat_gain_store(struct device *dev,
				       struct device_attribute *attr,
				       const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	char pct[8];
	int val, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &val);
	if (ret)
		return ret;
	val = clamp(val, 0, 65535);

	/* Reuse the wheel_strength store (native + compat send paths). */
	snprintf(pct, sizeof(pct), "%d", DIV_ROUND_CLOSEST(val * 100, 65535));
	ret = wheel_strength_store(dev, attr, pct, strlen(pct));
	if (ret < 0)
		return ret;
	/*
	 * wheel_strength_store already set ff->strength to the value it
	 * actually sent (percent -> u16). Leave it at that: caching the
	 * caller's exact raw value here would disagree with the hardware
	 * and with what the next settings re-query reads back, making
	 * Oversteer see a phantom external change. The <=0.15% rounding
	 * is below Oversteer's percent display resolution.
	 */
	return count;
}

static struct device_attribute dev_attr_wheel_compat_gain =
	__ATTR(gain, 0664, wheel_compat_gain_show, wheel_compat_gain_store);

/*
 * Oversteer-compatible 'autocenter' attribute: the emulated centring
 * spring (see the autocenter field and the effect-timer term). Raw
 * 0-65535 file scale per the evdev FF_AUTOCENTER / Oversteer
 * convention. Writing a nonzero value starts the effect timer so the
 * spring engages without any game effects playing.
 */
static ssize_t wheel_autocenter_show(struct device *dev, struct device_attribute *attr,
				    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->autocenter));
}

static ssize_t wheel_autocenter_store(struct device *dev, struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	int val, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &val);
	if (ret)
		return ret;

	WRITE_ONCE(ff->autocenter, clamp(val, 0, 65535));
	if (val && atomic_read(&ff->initialized))
		mod_timer(&ff->effect_timer, jiffies +
			  msecs_to_jiffies(HIDPP_DD_FF_TIMER_INTERVAL_MS));
	return count;
}

static struct device_attribute dev_attr_wheel_compat_autocenter =
	__ATTR(autocenter, 0664, wheel_autocenter_show, wheel_autocenter_store);

/*
 * Oversteer-compatible per-effect-class output scales, 0-100 percent,
 * default 100 (the new-lg4ff convention): spring_level, damper_level,
 * friction_level scale the emulated FF_SPRING / FF_DAMPER /
 * FF_FRICTION outputs respectively. Note damper_level scales DAMPER
 * EFFECTS from games; the wheel's own firmware damping stays on
 * wheel_damping.
 */
#define HIDPP_DD_LEVEL_ATTR(_name)						\
static ssize_t wheel_##_name##_show(struct device *dev,		\
				    struct device_attribute *attr,	\
				    char *buf)				\
{									\
	struct hid_device *hid = to_hid_device(dev);			\
	struct hidpp_device *hidpp = hid_get_drvdata(hid);		\
	struct hidpp_dd_ff_data *ff;					\
									\
	if (!hidpp)							\
		return -ENODEV;						\
	ff = READ_ONCE(hidpp->private_data);				\
	if (!ff)							\
		return -ENODEV;						\
	if (atomic_read_acquire(&ff->stopping))				\
		return -ENODEV;						\
	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->_name));		\
}									\
static ssize_t wheel_##_name##_store(struct device *dev,		\
				     struct device_attribute *attr,	\
				     const char *buf, size_t count)	\
{									\
	struct hid_device *hid = to_hid_device(dev);			\
	struct hidpp_device *hidpp = hid_get_drvdata(hid);		\
	struct hidpp_dd_ff_data *ff;					\
	int val, ret;							\
									\
	if (!hidpp)							\
		return -ENODEV;						\
	ff = READ_ONCE(hidpp->private_data);				\
	if (!ff)							\
		return -ENODEV;						\
	if (atomic_read_acquire(&ff->stopping))				\
		return -ENODEV;						\
	ret = kstrtoint(buf, 10, &val);					\
	if (ret)							\
		return ret;						\
	WRITE_ONCE(ff->_name, (u8)clamp(val, 0, 100));			\
	return count;							\
}									\
static struct device_attribute dev_attr_wheel_compat_##_name =		\
	__ATTR(_name, 0664, wheel_##_name##_show, wheel_##_name##_store)

HIDPP_DD_LEVEL_ATTR(spring_level);
HIDPP_DD_LEVEL_ATTR(friction_level);
HIDPP_DD_LEVEL_ATTR(damper_level);

static ssize_t wheel_damping_show(struct device *dev, struct device_attribute *attr,
				 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/* Convert from 0-65535 range to 0-100 percentage (rounded) */
	return sysfs_emit(buf, "%u\n", DIV_ROUND_CLOSEST(ff->damping * 100, 65535));
}

static ssize_t wheel_damping_store(struct device *dev, struct device_attribute *attr,
				  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int damping, ret;
	u16 value;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &damping);
	if (ret)
		return ret;

	/* Clamp to 0-100 */
	damping = clamp(damping, 0, 100);

	/* Convert to 0-65535 range */
	value = (damping * 65535) / 100;

	if (ff->idx_damping == HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * Compat mode: hidpp_dd_compat_set_damping() resolves the
		 * damping feature 0x8133 (verified fallback index 0x14, fn1).
		 * (The old comment here cited a disproven guess - 0x8137 is the
		 * Profile feature, not damping.)
		 */
		ret = hidpp_dd_compat_set_damping(hidpp, ff, value);
		if (ret)
			return ret;
	} else {
		params[0] = (value >> 8) & 0xFF;	/* High byte */
		params[1] = value & 0xFF;	/* Low byte */
		params[2] = 0;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_damping,
						  ff->fn_set_damping,
						  params, 3, &response);
		ret = hidpp_errno(hid, ret, "set damping");
		if (ret)
			return ret;
	}

	ff->damping = value;
	dd_info(hid, "Damping set to %d%%\n", damping);
	return count;
}

static DEVICE_ATTR(wheel_damping, 0664,
		   wheel_damping_show, wheel_damping_store);

/*
 * Oversteer-compatible 'damper_level' attribute - same as wheel_damping.
 */
/* TRUEFORCE - audio-haptic feedback intensity */
static ssize_t wheel_trueforce_show(struct device *dev, struct device_attribute *attr,
				   char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", DIV_ROUND_CLOSEST(ff->trueforce * 100, 65535));
}

static ssize_t wheel_trueforce_store(struct device *dev, struct device_attribute *attr,
				    const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int trueforce, ret;
	u16 value;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &trueforce);
	if (ret)
		return ret;

	trueforce = clamp(trueforce, 0, 100);
	value = (trueforce * 65535) / 100;

	if (ff->idx_trueforce == HIDPP_DD_FEATURE_NOT_FOUND) {
		/* Compat-mode fallback: feature index 0x19 fn 3 with the
		 * same 0..0xffff scale. See docs/HIDPP_DD_PROTOCOL_SPECIFICATION.md
		 * section 5.1. */
		ret = hidpp_dd_compat_set_trueforce(hidpp, ff, value);
		if (ret)
			return ret;
	} else {
		params[0] = (value >> 8) & 0xFF;
		params[1] = value & 0xFF;
		params[2] = 0;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_trueforce,
						  ff->fn_set_trueforce,
						  params, 3, &response);
		ret = hidpp_errno(hid, ret, "set TRUEFORCE");
		if (ret)
			return ret;
	}

	ff->trueforce = value;
	dd_info(hid, "TRUEFORCE set to %d%%\n", trueforce);
	return count;
}

static DEVICE_ATTR(wheel_trueforce, 0664,
		   wheel_trueforce_show, wheel_trueforce_store);

/* Brake Force - load cell threshold */
static ssize_t wheel_brake_force_show(struct device *dev, struct device_attribute *attr,
				     char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/*
	 * Brake force is only applied by the wheel in onboard mode; store
	 * rejects writes in desktop mode with -EPERM. Show always returns the
	 * last-known value so numeric parsers don't break; read wheel_mode if
	 * you need to know whether that value is currently in effect.
	 */
	return sysfs_emit(buf, "%u\n", DIV_ROUND_CLOSEST(ff->brake_force * 100, 65535));
}

static ssize_t wheel_brake_force_store(struct device *dev, struct device_attribute *attr,
				      const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int brake_force, ret;
	u16 value;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/* Brake force is only available in onboard mode (profiles 1-5) */
	if (ff->current_mode == 0) {
		dd_dbg(hid, "Brake force is only available in onboard mode\n");
		return -EPERM;
	}

	ret = kstrtoint(buf, 10, &brake_force);
	if (ret)
		return ret;

	if (ff->idx_brakeforce == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	brake_force = clamp(brake_force, 0, 100);
	value = (brake_force * 65535) / 100;

	params[0] = (value >> 8) & 0xFF;
	params[1] = value & 0xFF;
	params[2] = 0;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_brakeforce,
					  ff->fn_set_brakeforce, params, 3, &response);
	ret = hidpp_errno(hid, ret, "set brake force");
	if (ret)
		return ret;

	ff->brake_force = value;
	dd_info(hid, "Brake force set to %d%%\n", brake_force);
	return count;
}

static DEVICE_ATTR(wheel_brake_force, 0664,
		   wheel_brake_force_show, wheel_brake_force_store);

/*
 * Sensitivity - steering responsiveness (Desktop mode only).
 *
 * This is NOT feature 0x8040: hardware testing showed 0x8040 is plain
 * BrightnessControl (writes only dim the LEDs, steering is unchanged).
 * G Hub's Sensitivity slider is a 0x80A4 AxisResponseCurve upload on
 * steering axis 0, protocol-mapped from the 2026-01-30
 * desktop_sensitivity capture: for slider value v (0-100, s = v/100)
 * G Hub samples the cubic Bezier
 *
 *   B(t) = 3t(1-t)^2*P1 + 3t^2(1-t)*P2 + t^3*(1,1),
 *   P1 = (1-s, s), P2 = (s, 1-s)
 *
 * at 64 uniform t and uploads the (x,y) pairs scaled to 0-65535. The
 * captured slider positions 100/75/25/0 all match this model to within
 * 1 LSB; 50 gives the identity (G Hub sends fn6 revert-to-built-in
 * instead of uploading it). >50 sharpens the centre, <50 softens it,
 * symmetric about (32768, 32768). We reuse wheel_response_curve's
 * upload core, so this attribute is just a friendly 0-100 front-end
 * for the same hardware store.
 */
static ssize_t wheel_sensitivity_show(struct device *dev, struct device_attribute *attr,
				     char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/*
	 * The wheel has no readback for the slider value (the curve
	 * store only reports a point count), so this is a write-through
	 * cache: the last value stored here, defaulting to 50 (linear).
	 * Users who need to know the current mode can read wheel_mode.
	 */
	return sysfs_emit(buf, "%u\n", ff->sensitivity);
}

/* 64 "in:out" tokens of at most 12 chars ("65535:65535 ") + NUL. */
#define HIDPP_DD_SENS_CURVE_BUFSZ	(64 * 12 + 1)

/*
 * Render G Hub's symmetric-Bezier sensitivity curve for slider value
 * `sensitivity` (1-100 excluding 50) into `curve` (capacity `cap`) as the
 * "in:out" pair list the 0x80A4 uploader parses; returns the byte length.
 * Control coordinates scaled to 0-65535: P1 = (px, py), P2 = (py, px).
 * Integer math throughout; the +125023 / 250047 pair is round-to-nearest by
 * 63^3 (t = i/63). Verified against the capture: reproduces G Hub's uploaded
 * points for sliders 100/75/25/0 to within 1 LSB. Slider 50 is the linear
 * built-in curve and is handled by callers via fn6 revert, not here. Used for
 * both the steering (dev 0xff) and pedal (dev 0x02) sensitivity sliders.
 */
static size_t hidpp_dd_build_sensitivity_curve(int sensitivity, char *curve,
					       size_t cap)
{
	u32 px = (u32)(100 - sensitivity) * 65535 / 100;
	u32 py = (u32)sensitivity * 65535 / 100;
	u32 prev_in = 0;
	size_t len = 0;
	int i;

	for (i = 0; i < 64; i++) {
		/* Bernstein weights for t = i/63, scaled by 63^3 */
		u64 w1 = 3ULL * i * (63 - i) * (63 - i);
		u64 w2 = 3ULL * i * i * (63 - i);
		u64 w3 = (u64)i * i * i;
		u32 in = div_u64(w1 * px + w2 * py + w3 * 65535 + 125023, 250047);
		u32 out = div_u64(w1 * py + w2 * px + w3 * 65535 + 125023, 250047);

		/*
		 * x(t) is flat around t=0.5 at the slider extremes, so two
		 * samples can round to the same input value (G Hub itself
		 * uploads such duplicates at slider 0). The uploader demands
		 * strictly increasing inputs; dropping the duplicate is safe
		 * since its resampler interpolates across the gap.
		 */
		if (i > 0 && in <= prev_in)
			continue;
		prev_in = in;
		len += scnprintf(curve + len, cap - len, "%u:%u ", in, out);
	}
	return len;
}

static ssize_t wheel_sensitivity_store(struct device *dev, struct device_attribute *attr,
				      const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	int sensitivity, ret;
	size_t len;
	char *curve;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/* Sensitivity is only available in desktop mode (profile 0) */
	if (ff->current_mode != 0) {
		dd_dbg(hid, "Sensitivity is only available in desktop mode\n");
		return -EPERM;
	}

	ret = kstrtoint(buf, 10, &sensitivity);
	if (ret)
		return ret;

	if (ff->idx_response_curve == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	sensitivity = clamp(sensitivity, 0, 100);

	/*
	 * 50 = linear = the wheel's built-in curve. Mimic G Hub, which
	 * sends fn6 revert here instead of uploading an identity curve
	 * (equivalent steering-wise, but leaves the store reporting
	 * "0 = built-in" rather than 64 loaded points).
	 */
	if (sensitivity == 50) {
		ret = hidpp_dd_response_curve_revert(hidpp, 0xff, 0,
						     ff->idx_response_curve);
		ret = hidpp_errno(hid, ret, "set sensitivity");
		if (ret)
			return ret;
		ff->sensitivity = 50;
		dd_info(hid, "Sensitivity set to 50%% (linear, built-in curve)\n");
		return count;
	}

	curve = kmalloc(HIDPP_DD_SENS_CURVE_BUFSZ, GFP_KERNEL);
	if (!curve)
		return -ENOMEM;

	len = hidpp_dd_build_sensitivity_curve(sensitivity, curve,
					       HIDPP_DD_SENS_CURVE_BUFSZ);
	ret = hidpp_dd_response_curve_upload(hidpp, ff, 0xff, 0,
					     ff->idx_response_curve,
					     curve, len);
	kfree(curve);
	if (ret)
		return ret;

	ff->sensitivity = sensitivity;
	dd_info(hid, "Sensitivity set to %d%% (0x80A4 steering curve)\n",
		sensitivity);
	return count;
}

static DEVICE_ATTR(wheel_sensitivity, 0664,
		   wheel_sensitivity_show, wheel_sensitivity_store);

/* FFB Filter - smoothing level and auto toggle */
static ssize_t wheel_ffb_filter_show(struct device *dev, struct device_attribute *attr,
				    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", ff->ffb_filter);
}

static ssize_t wheel_ffb_filter_store(struct device *dev, struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int filter, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &filter);
	if (ret)
		return ret;

	/* Filter range: 1-15 (0x01-0x0F) */
	filter = clamp(filter, 1, 15);

	if (ff->idx_filter == HIDPP_DD_FEATURE_NOT_FOUND) {
		/*
		 * Compat-mode fallback: wheel does not advertise the
		 * native filter feature 0x8140 in the same place, but
		 * a sweep capture proved 10ff1a2d 00 00 <level> sets
		 * the filter. Wire format is simpler than native (no
		 * flags byte and no auto-mode encoding); compat mode
		 * has no auto path observable from the host.
		 */
		ret = hidpp_dd_compat_set_filter(hidpp, ff, (u8)filter);
		if (ret)
			return ret;
	} else {
		/*
		 * Native FFB Filter command: <flags> <0x00> <level>
		 *
		 * First byte is a small bitfield:
		 *   bit 0 (0x01): user explicitly set this level
		 *   bit 2 (0x04): auto mode enabled
		 *
		 * Captures across both wheels agree:
		 *   RS50 auto-only toggle (2026-01-26 auto_ffb_filter):  0x04 / 0x00
		 *   RS50 slider sweep (2026-01-26 ffb_filter_sweep):     0x01
		 *   G Pro slider + auto toggle (2026-04-18 round 1):     0x01 manual,
		 *                                                        0x05 auto
		 *
		 * wheel_ffb_filter is the explicit-level store, so bit 0 is always
		 * set here. wheel_ffb_filter_auto (below) owns the auto-only path
		 * and sends bare 0x00/0x04 to match G Hub's auto-toggle behaviour.
		 */
		params[0] = 0x01 | (ff->ffb_filter_auto ? 0x04 : 0x00);
		params[1] = 0x00;
		params[2] = filter;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_filter,
						  ff->fn_set_filter, params, 3,
						  &response);
		ret = hidpp_errno(hid, ret, "set FFB filter");
		if (ret)
			return ret;
	}

	ff->ffb_filter = filter;
	dd_info(hid, "FFB filter set to %d\n", filter);
	return count;
}

static DEVICE_ATTR(wheel_ffb_filter, 0664,
		   wheel_ffb_filter_show, wheel_ffb_filter_store);

/* FFB Filter Auto - automatic filter adjustment */
static ssize_t wheel_ffb_filter_auto_show(struct device *dev, struct device_attribute *attr,
					 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", ff->ffb_filter_auto);
}

static ssize_t wheel_ffb_filter_auto_store(struct device *dev, struct device_attribute *attr,
					  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int auto_mode, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &auto_mode);
	if (ret)
		return ret;

	if (ff->idx_filter == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	auto_mode = !!auto_mode; /* Normalize to 0 or 1 */

	/*
	 * Auto-only toggle: leave bit 0 (user-explicit level) clear,
	 * mirroring G Hub's auto-toggle path. See wheel_ffb_filter_store
	 * above for the full bitfield decode.
	 */
	params[0] = auto_mode ? 0x04 : 0x00;
	params[1] = 0x00;
	params[2] = ff->ffb_filter;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_filter,
					  ff->fn_set_filter, params, 3, &response);
	ret = hidpp_errno(hid, ret, "set FFB filter auto");
	if (ret)
		return ret;

	ff->ffb_filter_auto = auto_mode;
	dd_info(hid, "FFB filter auto %s\n", auto_mode ? "enabled" : "disabled");
	return count;
}

static DEVICE_ATTR(wheel_ffb_filter_auto, 0664,
		   wheel_ffb_filter_auto_show, wheel_ffb_filter_auto_store);

/*
 * LIGHTSYNC LED control sysfs attributes
 *
 * The RS50 wheel has 10 individually addressable RGB LEDs around the rim.
 * The LIGHTSYNC feature (0x807A) allows configuring 5 custom slots, each
 * with a direction (animation style) and per-LED RGB colors.
 *
 * Protocol (function 0x2C - Set RGB Zone Config):
 *   Byte 0:     Slot index (0-4 for CUSTOM 1-5)
 *   Byte 1:     Direction (0-3)
 *   Bytes 2-31: RGB values for 10 LEDs (3 bytes each)
 *               LED order is REVERSED: LED10 first, LED1 last
 */

/*
 * Initialize LIGHTSYNC LED subsystem using G Hub's exact sequence.
 * From capture analysis (lightsync_custom_save.pcapng), G Hub sends:
 *   - Feature 0x0B, Function 6 (0x6C): Enable with params [00 01 00 0a]
 * Then for RGB config:
 *   - Feature 0x0C, Function 2 (0x2C): SetConfig with [slot, effect_type, colors...]
 *   - Feature 0x0C, Function 3 (0x3C): Activate slot
 */
static int hidpp_dd_lightsync_enable(struct hidpp_device *hidpp, struct hidpp_dd_ff_data *ff)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[16];
	int ret;

	if (ff->idx_lightsync == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	dd_dbg(hid, "Enabling LIGHTSYNC (idx_ls=0x%02x, idx_rgb=0x%02x)\n",
		ff->idx_lightsync, ff->idx_rgb_config);

	memset(params, 0, sizeof(params));

	/*
	 * Query feature 0x0C fn0 (RGB config info). G Hub captures show
	 * the response as { slot_count, unused, name_len, ?, led_count }.
	 * We latch both counts into ff->ls_num_{slots,leds}, clamped to
	 * the compile-time maxima that size led_slots[]. Defaults stay
	 * in place if the query fails.
	 */
	ff->ls_num_slots = HIDPP_DD_LIGHTSYNC_NUM_SLOTS;
	ff->ls_num_leds  = HIDPP_DD_LIGHTSYNC_NUM_LEDS;
	if (ff->idx_rgb_config != HIDPP_DD_FEATURE_NOT_FOUND) {
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
						  0x00, params, 3, &response);
		if (ret == 0) {
			u8 slots = response.fap.params[0];
			u8 leds  = response.fap.params[4];

			if (slots > 0 && slots <= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
				ff->ls_num_slots = slots;
			if (leds > 0 && leds <= HIDPP_DD_LIGHTSYNC_NUM_LEDS)
				ff->ls_num_leds = leds;
			dd_dbg(hid,
				"LIGHTSYNC reports slots=%u leds=%u\n",
				ff->ls_num_slots, ff->ls_num_leds);
		}
	}

	/*
	 * Set LED count via function 4.
	 * From coldstart capture: 10ff0b4a 00 0a 00 - Function 4, Params: 00 0a 00
	 * G Hub gets error 5 here too, but continues.
	 */
	memset(params, 0, sizeof(params));
	params[0] = 0x00;
	params[1] = 0x0a;  /* 10 LEDs */
	params[2] = 0x00;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
					  HIDPP_DD_LIGHTSYNC_FN_SET_LEDS, params, 3, &response);
	dd_dbg(hid, "0x0B fn4(setLEDs) ret=%d resp: %02x %02x %02x %02x\n",
		ret, response.fap.params[0], response.fap.params[1],
		response.fap.params[2], response.fap.params[3]);

	/*
	 * Enable display via function 7.
	 * From coldstart capture: 10ff0b7a 00 00 00 - Function 7, Params: 00 00 00
	 * Response should be: 00 01 00 0a (enabled, 10 LEDs)
	 */
	memset(params, 0, 3);

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
					  HIDPP_DD_LIGHTSYNC_FN_ENABLE, params, 3, &response);
	dd_dbg(hid, "0x0B fn7(enable) ret=%d resp: %02x %02x %02x %02x\n",
		ret, response.fap.params[0], response.fap.params[1],
		response.fap.params[2], response.fap.params[3]);

	if (ret)
		dd_warn(hid, "LIGHTSYNC enable failed, but continuing\n");

	return 0;  /* Continue even if enable fails */
}

/*
 * Query slot name from device.
 * fn3 (0x30) on feature 0x0C returns: [slot] [len] [name...]
 */
static int hidpp_dd_lightsync_get_slot_name(struct hidpp_device *hidpp,
					struct hidpp_dd_ff_data *ff, u8 slot)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3];
	int ret, len;

	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -EINVAL;

	if (ff->idx_rgb_config == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	params[0] = slot;
	params[1] = 0;
	params[2] = 0;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
					  HIDPP_DD_RGB_FN_GET_NAME, params, 3, &response);
	if (ret) {
		dd_dbg(hid, "Failed to get slot %d name: %d\n", slot, ret);
		return ret;
	}

	/* Response: [slot] [len] [name...] */
	len = response.fap.params[1];
	if (len > HIDPP_DD_SLOT_NAME_MAX_LEN)
		len = HIDPP_DD_SLOT_NAME_MAX_LEN;

	memset(ff->led_slots[slot].name, 0, sizeof(ff->led_slots[slot].name));
	if (len > 0)
		memcpy(ff->led_slots[slot].name, &response.fap.params[2], len);

	dd_dbg(hid, "Slot %d name: \"%s\" (len=%d)\n",
		slot, ff->led_slots[slot].name, len);

	return 0;
}

/*
 * Query all slot names from device.
 */
static void hidpp_dd_lightsync_query_slot_names(struct hidpp_device *hidpp,
					    struct hidpp_dd_ff_data *ff)
{
	int i;

	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_SLOTS; i++)
		hidpp_dd_lightsync_get_slot_name(hidpp, ff, i);
}

/*
 * Query a slot's RGB config + direction from the device.
 * Closes PROBE.F4: without this we'd initialise the cache to all-white
 * and the first hidpp_dd_lightsync_apply_slot on load would stomp any
 * user-saved (or G Hub-programmed) colors. Response format is inferred
 * as the inverse of the SET wire format in hidpp_dd_lightsync_apply_slot:
 *   params[0]   = slot echo
 *   params[1]   = direction + 2
 *   params[2..] = 10 * RGB, LED10 first
 * If the response doesn't look like that (params[0] != slot, or the
 * call errors), leave the driver-default cache alone so the existing
 * behaviour is preserved.
 */
static int hidpp_dd_lightsync_get_slot_config(struct hidpp_device *hidpp,
					  struct hidpp_dd_ff_data *ff, u8 slot)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[3];
	int ret, i;

	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -EINVAL;
	if (ff->idx_rgb_config == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	params[0] = slot;
	params[1] = 0;
	params[2] = 0;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
					  HIDPP_DD_RGB_FN_GET_CONFIG, params, 3,
					  &response);
	if (ret) {
		dd_dbg(hid, "GET RGB slot %d ret=%d (keeping cached defaults)\n",
			slot, ret);
		return ret;
	}

	if (response.fap.params[0] != slot) {
		dd_dbg(hid, "GET RGB slot %d: echo mismatch (got %02x); keeping defaults\n",
			slot, response.fap.params[0]);
		return -EPROTO;
	}

	ff->led_slots[slot].direction = response.fap.params[1] >= 2 ?
		response.fap.params[1] - 2 : 0;
	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_LEDS; i++) {
		int src = 2 + (HIDPP_DD_LIGHTSYNC_NUM_LEDS - 1 - i) * 3;
		int dst = i * 3;

		ff->led_slots[slot].colors[dst + 0] = response.fap.params[src + 0];
		ff->led_slots[slot].colors[dst + 1] = response.fap.params[src + 1];
		ff->led_slots[slot].colors[dst + 2] = response.fap.params[src + 2];
	}
	return 0;
}

/*
 * Populate the in-driver RGB cache for every slot from the device,
 * so hidpp_dd_lightsync_apply_slot doesn't stomp user-saved state.
 */
static void hidpp_dd_lightsync_query_slot_configs(struct hidpp_device *hidpp,
					      struct hidpp_dd_ff_data *ff)
{
	int i;

	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_SLOTS; i++)
		hidpp_dd_lightsync_get_slot_config(hidpp, ff, i);
}

/*
 * Helper to send LIGHTSYNC config to device.
 * From capture analysis (lightsync_custom_save.pcapng), G Hub sequence is:
 *   1. Set effect mode to 5 (Custom) on feature 0x0B
 *   2. Set slot name on feature 0x0C (optional but G Hub does this)
 *   3. Set RGB config on feature 0x0C
 *   4. Activate slot on feature 0x0C
 */
static int hidpp_dd_lightsync_apply_slot(struct hidpp_device *hidpp,
				     struct hidpp_dd_ff_data *ff, u8 slot,
				     bool set_effect)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 params[32];  /* slot + direction + 30 bytes RGB */
	struct hidpp_dd_lightsync_slot *ls;
	int i, ret;

	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -EINVAL;

	if (ff->idx_rgb_config == HIDPP_DD_FEATURE_NOT_FOUND) {
		dd_warn(hid, "RGB config feature (0x807B) not found\n");
		return -EOPNOTSUPP;
	}

	ls = &ff->led_slots[slot];

	/*
	 * G Hub Color Change Sequence (from lightsync.pcapng):
	 *   1. Profile query (0x8137) - optional
	 *   2. Sync call (0x1BC0) - optional
	 *   3. SET_EFFECT fn3 on 0x0B (set mode 5 = static/custom)
	 *   4. RGB data fn2 on 0x0C
	 *   5. ACTIVATE fn3 on 0x0C
	 *
	 * NOTE: fn6/fn7 are NOT used during color changes - they cause errors.
	 * The device init sequence uses fn4/fn7 on 0x0B, not during runtime changes.
	 */

	/*
	 * Step 1: Query Profile feature (G Hub does this before effect changes).
	 * From capture: 10 FF 17 0C ...
	 */
	if (ff->idx_profile != HIDPP_DD_FEATURE_NOT_FOUND) {
		memset(params, 0, 3);
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_profile,
						  0x0C, params, 3, &response);
		dd_dbg(hid, "Profile query ret=%d\n", ret);
	}

	/*
	 * Step 2: Call Sync feature (G Hub does this before effect changes).
	 * From capture: 10 FF 09 0C 00 03 00
	 */
	if (ff->idx_sync != HIDPP_DD_FEATURE_NOT_FOUND) {
		params[0] = 0x00;
		params[1] = 0x03;
		params[2] = 0x00;
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_sync,
						  0x0C, params, 3, &response);
		dd_dbg(hid, "Sync call ret=%d\n", ret);
	}

	/*
	 * Step 3: Set effect mode 5 (static/custom) on feature 0x0B.
	 * From capture: 10 FF 0B 3C 05 00 00
	 * This tells the device we're using custom colors, not an animation.
	 *
	 * Gated on set_effect: user-initiated colour/direction/effect changes
	 * pass true (they intend Custom, to display the change). The load-time
	 * init passes false so it does NOT force Custom over an animated effect
	 * (modes 1-4) the wheel restored from its profile - the effect mode is
	 * never read back, so forcing 5 here would stomp it (cf. the brightness
	 * bug, issue #29).
	 */
	if (set_effect && ff->idx_lightsync != HIDPP_DD_FEATURE_NOT_FOUND) {
		params[0] = 0x05;  /* Effect mode 5 = static/custom */
		params[1] = 0x00;
		params[2] = 0x00;
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
						  HIDPP_DD_LIGHTSYNC_FN_SET_EFFECT,
						  params, 3, &response);
		dd_dbg(hid, "0x0B fn3(effect=5) ret=%d\n", ret);
	}

	/*
	 * Step 3b: Call fn6 (pre-config LONG) on 0x0B to prepare for RGB data.
	 * This seems required before sending RGB config to 0x0C.
	 * From capture: 11 ff 0b 6c 00 01 00 0a 00 00 00 00 00 00 00 00 00 00 00 00
	 */
	if (ff->idx_lightsync != HIDPP_DD_FEATURE_NOT_FOUND) {
		memset(params, 0, 16);
		params[0] = 0x00;
		params[1] = 0x01;
		params[2] = 0x00;
		params[3] = 0x0a;  /* 10 LEDs */
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
						  HIDPP_DD_LIGHTSYNC_FN_SET_CONFIG,
						  params, 16, &response);
		dd_dbg(hid, "0x0B fn6(pre-config) ret=%d\n", ret);
	}

	/*
	 * Step 4: Send RGB config packet to feature 0x0C (0x807B).
	 * From capture: 12 FF 0C 2C [slot] [type] [30 bytes RGB]
	 *   - byte 0: slot index (0-4)
	 *   - byte 1: type/direction byte - encodes LED animation direction
	 *             Observed values: 0x02, 0x03 in captures
	 *             Direction mapping: direction + 2 (0->2, 1->3, etc.)
	 *   - bytes 2-31: RGB colors (10 LEDs × 3 bytes, reversed order: LED10 first)
	 */
	params[0] = slot;
	params[1] = ls->direction + 2;  /* Direction encoding: 0->0x02, 1->0x03, etc. */

	/* LED colors reversed (LED10 first in protocol) */
	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_LEDS; i++) {
		int src = (HIDPP_DD_LIGHTSYNC_NUM_LEDS - 1 - i) * 3;
		int dst = 2 + i * 3;

		params[dst + 0] = ls->colors[src + 0];  /* R */
		params[dst + 1] = ls->colors[src + 1];  /* G */
		params[dst + 2] = ls->colors[src + 2];  /* B */
	}

	dd_dbg(hid, "0x0C fn2(RGB) slot=%d dir=%d RGB[0-2]: %02x%02x%02x %02x%02x%02x %02x%02x%02x\n",
		 params[0], params[1],
		 params[2], params[3], params[4],
		 params[5], params[6], params[7],
		 params[8], params[9], params[10]);

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
					  HIDPP_DD_RGB_FN_SET_CONFIG, params,
					  sizeof(params), &response);
	dd_dbg(hid, "0x0C fn2(setConfig) ret=%d\n", ret);
	ret = hidpp_errno(hid, ret, "set RGB config");
	if (ret)
		return ret;

	/*
	 * Step 5: Activate slot on feature 0x0C.
	 * From capture: 10 FF 0C 3C [slot] 00 00
	 */
	params[0] = slot;
	params[1] = 0x00;
	params[2] = 0x00;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
					  HIDPP_DD_RGB_FN_ACTIVATE, params, 3, &response);
	if (ret < 0) {
		dd_err(hid, "LIGHTSYNC activate bus error on slot %d: %d\n",
			slot, ret);
		return ret;
	}
	if (ret > 0)
		dd_warn(hid, "LIGHTSYNC activate HID++ error 0x%02x on slot %d\n",
			 ret, slot);

	/*
	 * Step 6: Call fn6 (commit) on 0x0B AFTER RGB config.
	 * From G Hub capture: 11 ff 0b 6c 00 01 00 0a 00 0a 00 ...
	 * Note: params[5] = 0x0a this time (was 0x00 in pre-config).
	 */
	if (ff->idx_lightsync != HIDPP_DD_FEATURE_NOT_FOUND) {
		memset(params, 0, 16);
		params[0] = 0x00;
		params[1] = 0x01;
		params[2] = 0x00;
		params[3] = 0x0a;  /* 10 LEDs */
		params[4] = 0x00;
		params[5] = 0x0a;  /* 10 LEDs - commit flag? */
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
						  HIDPP_DD_LIGHTSYNC_FN_SET_CONFIG,
						  params, 16, &response);
		dd_dbg(hid, "0x0B fn6(commit) ret=%d\n", ret);

		/*
		 * Step 7: Call fn7 (enable refresh) on 0x0B.
		 * From capture: 10 ff 0b 7c 00 00 00
		 */
		memset(params, 0, 3);
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
						  HIDPP_DD_LIGHTSYNC_FN_ENABLE,
						  params, 3, &response);
		dd_dbg(hid, "0x0B fn7(enable) ret=%d\n", ret);
	}

	/*
	 * Deliberately does NOT write brightness. Brightness is a separate
	 * feature (0x8040), owned by the wheel_led_slot_brightness sysfs
	 * handler and kept in sync by the BrightnessControl broadcast handler.
	 * The per-slot brightness cache is never read back from the wheel (the
	 * RGB GET returns only colours + direction), so it holds the driver
	 * default of 100%. Writing it on every apply_slot - the init apply and
	 * every colour/direction/effect/mode change - stomped the wheel's
	 * stored profile brightness back to 100%, winning the race against the
	 * profile load roughly half the time (issue #29). apply_slot now leaves
	 * brightness alone; the wheel keeps whatever its active profile holds,
	 * and the user changes it explicitly via wheel_led_slot_brightness.
	 */

	/*
	 * Desktop mode: G Hub issues seven fn1 writes on the sync feature
	 * (0x1BC0) after any LED change. Each write targets a secondary
	 * LED zone (shift lights at ids 0x0D-0x12 and an accent at 0x15).
	 * Onboard-mode captures don't contain these writes, so gate on
	 * current_mode == 0. The device seems to tolerate their absence
	 * in basic LIGHTSYNC scenarios, but skipping them may explain the
	 * occasional desktop-only LED update that doesn't stick.
	 */
	if (ff->current_mode == 0 &&
	    ff->idx_sync != HIDPP_DD_FEATURE_NOT_FOUND) {
		static const u8 desktop_sync_zones[] = {
			0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x15,
		};
		u8 sync_params[5];
		size_t i;
		int ok = 0;

		for (i = 0; i < ARRAY_SIZE(desktop_sync_zones); i++) {
			sync_params[0] = 0x01;
			sync_params[1] = 0x00;
			sync_params[2] = 0x09;
			sync_params[3] = 0x00;
			sync_params[4] = desktop_sync_zones[i];
			ret = hidpp_send_fap_command_sync(hidpp, ff->idx_sync,
							  0x10, sync_params,
							  sizeof(sync_params),
							  &response);
			if (ret)
				dd_dbg(hid,
					"desktop sync zone 0x%02x ret=%d\n",
					desktop_sync_zones[i], ret);
			else
				ok++;
		}
		dd_dbg(hid, "desktop sync sequence sent (%d/%zu ok)\n",
			ok, ARRAY_SIZE(desktop_sync_zones));
	}

	dd_dbg(hid, "apply_slot complete\n");
	return 0;
}

/* wheel_led_slot - select and apply active slot (0-4) */
static ssize_t wheel_led_slot_show(struct device *dev, struct device_attribute *attr,
				  char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->led_active_slot));
}

static ssize_t wheel_led_slot_store(struct device *dev, struct device_attribute *attr,
				   const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int slot;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtouint(buf, 10, &slot);
	if (ret)
		return ret;

	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -EINVAL;

	/* Apply the selected slot configuration to the device */
	ret = hidpp_dd_lightsync_apply_slot(hidpp, ff, slot, true);
	if (ret)
		return ret;

	/*
	 * led_active_slot is read (without ring_lock) from every other
	 * LIGHTSYNC sysfs handler. Publish via WRITE_ONCE so readers that
	 * aren't serialized against us see the value atomically; a racing
	 * reader still only sees an in-range slot because kstrtouint +
	 * the bound check above caught anything else.
	 */
	WRITE_ONCE(ff->led_active_slot, (u8)slot);
	dd_info(hid, "LIGHTSYNC slot set to %u\n", slot);
	return count;
}

static DEVICE_ATTR(wheel_led_slot, 0664, wheel_led_slot_show, wheel_led_slot_store);

/* wheel_led_slot_name - read/write name for current slot (max 8 chars) */
static ssize_t wheel_led_slot_name_show(struct device *dev, struct device_attribute *attr,
				       char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	u8 slot;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;

	return sysfs_emit(buf, "%s\n", ff->led_slots[slot].name);
}

static ssize_t wheel_led_slot_name_store(struct device *dev, struct device_attribute *attr,
					const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[16];
	u8 slot;
	size_t len;
	size_t i;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	if (ff->idx_rgb_config == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;

	/* Strip trailing newline */
	len = count;
	if (len > 0 && buf[len - 1] == '\n')
		len--;

	if (len > HIDPP_DD_SLOT_NAME_MAX_LEN)
		len = HIDPP_DD_SLOT_NAME_MAX_LEN;

	/*
	 * Reject embedded control bytes (including further newlines) so
	 * a user can't push a name that, once echoed back through show,
	 * breaks shell scripts that split on newline or that expect 7-bit
	 * printable ASCII. Space and tilde bracket printable ASCII.
	 */
	for (i = 0; i < len; i++) {
		unsigned char c = (unsigned char)buf[i];

		if (c < 0x20 || c > 0x7E)
			return -EINVAL;
	}

	/* fn4: SET_NAME - [slot] [len] [name...] */
	memset(params, 0, sizeof(params));
	params[0] = slot;
	params[1] = len;
	if (len > 0)
		memcpy(&params[2], buf, len);

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_rgb_config,
					  HIDPP_DD_RGB_FN_SET_NAME, params,
					  2 + len, &response);
	if (ret) {
		char op[24];

		scnprintf(op, sizeof(op), "set slot %u name", slot);
		return hidpp_errno(hid, ret, op);
	}

	/* Update cached name */
	memset(ff->led_slots[slot].name, 0, sizeof(ff->led_slots[slot].name));
	if (len > 0)
		memcpy(ff->led_slots[slot].name, buf, len);

	dd_info(hid, "Slot %d name set to \"%s\"\n", slot, ff->led_slots[slot].name);
	return count;
}

static DEVICE_ATTR(wheel_led_slot_name, 0664,
		   wheel_led_slot_name_show, wheel_led_slot_name_store);

/* wheel_led_slot_brightness - per-slot brightness (0-100) */
static ssize_t wheel_led_slot_brightness_show(struct device *dev,
					     struct device_attribute *attr,
					     char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	u8 slot;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;

	return sysfs_emit(buf, "%u\n", ff->led_slots[slot].brightness);
}

static ssize_t wheel_led_slot_brightness_store(struct device *dev,
					      struct device_attribute *attr,
					      const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	unsigned int brightness;
	u8 slot;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtouint(buf, 10, &brightness);
	if (ret)
		return ret;

	if (brightness > 100)
		brightness = 100;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;

	/* Apply to device first; cache on success so a failed write doesn't
	 * leave sysfs reporting a value the wheel never accepted.
	 */
	if (ff->idx_brightness != HIDPP_DD_FEATURE_NOT_FOUND) {
		params[0] = 0x00;
		params[1] = brightness;
		params[2] = 0x00;

		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_brightness,
						  ff->fn_set_brightness, params, 3, &response);
		ret = hidpp_errno(hid, ret, "set slot brightness");
		if (ret)
			return ret;
	}

	ff->led_slots[slot].brightness = brightness;
	/* Global brightness tracks the last-applied slot brightness */
	ff->led_brightness = brightness;

	dd_info(hid, "Slot %d brightness set to %u%%\n", slot, brightness);
	return count;
}

static DEVICE_ATTR(wheel_led_slot_brightness, 0664,
		   wheel_led_slot_brightness_show, wheel_led_slot_brightness_store);

/* wheel_led_direction - set direction for current slot (0-3) */
static ssize_t wheel_led_direction_show(struct device *dev, struct device_attribute *attr,
				       char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	u8 slot, dir;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;
	dir = ff->led_slots[slot].direction;

	return sysfs_emit(buf, "%u\n", dir);
}

static ssize_t wheel_led_direction_store(struct device *dev, struct device_attribute *attr,
					const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int dir;
	u8 slot;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtouint(buf, 10, &dir);
	if (ret)
		return ret;

	if (dir > HIDPP_DD_LIGHTSYNC_DIR_OUTSIDE_IN)
		return -EINVAL;

	slot = READ_ONCE(ff->led_active_slot);
	if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
		return -ERANGE;

	/*
	 * apply_slot reads led_slots[slot].direction to build the wire
	 * command, so we must stage the new value first. On failure,
	 * restore the previous value so sysfs doesn't diverge.
	 */
	{
		u8 prev = ff->led_slots[slot].direction;

		ff->led_slots[slot].direction = dir;
		ret = hidpp_dd_lightsync_apply_slot(hidpp, ff, slot, true);
		if (ret) {
			ff->led_slots[slot].direction = prev;
			return ret;
		}
	}

	dd_info(hid, "LIGHTSYNC direction set to %u\n", dir);
	return count;
}

static DEVICE_ATTR(wheel_led_direction, 0664,
		   wheel_led_direction_show, wheel_led_direction_store);

/*
 * wheel_led_colors - set all 10 LED colors for current slot
 * Format: "RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB RRGGBB"
 * (10 hex color values, space-separated, LED1 to LED10)
 */
static ssize_t wheel_led_colors_show(struct device *dev, struct device_attribute *attr,
				    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_dd_lightsync_slot *ls;
	int i, len = 0;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	{
		u8 slot = READ_ONCE(ff->led_active_slot);

		if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
			return -ERANGE;
		ls = &ff->led_slots[slot];
	}

	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_LEDS; i++) {
		u8 r = ls->colors[i * 3 + 0];
		u8 g = ls->colors[i * 3 + 1];
		u8 b = ls->colors[i * 3 + 2];

		len += sysfs_emit_at(buf, len, "%s%02X%02X%02X",
				     (i > 0) ? " " : "", r, g, b);
	}
	len += sysfs_emit_at(buf, len, "\n");

	return len;
}

static ssize_t wheel_led_colors_store(struct device *dev, struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_dd_lightsync_slot *ls;
	u8 colors[HIDPP_DD_LIGHTSYNC_NUM_LEDS * 3];
	const char *p = buf;
	int i, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	/* Parse 10 hex color values */
	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_LEDS; i++) {
		unsigned int color;
		char hex[7];
		int parsed;

		/* Skip whitespace */
		while (*p == ' ' || *p == '\t')
			p++;

		if (*p == '\0' || *p == '\n') {
			/* Not enough colors provided */
			return -EINVAL;
		}

		/* Extract 6-character hex value */
		parsed = 0;
		while (parsed < 6 && *p && *p != ' ' && *p != '\t' && *p != '\n') {
			hex[parsed++] = *p++;
		}
		hex[parsed] = '\0';

		if (parsed != 6)
			return -EINVAL;

		ret = kstrtouint(hex, 16, &color);
		if (ret)
			return ret;

		colors[i * 3 + 0] = (color >> 16) & 0xFF;  /* R */
		colors[i * 3 + 1] = (color >> 8) & 0xFF;   /* G */
		colors[i * 3 + 2] = color & 0xFF;          /* B */
	}

	/*
	 * apply_slot reads the slot colors to build the wire command, so
	 * stage the new values first and restore on failure. A show on
	 * the same attribute can't race because kernfs serializes
	 * show/store on a single attribute via of->mutex.
	 */
	{
		u8 slot = READ_ONCE(ff->led_active_slot);
		u8 prev[HIDPP_DD_LIGHTSYNC_NUM_LEDS * 3];

		if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
			return -ERANGE;
		ls = &ff->led_slots[slot];
		memcpy(prev, ls->colors, sizeof(prev));
		memcpy(ls->colors, colors, sizeof(colors));
		ret = hidpp_dd_lightsync_apply_slot(hidpp, ff, slot, true);
		if (ret) {
			memcpy(ls->colors, prev, sizeof(prev));
			return ret;
		}
	}

	dd_info(hid, "LIGHTSYNC colors updated\n");
	return count;
}

static DEVICE_ATTR(wheel_led_colors, 0664,
		   wheel_led_colors_show, wheel_led_colors_store);

/*
 * wheel_led_apply - write-only trigger to re-apply current slot config
 * Write any value to re-send the LIGHTSYNC config to the device.
 */
static ssize_t wheel_led_apply_store(struct device *dev, struct device_attribute *attr,
				    const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	{
		u8 slot = READ_ONCE(ff->led_active_slot);

		if (slot >= HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
			return -ERANGE;
		ret = hidpp_dd_lightsync_apply_slot(hidpp, ff, slot, true);
		if (ret)
			return ret;
		dd_info(hid, "LIGHTSYNC config applied to slot %u\n", slot);
	}
	return count;
}

static DEVICE_ATTR_WO(wheel_led_apply);

/*
 * wheel_led_effect - select LED effect mode (1-5)
 * 1=Inside→Out, 2=Outside→In, 3=Right→Left, 4=Left→Right, 5=Custom (static)
 * Must set to 5 for custom per-LED colors to be visible.
 */
static ssize_t wheel_led_effect_show(struct device *dev, struct device_attribute *attr,
				    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", ff->led_effect);
}

static ssize_t wheel_led_effect_store(struct device *dev, struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int effect, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &effect);
	if (ret)
		return ret;

	if (ff->idx_lightsync == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	/*
	 * Effect values 1-5 are labeled: 1=Inside->Out, 2=Outside->In,
	 * 3=Right->Left, 4=Left->Right, 5=Custom. The wheel's fn1
	 * supported-effect list additionally advertises 6-9
	 * (live-verified 2026-07-02: `12ff0b18 00 02 01 03 04 05 06 07
	 * 08 09` = cluster 0 + effect IDs 1..9), and effect-change
	 * broadcasts with values 6 and 9 appear in the G Hub captures.
	 * Accept the full advertised range; 6-9 remain visually
	 * unlabeled until someone watches the LEDs while cycling them.
	 */
	effect = clamp(effect, 1, 9);

	params[0] = effect;
	params[1] = 0x00;
	params[2] = 0x00;

	dd_info(hid, "LED effect: idx=0x%02x fn=0x%02x params=[%02x %02x %02x]\n",
		 ff->idx_lightsync, HIDPP_DD_LIGHTSYNC_FN_SET_EFFECT,
		 params[0], params[1], params[2]);

	/* Use SHORT report (0x10) with function 0x3C for effect selection */
	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_lightsync,
					  HIDPP_DD_LIGHTSYNC_FN_SET_EFFECT, params, 3, &response);
	ret = hidpp_errno(hid, ret, "set LED effect");
	if (ret)
		return ret;

	ff->led_effect = effect;

	/*
	 * Transitioning to custom mode (effect 5): push the active slot's
	 * RGB config so the new mode has something to show. apply_slot
	 * also sets effect = 5 internally, but the explicit SET_EFFECT
	 * above keeps the user's intent audible in the trace. For
	 * animated modes (1-4) apply_slot would stomp the effect back to
	 * 5, so skip it there.
	 */
	if (effect == 5) {
		u8 slot = READ_ONCE(ff->led_active_slot);

		if (slot < HIDPP_DD_LIGHTSYNC_NUM_SLOTS)
			hidpp_dd_lightsync_apply_slot(hidpp, ff, slot, true);
	}

	dd_info(hid, "LED effect set to %d (success)\n", effect);
	return count;
}

static DEVICE_ATTR(wheel_led_effect, 0664, wheel_led_effect_show, wheel_led_effect_store);

/*
 * wheel_rev_level: rev-light level for the REAL G PRO rim (0-10 = how
 * many LEDs lit). The G PRO's rim lights are level-based, not the
 * RS50's per-LED RGB LIGHTSYNC model: colours, direction and scaling
 * belong to the wheel's onboard profile, and the host only commands a
 * level. Protocol decoded by the TF4ALL project from a G HUB capture
 * (2026-05-16, see dev/docs/tf4all-analysis.md): a one-time arm burst
 * of SHORT sends on the 0x807A feature (fn0, fn1, fn2, fn3 param 0x02,
 * fn0, a few ms apart), then per update a SHORT fn2 + LONG fn6 pair
 * with the level in the LONG's byte 9. G HUB's sw-id nibble (0x0d) is
 * kept verbatim - this is the only known-working capture shape.
 *
 * Two cautions baked in, both from TF4ALL's testing on real hardware:
 * writes are fire-and-forget (no reply is read - the pair does not
 * reliably generate one, and a sync wait would eat timeouts), and
 * updates are floor-limited to G HUB's own ~160 ms cadence because
 * bursting level writes starves the wheel's shared HID++ command
 * processor. The wheel holds a level for a long time but reverts
 * eventually; a telemetry feeder should refresh at ~1 Hz or faster,
 * which anything driving rev lights does naturally.
 *
 * The cadence floor is enforced by a coalescing delayed_work
 * (hidpp_dd_rev_work_handler), not by sleeping in the store: a store
 * only validates, publishes the target level, and queues the worker for
 * the next allowed slot, returning immediately. Fast feeders (50-100 Hz)
 * therefore collapse to one send per slot with the newest level winning,
 * instead of blocking each write ~160 ms and draining stale
 * intermediates onto the wire.
 *
 * UNTESTED on real hardware (we develop on an RS50); gated to real
 * G PROs by dd_is_real_gpro() and needs a G PRO owner to validate.
 */
#define HIDPP_DD_REV_SWID		0x0d	/* G HUB's sw-id, kept verbatim */
#define HIDPP_DD_REV_MAX_LEVEL		10
#define HIDPP_DD_REV_MIN_GAP_MS		10	/* ~100 Hz. G HUB drives rev lights at ~127 Hz (~7.5 ms per pair); the old 160 ms was a misread and made a full 0->10 sweep take ~1.6 s. */
#define HIDPP_DD_REV_ARM_GAP_MS	4

static int hidpp_dd_rev_send_short(struct hidpp_device *hidpp, u8 idx, u8 fn,
				   u8 p0)
{
	struct hidpp_report *report;
	int ret;

	/*
	 * DMA-safe buffer: __hidpp_send_report() can hand this straight to a
	 * USB interrupt-OUT URB (on FORCE_OUTPUT_REPORTS wheels), and a stack
	 * buffer is not DMA-mappable - it WARNs in usb_hcd_map_urb_for_dma and
	 * the send fails with -EIO. Seen live on the RS50 the first time the
	 * rev-light path was exercised (it had only ever been gated on to the
	 * untested G PRO before). Mirrors hidpp_dd_ff_refresh_work's kzalloc.
	 */
	report = kzalloc(sizeof(*report), GFP_KERNEL);
	if (!report)
		return -ENOMEM;
	report->report_id = REPORT_ID_HIDPP_SHORT;
	report->device_index = 0xff;
	report->fap.feature_index = idx;
	report->fap.funcindex_clientid = (fn << 4) | HIDPP_DD_REV_SWID;
	report->fap.params[0] = p0;
	ret = __hidpp_send_report(hidpp->hid_dev, report);
	kfree(report);
	return ret;
}

static int hidpp_dd_rev_send_level(struct hidpp_device *hidpp, u8 idx, u8 level)
{
	struct hidpp_report *report;
	int ret;

	ret = hidpp_dd_rev_send_short(hidpp, idx, 2, 0);
	if (ret < 0)
		return ret;

	report = kzalloc(sizeof(*report), GFP_KERNEL);	/* DMA-safe, see above */
	if (!report)
		return -ENOMEM;
	report->report_id = REPORT_ID_HIDPP_LONG;
	report->device_index = 0xff;
	report->fap.feature_index = idx;
	report->fap.funcindex_clientid = (6 << 4) | HIDPP_DD_REV_SWID;
	/* params start at report byte 4: 00 01 00 0a 00 LL */
	report->fap.params[0] = 0x00;
	report->fap.params[1] = 0x01;
	report->fap.params[2] = 0x00;
	report->fap.params[3] = 0x0a;
	report->fap.params[4] = 0x00;
	report->fap.params[5] = level;
	ret = __hidpp_send_report(hidpp->hid_dev, report);
	kfree(report);
	return ret;
}

/*
 * Coalescing rev-light flush (process context, system_unbound_wq).
 *
 * Runs on system_unbound_wq, not ff->wq: it does synchronous-ish 0x807A
 * sends (the arm burst msleeps between packets) and must never
 * head-of-line-block the 500 Hz force stream on the singlethread ff->wq -
 * same rationale as tf_init_work / range_poll_work.
 *
 * Latest-value-wins: the store publishes rev_target and (re)queues us; we
 * always send whatever rev_target holds now, so a fast telemetry feeder
 * collapses to one send per cadence slot instead of queueing every stale
 * intermediate level. If the target moved again while we were sending, we
 * re-queue for the next slot.
 */
static void hidpp_dd_rev_work_handler(struct work_struct *work)
{
	struct hidpp_dd_ff_data *ff = container_of(work, struct hidpp_dd_ff_data,
						   rev_work.work);
	struct hidpp_device *hidpp = ff->hidpp;
	u8 target;
	int ret = 0, i;

	if (atomic_read_acquire(&ff->stopping) || !atomic_read(&ff->initialized))
		return;

	mutex_lock(&ff->rev_lock);

	/* Teardown may have flipped stopping while we waited for the lock. */
	if (atomic_read_acquire(&ff->stopping) ||
	    ff->idx_lightsync == HIDPP_DD_FEATURE_NOT_FOUND) {
		mutex_unlock(&ff->rev_lock);
		return;
	}

	target = READ_ONCE(ff->rev_target);

	/*
	 * Serialise the raw sends against the sync-transaction machinery:
	 * holding send_mutex guarantees no sync question is pending on this
	 * interface while our fire-and-forget 0x807A traffic can elicit
	 * replies. Without it, a rev-elicited reply whose sw-id the wheel
	 * zeroes could satisfy hidpp_match_answer's lenient (sw-id-stripped)
	 * path for a concurrent sync question on the same feature/function.
	 */
	mutex_lock(&hidpp->send_mutex);

	if (!ff->rev_armed) {
		static const u8 arm_fns[]    = { 0, 1, 2, 3, 0 };
		static const u8 arm_params[] = { 0, 0, 0, 2, 0 };

		for (i = 0; i < ARRAY_SIZE(arm_fns); i++) {
			ret = hidpp_dd_rev_send_short(hidpp, ff->idx_lightsync,
						      arm_fns[i], arm_params[i]);
			if (ret < 0)
				goto out_send;
			msleep(HIDPP_DD_REV_ARM_GAP_MS);
		}
		ff->rev_armed = true;
	}

	ret = hidpp_dd_rev_send_level(hidpp, ff->idx_lightsync, target);
out_send:
	mutex_unlock(&hidpp->send_mutex);

	/*
	 * Pace every attempt, not just successes: rev_last_write bounds the
	 * wire footprint regardless of outcome, so a persistent transport
	 * error can't turn a fast feeder into an unthrottled retry storm.
	 * The reported level (rev_level) only advances on a real send.
	 */
	ff->rev_last_write = jiffies;

	if (ret < 0) {
		/*
		 * The store can no longer surface send errors, so log here.
		 * Once per failure streak (cleared on the next success):
		 * __hidpp_send_report returns bare -1, report a real -EIO.
		 */
		if (!ff->rev_err_logged) {
			dd_warn(hidpp->hid_dev,
				"rev-light send failed: %d\n", -EIO);
			ff->rev_err_logged = true;
		}
	} else {
		WRITE_ONCE(ff->rev_level, target);
		ff->rev_err_logged = false;
	}

	/*
	 * The feeder moved the target while we were sending: schedule the
	 * next flush a full cadence gap out (rev_last_write is now), so the
	 * 160 ms floor holds across the hand-off.
	 */
	if (READ_ONCE(ff->rev_target) != target &&
	    !atomic_read_acquire(&ff->stopping) &&
	    atomic_read(&ff->initialized))
		queue_delayed_work(system_unbound_wq, &ff->rev_work,
				   msecs_to_jiffies(HIDPP_DD_REV_MIN_GAP_MS));

	mutex_unlock(&ff->rev_lock);
}

static ssize_t wheel_rev_level_show(struct device *dev,
				    struct device_attribute *attr, char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->rev_level));
}

static ssize_t wheel_rev_level_store(struct device *dev,
				     struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int level;
	unsigned long delay;
	long remaining;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtouint(buf, 10, &level);
	if (ret)
		return ret;
	if (level > HIDPP_DD_REV_MAX_LEVEL)
		return -EINVAL;
	if (ff->idx_lightsync == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	/*
	 * Latest-value-wins: publish the target, then queue the worker for
	 * the next cadence slot and return. No send, no sleep here - the
	 * worker (hidpp_dd_rev_work_handler) owns the arm burst, the sends
	 * and the send_mutex serialisation. queue_delayed_work is a no-op if
	 * the worker is already pending, so a burst of stores coalesces onto
	 * the single already-scheduled flush, which picks up this newest
	 * rev_target when it runs.
	 */
	WRITE_ONCE(ff->rev_target, level);

	mutex_lock(&ff->rev_lock);
	/* Re-check under the lock: teardown may have flipped stopping. */
	if (atomic_read_acquire(&ff->stopping)) {
		mutex_unlock(&ff->rev_lock);
		return -ENODEV;
	}
	/*
	 * Delay to the next allowed slot, computed ONCE as a signed delta:
	 * re-deriving it after a time_before() check races the jiffies tick,
	 * and an unsigned "deadline - jiffies" that crosses zero underflows
	 * into a near-infinite queue delay. Before the arm burst there is no
	 * prior pair to pace against, so fire immediately.
	 */
	delay = 0;
	remaining = (long)(ff->rev_last_write +
			   msecs_to_jiffies(HIDPP_DD_REV_MIN_GAP_MS) - jiffies);
	if (ff->rev_armed && remaining > 0)
		delay = remaining;
	queue_delayed_work(system_unbound_wq, &ff->rev_work, delay);
	mutex_unlock(&ff->rev_lock);

	return count;
}

static DEVICE_ATTR(wheel_rev_level, 0664, wheel_rev_level_show,
		   wheel_rev_level_store);

/* LED brightness */
static ssize_t wheel_led_brightness_show(struct device *dev, struct device_attribute *attr,
					char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%u\n", ff->led_brightness);
}

static ssize_t wheel_led_brightness_store(struct device *dev, struct device_attribute *attr,
					 const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	int brightness, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtoint(buf, 10, &brightness);
	if (ret)
		return ret;

	if (ff->idx_brightness == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	brightness = clamp(brightness, 0, 100);

	/* Brightness command: 00, value, 00 */
	params[0] = 0x00;
	params[1] = brightness;
	params[2] = 0x00;

	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_brightness,
					  ff->fn_set_brightness, params, 3, &response);
	ret = hidpp_errno(hid, ret, "set LED brightness");
	if (ret)
		return ret;

	/*
	 * Brightness only: 0x8040 was once believed to double as desktop
	 * sensitivity, but hardware testing disproved that (sensitivity
	 * is a 0x80A4 curve upload), so ff->sensitivity is not touched.
	 */
	ff->led_brightness = brightness;
	dd_info(hid, "LED brightness set to %d%%\n", brightness);
	return count;
}

static DEVICE_ATTR(wheel_led_brightness, 0664,
		   wheel_led_brightness_show, wheel_led_brightness_store);

#ifdef CONFIG_HID_LOGITECH_HIDPP_DEBUG
/*
 * wheel_hidpp_debug - Debug interface to probe arbitrary HID++ functions.
 * Write format: "feature_idx function [param0 param1 ...]" (hex values)
 * Example: "0b 5c 00 00 00" sends fn5 to feature 0x0B with params 00 00 00
 * Read shows the last command's response.
 *
 * Gated behind CONFIG_HID_LOGITECH_HIDPP_DEBUG (default off). The interface
 * is a root-only raw HID++ shell intended for protocol bring-up, not for
 * production use.
 */
static ssize_t wheel_hidpp_debug_show(struct device *dev, struct device_attribute *attr,
				     char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf,
			 "Last cmd: feature=0x%02x fn=0x%02x ret=%d\n"
			 "Response: %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x\n"
			 "Usage: echo \"feature fn [params...]\" > wheel_hidpp_debug\n"
			 "Example: echo \"0b 5c 00 00 00\" > wheel_hidpp_debug\n",
			 ff->debug_last_feature, ff->debug_last_function, ff->debug_last_ret,
			 ff->debug_last_response[0], ff->debug_last_response[1],
			 ff->debug_last_response[2], ff->debug_last_response[3],
			 ff->debug_last_response[4], ff->debug_last_response[5],
			 ff->debug_last_response[6], ff->debug_last_response[7],
			 ff->debug_last_response[8], ff->debug_last_response[9],
			 ff->debug_last_response[10], ff->debug_last_response[11],
			 ff->debug_last_response[12], ff->debug_last_response[13],
			 ff->debug_last_response[14], ff->debug_last_response[15]);
}

static ssize_t wheel_hidpp_debug_store(struct device *dev, struct device_attribute *attr,
				      const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[16];
	unsigned int feature, function;
	unsigned int p[16];
	int num_params, i, ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	memset(params, 0, sizeof(params));
	memset(p, 0, sizeof(p));

	/* Parse: feature function [param0 param1 ...] */
	num_params = sscanf(buf, "%x %x %x %x %x %x %x %x %x %x %x %x %x %x %x %x %x %x",
			    &feature, &function,
			    &p[0], &p[1], &p[2], &p[3], &p[4], &p[5], &p[6], &p[7],
			    &p[8], &p[9], &p[10], &p[11], &p[12], &p[13], &p[14], &p[15]);

	if (num_params < 2) {
		dd_err(hid, "debug: need at least feature and function\n");
		return -EINVAL;
	}

	/*
	 * Validate feature, function, and each param fit in a u8. sscanf
	 * with %x happily parses values > 0xFF and we'd silently truncate
	 * them, which makes debugging the debug-shell hard. Reject big
	 * values with -EINVAL so the caller knows to retype.
	 */
	if (feature > 0xFF || function > 0xFF) {
		dd_err(hid, "debug: feature/function must be 0-FF (got 0x%x / 0x%x)\n",
			feature, function);
		return -EINVAL;
	}

	num_params -= 2;  /* Subtract feature and function */
	for (i = 0; i < num_params && i < 16; i++) {
		if (p[i] > 0xFF) {
			dd_err(hid, "debug: param %d must be 0-FF (got 0x%x)\n",
				i, p[i]);
			return -EINVAL;
		}
		params[i] = (u8)p[i];
	}

	dd_info(hid, "debug: feature=0x%02x fn=0x%02x params=[%02x %02x %02x %02x %02x %02x] count=%d\n",
		 feature, function, params[0], params[1], params[2], params[3], params[4], params[5], num_params);

	memset(&response, 0, sizeof(response));
	ret = hidpp_send_fap_command_sync(hidpp, feature, function, params,
					  num_params > 0 ? num_params : 3, &response);

	/* Store results for read */
	ff->debug_last_feature = feature;
	ff->debug_last_function = function;
	ff->debug_last_ret = ret;
	memcpy(ff->debug_last_response, response.fap.params, 16);

	dd_info(hid, "debug: ret=%d response=[%02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x %02x]\n",
		 ret,
		 response.fap.params[0], response.fap.params[1],
		 response.fap.params[2], response.fap.params[3],
		 response.fap.params[4], response.fap.params[5],
		 response.fap.params[6], response.fap.params[7],
		 response.fap.params[8], response.fap.params[9],
		 response.fap.params[10], response.fap.params[11],
		 response.fap.params[12], response.fap.params[13],
		 response.fap.params[14], response.fap.params[15]);

	return count;
}

static DEVICE_ATTR(wheel_hidpp_debug, 0600, wheel_hidpp_debug_show, wheel_hidpp_debug_store);
#endif /* CONFIG_HID_LOGITECH_HIDPP_DEBUG */

/*
 * Direct-drive wheel mode/profile sysfs attributes
 *
 * Mode: "desktop" (profile 0) or "onboard" (profiles 1-5)
 * Profile: 0 = desktop, 1-5 = onboard profiles
 */
static ssize_t wheel_mode_show(struct device *dev, struct device_attribute *attr,
			      char *buf)
{
	struct hid_device *hdev = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%s\n",
			  ff->current_mode == 0 ? "desktop" : "onboard");
}

static ssize_t wheel_mode_store(struct device *dev, struct device_attribute *attr,
			       const char *buf, size_t count)
{
	struct hid_device *hdev = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_ff_data *ff;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	if (sysfs_streq(buf, "desktop")) {
		ret = hidpp_dd_set_mode(ff, 0);
	} else if (sysfs_streq(buf, "onboard")) {
		/* Switch to onboard - use current profile if already onboard, else profile 1 */
		u8 profile = (ff->current_profile >= 1 && ff->current_profile <= 5)
			     ? ff->current_profile : 1;
		ret = hidpp_dd_set_mode(ff, profile);
	} else {
		return -EINVAL;
	}

	return ret ? ret : count;
}

static DEVICE_ATTR(wheel_mode, 0664, wheel_mode_show, wheel_mode_store);

static ssize_t wheel_profile_show(struct device *dev, struct device_attribute *attr,
				 char *buf)
{
	struct hid_device *hdev = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	return sysfs_emit(buf, "%d\n", ff->current_profile);
}

static ssize_t wheel_profile_store(struct device *dev, struct device_attribute *attr,
				  const char *buf, size_t count)
{
	struct hid_device *hdev = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_ff_data *ff;
	unsigned int profile;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	ret = kstrtouint(buf, 10, &profile);
	if (ret)
		return ret;

	if (profile > 5) {
		dd_warn(hdev, "Invalid profile %u (must be 0-5)\n", profile);
		return -EINVAL;
	}

	ret = hidpp_dd_set_mode(ff, profile);
	return ret ? ret : count;
}

static DEVICE_ATTR(wheel_profile, 0664, wheel_profile_show, wheel_profile_store);

/*
 * wheel_calibrate: echo <0..65535> sets that raw encoder value as the
 * new centre. Captures show G Hub writes absolute position (not an
 * offset) to sub-device 0x05, feature 0x812C fn=3, big-endian u16 plus
 * a trailing 0x00. Userspace reads current position via evdev; we keep
 * no state in the driver to stay a thin primitive.
 */
static ssize_t wheel_calibrate_store(struct device *dev,
				     struct device_attribute *attr,
				     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	unsigned int value;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	if (ff->idx_calibrate == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	ret = kstrtouint(buf, 0, &value);
	if (ret)
		return ret;
	if (value > 0xFFFF)
		return -ERANGE;

	/* Payload: <hi> <lo> <reserved 0x00>, big-endian per captures. */
	params[0] = (value >> 8) & 0xFF;
	params[1] = value & 0xFF;
	params[2] = 0x00;

	ret = hidpp_send_fap_to_device_sync(hidpp, ff->calibrate_dev_idx,
					    ff->idx_calibrate,
					    0x30 /* fn=3 */,
					    params, 3, &response);
	ret = hidpp_errno(hid, ret, "apply wheel_calibrate");
	if (ret)
		return ret;

	dd_info(hid, "Calibrated centre to encoder value %u\n", value);
	return count;
}

static DEVICE_ATTR(wheel_calibrate, 0220, NULL, wheel_calibrate_store);

/*
 * wheel_calibrate_here: one-shot "use current physical position as the
 * new centre". Writes any non-empty value; the driver issues fn=1 GET on
 * feature 0x812C to read the wheel's current raw encoder, then fn=3 SET
 * with that same value. Mirrors what G Hub does when the user clicks
 * Calibrate on Windows. Works on both RS50 and G Pro: same feature, same
 * sub-device, same fn numbers.
 */
static ssize_t wheel_calibrate_here_store(struct device *dev,
					  struct device_attribute *attr,
					  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3];
	u16 value;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	if (ff->idx_calibrate == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	/* Step 1: fn=1 GET current raw encoder value. */
	params[0] = params[1] = params[2] = 0;
	ret = hidpp_send_fap_to_device_sync(hidpp, ff->calibrate_dev_idx,
					    ff->idx_calibrate,
					    0x10 /* fn=1 */,
					    params, 3, &response);
	ret = hidpp_errno(hid, ret, "read encoder for calibrate_here");
	if (ret)
		return ret;

	value = (response.fap.params[0] << 8) | response.fap.params[1];

	/* Step 2: fn=3 SET that value as the new centre. */
	params[0] = (value >> 8) & 0xFF;
	params[1] = value & 0xFF;
	params[2] = 0x00;
	ret = hidpp_send_fap_to_device_sync(hidpp, ff->calibrate_dev_idx,
					    ff->idx_calibrate,
					    0x30 /* fn=3 */,
					    params, 3, &response);
	ret = hidpp_errno(hid, ret, "apply calibrate_here");
	if (ret)
		return ret;

	dd_info(hid, "Calibrated centre to current position (encoder=%u)\n",
		 value);
	return count;
}

static DEVICE_ATTR(wheel_calibrate_here, 0220, NULL,
		   wheel_calibrate_here_store);

/*
 * wheel_ffb_constant_sign: 0 or 1. Controls whether the driver flips
 * the sign of FF_CONSTANT's level before sending to the wheel. Default
 * is 1 (flipped) because Wine/Proton's DirectInput path on games like
 * ACC lands an inverted level at our evdev upload. Set 0 to pass
 * through, which matches Linux's documented evdev sign convention and
 * is correct for native-evdev apps that upload directly via EVIOCSFF.
 * See the FF_CONSTANT comment in hidpp_dd_ff_effect_tick for context.
 */
static ssize_t wheel_ffb_constant_sign_show(struct device *dev,
					    struct device_attribute *attr,
					    char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	return sysfs_emit(buf, "%u\n",
			  READ_ONCE(ff->ffb_constant_sign) ? 1U : 0U);
}

static ssize_t wheel_ffb_constant_sign_store(struct device *dev,
					     struct device_attribute *attr,
					     const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int val;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	ret = kstrtouint(buf, 10, &val);
	if (ret)
		return ret;
	if (val > 1)
		return -EINVAL;
	WRITE_ONCE(ff->ffb_constant_sign, val != 0);
	return count;
}

static DEVICE_ATTR(wheel_ffb_constant_sign, 0664,
		   wheel_ffb_constant_sign_show,
		   wheel_ffb_constant_sign_store);

/*
 * wheel_spring_damping: synthetic damping for emulated SPRING effects,
 * percent (0-100) of a DAMPER at the spring's own coefficient. See the
 * spring_damping field comment for why an undamped emulated spring
 * rings on a direct-drive wheel. 0 disables.
 */
static ssize_t wheel_spring_damping_show(struct device *dev,
					 struct device_attribute *attr,
					 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->spring_damping));
}

static ssize_t wheel_spring_damping_store(struct device *dev,
					  struct device_attribute *attr,
					  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int val;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	ret = kstrtouint(buf, 10, &val);
	if (ret)
		return ret;
	if (val > 100)
		return -EINVAL;
	WRITE_ONCE(ff->spring_damping, (u8)val);
	return count;
}

static DEVICE_ATTR(wheel_spring_damping, 0664,
		   wheel_spring_damping_show,
		   wheel_spring_damping_store);

/*
 * wheel_texture_route: where vibration-class effects (FF_RUMBLE and
 * periodic effects at 20 Hz or faster) are actuated. "tf" (default)
 * streams them on the wheel's TrueForce audio-haptic channel, matching
 * the Windows KF/TF split; "kf" sums them into the steering force
 * (legacy behaviour, makes steering feel gritty under rumble - issue
 * #8). Takes effect on the next effect tick; a live TF stream gets a
 * clean STOP when switching back to kf.
 */
static ssize_t wheel_texture_route_show(struct device *dev,
					struct device_attribute *attr,
					char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	return sysfs_emit(buf, "%s\n",
			  READ_ONCE(ff->texture_route) ==
				  HIDPP_DD_TEXTURE_ROUTE_TF ? "tf" : "kf");
}

static ssize_t wheel_texture_route_store(struct device *dev,
					 struct device_attribute *attr,
					 const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	u8 route;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;

	if (sysfs_streq(buf, "tf") || sysfs_streq(buf, "1"))
		route = HIDPP_DD_TEXTURE_ROUTE_TF;
	else if (sysfs_streq(buf, "kf") || sysfs_streq(buf, "0"))
		route = HIDPP_DD_TEXTURE_ROUTE_KF;
	else
		return -EINVAL;

	WRITE_ONCE(ff->texture_route, route);
	return count;
}

static DEVICE_ATTR(wheel_texture_route, 0664,
		   wheel_texture_route_show,
		   wheel_texture_route_store);

/*
 * wheel_range_restore: automatically restore the rotation range after
 * an external silent reset (games' SDK sessions pushing an operating
 * range at start - AC EVO pushes 90). Heavily gated; see
 * hidpp_dd_ff_range_maybe_restore. 1 = on (default), 0 = detect-only.
 */
static ssize_t wheel_range_restore_show(struct device *dev,
					struct device_attribute *attr,
					char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	return sysfs_emit(buf, "%u\n", READ_ONCE(ff->range_restore) ? 1U : 0U);
}

static ssize_t wheel_range_restore_store(struct device *dev,
					 struct device_attribute *attr,
					 const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	unsigned int val;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	ret = kstrtouint(buf, 10, &val);
	if (ret)
		return ret;
	if (val > 1)
		return -EINVAL;
	WRITE_ONCE(ff->range_restore, val != 0);
	if (val)
		ff->range_restore_attempts = 0;
	return count;
}

static DEVICE_ATTR(wheel_range_restore, 0664,
		   wheel_range_restore_show, wheel_range_restore_store);

/*
 * wheel_response_curve: the steering axis's 64-point response curve
 * (HID++ feature 0x80A4 AxisResponseCurve) - the store behind G Hub's
 * Sensitivity slider, protocol-mapped from the
 * 2026-01-30_desktop_sensitivity capture (spec section 5.1):
 *
 *   fn3 (empty params)             open upload for axis 0 (steering)
 *   22x fn4 [n][(in,out) BE16 x n] curve points, n <= 3, 64 points
 *                                  total, monotonic, (0,0)..(FFFF,FFFF)
 *   fn5 (empty params)             commit (echoes [axis][00][0040])
 *   fn6 (empty params)             revert axis to the built-in curve
 *
 * Write syntax: "reset" reverts to the built-in curve; otherwise 2-64
 * whitespace-separated "in:out" pairs (decimal 0-65535), strictly
 * increasing in `in`, non-decreasing in `out`, first pair 0:0, last
 * 65535:65535. Fewer than 64 pairs are resampled to the 64 points the
 * wheel stores by linear interpolation, so e.g.
 *   echo "0:0 32768:16384 65535:65535" > wheel_response_curve
 * uploads a softened centre. Reads report the loaded/max point count
 * straight from the wheel (fn1).
 */
static ssize_t wheel_response_curve_show(struct device *dev,
					 struct device_attribute *attr,
					 char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	u8 params[3] = { 0, 0, 0 };	/* axis 0 = steering */
	u16 loaded, max;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (ff->idx_response_curve == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	memset(&response, 0, sizeof(response));
	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_response_curve,
					  0x10 /* fn1 axis info */,
					  params, 1, &response);
	if (ret)
		return hidpp_errno(hid, ret, "read response curve info");

	/*
	 * fn1 reply: [axis][00 01 00][usage][bit width][loaded u16 BE]
	 * [max u16 BE].
	 */
	loaded = get_unaligned_be16(&response.fap.params[6]);
	max = get_unaligned_be16(&response.fap.params[8]);
	return sysfs_emit(buf, "%u/%u points loaded (0 = built-in curve)\n",
			  loaded, max);
}

#define HIDPP_DD_CURVE_POINTS		64
#define HIDPP_DD_CURVE_CHUNK		3	/* points per fn4 send */

/*
 * Revert one 0x80A4 axis to its built-in curve (fn6 [axis]). Shared by the
 * user-facing "reset" escape hatch and the best-effort mid-upload cleanup.
 * `dev_idx` is 0xff for the wheel base (steering) or 0x02 for the pedal
 * unit; hidpp_send_fap_to_device_sync handles both (device_index 0xff is
 * what hidpp_send_message_sync rewrites the base's implicit 0 into anyway).
 * Returns the raw send result: 0, a negative transport errno, or a positive
 * HID++ error byte, for the caller to log via hidpp_errno.
 */
static int hidpp_dd_response_curve_revert(struct hidpp_device *hidpp,
					  u8 dev_idx, u8 axis, u8 idx)
{
	struct hidpp_report response;
	u8 params[1] = { axis };

	memset(&response, 0, sizeof(response));
	return hidpp_send_fap_to_device_sync(hidpp, dev_idx, idx,
					     0x60 /* fn6 revert */,
					     params, 1, &response);
}

/*
 * Parse an "in:out" pair list, validate it, resample to the wheel's 64-point
 * store, and upload it to one 0x80A4 axis via fn3 open / 22x fn4 chunks /
 * fn5 commit. This is the shared body behind both the steering
 * (dev 0xff, axis 0) and pedal (dev 0x02, axis 0-2) store paths.
 *
 * `buf`/`count` cover just the pair list (the pedal store strips its leading
 * axis token first). Every send goes through hidpp_send_fap_to_device_sync:
 * for the 0xff base that is equivalent to hidpp_send_fap_command_sync (same
 * report-type choice for the 0/1/5/13-byte payloads used here, and the same
 * 0xff device index after the implicit-0 rewrite), so one path serves both.
 * fn3 open and fn6 revert carry [axis]; a SHORT report with param [0] is
 * byte-identical to the empty-param "axis 0" form the earlier steering-only
 * code sent. Stopping is re-checked between chunks (teardown mid-upload would
 * otherwise ride each remaining sync send to its full timeout), and a failed
 * chunk/commit triggers a best-effort fn6 revert so the wheel is not left
 * with a half-written store. Returns 0 or a negative errno.
 */
static int hidpp_dd_response_curve_upload(struct hidpp_device *hidpp,
					  struct hidpp_dd_ff_data *ff,
					  u8 dev_idx, u8 axis, u8 idx,
					  const char *buf, size_t count)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_report response;
	u8 openp[1] = { axis };
	u16 (*pts)[2];
	u16 curve[HIDPP_DD_CURVE_POINTS];
	int npts = 0, i, sent, ret;
	char *dup, *tok, *cur_pos;

	pts = kmalloc_array(HIDPP_DD_CURVE_POINTS, sizeof(*pts), GFP_KERNEL);
	if (!pts)
		return -ENOMEM;
	dup = kstrndup(buf, count, GFP_KERNEL);
	if (!dup) {
		kfree(pts);
		return -ENOMEM;
	}

	cur_pos = dup;
	while ((tok = strsep(&cur_pos, " \t\n")) != NULL) {
		unsigned int in, out;
		char extra;

		if (!*tok)
			continue;
		/*
		 * "%c" catches trailing junk glued to a pair ("30000:40000x"
		 * or "5:5:5"): sscanf stops at the first non-match and would
		 * otherwise return 2 for the numeric prefix, silently
		 * accepting the malformed token. A clean pair leaves nothing
		 * for %c, so a valid token returns exactly 2.
		 */
		if (npts >= HIDPP_DD_CURVE_POINTS ||
		    sscanf(tok, "%u:%u%c", &in, &out, &extra) != 2 ||
		    in > 0xFFFF || out > 0xFFFF) {
			ret = -EINVAL;
			goto out_free;
		}
		/* Strictly increasing in, non-decreasing out. */
		if (npts > 0 && (in <= pts[npts - 1][0] ||
				 out < pts[npts - 1][1])) {
			ret = -EINVAL;
			goto out_free;
		}
		pts[npts][0] = in;
		pts[npts][1] = out;
		npts++;
	}
	if (npts < 2 || pts[0][0] != 0 || pts[0][1] != 0 ||
	    pts[npts - 1][0] != 0xFFFF || pts[npts - 1][1] != 0xFFFF) {
		ret = -EINVAL;
		goto out_free;
	}

	/*
	 * Resample to the 64 evenly-spaced input positions the wheel
	 * stores, linearly interpolating output between user pairs.
	 */
	{
		int seg = 0;

		for (i = 0; i < HIDPP_DD_CURVE_POINTS; i++) {
			u32 in = (u32)i * 0xFFFF /
				 (HIDPP_DD_CURVE_POINTS - 1);
			u32 in0, in1, out0, out1;

			while (seg < npts - 2 && pts[seg + 1][0] < in)
				seg++;
			in0 = pts[seg][0];
			in1 = pts[seg + 1][0];
			out0 = pts[seg][1];
			out1 = pts[seg + 1][1];
			curve[i] = out0 + (u32)(out1 - out0) * (in - in0) /
					   (in1 - in0);
		}
	}

	/* fn3: open the upload for this axis. */
	memset(&response, 0, sizeof(response));
	ret = hidpp_send_fap_to_device_sync(hidpp, dev_idx, idx,
					    0x30 /* fn3 open */, openp, 1,
					    &response);
	if (ret) {
		ret = hidpp_errno(hid, ret, "open response curve upload");
		goto out_free;
	}

	for (sent = 0; sent < HIDPP_DD_CURVE_POINTS; ) {
		u8 chunk[1 + HIDPP_DD_CURVE_CHUNK * 4];
		int n = min(HIDPP_DD_CURVE_POINTS - sent,
			    HIDPP_DD_CURVE_CHUNK);

		/*
		 * Teardown can begin mid-upload; each remaining sync
		 * send would ride its timeout against a dead device.
		 */
		if (atomic_read_acquire(&ff->stopping)) {
			ret = -ENODEV;
			goto out_revert;
		}

		chunk[0] = n;
		for (i = 0; i < n; i++) {
			u32 in = (u32)(sent + i) * 0xFFFF /
				 (HIDPP_DD_CURVE_POINTS - 1);

			put_unaligned_be16(in, &chunk[1 + i * 4]);
			put_unaligned_be16(curve[sent + i],
					   &chunk[3 + i * 4]);
		}
		memset(&response, 0, sizeof(response));
		ret = hidpp_send_fap_to_device_sync(hidpp, dev_idx, idx,
						    0x40 /* fn4 points */,
						    chunk, 1 + n * 4,
						    &response);
		if (ret) {
			ret = hidpp_errno(hid, ret,
					  "upload response curve points");
			goto out_revert;
		}
		sent += n;
	}

	/* fn5: commit. */
	memset(&response, 0, sizeof(response));
	ret = hidpp_send_fap_to_device_sync(hidpp, dev_idx, idx,
					    0x50 /* fn5 commit */, NULL, 0,
					    &response);
	if (ret) {
		ret = hidpp_errno(hid, ret, "commit response curve");
		goto out_revert;
	}

	dd_info(hid, "Response curve uploaded (dev 0x%02x axis %u, %d user points resampled to %d)\n",
		dev_idx, axis, npts, HIDPP_DD_CURVE_POINTS);
	ret = 0;
	goto out_free;

out_revert:
	/*
	 * Best effort: abandon the half-open upload so the wheel is not
	 * left with a partially-written store. fn6 falls back to the
	 * built-in curve, which is the predictable state.
	 */
	hidpp_dd_response_curve_revert(hidpp, dev_idx, axis, idx);
out_free:
	kfree(dup);
	kfree(pts);
	return ret;
}

static ssize_t wheel_response_curve_store(struct device *dev,
					  struct device_attribute *attr,
					  const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	int ret;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (ff->idx_response_curve == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	if (sysfs_streq(buf, "reset")) {
		ret = hidpp_dd_response_curve_revert(hidpp, 0xff, 0,
						     ff->idx_response_curve);
		return ret ? hidpp_errno(hid, ret, "reset response curve")
			   : count;
	}

	ret = hidpp_dd_response_curve_upload(hidpp, ff, 0xff, 0,
					     ff->idx_response_curve, buf, count);
	return ret < 0 ? ret : count;
}

static DEVICE_ATTR(wheel_response_curve, 0664, wheel_response_curve_show,
		   wheel_response_curve_store);

/*
 * wheel_serial / wheel_firmware: read-only identity from DeviceInfo
 * (feature 0x0003), read once at init. The serial is the real
 * 12-character device serial (matches the USB iSerial); firmware shows
 * the base main FW and the motor unit's servo FW (sub-device 0x05).
 */
static ssize_t wheel_serial_show(struct device *dev,
				 struct device_attribute *attr, char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (!ff->serial[0])
		return -EOPNOTSUPP;
	return sysfs_emit(buf, "%s\n", ff->serial);
}
static DEVICE_ATTR(wheel_serial, 0444, wheel_serial_show, NULL);

static ssize_t wheel_firmware_show(struct device *dev,
				   struct device_attribute *attr, char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (!ff->fw_main[0] && !ff->fw_motor[0])
		return -EOPNOTSUPP;
	return sysfs_emit(buf, "base: %s\nmotor: %s\n",
			  ff->fw_main[0] ? ff->fw_main : "?",
			  ff->fw_motor[0] ? ff->fw_motor : "?");
}
static DEVICE_ATTR(wheel_firmware, 0444, wheel_firmware_show, NULL);

/*
 * wheel_profile_names: the onboard slots' user-assigned names, from
 * feature 0x8137 fn=3 (from the G Hub captures: `10ff173c 01` ->
 * `12ff173c 01 06 "AC EVO"` = [slot][length][ASCII name]; verified
 * against the wheel's OLED profile list). One line per slot. Reads
 * query the wheel live - they are rare and the names can change from
 * the wheel's own menu.
 */
static ssize_t wheel_profile_names_show(struct device *dev,
					struct device_attribute *attr,
					char *buf)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	ssize_t len = 0;
	u8 slot;

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (ff->idx_profile == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	for (slot = 1; slot <= 5; slot++) {
		u8 params[3] = { slot, 0, 0 };
		char name[17];
		int ret, n;

		/*
		 * Re-check between slots: teardown can start mid-loop, and
		 * each remaining sync send would then ride its full timeout
		 * against a dead device (5 slots back to back).
		 */
		if (atomic_read_acquire(&ff->stopping))
			return -ENODEV;

		/*
		 * Zero the response first: on a SHORT/LONG reply only the
		 * first few params bytes are received, and the device-
		 * reported length byte is untrusted. Without this, a length
		 * spanning unreceived bytes would emit stale stack contents
		 * through this world-readable attribute (infoleak).
		 */
		memset(&response, 0, sizeof(response));
		ret = hidpp_send_fap_command_sync(hidpp, ff->idx_profile,
						  0x30 /* fn3 getProfileName */,
						  params, 1, &response);
		if (ret) {
			len += sysfs_emit_at(buf, len, "%u: ?\n", slot);
			continue;
		}
		n = min_t(int, response.fap.params[1], sizeof(name) - 1);
		memcpy(name, &response.fap.params[2], n);
		name[n] = '\0';
		len += sysfs_emit_at(buf, len, "%u: %s\n", slot, name);
	}
	return len;
}

/*
 * Rename one onboard slot. Feature 0x8137 fn4 is the write counterpart of the
 * fn3 read above; from the G Hub capture (dev/captures/2026-07-14_profile_rename)
 * a rename to "QZX7" of slot 1 was: `11ff17 4a 01 04 51 5a 58 37` =
 * [slot][length][ASCII name]. There is no separate save/commit - the wheel
 * persists the name to its own NVM on this one write.
 *
 * Write syntax mirrors the show format's "N: name" line:
 *   echo "3:RACE" > wheel_profile_names
 * slot is 1-5; the name is the rest of the line (a leading space after the
 * colon and one trailing newline are stripped). The length check below is the
 * transport limit: 1-14 bytes, so [slot][len][name] fits one long HID++ report.
 *
 * The wheel is stricter than the transport. An RS50 accepts at most 9
 * characters (its own stock names are "PROFILE 3"/"PROFILE 4") and fails a
 * longer name at the HID++ layer, which surfaces here as -EIO rather than
 * -EINVAL; it also stores names uppercased, and accepts spaces. We keep the
 * check at the transport limit rather than hard-coding 9, because that bound is
 * observed on the RS50 and other wheels in this family may differ - the device
 * remains the authority, and its refusal is reported.
 */
static ssize_t wheel_profile_names_store(struct device *dev,
					 struct device_attribute *attr,
					 const char *buf, size_t count)
{
	struct hid_device *hid = to_hid_device(dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hid);
	struct hidpp_dd_ff_data *ff;
	struct hidpp_report response;
	unsigned int slot;
	int consumed = 0, namelen, ret;
	const char *name;
	u8 params[16];

	if (!hidpp)
		return -ENODEV;
	ff = READ_ONCE(hidpp->private_data);
	if (!ff)
		return -ENODEV;
	if (atomic_read_acquire(&ff->stopping))
		return -ENODEV;
	if (ff->idx_profile == HIDPP_DD_FEATURE_NOT_FOUND)
		return -EOPNOTSUPP;

	if (sscanf(buf, "%u:%n", &slot, &consumed) < 1 || slot < 1 || slot > 5)
		return -EINVAL;
	name = buf + consumed;
	if (*name == ' ')	/* skip the "N: name" colon-space */
		name++;
	namelen = strlen(name);
	if (namelen > 0 && name[namelen - 1] == '\n')
		namelen--;
	/* [slot][len][name...] must fit one long report's 16 param bytes. */
	if (namelen < 1 || namelen > (int)sizeof(params) - 2)
		return -EINVAL;

	params[0] = slot;
	params[1] = namelen;
	memcpy(&params[2], name, namelen);

	memset(&response, 0, sizeof(response));
	ret = hidpp_send_fap_command_sync(hidpp, ff->idx_profile,
					  0x40 /* fn4 setProfileName */,
					  params, 2 + namelen, &response);
	if (ret)
		return hidpp_errno(hid, ret, "set profile name");

	dd_info(hid, "Profile %u renamed (%d chars)\n", slot, namelen);
	return count;
}
static DEVICE_ATTR(wheel_profile_names, 0664, wheel_profile_names_show,
		   wheel_profile_names_store);

/*
 * Sysfs attribute groups.
 *
 * Each wheel carries its own attribute set. Keeping the list in one place
 * means a new attribute lands with a single entry here instead of paired
 * device_create_file / device_remove_file calls in four locations
 * (probe + destroy for RS50 and G Pro) that used to drift whenever someone
 * added or removed an attribute.
 *
 * G Pro's wheel_calibrate is gated at visibility time on whether the
 * 0x812C feature was discovered on sub-device 0x05. The visibility
 * callback runs at sysfs_create_group() time, after feature discovery
 * has populated idx_calibrate, so the gate reflects the live state.
 */
static struct attribute *hidpp_dd_wheel_group_attrs[] = {
	&dev_attr_wheel_range.attr,
	&dev_attr_wheel_strength.attr,
	&dev_attr_wheel_damping.attr,
	&dev_attr_wheel_trueforce.attr,
	&dev_attr_wheel_brake_force.attr,
	&dev_attr_wheel_sensitivity.attr,
	&dev_attr_wheel_ffb_filter.attr,
	&dev_attr_wheel_ffb_filter_auto.attr,
	&dev_attr_wheel_led_slot.attr,
	&dev_attr_wheel_led_slot_name.attr,
	&dev_attr_wheel_led_slot_brightness.attr,
	&dev_attr_wheel_led_direction.attr,
	&dev_attr_wheel_led_colors.attr,
	&dev_attr_wheel_led_apply.attr,
	&dev_attr_wheel_led_brightness.attr,
	&dev_attr_wheel_led_effect.attr,
	&dev_attr_wheel_rev_level.attr,
#ifdef CONFIG_HID_LOGITECH_HIDPP_DEBUG
	&dev_attr_wheel_hidpp_debug.attr,
#endif
	&dev_attr_wheel_mode.attr,
	&dev_attr_wheel_profile.attr,
	&dev_attr_wheel_calibrate.attr,
	&dev_attr_wheel_calibrate_here.attr,
	&dev_attr_wheel_ffb_constant_sign.attr,
	&dev_attr_wheel_spring_damping.attr,
	&dev_attr_wheel_texture_route.attr,
	&dev_attr_wheel_serial.attr,
	&dev_attr_wheel_firmware.attr,
	&dev_attr_wheel_profile_names.attr,
	&dev_attr_wheel_range_restore.attr,
	&dev_attr_wheel_response_curve.attr,
	&dev_attr_wheel_compat_range.attr,
	&dev_attr_wheel_compat_gain.attr,
	&dev_attr_wheel_compat_autocenter.attr,
	&dev_attr_wheel_compat_spring_level.attr,
	&dev_attr_wheel_compat_damper_level.attr,
	&dev_attr_wheel_compat_friction_level.attr,
	NULL,
};

/*
 * Per-model LED surface gating. The RS50 rim/base carries LIGHTSYNC
 * per-LED RGB hardware (the wheel_led_* attributes drive it in native
 * AND compat mode - verified live 2026-04-29); the real G PRO rim has
 * level-based rev lights with onboard-profile-owned colours and no
 * per-LED RGB at all (TF4ALL capture decode). Showing the wrong
 * surface would mean attributes that write protocol the rim does not
 * speak, so each identity gets only its own: LIGHTSYNC slots for
 * RS50s, wheel_rev_level for real G PROs. Identity is known at probe
 * time (product ID + product string), so evaluating at
 * sysfs_create_group() time is safe.
 */
static umode_t hidpp_dd_wheel_group_is_visible(struct kobject *kobj,
					       struct attribute *attr, int idx)
{
	struct device *dev = kobj_to_dev(kobj);
	struct hid_device *hid = to_hid_device(dev);
	bool real_gpro = dd_is_real_gpro(hid);

	/*
	 * Rev-lights use the same 0x807A level protocol on both the RS50
	 * (10-LED faceplate strip) and the G PRO rim - G HUB captures show the
	 * identical arm-burst + level pairs on RS50 native, so expose the
	 * attribute on both. The store gates on the LIGHTSYNC feature at
	 * runtime; a wheel without it returns -EOPNOTSUPP.
	 */
	if (attr == &dev_attr_wheel_rev_level.attr)
		return attr->mode;

	/*
	 * Hide every wheel_led_* LIGHTSYNC attribute on a real G PRO by
	 * NAME PREFIX rather than an explicit list, so the next
	 * LIGHTSYNC attribute added to the group is gated automatically
	 * instead of silently appearing on hardware whose 0x807A speaks
	 * a different dialect. The one deliberate exception is
	 * wheel_led_brightness: it drives feature 0x8040
	 * (BrightnessControl, an official cross-device HID++ feature),
	 * not LIGHTSYNC, and its store is feature-gated at runtime
	 * anyway.
	 */
	if (real_gpro &&
	    strncmp(attr->name, "wheel_led_", strlen("wheel_led_")) == 0 &&
	    attr != &dev_attr_wheel_led_brightness.attr)
		return 0;

	return attr->mode;
}

static const struct attribute_group hidpp_dd_wheel_group = {
	.attrs = hidpp_dd_wheel_group_attrs,
	.is_visible = hidpp_dd_wheel_group_is_visible,
};

/*
 * RS50 input mapping - filter phantom buttons declared in HID descriptor.
 *
 * The RS50 HID descriptor declares buttons 1-92 but only ~20 physically exist.
 * Buttons 81+ overflow past Linux's valid input code range, causing kernel
 * errors like "Invalid code 768 type 1".
 *
 * We only filter phantom buttons here and let HID core handle valid buttons
 * with its default sequential joystick mapping (BTN_TRIGGER, BTN_THUMB, etc.).
 * This maintains button index compatibility with Windows DirectInput.
 */
static int hidpp_dd_input_mapping(struct hid_device *hdev, struct hid_input *hi,
			      struct hid_field *field, struct hid_usage *usage,
			      unsigned long **bit, int *max)
{
	unsigned int button;

	/* Only handle Button page usages */
	if ((usage->hid & HID_USAGE_PAGE) != HID_UP_BUTTON)
		return 0;

	button = usage->hid & HID_USAGE;

	/*
	 * Filter phantom buttons that would overflow Linux input codes.
	 * Buttons 1-80 map to valid BTN_JOYSTICK + n codes.
	 */
	if (button > HIDPP_DD_MAX_BUTTON_USAGE) {
		dd_dbg(hdev, "Ignoring phantom button %u\n", button);
		return -1;
	}

	/* Let HID core use default sequential joystick mapping */
	return 0;
}

/*
 * Replay udev rules now that the wheel_* / compat sysfs attributes exist.
 *
 * The permissions rule (udev/70-logitech-trueforce.rules) RUNs a chmod/chgrp
 * over the attribute files when the hidraw device appears. That "add"
 * uevent is emitted from hid_connect(), BEFORE probe reaches the
 * sysfs_create_group() calls below, so udev can (and in practice does)
 * execute the RUN while the files don't exist yet, leaving them
 * root-only until a manual `udevadm trigger`. Emitting a "change"
 * uevent on the hidraw device after the group is in place makes udev
 * run the rule a second time with the files present. udev serialises
 * events per device, so this cannot race the original "add".
 */
static void hidpp_dd_sysfs_uevent_replay(struct hid_device *hid)
{
#if IS_ENABLED(CONFIG_HIDRAW)
	/* hid_device.hidraw is declared void * (opaque outside hidraw.c) */
	struct hidraw *hidraw = hid->hidraw;

	if (hidraw && hidraw->dev)
		kobject_uevent(&hidraw->dev->kobj, KOBJ_CHANGE);
#endif
}

static int hidpp_dd_ff_init(struct hidpp_device *hidpp)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_dd_ff_data *ff;
	int i;
	int ret;

	dd_dbg(hid, "%s started\n", __func__);

	if (!hid_is_usb(hid)) {
		dd_err(hid, "Force feedback requires USB connection (Bluetooth not supported)\n");
		return -ENODEV;
	}

	/*
	 * Check if ff_data already exists on a sibling interface.
	 * These wheels have 3 HID interfaces and probe runs for each one.
	 * We only want ONE ff_data instance with ONE timer.
	 */
	ff = hidpp_dd_find_ff_data(hid);
	if (ff) {
		dd_info(hid, "FF data already exists on sibling, skipping init\n");
		/* Store reference so this interface can use the shared ff_data */
		hidpp->private_data = ff;
		return 0;
	}

	dd_dbg(hid, "Allocating FF data\n");
	/* Allocate private data */
	ff = kzalloc(sizeof(*ff), GFP_KERNEL);
	if (!ff)
		return -ENOMEM;

	dd_dbg(hid, "Creating workqueue\n");
	/* Create workqueue for async USB transfers */
	ff->wq = create_singlethread_workqueue("hidpp-dd-ffb");
	if (!ff->wq) {
		kfree(ff);
		return -ENOMEM;
	}

	ff->hidpp = hidpp;
	ff->owner_hidpp = hidpp;	/* Track who allocated for cleanup */
	ff->range = 1080;	/* Direct-drive default: 1080 degrees */
	ff->strength = 65535;	/* Default: 100% */
	ff->damping = 0;	/* Default: 0% */
	ff->trueforce = 65535;	/* Default: 100% */
	ff->brake_force = 65535;/* Default: 100% */
	ff->ffb_filter = 11;	/* Default: ~mid-range */
	ff->ffb_filter_auto = 0;/* Default: off */
	ff->led_brightness = 100;/* Default: 100% */
	ff->led_effect = 5;	/* Default: 5=custom mode (shows custom slot colors) */

	/* Initialize LIGHTSYNC slots with default white LEDs */
	ff->led_active_slot = 0;
	for (i = 0; i < HIDPP_DD_LIGHTSYNC_NUM_SLOTS; i++) {
		int j;

		ff->led_slots[i].direction = HIDPP_DD_LIGHTSYNC_DIR_LEFT_RIGHT;
		ff->led_slots[i].brightness = 100;  /* Default: 100% */
		for (j = 0; j < HIDPP_DD_LIGHTSYNC_NUM_LEDS; j++) {
			/* Default: white (0xFF, 0xFF, 0xFF) for all LEDs */
			ff->led_slots[i].colors[j * 3 + 0] = 0xFF;
			ff->led_slots[i].colors[j * 3 + 1] = 0xFF;
			ff->led_slots[i].colors[j * 3 + 2] = 0xFF;
		}
	}

	ff->constant_force = 0;
	ff->last_force = 0;
	ff->gain = 0xFFFF;		/* 100%, games scale down from here */
	ff->ffb_constant_sign = true;	/* invert by default; Wine/Proton games rely on this */
	ff->spring_damping = HIDPP_DD_FF_SPRING_DAMPING_DEFAULT;
	ff->spring_level = 100;		/* per-class scales: neutral */
	ff->damper_level = 100;
	ff->friction_level = 100;
	ff->texture_route = HIDPP_DD_TEXTURE_ROUTE_TF;
	ff->range_restore = true;
	ff->range_restore_attempts = 0;
	ff->tf_ready = false;
	ff->tf_init_queued = false;
	ff->tf_streaming = false;
	ff->tf_recentre_sent = false;
	ff->tf_init_attempts = 0;
	mutex_init(&ff->rev_lock);
	memset16(ff->tf_window, 0x8000, HIDPP_DD_TF_WINDOW); /* offset-binary centre */
	spin_lock_init(&ff->effects_lock);
	atomic_set(&ff->sequence, 0);
	atomic_set(&ff->pending_work, 0);
	atomic_set(&ff->stopping, 0);
	atomic_set(&ff->initialized, 0);
	ff->last_err_log = 0;
	ff->err_count = 0;

	/*
	 * Initialize feature indices to "not found" so sysfs callbacks fail
	 * gracefully if accessed before deferred initialization completes.
	 * discover_features() will set valid indices for supported features.
	 */
	ff->idx_range = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_strength = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_damping = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_trueforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_brakeforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_filter = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_response_curve = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_brightness = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_lightsync = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_rgb_config = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_profile = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_profile_notify = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_sync = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_calibrate = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_angle = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_strength = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_trueforce = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_damping = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->idx_compat_filter = HIDPP_DD_FEATURE_NOT_FOUND;
	ff->calibrate_dev_idx = 0x05;	/* Centre calibration sub-device (matches G Pro) */

	/*
	 * Default SET function numbers (verified from archived G Hub captures on RS50):
	 *   range / strength / brakeforce / filter /
	 *   brightness   -> fn=2 (0x20)    (e.g. rotation_sweep shows
	 *                                  10ff182d for feature 0x18 RANGE)
	 *   damping      -> fn=1 (0x10)    damping_sweep shows 10ff141d...
	 *                                  where 1d = fn=1 (matches G Pro)
	 *   trueforce    -> fn=3 (0x30)    trueforce_sweep shows 10ff193d...
	 *                                  where 3d = fn=3 (matches G Pro)
	 */
	ff->fn_set_range = HIDPP_DD_HIDPP_FN_SET;
	ff->fn_set_strength = HIDPP_DD_HIDPP_FN_SET;
	ff->fn_set_damping = 0x10;
	ff->fn_set_trueforce = 0x30;
	ff->fn_set_brakeforce = HIDPP_DD_HIDPP_FN_SET;
	ff->fn_set_filter = HIDPP_DD_HIDPP_FN_SET;
	ff->fn_set_brightness = HIDPP_DD_HIDPP_FN_SET;

	/*
	 * Sensitivity has no readback (the 0x80A4 curve store only
	 * reports a point count); seed the write-through cache with the
	 * neutral slider midpoint (50 = linear).
	 */
	ff->sensitivity = 50;

	/*
	 * Initialize effect timer early so timer_delete_sync() in destroy
	 * is always safe, even if deferred init never runs (early unbind).
	 * The timer callback checks 'initialized' and won't do anything
	 * until hidpp_dd_ff_init_work() completes and calls mod_timer().
	 */
	timer_setup(&ff->effect_timer, hidpp_dd_ff_effect_timer_callback, 0);

	/*
	 * Initialize delayed works early so cancel_delayed_work_sync() in
	 * destroy is always safe, even if unbind happens during sysfs setup.
	 * The work functions check 'stopping' flag and exit early if set.
	 */
	INIT_DELAYED_WORK(&ff->init_work, hidpp_dd_ff_init_work);
	INIT_DELAYED_WORK(&ff->refresh_work, hidpp_dd_ff_refresh_work);
	INIT_DELAYED_WORK(&ff->range_poll_work, hidpp_dd_ff_range_poll_work);
	INIT_DELAYED_WORK(&ff->rev_work, hidpp_dd_rev_work_handler);
	INIT_WORK(&ff->settings_refresh_work, hidpp_dd_ff_settings_refresh_work);
	INIT_WORK(&ff->tf_init_work, hidpp_dd_tf_init_work_handler);

	/* Store for cleanup in hidpp_remove() */
	hidpp->private_data = ff;

	/* Create all wheel sysfs attributes in one pass (warnings non-fatal) */
	ret = sysfs_create_group(&hid->dev.kobj, &hidpp_dd_wheel_group);
	if (ret)
		dd_warn(hid, "sysfs group creation failed: %d\n", ret);
	else
		hidpp_dd_sysfs_uevent_replay(hid);

	/*
	 * Schedule deferred initialization with event-based retry.
	 * First attempt after HIDPP_DD_FF_INIT_DELAY_MS, then retry every
	 * HIDPP_DD_FF_INIT_RETRY_MS until interfaces are ready or max retries.
	 * Note: INIT_DELAYED_WORK was done early (before private_data set)
	 * to ensure cancel_delayed_work_sync is safe during early unbind.
	 */
	ff->init_retries = 0;
	queue_delayed_work(ff->wq, &ff->init_work,
			   msecs_to_jiffies(HIDPP_DD_FF_INIT_DELAY_MS));

	dd_info(hid, "Initializing force feedback...\n");
	dd_dbg(hid, "%s completed, init scheduled in %dms\n",
		__func__, HIDPP_DD_FF_INIT_DELAY_MS);
	return 0;
}

static void hidpp_dd_ff_destroy(struct hidpp_device *hidpp)
{
	struct hid_device *hid = hidpp->hid_dev;
	struct hidpp_dd_ff_data *ff = hidpp->private_data;
	struct hid_device *ff_hdev_cached;

	dd_dbg(hid, "%s started\n", __func__);

	if (!ff) {
		dd_dbg(hid, "FF is NULL, nothing to destroy\n");
		return;
	}

	/*
	 * Clear private_data FIRST to prevent any concurrent readers
	 * (e.g., raw_event callbacks) from accessing ff while we destroy it.
	 * This is defense-in-depth since hid_hw_stop() should be called
	 * before this function, but protects against edge cases.
	 */
	WRITE_ONCE(hidpp->private_data, NULL);

	/*
	 * Only the owner that allocated ff_data should do full cleanup.
	 * Other interfaces may share the ff_data pointer but shouldn't free it.
	 */
	if (ff->owner_hidpp != hidpp) {
		dd_dbg(hid, "Not ff owner, skipping full cleanup\n");
		return;
	}

	dd_dbg(hid, "Setting stopping flag\n");
	/*
	 * Signal shutdown to prevent new work and allow in-progress work
	 * to exit early. This must be done first.
	 * Use release semantics to ensure other CPUs see this before
	 * subsequent cleanup operations.
	 */
	atomic_set_release(&ff->stopping, 1);

	/*
	 * Remove the sysfs surface EARLY, right after stopping is set:
	 * sysfs_remove_group waits out any in-flight attribute handler,
	 * and once it returns no new handler can run. Several stores can
	 * re-arm the effect timer (wheel_autocenter_store) or queue work
	 * (profile/mode stores) - removing the group before the timer
	 * deletes and workqueue teardown below closes the window where a
	 * store that passed its stopping check pre-teardown re-arms a
	 * timer after the final timer_delete_sync (which would then fire
	 * on freed memory). Previously the group was removed AFTER the
	 * last timer delete, which the FFB.F4 double-delete did not cover.
	 */
	sysfs_remove_group(&hidpp->hid_dev->dev.kobj, &hidpp_dd_wheel_group);

	/*
	 * NOTE: We do NOT access ff->input->ff->private here because
	 * ff->input may already be freed if interface 0 was removed first.
	 * The input->ff->private pointer is handled in hidpp_remove()
	 * BEFORE hid_hw_stop() for both interface removal orderings.
	 */

	/*
	 * Cache ff_hdev before clearing so we can still call hid_hw_close
	 * below; the WRITE_ONCE(ff_hdev, NULL) has to happen before we
	 * cancel timers/work so late callbacks see the NULL and bail.
	 */
	ff_hdev_cached = ff->ff_hdev;

	/*
	 * Invalidate interface 0's cached copy of the shared ff pointer
	 * BEFORE this struct is freed: hidpp_dd_track_wheel_pos caches ff into
	 * the input interface's hidpp->private_data, and if that iface
	 * stays bound while the owner is torn down, its 500 Hz raw-event
	 * path would keep writing wheel_pos through a dangling pointer.
	 * The owner's own private_data was already NULLed above, so a
	 * concurrent report that re-walks the siblings finds nothing and
	 * cannot re-cache.
	 */
	if (ff->input && ff->input->dev.parent) {
		struct hid_device *in_hdev =
			to_hid_device(ff->input->dev.parent);
		struct hidpp_device *in_hidpp =
			in_hdev ? hid_get_drvdata(in_hdev) : NULL;

		if (in_hidpp && in_hidpp != hidpp &&
		    READ_ONCE(in_hidpp->private_data) == (void *)ff)
			WRITE_ONCE(in_hidpp->private_data, NULL);
	}

	/*
	 * Clear cross-interface pointers using WRITE_ONCE so timer callback
	 * and other contexts see the NULL and exit safely. This reduces the
	 * race window if sibling interfaces are removed before this one.
	 */
	WRITE_ONCE(ff->input, NULL);
	WRITE_ONCE(ff->ff_hdev, NULL);

	dd_dbg(hid, "Cancelling deferred init work\n");
	/*
	 * Cancel deferred init first: if it's still in flight it can
	 * queue refresh_work, so cancelling refresh_work before init_work
	 * would let a later init run re-arm it. Order init -> refresh.
	 * cancel_delayed_work_sync waits if the work is currently running.
	 */
	cancel_delayed_work_sync(&ff->init_work);

	dd_dbg(hid, "Cancelling refresh timer\n");
	cancel_delayed_work_sync(&ff->refresh_work);
	cancel_delayed_work_sync(&ff->range_poll_work);
	/*
	 * rev_work runs on system_unbound_wq (not ff->wq), so drain_workqueue
	 * below won't reach it - cancel it explicitly like range_poll_work.
	 * stopping is already set, so a running instance can't self-requeue.
	 */
	cancel_delayed_work_sync(&ff->rev_work);
	cancel_work_sync(&ff->settings_refresh_work);
	cancel_work_sync(&ff->tf_init_work);

	dd_dbg(hid, "Cancelling effect timer\n");
	timer_delete_sync(&ff->effect_timer);

	dd_dbg(hid, "Draining workqueue\n");
	/*
	 * Drain the workqueue - this waits for all pending work to complete
	 * and prevents new work from being queued. More robust than manual polling.
	 */
	drain_workqueue(ff->wq);

	/*
	 * Second timer_delete_sync closes FFB.F4: an input FF callback
	 * (upload/playback) that read stopping=0 before we flipped it, but
	 * hadn't yet called mod_timer, can re-arm the timer while or after
	 * the first delete_sync runs. Redo it after drain_workqueue so any
	 * such late re-arm is gone before we destroy the workqueue and kfree.
	 */
	timer_delete_sync(&ff->effect_timer);

	dd_dbg(hid, "Destroying workqueue\n");
	/*
	 * Now safe to destroy workqueue.
	 */
	destroy_workqueue(ff->wq);

	/*
	 * Close interface 2's HID device if we opened it. Use the local
	 * cache taken before WRITE_ONCE(ff_hdev, NULL) above, otherwise
	 * this branch would always short-circuit and hid_hw_close would
	 * never run (FFB.F15).
	 */
	if (ff->ff_hdev_open && ff_hdev_cached) {
		hid_hw_close(ff_hdev_cached);
		ff->ff_hdev_open = false;
	}

	dd_dbg(hid, "Freeing resources\n");
	/* ff_hdev was cleared by the WRITE_ONCE above; no redundant clear here. */

	kfree(ff);
	/* Note: hidpp->private_data was cleared at function start */

	dd_info(hid, "Force feedback unloaded\n");
	dd_dbg(hid, "%s completed\n", __func__);
}

/* -------------------------------------------------------------------------- */
/* Logitech Dinovo Mini keyboard with builtin touchpad                        */
/* -------------------------------------------------------------------------- */
#define DINOVO_MINI_PRODUCT_ID		0xb30c

static int lg_dinovo_input_mapping(struct hid_device *hdev, struct hid_input *hi,
		struct hid_field *field, struct hid_usage *usage,
		unsigned long **bit, int *max)
{
	if ((usage->hid & HID_USAGE_PAGE) != HID_UP_LOGIVENDOR)
		return 0;

	switch (usage->hid & HID_USAGE) {
	case 0x00d:
		lg_map_key_clear(KEY_MEDIA);
		break;
	default:
		return 0;
	}
	return 1;
}

/* -------------------------------------------------------------------------- */
/* HID++1.0 devices which use HID++ reports for their wheels                  */
/* -------------------------------------------------------------------------- */
static int hidpp10_wheel_connect(struct hidpp_device *hidpp)
{
	return hidpp10_set_register(hidpp, HIDPP_REG_ENABLE_REPORTS, 0,
			HIDPP_ENABLE_WHEEL_REPORT | HIDPP_ENABLE_HWHEEL_REPORT,
			HIDPP_ENABLE_WHEEL_REPORT | HIDPP_ENABLE_HWHEEL_REPORT);
}

static int hidpp10_wheel_raw_event(struct hidpp_device *hidpp,
				   u8 *data, int size)
{
	s8 value, hvalue;

	if (!hidpp->input)
		return -EINVAL;

	if (size < 7)
		return 0;

	if (data[0] != REPORT_ID_HIDPP_SHORT || data[2] != HIDPP_SUB_ID_ROLLER)
		return 0;

	value = data[3];
	hvalue = data[4];

	input_report_rel(hidpp->input, REL_WHEEL, value);
	input_report_rel(hidpp->input, REL_WHEEL_HI_RES, value * 120);
	input_report_rel(hidpp->input, REL_HWHEEL, hvalue);
	input_report_rel(hidpp->input, REL_HWHEEL_HI_RES, hvalue * 120);
	input_sync(hidpp->input);

	return 1;
}

static void hidpp10_wheel_populate_input(struct hidpp_device *hidpp,
					 struct input_dev *input_dev)
{
	__set_bit(EV_REL, input_dev->evbit);
	__set_bit(REL_WHEEL, input_dev->relbit);
	__set_bit(REL_WHEEL_HI_RES, input_dev->relbit);
	__set_bit(REL_HWHEEL, input_dev->relbit);
	__set_bit(REL_HWHEEL_HI_RES, input_dev->relbit);
}

/* -------------------------------------------------------------------------- */
/* HID++1.0 mice which use HID++ reports for extra mouse buttons              */
/* -------------------------------------------------------------------------- */
static int hidpp10_extra_mouse_buttons_connect(struct hidpp_device *hidpp)
{
	return hidpp10_set_register(hidpp, HIDPP_REG_ENABLE_REPORTS, 0,
				    HIDPP_ENABLE_MOUSE_EXTRA_BTN_REPORT,
				    HIDPP_ENABLE_MOUSE_EXTRA_BTN_REPORT);
}

static int hidpp10_extra_mouse_buttons_raw_event(struct hidpp_device *hidpp,
				    u8 *data, int size)
{
	int i;

	if (!hidpp->input)
		return -EINVAL;

	if (size < 7)
		return 0;

	if (data[0] != REPORT_ID_HIDPP_SHORT ||
	    data[2] != HIDPP_SUB_ID_MOUSE_EXTRA_BTNS)
		return 0;

	/*
	 * Buttons are either delivered through the regular mouse report *or*
	 * through the extra buttons report. At least for button 6 how it is
	 * delivered differs per receiver firmware version. Even receivers with
	 * the same usb-id show different behavior, so we handle both cases.
	 */
	for (i = 0; i < 8; i++)
		input_report_key(hidpp->input, BTN_MOUSE + i,
				 (data[3] & (1 << i)));

	/* Some mice report events on button 9+, use BTN_MISC */
	for (i = 0; i < 8; i++)
		input_report_key(hidpp->input, BTN_MISC + i,
				 (data[4] & (1 << i)));

	input_sync(hidpp->input);
	return 1;
}

static void hidpp10_extra_mouse_buttons_populate_input(
			struct hidpp_device *hidpp, struct input_dev *input_dev)
{
	/* BTN_MOUSE - BTN_MOUSE+7 are set already by the descriptor */
	__set_bit(BTN_0, input_dev->keybit);
	__set_bit(BTN_1, input_dev->keybit);
	__set_bit(BTN_2, input_dev->keybit);
	__set_bit(BTN_3, input_dev->keybit);
	__set_bit(BTN_4, input_dev->keybit);
	__set_bit(BTN_5, input_dev->keybit);
	__set_bit(BTN_6, input_dev->keybit);
	__set_bit(BTN_7, input_dev->keybit);
}

/* -------------------------------------------------------------------------- */
/* HID++1.0 kbds which only report 0x10xx consumer usages through sub-id 0x03 */
/* -------------------------------------------------------------------------- */

/* Find the consumer-page input report desc and change Maximums to 0x107f */
static u8 *hidpp10_consumer_keys_report_fixup(struct hidpp_device *hidpp,
					      u8 *_rdesc, unsigned int *rsize)
{
	/* Note 0 terminated so we can use strnstr to search for this. */
	static const char consumer_rdesc_start[] = {
		0x05, 0x0C,	/* USAGE_PAGE (Consumer Devices)       */
		0x09, 0x01,	/* USAGE (Consumer Control)            */
		0xA1, 0x01,	/* COLLECTION (Application)            */
		0x85, 0x03,	/* REPORT_ID = 3                       */
		0x75, 0x10,	/* REPORT_SIZE (16)                    */
		0x95, 0x02,	/* REPORT_COUNT (2)                    */
		0x15, 0x01,	/* LOGICAL_MIN (1)                     */
		0x26, 0x00	/* LOGICAL_MAX (...                    */
	};
	char *consumer_rdesc, *rdesc = (char *)_rdesc;
	unsigned int size;

	consumer_rdesc = strnstr(rdesc, consumer_rdesc_start, *rsize);
	size = *rsize - (consumer_rdesc - rdesc);
	if (consumer_rdesc && size >= 25) {
		consumer_rdesc[15] = 0x7f;
		consumer_rdesc[16] = 0x10;
		consumer_rdesc[20] = 0x7f;
		consumer_rdesc[21] = 0x10;
	}
	return _rdesc;
}

static int hidpp10_consumer_keys_connect(struct hidpp_device *hidpp)
{
	return hidpp10_set_register(hidpp, HIDPP_REG_ENABLE_REPORTS, 0,
				    HIDPP_ENABLE_CONSUMER_REPORT,
				    HIDPP_ENABLE_CONSUMER_REPORT);
}

static int hidpp10_consumer_keys_raw_event(struct hidpp_device *hidpp,
					   u8 *data, int size)
{
	u8 consumer_report[5];

	if (size < 7)
		return 0;

	if (data[0] != REPORT_ID_HIDPP_SHORT ||
	    data[2] != HIDPP_SUB_ID_CONSUMER_VENDOR_KEYS)
		return 0;

	/*
	 * Build a normal consumer report (3) out of the data, this detour
	 * is necessary to get some keyboards to report their 0x10xx usages.
	 */
	consumer_report[0] = 0x03;
	memcpy(&consumer_report[1], &data[3], 4);
	/* We are called from atomic context */
	/*
	 * hid_report_raw_event() gained a buffer-size parameter in mainline
	 * v7.1 (backported into the v7.0.x stable series). Kbuild defines
	 * HID_RRE_HAS_BUFSIZE when the 6-argument prototype is present, probed
	 * by arity rather than kernel version because the change was backported
	 * mid-point-release (issue #24).
	 */
#ifdef HID_RRE_HAS_BUFSIZE
	hid_report_raw_event(hidpp->hid_dev, HID_INPUT_REPORT,
			     consumer_report, sizeof(consumer_report), 5, 1);
#else
	hid_report_raw_event(hidpp->hid_dev, HID_INPUT_REPORT,
			     consumer_report, 5, 1);
#endif

	return 1;
}

/* -------------------------------------------------------------------------- */
/* High-resolution scroll wheels                                              */
/* -------------------------------------------------------------------------- */

static int hi_res_scroll_enable(struct hidpp_device *hidpp)
{
	int ret;
	u8 multiplier = 1;

	if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP20_HI_RES_WHEEL) {
		ret = hidpp_hrw_set_wheel_mode(hidpp, false, true, false);
		if (ret == 0)
			ret = hidpp_hrw_get_wheel_capability(hidpp, &multiplier);
	} else if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP20_HI_RES_SCROLL) {
		ret = hidpp_hrs_set_highres_scrolling_mode(hidpp, true,
							   &multiplier);
	} else /* if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP10_FAST_SCROLL) */ {
		ret = hidpp10_enable_scrolling_acceleration(hidpp);
		multiplier = 8;
	}
	if (ret) {
		hid_dbg(hidpp->hid_dev,
			"Could not enable hi-res scrolling: %d\n", ret);
		return ret;
	}

	if (multiplier == 0) {
		hid_dbg(hidpp->hid_dev,
			"Invalid multiplier 0 from device, setting it to 1\n");
		multiplier = 1;
	}

	hidpp->vertical_wheel_counter.wheel_multiplier = multiplier;
	hid_dbg(hidpp->hid_dev, "wheel multiplier = %d\n", multiplier);
	return 0;
}

static int hidpp_initialize_hires_scroll(struct hidpp_device *hidpp)
{
	int ret;
	unsigned long capabilities;

	capabilities = hidpp->capabilities;

	if (hidpp->protocol_major >= 2) {
		u8 feature_index;

		ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_HIRES_WHEEL,
					     &feature_index);
		if (!ret) {
			hidpp->capabilities |= HIDPP_CAPABILITY_HIDPP20_HI_RES_WHEEL;
			hid_dbg(hidpp->hid_dev, "Detected HID++ 2.0 hi-res scroll wheel\n");
			return 0;
		}
		ret = hidpp_root_get_feature(hidpp, HIDPP_PAGE_HI_RESOLUTION_SCROLLING,
					     &feature_index);
		if (!ret) {
			hidpp->capabilities |= HIDPP_CAPABILITY_HIDPP20_HI_RES_SCROLL;
			hid_dbg(hidpp->hid_dev, "Detected HID++ 2.0 hi-res scrolling\n");
		}
	} else {
		/* We cannot detect fast scrolling support on HID++ 1.0 devices */
		if (hidpp->quirks & HIDPP_QUIRK_HI_RES_SCROLL_1P0) {
			hidpp->capabilities |= HIDPP_CAPABILITY_HIDPP10_FAST_SCROLL;
			hid_dbg(hidpp->hid_dev, "Detected HID++ 1.0 fast scroll\n");
		}
	}

	if (hidpp->capabilities == capabilities)
		hid_dbg(hidpp->hid_dev, "Did not detect HID++ hi-res scrolling hardware support\n");
	return 0;
}

/* -------------------------------------------------------------------------- */
/* PID (USB HID Physical Input Device) output collection injection            */
/*                                                                            */
/* Wine's dinput hid_joystick backend drives FFB by writing PID Page 0x0F     */
/* output reports to /dev/hidraw. Our wheel's native interface 0 descriptor   */
/* has no PID collection, so those writes have nowhere to land and FFB is     */
/* silent under Proton's default (non-PROTON_ENABLE_HIDRAW) hidraw-backed     */
/* joystick path. When inject_pid=1 we append a full PID output collection    */
/* to interface 0's descriptor during .report_fixup, and install an ll_driver */
/* override that intercepts userspace output_report / raw_request calls for   */
/* the injected report IDs and translates them into our hidpp_dd_ff_* evdev FFB   */
/* path (which writes to the wheel via interface 2, the real FFB endpoint).  */
/*                                                                            */
/* The descriptor is a straight USB HID PID Page 0x0F output collection: the */
/* USB-IF's Physical Interface Device spec is vendor-neutral and the report */
/* usages (Set Effect 0x21, Effect Operation 0x77, Set Condition 0x5F,      */
/* etc.) are what Wine's dlls/dinput/joystick_hid.c matches on when it      */
/* walks the descriptor to find FFB reports - the report *IDs* are our      */
/* private choice. We ship Device Control + Set Effect + Set Envelope +     */
/* Set Condition + Set Periodic + Set Constant + Set Ramp + Effect Op +    */
/* Device Gain + Create New Effect + Block Load + Pool + Block Free, which */
/* is the full set Wine's PID parser looks up. The layout matches the       */
/* Appendix E example descriptor in the USB PID 1.0 spec.                   */
/* -------------------------------------------------------------------------- */

/*
 * Report IDs are arbitrary HID descriptor choices (the USB PID spec is silent
 * on which numeric values to use); Wine's hid_joystick PID parser walks the
 * usages, not the IDs. We pick the 0x50..0x5D range deliberately to stay
 * clear of everything else this driver already defines on the same device:
 *   0x01 - HIDPP_DD_FF_REPORT_ID         (interface 2, vendor FFB protocol)
 *   0x05 - HIDPP_DD_FF_REFRESH_ID        (interface 2)
 *   0x10 - REPORT_ID_HIDPP_SHORT
 *   0x11 - REPORT_ID_HIDPP_LONG
 *   0x12 - REPORT_ID_HIDPP_VERY_LONG
 * If any of those collide, hidpp_raw_event misinterprets a frame of our
 * synthesised reports as HID++, which is how we got a wheel-slam and a
 * "received hid++ report of bad size" storm in the first test.
 */
#define HIDPP_DD_PID_REPORT_STATE           0x50  /* Device State input (usage 0x92) */
#define HIDPP_DD_PID_REPORT_DEVICE_CONTROL  0x50  /* Device Control output (usage 0x96) - same collection as STATE */
#define HIDPP_DD_PID_REPORT_SET_EFFECT      0x51  /* Set Effect Report (usage 0x21) */
#define HIDPP_DD_PID_REPORT_SET_ENVELOPE    0x52  /* Set Envelope Report (usage 0x5A) */
#define HIDPP_DD_PID_REPORT_SET_CONDITION   0x53  /* Set Condition Report (usage 0x5F) */
#define HIDPP_DD_PID_REPORT_CREATE_NEW_EFFECT 0x54 /* Create New Effect (usage 0xAB feature) */
#define HIDPP_DD_PID_REPORT_SET_CONSTANT    0x55  /* Set Constant Force (usage 0x73) */
#define HIDPP_DD_PID_REPORT_BLOCK_LOAD      0x56  /* PID Block Load (usage 0x89 feature) */
#define HIDPP_DD_PID_REPORT_PID_POOL        0x57  /* PID Pool (usage 0x7F feature) */
#define HIDPP_DD_PID_REPORT_SET_RAMP        0x58  /* Set Ramp Force (usage 0x74) */
#define HIDPP_DD_PID_REPORT_DEVICE_GAIN     0x59  /* Device Gain (usage 0x7D) */
#define HIDPP_DD_PID_REPORT_EFFECT_OP       0x5A  /* Effect Operation (usage 0x77) */
#define HIDPP_DD_PID_REPORT_BLOCK_FREE      0x5B  /* PID Block Free (usage 0x90) */
#define HIDPP_DD_PID_REPORT_SET_PERIODIC    0x5D  /* Set Periodic (usage 0x6E) */

static const u8 hidpp_dd_pid_rdesc[] = {
	0x35, 0x00,		/* Physical Minimum (0)                      */
	0x45, 0x00,		/* Physical Maximum (0)                      */
	0x05, 0x0F,		/* Usage Page (PID)                          */
	0x09, 0x92,		/* Usage (PID State Report)                  */
	0xA1, 0x02,		/* Collection (Logical)                      */
	0x85, 0x50,		/*   Report ID (STATE/DEVICE_CONTROL) input  */
	0x09, 0x9F, 0x09, 0xA0, 0x09, 0x94,
	0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x07, 0x95, 0x01, 0x81, 0x02,
	0xC0,			/* End Collection                            */
	0x09, 0x21,		/* Usage (Set Effect Report)                 */
	0xA1, 0x02,		/* Collection (Logical)                      */
	0x85, 0x51,		/*   Report ID (SET_EFFECT)                  */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x25,		/*   Usage (Effect Type)                     */
	0xA1, 0x02,		/*   Collection (Logical)                    */
	0x09, 0x26, 0x09, 0x27, 0x09, 0x28, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32,
	0x09, 0x33, 0x09, 0x34, 0x09, 0x40, 0x09, 0x41, 0x09, 0x42, 0x09, 0x43,
	0x15, 0x01, 0x25, 0x12, 0x75, 0x08, 0x95, 0x01, 0x91, 0x00,
	0xC0,			/*   End Collection (Effect Type)            */
	0x09, 0x50, 0x09, 0x54, 0x09, 0x51, 0x09, 0xA7,
	0x15, 0x00, 0x26, 0xFF, 0x7F,
	0x66, 0x03, 0x10, 0x55, 0xFD,
	0x75, 0x10, 0x95, 0x04, 0x91, 0x02,
	0x55, 0x00, 0x66, 0x00, 0x00,
	0x09, 0x52, 0x15, 0x00, 0x26, 0x64, 0x00, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x53, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x55,		/*   Usage (Axes Enable)                     */
	0xA1, 0x02,
	0x0B, 0x30, 0x00, 0x01, 0x00,
	0x0B, 0x31, 0x00, 0x01, 0x00,
	0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x91, 0x02,
	0xC0,			/*   End Collection (Axes Enable)            */
	0x09, 0x56, 0x75, 0x01, 0x95, 0x01, 0x91, 0x02,
	0x75, 0x05, 0x95, 0x01, 0x91, 0x03,
	0x09, 0x57,		/*   Usage (Direction)                       */
	0xA1, 0x02,
	0x0B, 0x01, 0x00, 0x0A, 0x00,
	0x0B, 0x02, 0x00, 0x0A, 0x00,
	0x66, 0x14, 0x00, 0x55, 0xFE,
	0x15, 0x00, 0x27, 0x3C, 0x8C, 0x00, 0x00,
	0x75, 0x10, 0x95, 0x02, 0x91, 0x02,
	0x55, 0x00, 0x66, 0x00, 0x00,
	0xC0,			/*   End Collection (Direction)              */
	0xC0,			/* End Collection (Set Effect)               */
	0x05, 0x0F,
	0x09, 0x5A,		/* Usage (Set Envelope Report)               */
	0xA1, 0x02,
	0x85, 0x52,		/*   Report ID (SET_ENVELOPE)                */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x5B, 0x09, 0x5D,
	0x15, 0x00, 0x26, 0xFF, 0x7F,
	0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x02, 0x91, 0x02,
	0x09, 0x5C, 0x09, 0x5E,
	0x66, 0x03, 0x10, 0x55, 0xFD,
	0x26, 0xFF, 0x7F,
	0x75, 0x10, 0x95, 0x02, 0x91, 0x02,
	0x45, 0x00, 0x66, 0x00, 0x00, 0x55, 0x00,
	0xC0,			/* End Collection (Set Envelope)             */
	0x09, 0x5F,		/* Usage (Set Condition Report)              */
	0xA1, 0x02,
	0x85, 0x53,		/*   Report ID (SET_CONDITION)               */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x23, 0x15, 0x00, 0x25, 0x01, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x60, 0x09, 0x61, 0x09, 0x62,
	0x16, 0x00, 0x80, 0x26, 0xFF, 0x7F,
	0x36, 0xF0, 0xD8, 0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x03, 0x91, 0x02,
	0x09, 0x63, 0x09, 0x64, 0x09, 0x65,
	0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00,
	0x35, 0x00, 0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x03, 0x91, 0x02,
	0x45, 0x00,
	0xC0,			/* End Collection (Set Condition)            */
	0x09, 0x6E,		/* Usage (Set Periodic Report)               */
	0xA1, 0x02,
	0x85, 0x5D,		/*   Report ID (SET_PERIODIC)                */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x70, 0x15, 0x00, 0x26, 0xFF, 0x7F,
	0x35, 0x00, 0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x6F, 0x16, 0x00, 0x80, 0x26, 0xFF, 0x7F,
	0x36, 0xF0, 0xD8, 0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
	0x35, 0x00, 0x45, 0x00,
	0x09, 0x71, 0x15, 0x00, 0x27, 0x3C, 0x8C, 0x00, 0x00,
	0x66, 0x14, 0x00, 0x55, 0xFE,
	0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x72, 0x15, 0x00, 0x26, 0xFF, 0x7F,
	0x66, 0x03, 0x10, 0x55, 0xFD,
	0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
	0x65, 0x00, 0x55, 0x00,
	0xC0,			/* End Collection (Set Periodic)             */
	0x09, 0x73,		/* Usage (Set Constant Force Report)         */
	0xA1, 0x02,
	0x85, 0x55,		/*   Report ID (SET_CONSTANT)                */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x70, 0x16, 0x00, 0x80, 0x26, 0xFF, 0x7F,
	0x36, 0xF0, 0xD8, 0x46, 0x10, 0x27,
	0x75, 0x10, 0x95, 0x01, 0x91, 0x02,
	0x35, 0x00, 0x45, 0x00,
	0xC0,			/* End Collection (Set Constant Force)       */
	0x05, 0x0F,
	0x09, 0x77,		/* Usage (Effect Operation Report)           */
	0xA1, 0x02,
	0x85, 0x5A,		/*   Report ID (EFFECT_OP)                   */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28,
	0x35, 0x01, 0x45, 0x28,
	0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x35, 0x00, 0x45, 0x00,
	0x09, 0x78,		/*   Usage (Effect Operation)                */
	0xA1, 0x02,
	0x09, 0x79, 0x09, 0x7A, 0x09, 0x7B,
	0x15, 0x01, 0x25, 0x03, 0x75, 0x08, 0x95, 0x01, 0x91, 0x00,
	0xC0,
	0x09, 0x7C, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x46, 0xFF, 0x00, 0x91, 0x02,
	0x45, 0x00,
	0xC0,			/* End Collection (Effect Operation)         */
	0x09, 0x96,		/* Usage (Device Control)                    */
	0xA1, 0x02,
	0x85, 0x50,		/*   Report ID (DEVICE_CONTROL output)       */
	0x09, 0x97, 0x09, 0x98, 0x09, 0x99, 0x09, 0x9A, 0x09, 0x9B, 0x09, 0x9C,
	0x15, 0x01, 0x25, 0x06, 0x75, 0x08, 0x95, 0x01, 0x91, 0x00,
	0xC0,			/* End Collection (Device Control)           */
	0x09, 0xAB,		/* Usage (Create New Effect Report)          */
	0xA1, 0x02,
	0x85, 0x54,		/*   Report ID (CREATE_NEW_EFFECT)           */
	0x09, 0x25,		/*   Usage (Effect Type)                     */
	0xA1, 0x02,
	0x09, 0x26, 0x09, 0x27, 0x09, 0x28, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32,
	0x09, 0x33, 0x09, 0x34, 0x09, 0x40, 0x09, 0x41, 0x09, 0x42, 0x09, 0x43,
	0x15, 0x01, 0x25, 0x12, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x00,
	0xC0,
	0x05, 0x01,		/*   Usage Page (Generic Desktop)            */
	0x09, 0x3B,		/*   Usage (Byte Count)                      */
	0x15, 0x00, 0x26, 0xFF, 0x01, 0x46, 0xFF, 0x01,
	0x75, 0x0A, 0x95, 0x01, 0xB1, 0x02,
	0x75, 0x06, 0xB1, 0x01,
	0x45, 0x00,
	0xC0,			/* End Collection (Create New Effect)        */
	0x05, 0x0F,
	0x09, 0x89,		/* Usage (PID Block Load Report)             */
	0xA1, 0x02,
	0x85, 0x56,		/*   Report ID (BLOCK_LOAD)                  */
	0x09, 0x22, 0x25, 0x28, 0x15, 0x01, 0x35, 0x01, 0x45, 0x28,
	0x75, 0x08, 0x95, 0x01, 0xB1, 0x02,
	0x09, 0x8B,		/*   Usage (Block Load Status)               */
	0xA1, 0x02,
	0x09, 0x8C, 0x09, 0x8D, 0x09, 0x8E,
	0x25, 0x03, 0x15, 0x01, 0x35, 0x01, 0x45, 0x03,
	0x75, 0x08, 0x95, 0x01, 0xB1, 0x00,
	0xC0,
	0x09, 0xAC,		/*   Usage (RAM Pool Available)              */
	0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00,
	0x35, 0x00, 0x47, 0xFF, 0xFF, 0x00, 0x00,
	0x75, 0x10, 0x95, 0x01, 0xB1, 0x00,
	0x45, 0x00,
	0xC0,			/* End Collection (PID Block Load)           */
	0x09, 0x7F,		/* Usage (PID Pool Report)                   */
	0xA1, 0x02,
	0x85, 0x57,		/*   Report ID (PID_POOL)                    */
	0x09, 0x80,		/*   Usage (RAM Pool Size)                   */
	0x75, 0x10, 0x95, 0x01, 0x15, 0x00, 0x27, 0xFF, 0xFF, 0x00, 0x00, 0xB1, 0x02,
	0x09, 0x83,		/*   Usage (Simultaneous Effects Max)        */
	0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x02,
	0x09, 0xA9, 0x09, 0xAA,
	0x75, 0x01, 0x95, 0x02, 0x15, 0x00, 0x25, 0x01, 0xB1, 0x02,
	0x75, 0x06, 0x95, 0x01, 0xB1, 0x03,
	0xC0,			/* End Collection (PID Pool)                 */
	0x09, 0x7D,		/* Usage (Device Gain Report)                */
	0xA1, 0x02,
	0x85, 0x59,		/*   Report ID (DEVICE_GAIN)                 */
	0x09, 0x7E, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0xC0,			/* End Collection (Device Gain)              */
	0x09, 0x74,		/* Usage (Set Ramp Force Report)             */
	0xA1, 0x02,
	0x85, 0x58,		/*   Report ID (SET_RAMP)                    */
	0x09, 0x22, 0x15, 0x01, 0x25, 0x28, 0x75, 0x08, 0x95, 0x01, 0x91, 0x02,
	0x09, 0x75, 0x09, 0x76,
	0x15, 0x00, 0x26, 0xFF, 0x00,
	0x75, 0x08, 0x95, 0x02, 0x91, 0x02,
	0xC0,			/* End Collection (Set Ramp Force)           */
};

/*
 * Walk the original interface 0 descriptor and produce a new one with the
 * PID output collection spliced in just before the top-level closing
 * End Collection of the joystick application. Pattern matches fanatec's
 * "depth==0 && previous report_id==1" hook. The joystick's report id is
 * 1 on our wheel just like fanatec's. The output buffer is pre-allocated
 * to (original_size + sizeof(hidpp_dd_pid_rdesc) + slack).
 */
static int hidpp_dd_pid_splice_rdesc(const u8 *src, unsigned int src_size,
				 u8 *dst, unsigned int dst_cap,
				 unsigned int *out_size)
{
	const u8 *p = src, *end = src + src_size;
	u8 *q = dst, *qend = dst + dst_cap;
	unsigned depth = 0;
	bool spliced = false;
	u8 item_size;

	while (p < end) {
		/*
		 * Splice the PID collection in right before the first End
		 * Collection that brings us back to depth 0 (closing the
		 * outermost application collection). HID descriptors for
		 * joysticks pretty much always have a single top-level
		 * application collection, so this is unambiguous.
		 */
		if (*p == 0xC0 /* End Collection */) {
			if (depth == 1 && !spliced) {
				if (q + sizeof(hidpp_dd_pid_rdesc) > qend)
					return -ENOSPC;
				memcpy(q, hidpp_dd_pid_rdesc,
				       sizeof(hidpp_dd_pid_rdesc));
				q += sizeof(hidpp_dd_pid_rdesc);
				spliced = true;
			}
			if (depth > 0)
				depth--;
		}
		item_size = *p & 0x03;
		if (item_size == 3)
			item_size = 4;
		if (p + item_size + 1 > end ||
		    q + item_size + 1 > qend)
			return -ENOSPC;
		memcpy(q, p, item_size + 1);
		if (*p == 0xA1 /* Collection (Application/Logical/Physical) */)
			depth++;
		p += item_size + 1;
		q += item_size + 1;
	}
	*out_size = q - dst;
	return spliced ? 0 : -ENOENT;
}

/* -------------------------------------------------------------------------- */
/* Generic HID++ devices                                                      */
/* -------------------------------------------------------------------------- */

/*
 * If we should inject the PID output collection into this device's
 * interface 0 descriptor, do so and update *rsize. Stashes the allocation
 * on hidpp so the rewritten descriptor outlives hid_parse. Returns the
 * (possibly new) descriptor pointer the caller should hand back to
 * hid_parse. On any failure (memory, splice error, wrong interface,
 * disabled by module param) returns the original rdesc unchanged.
 */
static u8 *hidpp_dd_maybe_inject_pid_descriptor(struct hid_device *hdev,
					    struct hidpp_device *hidpp,
					    u8 *rdesc, unsigned int *rsize)
{
	struct usb_interface *intf;
	unsigned int orig_size, new_size = 0, cap;
	u8 *buf;
	int ifnum, ret;

	if (!inject_pid || !hid_is_usb(hdev))
		return rdesc;
	if (hdev->product != USB_DEVICE_ID_LOGITECH_RS50 &&
	    hdev->product != USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL &&
	    hdev->product != USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
		return rdesc;
	intf = to_usb_interface(hdev->dev.parent);
	ifnum = intf->cur_altsetting->desc.bInterfaceNumber;
	if (ifnum != 0 || hidpp->pid_fixup_buf)
		return rdesc;

	orig_size = *rsize;
	cap = orig_size + sizeof(hidpp_dd_pid_rdesc) + 16;
	buf = devm_kzalloc(&hdev->dev, cap, GFP_KERNEL);
	if (!buf) {
		dd_warn(hdev, "PID inject: out of memory, skipping\n");
		return rdesc;
	}
	ret = hidpp_dd_pid_splice_rdesc(rdesc, orig_size, buf, cap, &new_size);
	if (ret) {
		dd_warn(hdev,
			 "PID inject: splice failed %d, keeping original descriptor\n",
			 ret);
		devm_kfree(&hdev->dev, buf);
		return rdesc;
	}
	hidpp->pid_fixup_buf = buf;
	*rsize = new_size;
	dd_info(hdev,
		 "PID inject: interface 0 descriptor extended %u -> %u bytes\n",
		 orig_size, new_size);
	return buf;
}

static HIDPP_REPORT_FIXUP_RETURN_TYPE hidpp_report_fixup(struct hid_device *hdev,
				    u8 *rdesc, unsigned int *rsize)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	if (!hidpp)
		return rdesc;

	rdesc = hidpp_dd_maybe_inject_pid_descriptor(hdev, hidpp, rdesc, rsize);

	/* For 27 MHz keyboards the quirk gets set after hid_parse. */
	if (hdev->group == HID_GROUP_LOGITECH_27MHZ_DEVICE ||
	    (hidpp->quirks & HIDPP_QUIRK_HIDPP_CONSUMER_VENDOR_KEYS))
		rdesc = hidpp10_consumer_keys_report_fixup(hidpp, rdesc, rsize);

	return rdesc;
}

/* -------------------------------------------------------------------------- */
/* PID output-report translation                                              */
/*                                                                            */
/* Wine writes PID Page 0x0F output reports to /dev/hidraw14 (interface 0).   */
/* Userspace write() → hidraw_write() → hid_hw_output_report() →              */
/* ll_driver->output_report(). hid_driver callbacks are NOT invoked for       */
/* userspace-originated writes on the output path, so to intercept we must    */
/* override the ll_driver itself. We duplicate the real one in                */
/* hidpp_dd_pid_install(), override output_report / raw_request, and forward      */
/* any non-PID-report-ID calls to the saved real callbacks.                   */
/* -------------------------------------------------------------------------- */

/* Per-effect-slot state tracking. Wine's PID effect ID is 1-based (1..40);   */
/* we allocate slots 0..HIDPP_DD_FF_MAX_EFFECTS-1. Only allocated slots hold      */
/* meaningful data. `last_block_load_id` is the id Wine asked to be created  */
/* most recently and is returned by the next GET_REPORT(BLOCK_LOAD feature). */
struct hidpp_dd_pid_effect_slot {
	bool allocated;
	u8 type;		/* PID effect type index (1..12) as sent in CREATE_NEW_EFFECT / SET_EFFECT */
	u16 duration_ms;	/* 0x7FFF == infinite, per PID spec */
	u16 direction;		/* 0..35900 (hundredths of degrees) */
};

struct hidpp_dd_pid_state {
	spinlock_t lock;
	struct hidpp_dd_pid_effect_slot slots[HIDPP_DD_FF_MAX_EFFECTS];
	u8 last_block_load_id;		/* 1-based PID id from last CREATE_NEW_EFFECT */
	u8 last_block_load_status;	/* 1 == success, 2 == full, 3 == error */

	/*
	 * torn_down is flipped in hidpp_dd_pid_uninstall. Once set, our override
	 * callbacks stop dispatching to real_* (which may be pointing at
	 * usbhid internals that are themselves being torn down) and just
	 * return quickly. Avoids the fragile "swap ll_driver back" pattern
	 * that can race against in-flight hidraw calls during teardown.
	 */
	bool torn_down;

	/* Saved real ll_driver pointers so we can pass non-PID calls through. */
	const struct hid_ll_driver *real_ll_driver;
	int (*real_output_report)(struct hid_device *hdev, u8 *buf, size_t count);
	int (*real_raw_request)(struct hid_device *hdev, unsigned char reportnum,
				u8 *buf, size_t count, unsigned char rtype, int reqtype);

	/* Our mutable copy of the ll_driver for hdev->ll_driver = &this->over. */
	struct hid_ll_driver over;
};

/*
 * Map the 1-based PID effect type (as encoded in PID SET_EFFECT byte 2 and
 * CREATE_NEW_EFFECT byte 1, per USB PID spec Appendix A Usage Table) to the
 * Linux evdev FF_* constant. Undefined slots map to 0 (skipped).
 */
static u16 hidpp_dd_pid_type_to_ff(u8 pid_type)
{
	switch (pid_type) {
	case 1:  return FF_CONSTANT;
	case 2:  return FF_RAMP;
	case 3:  return FF_CUSTOM;
	case 4:  return FF_SQUARE;	/* Square periodic */
	case 5:  return FF_SINE;
	case 6:  return FF_TRIANGLE;
	case 7:  return FF_SAW_UP;
	case 8:  return FF_SAW_DOWN;
	case 9:  return FF_SPRING;
	case 10: return FF_DAMPER;
	case 11: return FF_INERTIA;
	case 12: return FF_FRICTION;
	default: return 0;
	}
}

/*
 * Look up hidpp_dd_ff_data (which lives on interface 1 / 2 for this wheel)
 * starting from interface 0's hid_device. Returns NULL if FFB hasn't
 * finished initialising yet (which is fine - output silently dropped).
 */
/*
 * Sibling-walk variant that doesn't use hid_is_usb(). hid_is_usb compares
 * hdev->ll_driver against usb_hid_driver, but our PID injection swaps
 * ll_driver to point at our override copy, so hid_is_usb returns false on
 * the very interface 0 we care about. hidpp_dd_find_ff_data short-circuits as
 * "not USB" and the hidpp_dd_ff_data is unfindable. Here we know we were
 * called from inside our own ll_driver override, which we only install on
 * USB interface 0, so we can take to_usb_interface(hdev->dev.parent)
 * directly and iterate the USB siblings ourselves.
 */
static struct hidpp_dd_ff_data *hidpp_dd_pid_get_ff(struct hid_device *if0_hdev)
{
	struct usb_interface *intf = to_usb_interface(if0_hdev->dev.parent);
	struct usb_device *udev = interface_to_usbdev(intf);
	struct hidpp_dd_ff_data *ff = NULL;
	int i;

	for (i = 0; i < USB_MAXINTERFACES; i++) {
		struct usb_interface *sibling = usb_ifnum_to_if(udev, i);
		struct hid_device *sibling_hid;
		struct hidpp_device *sibling_hidpp;

		if (!sibling || !sibling->dev.driver)
			continue;
		sibling_hid = usb_get_intfdata(sibling);
		if (!sibling_hid)
			continue;
		sibling_hidpp = hid_get_drvdata(sibling_hid);
		if (sibling_hidpp && sibling_hidpp->private_data &&
		    (sibling_hidpp->quirks & HIDPP_QUIRK_DD_FFB)) {
			ff = sibling_hidpp->private_data;
			break;
		}
	}

	if (!ff || !ff->input || !ff->input->ff || !ff->input->ff->upload)
		return NULL;
	if (!atomic_read(&ff->initialized))
		return NULL;
	return ff;
}

/*
 * Actuation helpers. All four gate on inject_pid >= 2 so the dry-run mode
 * (inject_pid==1) can exercise the full descriptor + intercept path while
 * the wheel stays completely idle. id < 0 isn't used; slot is always the
 * Wine PID effect id minus 1.
 */
static int hidpp_dd_pid_push_effect(struct hidpp_dd_ff_data *ff,
				struct ff_effect *eff)
{
	struct input_dev *in = ff->input;
	int ret;

	if (inject_pid < 2) {
		dd_info(ff->hidpp->hid_dev,
			 "PID [dry]: would upload slot=%d type=0x%x direction=%u\n",
			 eff->id, eff->type, eff->direction);
		return 0;
	}
	if (!in->ff || !in->ff->upload)
		return -ENODEV;
	ret = in->ff->upload(in, eff, NULL);
	if (ret)
		dd_dbg(ff->hidpp->hid_dev,
			"PID: upload slot=%d type=0x%x -> %d\n",
			eff->id, eff->type, ret);
	return ret;
}

static int hidpp_dd_pid_playback(struct hidpp_dd_ff_data *ff, int slot, int value)
{
	struct input_dev *in = ff->input;

	if (inject_pid < 2) {
		dd_info(ff->hidpp->hid_dev,
			 "PID [dry]: would playback slot=%d value=%d\n",
			 slot, value);
		return 0;
	}
	if (!in->ff || !in->ff->playback)
		return -ENODEV;
	return in->ff->playback(in, slot, value);
}

static int hidpp_dd_pid_erase(struct hidpp_dd_ff_data *ff, int slot)
{
	struct input_dev *in = ff->input;

	if (inject_pid < 2) {
		dd_info(ff->hidpp->hid_dev,
			 "PID [dry]: would erase slot=%d\n", slot);
		return 0;
	}
	if (!in->ff || !in->ff->erase)
		return -ENODEV;
	return in->ff->erase(in, slot);
}

static void hidpp_dd_pid_set_gain(struct hidpp_dd_ff_data *ff, u16 gain)
{
	struct input_dev *in = ff->input;

	if (inject_pid < 2) {
		dd_info(ff->hidpp->hid_dev,
			 "PID [dry]: would set_gain=%u\n", gain);
		return;
	}
	if (in->ff && in->ff->set_gain)
		in->ff->set_gain(in, gain);
}

/* Allocate / reuse / validate a slot index. Returns -1 on overflow. */
static int hidpp_dd_pid_alloc_slot(struct hidpp_dd_pid_state *ps, u8 pid_type)
{
	int i;

	for (i = 0; i < HIDPP_DD_FF_MAX_EFFECTS; i++) {
		if (!ps->slots[i].allocated) {
			ps->slots[i].allocated = true;
			ps->slots[i].type = pid_type;
			ps->slots[i].duration_ms = 0x7FFF;
			ps->slots[i].direction = 0;
			return i;
		}
	}
	return -1;
}

/*
 * Common ff_effect base for all translated PID effects. Direction is the
 * PID report's 16-bit direction field in hundredths of a degree, which we
 * normalise to the evdev 0..65535 unit circle (0 == north, increases
 * clockwise). ACC writes direction==0 for both left-on-right and
 * centre-pull constant force, relying on sign of magnitude.
 */
static void hidpp_dd_pid_fill_common(struct ff_effect *eff, int slot,
				 u16 pid_direction, u16 duration_ms)
{
	eff->id = slot;
	/* evdev direction is 16-bit, PID direction is 0..35900 (hundredths).
	 * 36000 hundredths * 65536 / 36000 == 65536, so bump by * 65536 /
	 * 36000 to stay well-defined for inputs in-range. */
	eff->direction = min_t(u32,
		((u32)pid_direction * 65536u) / 36000u, 65535u);
	eff->trigger.button = 0;
	eff->trigger.interval = 0;
	eff->replay.length = duration_ms == 0x7FFF ? 0 : duration_ms;
	eff->replay.delay = 0;
}

/*
 * Dispatch a single PID output report from userspace to the wheel's evdev
 * pipeline. Returns the number of bytes consumed (== count on success)
 * or a negative errno on failure. Caller is responsible for bounds
 * checking buf[0] against PID report IDs.
 */
static int hidpp_dd_pid_handle_output(struct hid_device *hdev, u8 *buf,
				  size_t count)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_pid_state *ps;
	struct hidpp_dd_ff_data *ff;
	unsigned long flags;
	struct ff_effect eff = {0};
	u8 rid, pid_id, op;
	int slot, ret = 0;

	if (!hidpp || !hidpp->pid_state || count < 2)
		return -EINVAL;
	ps = hidpp->pid_state;
	rid = buf[0];
	pid_id = buf[1];	/* 1-based PID effect id for most output reports */

	/* Full byte-level trace of every intercepted PID output report,
	 * rate-limited to protect the log under ACC's ~1 kHz effect stream. */
	if (printk_ratelimit())
		print_hex_dump_bytes("DD PID out: ", DUMP_PREFIX_NONE,
				     buf, min_t(size_t, count, 32));

	ff = hidpp_dd_pid_get_ff(hdev);
	/* If FFB isn't up yet, silently accept to keep Wine's enumeration
	 * happy. We'll apply effects once ff comes online. */
	switch (rid) {
	case HIDPP_DD_PID_REPORT_DEVICE_CONTROL:
		/* buf[1] = control op: 1=enable-actuators, 2=disable, 3=stop-all,
		 * 4=reset, 5=pause, 6=continue. We treat reset/stop-all as
		 * "erase all slots"; enable/disable are no-ops because the
		 * wheel is always on. */
		dd_dbg(hdev, "PID: DEVICE_CONTROL op=%u\n", buf[1]);
		if (buf[1] == 3 /* stop-all */ || buf[1] == 4 /* reset */) {
			spin_lock_irqsave(&ps->lock, flags);
			for (slot = 0; slot < HIDPP_DD_FF_MAX_EFFECTS; slot++) {
				if (ps->slots[slot].allocated && ff)
					hidpp_dd_pid_erase(ff, slot);
				ps->slots[slot].allocated = false;
				ps->slots[slot].type = 0;
			}
			spin_unlock_irqrestore(&ps->lock, flags);
		}
		return count;

	case HIDPP_DD_PID_REPORT_DEVICE_GAIN:
		dd_dbg(hdev, "PID: DEVICE_GAIN %u/255\n", buf[1]);
		if (ff)
			hidpp_dd_pid_set_gain(ff, (u16)buf[1] * 0xFFFFu / 255u);
		return count;

	case HIDPP_DD_PID_REPORT_SET_EFFECT:
		/*
		 * USB PID 1.0 spec Section 5.2 "Set Effect" Report layout:
		 *   [0]  report id
		 *   [1]  effect block index (1..N)
		 *   [2]  effect type (1..12)
		 *   [3..4] duration in ms (u16 LE, 0x7FFF == infinite)
		 *   [5..6] trigger repeat interval (u16)
		 *   [7..8] sample period (u16)
		 *   [9..10] start delay (u16)
		 *   [11] gain (u8, 0..100)
		 *   [12] trigger button (u8)
		 *   [13] axes enable bits (u8, bit0=X, bit1=Y)
		 *   [14..15] direction X (u16, 0..35900 hundredths of deg)
		 *   [16..17] direction Y (u16)
		 * Total = 18 bytes.
		 */
		if (count < 18)
			return -EINVAL;
		if (pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		spin_lock_irqsave(&ps->lock, flags);
		if (!ps->slots[slot].allocated)
			ps->slots[slot].allocated = true;
		ps->slots[slot].type = buf[2];
		ps->slots[slot].duration_ms = get_unaligned_le16(&buf[3]);
		ps->slots[slot].direction = get_unaligned_le16(&buf[14]);
		spin_unlock_irqrestore(&ps->lock, flags);
		dd_dbg(hdev,
			"PID: SET_EFFECT slot=%d type=%u dur=%u dir=%u\n",
			slot, buf[2], ps->slots[slot].duration_ms,
			ps->slots[slot].direction);
		return count;

	case HIDPP_DD_PID_REPORT_SET_CONSTANT: {
		s16 level;

		if (count < 4 || pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		level = (s16)get_unaligned_le16(&buf[2]);

		dd_dbg(hdev, "PID: SET_CONSTANT slot=%d level=%d\n",
			slot, level);

		if (!ff)
			return count;

		spin_lock_irqsave(&ps->lock, flags);
		/* Gate: don't upload unless Wine explicitly created or set
		 * this slot. Prevents random post-reset slots from becoming
		 * live with stale direction/duration. */
		if (!ps->slots[slot].allocated) {
			spin_unlock_irqrestore(&ps->lock, flags);
			return count;
		}
		hidpp_dd_pid_fill_common(&eff, slot,
				     ps->slots[slot].direction,
				     ps->slots[slot].duration_ms);
		spin_unlock_irqrestore(&ps->lock, flags);
		eff.type = FF_CONSTANT;
		eff.u.constant.level = level;
		hidpp_dd_pid_push_effect(ff, &eff);
		return count;
	}

	case HIDPP_DD_PID_REPORT_SET_CONDITION: {
		s16 center, pos_coeff, neg_coeff;
		u16 pos_sat, neg_sat, dead_band;
		u8 block_offset;
		u16 ff_type;

		/*
		 * USB PID 1.0 Section 5.3 "Set Condition" Report:
		 *   [0] report id  [1] effect block index  [2] parameter block offset
		 *   [3..4] center offset (s16)
		 *   [5..6] positive coefficient (s16)
		 *   [7..8] negative coefficient (s16)
		 *   [9..10] positive saturation (u16)
		 *   [11..12] negative saturation (u16)
		 *   [13..14] dead band (u16)
		 * Total = 15 bytes.
		 */
		if (count < 15 || pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		block_offset = buf[2];
		center     = (s16)get_unaligned_le16(&buf[3]);
		pos_coeff  = (s16)get_unaligned_le16(&buf[5]);
		neg_coeff  = (s16)get_unaligned_le16(&buf[7]);
		pos_sat    = get_unaligned_le16(&buf[9]);
		neg_sat    = get_unaligned_le16(&buf[11]);
		dead_band  = get_unaligned_le16(&buf[13]);

		dd_dbg(hdev,
			"PID: SET_CONDITION slot=%d blk=%u center=%d pcoef=%d ncoef=%d psat=%u nsat=%u dead=%u\n",
			slot, block_offset, center, pos_coeff, neg_coeff,
			pos_sat, neg_sat, dead_band);

		if (!ff || block_offset != 0)
			return count;

		spin_lock_irqsave(&ps->lock, flags);
		if (!ps->slots[slot].allocated) {
			spin_unlock_irqrestore(&ps->lock, flags);
			return count;
		}
		hidpp_dd_pid_fill_common(&eff, slot,
				     ps->slots[slot].direction,
				     ps->slots[slot].duration_ms);
		ff_type = hidpp_dd_pid_type_to_ff(ps->slots[slot].type);
		spin_unlock_irqrestore(&ps->lock, flags);
		/*
		 * If CREATE_NEW_EFFECT / SET_EFFECT didn't pin a condition
		 * effect type, drop the report. We'd rather do nothing than
		 * guess and slam the wheel with an unintended DAMPER/SPRING.
		 */
		if (ff_type != FF_SPRING && ff_type != FF_DAMPER &&
		    ff_type != FF_FRICTION && ff_type != FF_INERTIA)
			return count;
		eff.type = ff_type;
		eff.u.condition[0].right_saturation = pos_sat;
		eff.u.condition[0].left_saturation  = neg_sat;
		eff.u.condition[0].right_coeff      = pos_coeff;
		eff.u.condition[0].left_coeff       = neg_coeff;
		eff.u.condition[0].deadband         = dead_band;
		eff.u.condition[0].center           = center;
		hidpp_dd_pid_push_effect(ff, &eff);
		return count;
	}

	case HIDPP_DD_PID_REPORT_SET_PERIODIC: {
		u16 magnitude, period, phase;
		s16 offset;
		u16 ff_type;

		if (count < 12 || pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		magnitude = get_unaligned_le16(&buf[2]);
		offset    = (s16)get_unaligned_le16(&buf[4]);
		phase     = get_unaligned_le16(&buf[6]);
		period    = get_unaligned_le16(&buf[8]);

		dd_dbg(hdev,
			"PID: SET_PERIODIC slot=%d mag=%u off=%d phase=%u period=%u\n",
			slot, magnitude, offset, phase, period);

		if (!ff)
			return count;

		spin_lock_irqsave(&ps->lock, flags);
		if (!ps->slots[slot].allocated) {
			spin_unlock_irqrestore(&ps->lock, flags);
			return count;
		}
		hidpp_dd_pid_fill_common(&eff, slot,
				     ps->slots[slot].direction,
				     ps->slots[slot].duration_ms);
		ff_type = hidpp_dd_pid_type_to_ff(ps->slots[slot].type);
		spin_unlock_irqrestore(&ps->lock, flags);
		if (ff_type != FF_SINE && ff_type != FF_SQUARE &&
		    ff_type != FF_TRIANGLE && ff_type != FF_SAW_UP &&
		    ff_type != FF_SAW_DOWN)
			return count;	/* not a periodic effect, skip */
		eff.type = FF_PERIODIC;
		eff.u.periodic.waveform  = ff_type;
		eff.u.periodic.magnitude = magnitude;
		eff.u.periodic.offset    = offset;
		eff.u.periodic.phase     = phase;
		eff.u.periodic.period    = period;
		hidpp_dd_pid_push_effect(ff, &eff);
		return count;
	}

	case HIDPP_DD_PID_REPORT_EFFECT_OP:
		if (count < 4 || pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		op = buf[2];
		dd_dbg(hdev, "PID: EFFECT_OP slot=%d op=%u count=%u\n",
			slot, op, buf[3]);
		if (!ff)
			return count;
		/*
		 * Only start if slot has been both allocated and had a type
		 * set. Otherwise Wine's first-touch probing can start stale
		 * or partially-configured slots and slam the wheel.
		 */
		spin_lock_irqsave(&ps->lock, flags);
		if (!ps->slots[slot].allocated || ps->slots[slot].type == 0) {
			spin_unlock_irqrestore(&ps->lock, flags);
			return count;
		}
		spin_unlock_irqrestore(&ps->lock, flags);
		/* op: 1=start, 2=start-solo, 3=stop */
		if (op == 1 || op == 2)
			hidpp_dd_pid_playback(ff, slot, buf[3] ? buf[3] : 1);
		else if (op == 3)
			hidpp_dd_pid_playback(ff, slot, 0);
		return count;

	case HIDPP_DD_PID_REPORT_SET_RAMP: {
		s16 start, end;

		if (count < 6 || pid_id < 1 || pid_id > HIDPP_DD_FF_MAX_EFFECTS)
			return count;
		slot = pid_id - 1;
		start = (s16)get_unaligned_le16(&buf[2]);
		end   = (s16)get_unaligned_le16(&buf[4]);

		dd_dbg(hdev, "PID: SET_RAMP slot=%d start=%d end=%d\n",
			slot, start, end);

		if (!ff)
			return count;
		spin_lock_irqsave(&ps->lock, flags);
		if (!ps->slots[slot].allocated) {
			spin_unlock_irqrestore(&ps->lock, flags);
			return count;
		}
		hidpp_dd_pid_fill_common(&eff, slot,
				     ps->slots[slot].direction,
				     ps->slots[slot].duration_ms);
		spin_unlock_irqrestore(&ps->lock, flags);
		eff.type = FF_RAMP;
		eff.u.ramp.start_level = start;
		eff.u.ramp.end_level   = end;
		hidpp_dd_pid_push_effect(ff, &eff);
		return count;
	}

	default:
		return count;	/* unknown - swallow silently */
	}
	return ret;
}

/* ll_driver override: intercept output_report. Non-PID reports are passed   */
/* through to usbhid. Our PID report IDs (10..29) are always consumed here. */
static int hidpp_dd_pid_ll_output_report(struct hid_device *hdev, u8 *buf,
				     size_t count)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_pid_state *ps = hidpp ? hidpp->pid_state : NULL;

	if (!ps || ps->torn_down) {
		/*
		 * Override already marked for teardown, or never installed.
		 * Don't touch real_output_report because its backing memory
		 * may be unwinding; just refuse cleanly.
		 */
		return -ENODEV;
	}

	if (count >= 1 && buf[0] >= HIDPP_DD_PID_REPORT_STATE &&
	    buf[0] <= HIDPP_DD_PID_REPORT_SET_PERIODIC) {
		int ret = hidpp_dd_pid_handle_output(hdev, buf, count);

		return ret < 0 ? ret : (int)count;
	}
	if (ps->real_output_report)
		return ps->real_output_report(hdev, buf, count);
	return -ENOSYS;
}

/*
 * ll_driver override: intercept raw_request. Handles:
 *   - HID_REQ_SET_REPORT for PID output reports (treat as output)
 *   - HID_REQ_SET_REPORT for PID feature reports (CREATE_NEW_EFFECT: track id)
 *   - HID_REQ_GET_REPORT for PID feature reports (BLOCK_LOAD, POOL)
 * Non-PID reports flow through to usbhid.
 */
static int hidpp_dd_pid_ll_raw_request(struct hid_device *hdev,
				   unsigned char reportnum, u8 *buf,
				   size_t count, unsigned char rtype,
				   int reqtype)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_pid_state *ps = hidpp ? hidpp->pid_state : NULL;
	unsigned long flags;

	if (!ps || ps->torn_down)
		return -ENODEV;

	/* Output reports coming through SET_REPORT (vs write()) still target
	 * us. Treat identically to write() path. */
	if (reqtype == HID_REQ_SET_REPORT && rtype == HID_OUTPUT_REPORT &&
	    count >= 1 && buf[0] >= HIDPP_DD_PID_REPORT_STATE &&
	    buf[0] <= HIDPP_DD_PID_REPORT_SET_PERIODIC) {
		int ret = hidpp_dd_pid_handle_output(hdev, buf, count);

		return ret < 0 ? ret : (int)count;
	}

	/* Feature SET: CREATE_NEW_EFFECT (20) picks an effect type and we
	 * must pick an id so BLOCK_LOAD GET returns it. Layout:
	 *   buf[0] = 20 (report id)
	 *   buf[1] = effect type (1..12)
	 *   buf[2..3] = byte_count (ignored)
	 */
	if (reqtype == HID_REQ_SET_REPORT && rtype == HID_FEATURE_REPORT &&
	    reportnum == HIDPP_DD_PID_REPORT_CREATE_NEW_EFFECT && count >= 2) {
		int slot;

		spin_lock_irqsave(&ps->lock, flags);
		slot = hidpp_dd_pid_alloc_slot(ps, buf[1]);
		if (slot < 0) {
			ps->last_block_load_id = 0;
			ps->last_block_load_status = 2; /* full */
		} else {
			ps->last_block_load_id = slot + 1;
			ps->last_block_load_status = 1; /* success */
		}
		spin_unlock_irqrestore(&ps->lock, flags);
		return count;
	}

	/* Feature GET: BLOCK_LOAD (22). Return id+status+ram_pool_avail. */
	if (reqtype == HID_REQ_GET_REPORT && rtype == HID_FEATURE_REPORT &&
	    reportnum == HIDPP_DD_PID_REPORT_BLOCK_LOAD && count >= 5) {
		int free_slots, i;

		spin_lock_irqsave(&ps->lock, flags);
		free_slots = 0;
		for (i = 0; i < HIDPP_DD_FF_MAX_EFFECTS; i++)
			if (!ps->slots[i].allocated)
				free_slots++;
		buf[0] = HIDPP_DD_PID_REPORT_BLOCK_LOAD;
		buf[1] = ps->last_block_load_id;
		buf[2] = ps->last_block_load_status;
		put_unaligned_le16(free_slots * 64, &buf[3]);
		spin_unlock_irqrestore(&ps->lock, flags);
		return count;
	}

	/* Feature GET: PID_POOL (23). Return pool_size, simultaneous_max,
	 * and device-managed flags. */
	if (reqtype == HID_REQ_GET_REPORT && rtype == HID_FEATURE_REPORT &&
	    reportnum == HIDPP_DD_PID_REPORT_PID_POOL && count >= 5) {
		buf[0] = HIDPP_DD_PID_REPORT_PID_POOL;
		put_unaligned_le16(HIDPP_DD_FF_MAX_EFFECTS * 64, &buf[1]); /* ram pool size */
		buf[3] = HIDPP_DD_FF_MAX_EFFECTS;	/* simultaneous effects max */
		buf[4] = 0x03;	/* device-managed + shared pool flags */
		return count;
	}

	/* BLOCK_FREE on output path - erase effect */
	if (reqtype == HID_REQ_SET_REPORT && rtype == HID_OUTPUT_REPORT &&
	    reportnum == HIDPP_DD_PID_REPORT_BLOCK_FREE && count >= 2) {
		struct hidpp_dd_ff_data *ff = hidpp_dd_pid_get_ff(hdev);
		u8 pid_id = buf[1];

		if (pid_id >= 1 && pid_id <= HIDPP_DD_FF_MAX_EFFECTS) {
			int slot = pid_id - 1;

			spin_lock_irqsave(&ps->lock, flags);
			ps->slots[slot].allocated = false;
			spin_unlock_irqrestore(&ps->lock, flags);
			if (ff)
				hidpp_dd_pid_erase(ff, slot);
		}
		return count;
	}

	if (ps->real_raw_request)
		return ps->real_raw_request(hdev, reportnum, buf, count,
					    rtype, reqtype);
	return -ENOSYS;
}

/*
 * Install the ll_driver override on interface 0's hid_device. Must be called
 * AFTER hid_parse and BEFORE hid_hw_start so subsequent hid_hw_output_report
 * calls dispatch through us. Does nothing if inject_pid is off, if we're not
 * on interface 0, or if the original ll_driver is missing key callbacks.
 */
static int hidpp_dd_pid_install(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_pid_state *ps;
	struct usb_interface *intf;
	int ifnum;

	if (!inject_pid || !hidpp || !hid_is_usb(hdev))
		return 0;
	if (hdev->product != USB_DEVICE_ID_LOGITECH_RS50 &&
	    hdev->product != USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL &&
	    hdev->product != USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
		return 0;
	intf = to_usb_interface(hdev->dev.parent);
	ifnum = intf->cur_altsetting->desc.bInterfaceNumber;
	if (ifnum != 0)
		return 0;
	if (!hidpp->pid_fixup_buf) {
		/* Descriptor injection failed earlier - skip install too so
		 * Wine sees no PID collection and no override. */
		return 0;
	}
	if (hidpp->pid_state) {
		dd_warn(hdev, "PID: pid_state already set, refusing to install override\n");
		return -EBUSY;
	}
	if (!hdev->ll_driver || !hdev->ll_driver->raw_request) {
		dd_warn(hdev,
			 "PID: cannot install override, no real ll_driver\n");
		return -EINVAL;
	}
	ps = devm_kzalloc(&hdev->dev, sizeof(*ps), GFP_KERNEL);
	if (!ps)
		return -ENOMEM;
	spin_lock_init(&ps->lock);
	ps->real_ll_driver     = hdev->ll_driver;
	ps->real_output_report = hdev->ll_driver->output_report;
	ps->real_raw_request   = hdev->ll_driver->raw_request;
	ps->over               = *hdev->ll_driver;
	ps->over.output_report = hidpp_dd_pid_ll_output_report;
	ps->over.raw_request   = hidpp_dd_pid_ll_raw_request;

	hidpp->pid_state = ps;
	hdev->ll_driver = &ps->over;
	dd_info(hdev,
		 "PID: installed ll_driver override on interface 0 (real=%p over=%p)\n",
		 ps->real_ll_driver, &ps->over);
	return 0;
}

/*
 * Teardown: restore hdev->ll_driver to the real one and mark our state
 * dormant. The original "don't swap back" approach left hdev->ll_driver
 * pointing at the devm-allocated `over` struct after module unload,
 * causing a NULL deref in hid_hw_close when the kernel later operated
 * on the same hdev (observed: insmod-after-rmmod crashed in
 * device_reprobe -> hidinput_disconnect -> joydev_disconnect ->
 * input_close_device -> hid_hw_close calling ll_driver->close which
 * was now garbage). Restoring the pointer is correct - the previously
 * suspected race against in-flight hidraw output_report calls turned
 * out not to be the actual cause of the original rmmod crash; that
 * was a separate teardown-asymmetry issue, since fixed.
 */
static void hidpp_dd_pid_uninstall(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_pid_state *ps;

	if (!hidpp || !hidpp->pid_state)
		return;
	ps = hidpp->pid_state;
	WRITE_ONCE(ps->torn_down, true);
	if (hdev->ll_driver == &ps->over && ps->real_ll_driver)
		hdev->ll_driver = ps->real_ll_driver;
	smp_wmb();
	hidpp->pid_state = NULL;
}

static int hidpp_input_mapping(struct hid_device *hdev, struct hid_input *hi,
		struct hid_field *field, struct hid_usage *usage,
		unsigned long **bit, int *max)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	/*
	 * Direct-drive wheel button remapping works by product ID alone - it doesn't need
	 * the hidpp structure. The joystick interface has no HID++ reports,
	 * so hidpp will be NULL, but we still need to remap buttons.
	 */
	if (hdev->product == USB_DEVICE_ID_LOGITECH_RS50 ||
	    hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
	    hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
		return hidpp_dd_input_mapping(hdev, hi, field, usage, bit, max);

	if (!hidpp)
		return 0;

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_WTP)
		return wtp_input_mapping(hdev, hi, field, usage, bit, max);
	else if (hidpp->quirks & HIDPP_QUIRK_CLASS_M560 &&
			field->application != HID_GD_MOUSE)
		return m560_input_mapping(hdev, hi, field, usage, bit, max);

	if (hdev->product == DINOVO_MINI_PRODUCT_ID)
		return lg_dinovo_input_mapping(hdev, hi, field, usage, bit, max);

	return 0;
}

static int hidpp_input_mapped(struct hid_device *hdev, struct hid_input *hi,
		struct hid_field *field, struct hid_usage *usage,
		unsigned long **bit, int *max)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	if (!hidpp)
		return 0;

	/* Ensure that Logitech G920 is not given a default fuzz/flat value */
	if (hidpp->quirks & HIDPP_QUIRK_CLASS_G920) {
		if (usage->type == EV_ABS && (usage->code == ABS_X ||
				usage->code == ABS_Y || usage->code == ABS_Z ||
				usage->code == ABS_RZ)) {
			field->application = HID_GD_MULTIAXIS;
		}
	}

	return 0;
}


static void hidpp_populate_input(struct hidpp_device *hidpp,
				 struct input_dev *input)
{
	hidpp->input = input;

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_WTP)
		wtp_populate_input(hidpp, input);
	else if (hidpp->quirks & HIDPP_QUIRK_CLASS_M560)
		m560_populate_input(hidpp, input);

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_WHEELS)
		hidpp10_wheel_populate_input(hidpp, input);

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_EXTRA_MOUSE_BTNS)
		hidpp10_extra_mouse_buttons_populate_input(hidpp, input);
}

static int hidpp_input_configured(struct hid_device *hdev,
				struct hid_input *hidinput)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct input_dev *input = hidinput->input;

	if (!hidpp)
		return 0;

	hidpp_populate_input(hidpp, input);

	return 0;
}

static int hidpp_raw_hidpp_event(struct hidpp_device *hidpp, u8 *data,
		int size)
{
	struct hidpp_report *question = hidpp->send_receive_buf;
	struct hidpp_report *answer = hidpp->send_receive_buf;
	struct hidpp_report *report = (struct hidpp_report *)data;
	int ret;
	int last_online;

	/*
	 * If the mutex is locked then we have a pending answer from a
	 * previously sent command.
	 */
	if (unlikely(mutex_is_locked(&hidpp->send_mutex))) {
		/*
		 * Check for a correct hidpp20 answer or the corresponding
		 * error
		 */
		if (hidpp_match_answer(hidpp, question, report) ||
				hidpp_match_error(hidpp, question, report)) {
			*answer = *report;
			hidpp->answer_available = true;
			wake_up(&hidpp->wait);
			/*
			 * This was an answer to a command that this driver sent
			 * We return 1 to hid-core to avoid forwarding the
			 * command upstream as it has been treated by the driver
			 */

			return 1;
		}
	}

	if (unlikely(hidpp_report_is_connect_event(hidpp, report))) {
		if (schedule_work(&hidpp->work) == 0)
			dbg_hid("%s: connect event already queued\n", __func__);
		return 1;
	}

	if (hidpp->hid_dev->group == HID_GROUP_LOGITECH_27MHZ_DEVICE &&
	    data[0] == REPORT_ID_HIDPP_SHORT &&
	    data[2] == HIDPP_SUB_ID_USER_IFACE_EVENT &&
	    (data[3] & HIDPP_USER_IFACE_EVENT_ENCRYPTION_KEY_LOST)) {
		dev_err_ratelimited(&hidpp->hid_dev->dev,
			"Error the keyboard's wireless encryption key has been lost, your keyboard will not work unless you re-configure encryption.\n");
		dev_err_ratelimited(&hidpp->hid_dev->dev,
			"See: https://gitlab.freedesktop.org/jwrdegoede/logitech-27mhz-keyboard-encryption-setup/\n");
	}

	last_online = hidpp->battery.online;
	if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP20_BATTERY) {
		ret = hidpp20_battery_event_1000(hidpp, data, size);
		if (ret != 0)
			return ret;
		ret = hidpp20_battery_event_1004(hidpp, data, size);
		if (ret != 0)
			return ret;
		ret = hidpp_solar_battery_event(hidpp, data, size);
		if (ret != 0)
			return ret;
		ret = hidpp20_battery_voltage_event(hidpp, data, size);
		if (ret != 0)
			return ret;
		ret = hidpp20_adc_measurement_event_1f20(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP10_BATTERY) {
		ret = hidpp10_battery_event(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	if (hidpp->quirks & HIDPP_QUIRK_RESET_HI_RES_SCROLL) {
		if (last_online == 0 && hidpp->battery.online == 1)
			schedule_work(&hidpp->reset_hi_res_work);
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_WHEELS) {
		ret = hidpp10_wheel_raw_event(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_EXTRA_MOUSE_BTNS) {
		ret = hidpp10_extra_mouse_buttons_raw_event(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_CONSUMER_VENDOR_KEYS) {
		ret = hidpp10_consumer_keys_raw_event(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	if (hidpp->quirks & HIDPP_QUIRK_DD_FFB) {
		ret = hidpp_dd_ff_raw_hidpp_event(hidpp, data, size);
		if (ret != 0)
			return ret;
	}

	return 0;
}

/*
 * Find direct-drive FF data from any interface by searching sibling interfaces.
 * This is needed because joystick reports come on interface 0, which has
 * no hidpp structure, but we need to update the wheel position in the
 * FF data stored on interface 1.
 */
/*
 * Locate the shared hidpp_dd_ff_data allocated by the FFB-owning interface
 * (interface 1) from any other interface of the same USB device.
 *
 * Serialization (PROBE.F24): USB core tears down interfaces of a
 * multi-interface device in reverse order under the usb_device's lock
 * (`usb_disable_device`), so interface 1's `hidpp_dd_ff_destroy` runs to
 * completion (WRITE_ONCE(private_data, NULL); kfree(ff)) before
 * interface 0's `hidpp_remove` starts. By then interface 1's
 * private_data is already NULL, so this lookup returns NULL on
 * interface 0's remove path and the interface-0 cleanup becomes a
 * safe no-op. No explicit module-level lock is needed.
 */
static struct hidpp_dd_ff_data *hidpp_dd_find_ff_data(struct hid_device *hdev)
{
	struct usb_interface *intf;
	struct usb_device *udev;
	int i;

	if (!hid_is_usb(hdev)) {
		dd_dbg(hdev, "find_ff_data: not USB device\n");
		return NULL;
	}

	intf = to_usb_interface(hdev->dev.parent);
	udev = interface_to_usbdev(intf);

	/* Search all interfaces for the one with direct-drive FF data */
	for (i = 0; i < USB_MAXINTERFACES; i++) {
		struct usb_interface *sibling = usb_ifnum_to_if(udev, i);
		struct hid_device *sibling_hid;
		struct hidpp_device *sibling_hidpp;

		if (!sibling || !sibling->dev.driver)
			continue;

		/* Check if this interface has an hid_device */
		sibling_hid = usb_get_intfdata(sibling);
		if (!sibling_hid) {
			dd_dbg(hdev, "find_ff_data: intf %d no hid_device\n", i);
			continue;
		}

		sibling_hidpp = hid_get_drvdata(sibling_hid);
		dd_dbg(hdev, "find_ff_data: intf %d hidpp=%p private=%p quirks=0x%lx\n",
			i, sibling_hidpp,
			sibling_hidpp ? sibling_hidpp->private_data : NULL,
			sibling_hidpp ? sibling_hidpp->quirks : 0);

		if (sibling_hidpp && sibling_hidpp->private_data &&
		    (sibling_hidpp->quirks & HIDPP_QUIRK_DD_FFB)) {
			struct hidpp_dd_ff_data *ff = sibling_hidpp->private_data;
			/*
			 * Return the ff regardless of the sibling's stopping
			 * flag. hidpp_remove's interface-0 path needs to clear
			 * input->ff->private even when interface 1 has already
			 * flipped stopping=1; otherwise hid_hw_stop's
			 * input_ff_destroy would kfree the same pointer that
			 * interface 1's hidpp_dd_ff_destroy is about to kfree
			 * (FFB.F22). ff is kept alive by interface 1 until
			 * its own kfree at the very end of hidpp_dd_ff_destroy,
			 * which runs long after all the field accesses here
			 * would complete. Runtime callers (hidpp_dd_ff_init,
			 * hidpp_dd_ff_refresh_work) already re-check stopping
			 * themselves, so losing the check here is safe.
			 */
			dd_dbg(hdev, "find_ff_data: FOUND on intf %d\n", i);
			return ff;
		}
	}

	dd_dbg(hdev, "find_ff_data: NOT FOUND\n");
	return NULL;
}

static int hidpp_raw_event(struct hid_device *hdev, struct hid_report *report,
		u8 *data, int size)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	int ret = 0;

	if (!hidpp)
		return 0;

	/*
	 * Generic HID++ processing, but only on an interface that actually
	 * carries HID++.
	 *
	 * A direct-drive wheel's interface 0 is a joystick whose input report
	 * declares no report ID, so data[0] is the first DATA byte: the 4-bit
	 * hat switch plus buttons 1-4. That byte can equal a HID++ report ID
	 * (D-pad Up + button 1 is 0x10 = REPORT_ID_HIDPP_SHORT; Up-Right and
	 * Right give 0x11 and 0x12). Parsing such a frame as HID++ hit the
	 * size check below, logged a "bad size" storm and - because a non-zero
	 * return tells hid-core the report was consumed - dropped the frame
	 * outright, freezing steering, pedals and buttons for as long as the
	 * combination was held.
	 */
	if (!hidpp->no_hidpp_reports) {
		switch (data[0]) {
		case REPORT_ID_HIDPP_VERY_LONG:
			if (size != hidpp->very_long_report_length) {
				hid_err(hdev, "received hid++ report of bad size (%d)",
					size);
				return 1;
			}
			ret = hidpp_raw_hidpp_event(hidpp, data, size);
			break;
		case REPORT_ID_HIDPP_LONG:
			if (size != HIDPP_REPORT_LONG_LENGTH) {
				hid_err(hdev, "received hid++ report of bad size (%d)",
					size);
				return 1;
			}
			ret = hidpp_raw_hidpp_event(hidpp, data, size);
			break;
		case REPORT_ID_HIDPP_SHORT:
			if (size != HIDPP_REPORT_SHORT_LENGTH) {
				hid_err(hdev, "received hid++ report of bad size (%d)",
					size);
				return 1;
			}
			ret = hidpp_raw_hidpp_event(hidpp, data, size);
			break;
		}

		/* If no report is available for further processing, skip calling
		 * raw_event of subclasses. */
		if (ret != 0)
			return ret;
	}

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_WTP)
		return wtp_raw_event(hdev, data, size);
	else if (hidpp->quirks & HIDPP_QUIRK_CLASS_M560)
		return m560_raw_event(hdev, data, size);

	/*
	 * Process direct-drive joystick reports for steering-position tracking.
	 * Only process 30-byte reports from interface 0 (joystick).
	 * Checking the interface number first guards against a 30-byte
	 * non-HID++ report arriving on interface 1 or 2 being mistaken for
	 * a joystick report.
	 *
	 * Pedal axes (throttle/brake/clutch) are left untouched here and
	 * reach userspace raw; shaping them is done in HID-BPF, not this
	 * driver.
	 *
	 * The D-pad is left to hid-input's native hat-switch mapping: the
	 * interface-0 descriptor declares a standard Hat Switch usage (logical
	 * 0-7 over 0-315 degrees) that the HID core decodes correctly. An
	 * earlier hand-rolled byte-0 decode assumed a non-standard encoding and
	 * emitted scrambled directions (e.g. Left reported as Down), so it was
	 * removed (issue #22).
	 */
	if ((hidpp->quirks & HIDPP_QUIRK_DD_FFB) &&
	    size == HIDPP_DD_INPUT_REPORT_SIZE &&
	    data[0] != REPORT_ID_HIDPP_SHORT &&
	    data[0] != REPORT_ID_HIDPP_LONG &&
	    data[0] != REPORT_ID_HIDPP_VERY_LONG &&
	    hid_is_usb(hdev)) {
		struct usb_interface *intf = to_usb_interface(hdev->dev.parent);

		if (intf->cur_altsetting->desc.bInterfaceNumber == 0)
			hidpp_dd_track_wheel_pos(hidpp, data, size);
	}

	return 0;
}

/*
 * Track the live steering-wheel position from interface-0 input reports.
 * Pedal shaping (curves/deadzones/combined mode) used to happen here in
 * software; it never actually worked (a report-propagation bug meant the
 * G Hub pedal UI settings never reached this path) and has been removed.
 * Pedal axes now reach userspace raw; shaping them is HID-BPF's job.
 *
 * Joystick report format (30 bytes, offset 4+):
 *   Offset 4-5: Wheel position (u16 LE)
 *   Offset 6-7: Accelerator/Throttle (u16 LE, passed through raw)
 *   Offset 8-9: Brake (u16 LE, passed through raw)
 *   Offset 10-11: Clutch (u16 LE, passed through raw)
 */
static void hidpp_dd_track_wheel_pos(struct hidpp_device *hidpp, u8 *data, int size)
{
	struct hidpp_dd_ff_data *ff;

	if (!hidpp || !(hidpp->quirks & HIDPP_QUIRK_DD_FFB))
		return;

	/*
	 * Interface 0's hidpp is brought up via hidpp_dd_minimal_probe which
	 * doesn't run hidpp_dd_ff_init and therefore never writes to
	 * hidpp->private_data. At raw_event time the shared ff_data lives
	 * on interface 1's hidpp instead. If our own slot is empty, walk
	 * the siblings, cache the pointer, and use it. This is what kept
	 * the interface-0 input path from ever updating wheel_pos before
	 * this commit.
	 */
	ff = READ_ONCE(hidpp->private_data);
	if (!ff) {
		ff = hidpp_dd_find_ff_data(hidpp->hid_dev);
		if (!ff)
			return;
		WRITE_ONCE(hidpp->private_data, ff);
	}

	/* Don't process during shutdown */
	if (atomic_read_acquire(&ff->stopping))
		return;

	/* Need at least the wheel-position field */
	if (size < 6)
		return;

	/*
	 * Steering axis lives at report bytes 4-5 as a little-endian u16
	 * (0x0000 full left, 0x8000 centre, 0xFFFF full right), per the
	 * interface-0 HID descriptor (usage page 0x01 generic desktop,
	 * usage 0x30 X). Publish it lock-free for the FFB condition-
	 * effect tick (SPRING/DAMPER/FRICTION/INERTIA rely on it).
	 */
	WRITE_ONCE(ff->wheel_pos, get_unaligned_le16(&data[4]));
	if (!READ_ONCE(ff->wheel_pos_seen))
		WRITE_ONCE(ff->wheel_pos_seen, true);
}

static int hidpp_event(struct hid_device *hdev, struct hid_field *field,
	struct hid_usage *usage, __s32 value)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_scroll_counter *counter;

	if (!hidpp)
		return 0;

	counter = &hidpp->vertical_wheel_counter;
	/* A scroll event may occur before the multiplier has been retrieved or
	 * the input device set, or high-res scroll enabling may fail. In such
	 * cases we must return early (falling back to default behaviour) to
	 * avoid a crash in hidpp_scroll_counter_handle_scroll.
	 */
	if (!(hidpp->capabilities & HIDPP_CAPABILITY_HI_RES_SCROLL)
	    || value == 0 || hidpp->input == NULL
	    || counter->wheel_multiplier == 0)
		return 0;

	hidpp_scroll_counter_handle_scroll(hidpp->input, counter, value);
	return 1;
}

static int hidpp_initialize_battery(struct hidpp_device *hidpp)
{
	static atomic_t battery_no = ATOMIC_INIT(0);
	struct power_supply_config cfg = { .drv_data = hidpp };
	struct power_supply_desc *desc = &hidpp->battery.desc;
	enum power_supply_property *battery_props;
	struct hidpp_battery *battery;
	unsigned int num_battery_props;
	unsigned long n;
	int ret;

	if (hidpp->battery.ps)
		return 0;

	hidpp->battery.feature_index = 0xff;
	hidpp->battery.solar_feature_index = 0xff;
	hidpp->battery.voltage_feature_index = 0xff;
	hidpp->battery.adc_measurement_feature_index = 0xff;

	if (hidpp->protocol_major >= 2) {
		if (hidpp->quirks & HIDPP_QUIRK_CLASS_K750)
			ret = hidpp_solar_request_battery_event(hidpp);
		else {
			/* we only support one battery feature right now, so let's
			   first check the ones that support battery level first
			   and leave voltage for last */
			ret = hidpp20_query_battery_info_1000(hidpp);
			if (ret)
				ret = hidpp20_query_battery_info_1004(hidpp);
			if (ret)
				ret = hidpp20_query_battery_voltage_info(hidpp);
			if (ret)
				ret = hidpp20_query_adc_measurement_info_1f20(hidpp);
		}

		if (ret)
			return ret;
		hidpp->capabilities |= HIDPP_CAPABILITY_HIDPP20_BATTERY;
	} else {
		ret = hidpp10_query_battery_status(hidpp);
		if (ret) {
			ret = hidpp10_query_battery_mileage(hidpp);
			if (ret)
				return -ENOENT;
			hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_MILEAGE;
		} else {
			hidpp->capabilities |= HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS;
		}
		hidpp->capabilities |= HIDPP_CAPABILITY_HIDPP10_BATTERY;
	}

	battery_props = devm_kmemdup(&hidpp->hid_dev->dev,
				     hidpp_battery_props,
				     sizeof(hidpp_battery_props),
				     GFP_KERNEL);
	if (!battery_props)
		return -ENOMEM;

	num_battery_props = ARRAY_SIZE(hidpp_battery_props) - 3;

	if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_MILEAGE ||
	    hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_PERCENTAGE ||
	    hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_VOLTAGE ||
	    hidpp->capabilities & HIDPP_CAPABILITY_ADC_MEASUREMENT)
		battery_props[num_battery_props++] =
				POWER_SUPPLY_PROP_CAPACITY;

	if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_LEVEL_STATUS)
		battery_props[num_battery_props++] =
				POWER_SUPPLY_PROP_CAPACITY_LEVEL;

	if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_VOLTAGE ||
	    hidpp->capabilities & HIDPP_CAPABILITY_ADC_MEASUREMENT)
		battery_props[num_battery_props++] =
			POWER_SUPPLY_PROP_VOLTAGE_NOW;

	battery = &hidpp->battery;

	n = atomic_inc_return(&battery_no) - 1;
	desc->properties = battery_props;
	desc->num_properties = num_battery_props;
	desc->get_property = hidpp_battery_get_property;
	sprintf(battery->name, "hidpp_battery_%ld", n);
	desc->name = battery->name;
	desc->type = POWER_SUPPLY_TYPE_BATTERY;
	desc->use_for_apm = 0;

	battery->ps = devm_power_supply_register(&hidpp->hid_dev->dev,
						 &battery->desc,
						 &cfg);
	if (IS_ERR(battery->ps))
		return PTR_ERR(battery->ps);

	power_supply_powers(battery->ps, &hidpp->hid_dev->dev);

	return ret;
}

/* Get name + serial for USB and Bluetooth HID++ devices */
static void hidpp_non_unifying_init(struct hidpp_device *hidpp)
{
	struct hid_device *hdev = hidpp->hid_dev;
	char *name;

	/* Bluetooth devices already have their serialnr set */
	if (hid_is_usb(hdev))
		hidpp_serial_init(hidpp);

	name = hidpp_get_device_name(hidpp);
	if (name) {
		dbg_hid("HID++: Got name: %s\n", name);
		snprintf(hdev->name, sizeof(hdev->name), "%s", name);
		kfree(name);
	}
}

static int hidpp_input_open(struct input_dev *dev)
{
	struct hid_device *hid = input_get_drvdata(dev);

	return hid_hw_open(hid);
}

static void hidpp_input_close(struct input_dev *dev)
{
	struct hid_device *hid = input_get_drvdata(dev);

	hid_hw_close(hid);
}

static struct input_dev *hidpp_allocate_input(struct hid_device *hdev)
{
	struct input_dev *input_dev = devm_input_allocate_device(&hdev->dev);
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);

	if (!input_dev)
		return NULL;

	input_set_drvdata(input_dev, hdev);
	input_dev->open = hidpp_input_open;
	input_dev->close = hidpp_input_close;

	input_dev->name = hidpp->name;
	input_dev->phys = hdev->phys;
	input_dev->uniq = hdev->uniq;
	input_dev->id.bustype = hdev->bus;
	input_dev->id.vendor  = hdev->vendor;
	input_dev->id.product = hdev->product;
	input_dev->id.version = hdev->version;
	input_dev->dev.parent = &hdev->dev;

	return input_dev;
}

static void hidpp_connect_event(struct work_struct *work)
{
	struct hidpp_device *hidpp = container_of(work, struct hidpp_device, work);
	struct hid_device *hdev = hidpp->hid_dev;
	struct input_dev *input;
	char *name, *devm_name;
	int ret;

	/* Get device version to check if it is connected */
	ret = hidpp_root_get_protocol_version(hidpp);
	if (ret) {
		hid_dbg(hidpp->hid_dev, "Disconnected\n");
		if (hidpp->battery.ps) {
			hidpp->battery.online = false;
			hidpp->battery.status = POWER_SUPPLY_STATUS_UNKNOWN;
			hidpp->battery.level = POWER_SUPPLY_CAPACITY_LEVEL_UNKNOWN;
			power_supply_changed(hidpp->battery.ps);
		}
		return;
	}

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_WTP) {
		ret = wtp_connect(hdev);
		if (ret)
			return;
	} else if (hidpp->quirks & HIDPP_QUIRK_CLASS_M560) {
		ret = m560_send_config_command(hdev);
		if (ret)
			return;
	} else if (hidpp->quirks & HIDPP_QUIRK_CLASS_K400) {
		ret = k400_connect(hdev);
		if (ret)
			return;
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_WHEELS) {
		ret = hidpp10_wheel_connect(hidpp);
		if (ret)
			return;
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_EXTRA_MOUSE_BTNS) {
		ret = hidpp10_extra_mouse_buttons_connect(hidpp);
		if (ret)
			return;
	}

	if (hidpp->quirks & HIDPP_QUIRK_HIDPP_CONSUMER_VENDOR_KEYS) {
		ret = hidpp10_consumer_keys_connect(hidpp);
		if (ret)
			return;
	}

	if (hidpp->protocol_major >= 2) {
		u8 feature_index;

		if (!hidpp_get_wireless_feature_index(hidpp, &feature_index))
			hidpp->wireless_feature_index = feature_index;
	}

	if (hidpp->name == hdev->name && hidpp->protocol_major >= 2) {
		name = hidpp_get_device_name(hidpp);
		if (name) {
			devm_name = devm_kasprintf(&hdev->dev, GFP_KERNEL,
						   "%s", name);
			kfree(name);
			if (!devm_name)
				return;

			hidpp->name = devm_name;
		}
	}

	hidpp_initialize_battery(hidpp);
	if (!hid_is_usb(hidpp->hid_dev))
		hidpp_initialize_hires_scroll(hidpp);

	/* forward current battery state */
	if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP10_BATTERY) {
		hidpp10_enable_battery_reporting(hidpp);
		if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_MILEAGE)
			hidpp10_query_battery_mileage(hidpp);
		else
			hidpp10_query_battery_status(hidpp);
	} else if (hidpp->capabilities & HIDPP_CAPABILITY_HIDPP20_BATTERY) {
		if (hidpp->capabilities & HIDPP_CAPABILITY_BATTERY_VOLTAGE)
			hidpp20_query_battery_voltage_info(hidpp);
		else if (hidpp->capabilities & HIDPP_CAPABILITY_UNIFIED_BATTERY)
			hidpp20_query_battery_info_1004(hidpp);
		else if (hidpp->capabilities & HIDPP_CAPABILITY_ADC_MEASUREMENT)
			hidpp20_query_adc_measurement_info_1f20(hidpp);
		else
			hidpp20_query_battery_info_1000(hidpp);
	}
	if (hidpp->battery.ps)
		power_supply_changed(hidpp->battery.ps);

	if (hidpp->capabilities & HIDPP_CAPABILITY_HI_RES_SCROLL)
		hi_res_scroll_enable(hidpp);

	if (!(hidpp->quirks & HIDPP_QUIRK_DELAYED_INIT) || hidpp->delayed_input)
		/* if the input nodes are already created, we can stop now */
		return;

	input = hidpp_allocate_input(hdev);
	if (!input) {
		hid_err(hdev, "cannot allocate new input device: %d\n", ret);
		return;
	}

	hidpp_populate_input(hidpp, input);

	ret = input_register_device(input);
	if (ret) {
		input_free_device(input);
		return;
	}

	hidpp->delayed_input = input;
}

static void hidpp_reset_hi_res_handler(struct work_struct *work)
{
	struct hidpp_device *hidpp = container_of(work, struct hidpp_device, reset_hi_res_work);

	hi_res_scroll_enable(hidpp);
}

static DEVICE_ATTR(builtin_power_supply, 0000, NULL, NULL);

static struct attribute *sysfs_attrs[] = {
	&dev_attr_builtin_power_supply.attr,
	NULL
};

static const struct attribute_group ps_attribute_group = {
	.attrs = sysfs_attrs
};

static int hidpp_get_report_length(struct hid_device *hdev, int id)
{
	struct hid_report_enum *re;
	struct hid_report *report;

	re = &(hdev->report_enum[HID_OUTPUT_REPORT]);
	report = re->report_id_hash[id];
	if (!report)
		return 0;

	return report->field[0]->report_count + 1;
}

static u8 hidpp_validate_device(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	int id, report_length;
	u8 supported_reports = 0;

	id = REPORT_ID_HIDPP_SHORT;
	report_length = hidpp_get_report_length(hdev, id);
	if (report_length) {
		if (report_length < HIDPP_REPORT_SHORT_LENGTH)
			goto bad_device;

		supported_reports |= HIDPP_REPORT_SHORT_SUPPORTED;
	}

	id = REPORT_ID_HIDPP_LONG;
	report_length = hidpp_get_report_length(hdev, id);
	if (report_length) {
		if (report_length < HIDPP_REPORT_LONG_LENGTH)
			goto bad_device;

		supported_reports |= HIDPP_REPORT_LONG_SUPPORTED;
	}

	id = REPORT_ID_HIDPP_VERY_LONG;
	report_length = hidpp_get_report_length(hdev, id);
	if (report_length) {
		if (report_length < HIDPP_REPORT_LONG_LENGTH ||
		    report_length > HIDPP_REPORT_VERY_LONG_MAX_LENGTH)
			goto bad_device;

		supported_reports |= HIDPP_REPORT_VERY_LONG_SUPPORTED;
		hidpp->very_long_report_length = report_length;
	}

	return supported_reports;

bad_device:
	hid_warn(hdev, "not enough values in hidpp report %d\n", id);
	return false;
}

static bool hidpp_application_equals(struct hid_device *hdev,
				     unsigned int application)
{
	struct list_head *report_list;
	struct hid_report *report;

	report_list = &hdev->report_enum[HID_INPUT_REPORT].report_list;
	report = list_first_entry_or_null(report_list, struct hid_report, list);
	return report && report->application == application;
}

/*
 * Minimal probe path for RS50-family joystick interface 0 (non-HID++):
 * keep hidpp attached so hidpp_raw_event still runs, but skip all the
 * HID++ infrastructure (work queues, send_mutex, battery sysfs group,
 * two-phase hid_hw_start) that would otherwise run unused on a plain
 * HID joystick interface. Used for both RS50 and G Pro interface 0.
 */
static int hidpp_dd_minimal_probe(struct hid_device *hdev)
{
	int ret;

	/*
	 * Install the PID ll_driver override here, between hid_parse (which
	 * already ran via hidpp_probe) and hid_hw_start. hidpp_dd_pid_install
	 * no-ops when inject_pid is off or we're not on interface 0.
	 */
	ret = hidpp_dd_pid_install(hdev);
	if (ret)
		dd_warn(hdev, "minimal probe: pid install failed: %d\n",
			ret);

	ret = hid_hw_start(hdev, HID_CONNECT_DEFAULT);
	if (ret) {
		dd_err(hdev, "minimal probe: hid_hw_start failed: %d\n", ret);
		hidpp_dd_pid_uninstall(hdev);
	}
	return ret;
}

static int hidpp_probe(struct hid_device *hdev, const struct hid_device_id *id)
{
	struct hidpp_device *hidpp;
	int ret;
	unsigned int connect_mask = HID_CONNECT_DEFAULT | HID_CONNECT_DRIVER;

	/* report_fixup needs drvdata to be set before we call hid_parse */
	hidpp = devm_kzalloc(&hdev->dev, sizeof(*hidpp), GFP_KERNEL);
	if (!hidpp)
		return -ENOMEM;

	hidpp->hid_dev = hdev;
	hidpp->name = hdev->name;
	hidpp->quirks = id->driver_data;

	/*
	 * Both the real G PRO and RS50-in-G-PRO-compat-mode enumerate
	 * with the same VID/PID (C272 Xbox or C268 PS) and run the same
	 * direct-drive firmware architecture. Both get HIDPP_QUIRK_DD_FFB
	 * directly from the id-table so hidpp_dd_ff_init runs instead of
	 * hidpp_ff_init - avoiding the G920 HID++ 0x8123 FFB path's
	 * transport / queue limitations on direct-drive hardware.
	 */
	if (hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
	    hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL)
		dd_info(hdev, "using direct-drive FFB path\n");

	hid_set_drvdata(hdev, hidpp);

	/*
	 * Initialise early so cancel_work_sync in hidpp_remove is always
	 * safe. hidpp_dd_minimal_probe returns before the full HID++ path, so
	 * without this those work_structs would still be all-zero and
	 * WARN_ON_ONCE(!work->func) would fire in __flush_work on rmmod.
	 */
	INIT_WORK(&hidpp->work, hidpp_connect_event);
	INIT_WORK(&hidpp->reset_hi_res_work, hidpp_reset_hi_res_handler);
	INIT_DELAYED_WORK(&hidpp->ff_retry_work, hidpp_ff_retry_work);

	ret = hid_parse(hdev);
	if (ret) {
		hid_err(hdev, "%s:parse failed\n", __func__);
		return ret;
	}

	/*
	 * Make sure the device is HID++ capable, otherwise treat as generic HID.
	 */
	hidpp->supported_reports = hidpp_validate_device(hdev);

	if (!hidpp->supported_reports) {
		/*
		 * The direct-drive wheels have 3 interfaces:
		 *   0 = Joystick (wheel/pedals) - claim for pedal processing
		 *   1 = HID++ (configuration) - has HID++ support, handled below
		 *   2 = FFB output endpoint - let hid-generic handle
		 *
		 * We claim interface 0 to intercept raw_event and apply pedal
		 * deadzones, curves, and combined pedal mode. The joystick input
		 * still works normally via HID_CONNECT_DEFAULT.
		 */
		if (hdev->product == USB_DEVICE_ID_LOGITECH_RS50 && hid_is_usb(hdev)) {
			struct usb_interface *intf = to_usb_interface(hdev->dev.parent);
			int ifnum = intf->cur_altsetting->desc.bInterfaceNumber;

			if (ifnum == 0) {
				dd_info(hdev, "Claiming interface 0 for steering-position tracking\n");
				/*
				 * We need raw_event to track the steering axis
				 * but nothing HID++ below applies: take the
				 * minimal path that just registers the input
				 * device and keeps hidpp attached so raw_event
				 * reaches us. This interface carries no HID++
				 * reports and its report has no report ID.
				 */
				hidpp->no_hidpp_reports = true;
				return hidpp_dd_minimal_probe(hdev);
			}
			dd_info(hdev, "Letting hid-generic handle interface %d\n", ifnum);
		}
		if ((hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL ||
		     hdev->product == USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL) &&
		    hid_is_usb(hdev)) {
			struct usb_interface *intf = to_usb_interface(hdev->dev.parent);
			int ifnum = intf->cur_altsetting->desc.bInterfaceNumber;

			if (ifnum == 0) {
				dd_info(hdev, "Claiming interface 0 for input\n");
				hidpp->no_hidpp_reports = true;
				return hidpp_dd_minimal_probe(hdev);
			}
			dd_info(hdev, "Letting hid-generic handle interface %d\n", ifnum);
		}
		hid_set_drvdata(hdev, NULL);
		devm_kfree(&hdev->dev, hidpp);
		return hid_hw_start(hdev, HID_CONNECT_DEFAULT);
	}


	if (id->group == HID_GROUP_LOGITECH_27MHZ_DEVICE &&
	    hidpp_application_equals(hdev, HID_GD_MOUSE))
		hidpp->quirks |= HIDPP_QUIRK_HIDPP_WHEELS |
				 HIDPP_QUIRK_HIDPP_EXTRA_MOUSE_BTNS;

	if (id->group == HID_GROUP_LOGITECH_27MHZ_DEVICE &&
	    hidpp_application_equals(hdev, HID_GD_KEYBOARD))
		hidpp->quirks |= HIDPP_QUIRK_HIDPP_CONSUMER_VENDOR_KEYS;

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_WTP) {
		ret = wtp_allocate(hdev, id);
		if (ret)
			return ret;
	} else if (hidpp->quirks & HIDPP_QUIRK_CLASS_K400) {
		ret = k400_allocate(hdev);
		if (ret)
			return ret;
	}

	/* hidpp->work / reset_hi_res_work already initialised above. */
	mutex_init(&hidpp->send_mutex);
	init_waitqueue_head(&hidpp->wait);

	/* indicates we are handling the battery properties in the kernel */
	ret = sysfs_create_group(&hdev->dev.kobj, &ps_attribute_group);
	if (ret)
		hid_warn(hdev, "Cannot allocate sysfs group for %s\n",
			 hdev->name);

	/*
	 * First call hid_hw_start(hdev, 0) to allow IO without connecting any
	 * hid subdrivers (hid-input, hidraw). This allows retrieving the dev's
	 * name and serial number and store these in hdev->name and hdev->uniq,
	 * before the hid-input and hidraw drivers expose these to userspace.
	 */
	ret = hid_hw_start(hdev, 0);
	if (ret) {
		hid_err(hdev, "hw start failed\n");
		goto hid_hw_start_fail;
	}

	ret = hid_hw_open(hdev);
	if (ret < 0) {
		dev_err(&hdev->dev, "%s:hid_hw_open returned error:%d\n",
			__func__, ret);
		goto hid_hw_open_fail;
	}

	/* Allow incoming packets */
	hid_device_io_start(hdev);

	/* Get name + serial, store in hdev->name + hdev->uniq */
	/* Skip HID++ queries for RS50 interfaces without HID++ support */
	if (id->group == HID_GROUP_LOGITECH_DJ_DEVICE)
		hidpp_unifying_init(hidpp);
	else if (hidpp->supported_reports)
		hidpp_non_unifying_init(hidpp);
	else if (hdev->product == USB_DEVICE_ID_LOGITECH_RS50)
		dd_info(hdev, "Skipping HID++ init for non-HID++ interface\n");

	if (hidpp->quirks & HIDPP_QUIRK_DELAYED_INIT)
		connect_mask &= ~HID_CONNECT_HIDINPUT;

	/* Now export the actual inputs and hidraw nodes to the world */
	hid_device_io_stop(hdev);
	ret = hid_connect(hdev, connect_mask);
	if (ret) {
		hid_err(hdev, "%s:hid_connect returned error %d\n", __func__, ret);
		goto hid_hw_init_fail;
	}

	/* Check for connected devices now that incoming packets will not be disabled again */
	hid_device_io_start(hdev);
	schedule_work(&hidpp->work);
	flush_work(&hidpp->work);

	if (hidpp->quirks & HIDPP_QUIRK_CLASS_G920) {
		if (hidpp->quirks & HIDPP_QUIRK_DD_FFB) {
			/*
			 * Direct-drive wheels (RS50, G Pro) use dedicated endpoint FFB, not HID++ feature 0x8123.
			 * Skip G920 config and use the direct-drive initialization path.
			 * IMPORTANT: Only init FFB on interface with HID++ support
			 * (interface 1), not the joystick interface (interface 0).
			 */
			if (hidpp->supported_reports) {
				ret = hidpp_dd_ff_init(hidpp);
				if (ret)
					dd_warn(hidpp->hid_dev,
						 "Force feedback setup failed (error %d)\n", ret);
			} else {
				dd_info(hidpp->hid_dev,
					 "Skipping FFB init on non-HID++ interface\n");
			}
		} else if (hidpp->supported_reports) {
			/*
			 * G920/G923: single-interface, always has HID++ support.
			 * G Pro: multi-interface, only interface 1 has HID++.
			 * Skip FFB init on interfaces without HID++ support.
			 */
			struct hidpp_ff_private_data data;
			int cfg_ret;

			hid_info(hidpp->hid_dev,
				 "G920 FFB init: starting (quirks=0x%lx, reports=0x%02x)\n",
				 hidpp->quirks, hidpp->supported_reports);

			cfg_ret = g920_get_config(hidpp, &data);
			if (cfg_ret) {
				hid_warn(hidpp->hid_dev,
					 "g920_get_config failed: errno %d (FFB will not register)\n",
					 cfg_ret);
				ret = cfg_ret;
			} else {
				hid_info(hidpp->hid_dev,
					 "g920_get_config ok: num_effects=%d range=%u gain=0x%04x\n",
					 data.num_effects, data.range, data.gain);
				ret = hidpp_ff_init(hidpp, &data);
				if (ret == -ENODEV) {
					hid_info(hidpp->hid_dev,
						 "FF init: sibling inputs not ready yet, scheduling retry\n");
					queue_delayed_work(system_long_wq,
							   &hidpp->ff_retry_work,
							   msecs_to_jiffies(HIDPP_FF_INIT_RETRY_MS));
					ret = 0;
				} else if (ret) {
					hid_warn(hidpp->hid_dev,
						 "hidpp_ff_init failed: errno %d\n",
						 ret);
				}
			}
		}
	}

	/*
	 * This relies on logi_dj_ll_close() being a no-op so that DJ connection
	 * events will still be received.
	 *
	 * For RS50, keep the joystick interface (no HID++ support) open so we
	 * continue receiving raw_event callbacks for wheel position updates.
	 */
	if (!(hdev->product == USB_DEVICE_ID_LOGITECH_RS50 &&
	      !hidpp->supported_reports)) {
		hid_hw_close(hdev);
	} else {
		dd_info(hdev, "Keeping interface open for raw_event\n");
	}
	return ret;

hid_hw_init_fail:
	hid_hw_close(hdev);
hid_hw_open_fail:
	hid_hw_stop(hdev);
hid_hw_start_fail:
	sysfs_remove_group(&hdev->dev.kobj, &ps_attribute_group);
	cancel_work_sync(&hidpp->work);
	mutex_destroy(&hidpp->send_mutex);
	return ret;
}

static void hidpp_remove(struct hid_device *hdev)
{
	struct hidpp_device *hidpp = hid_get_drvdata(hdev);
	struct hidpp_dd_ff_data *ff;

	/*
	 * Restore the real ll_driver on interface 0 BEFORE hid_hw_stop so
	 * the core's teardown uses the real callbacks. No-op if PID inject
	 * was off or this isn't interface 0. pid_state is only populated
	 * on interface 0, so the check is self-guarding.
	 */
	if (hidpp)
		hidpp_dd_pid_uninstall(hdev);

	if (!hidpp) {
		/*
		 * Thin-probe fall-through: interfaces that don't support
		 * HID++ (RS50 interface 2, G Pro interfaces 1 and 2 in the
		 * non-HID++ enumeration) had drvdata cleared and were handed
		 * to the kernel default HID layers via hid_hw_start
		 * (HID_CONNECT_DEFAULT). Those hid_devices are still bound
		 * to our driver, so module unload runs hid_hw_stop on them
		 * below.
		 *
		 * The RS50 FFB path on interface 1 keeps a cached pointer to
		 * interface 2's hid_device in ff->ff_hdev. If interface 2's
		 * remove runs first during rmmod, we must invalidate that
		 * cache before stopping this hdev. Otherwise interface 1's
		 * later hidpp_dd_ff_destroy calls hid_hw_close on an hid_device
		 * whose ll_driver has already been cleared, producing a
		 * KASAN null-ptr-deref at hid_hw_close+0xe9.
		 *
		 * hidpp_dd_find_ff_data only matches HIDPP_DD_FFB quirk devices, so
		 * this is a no-op on G Pro which uses the G920 FFB path and
		 * doesn't hold an ff_hdev cache of this kind.
		 */
		ff = hidpp_dd_find_ff_data(hdev);
		if (ff && ff->ff_hdev == hdev) {
			WRITE_ONCE(ff->ff_hdev, NULL);
			ff->ff_hdev_open = false;
			/*
			 * Workers that cached this hdev may be mid-flight
			 * (the TF session init sends up to 136 packets over
			 * ~100+ ms; the keepalive fires every 20 s). They
			 * re-read ff_hdev per send, but that still races
			 * the hid_hw_stop below - and on physical unplug
			 * this hid_device is freed right after. Flush them
			 * while the device is still valid; the owner's
			 * hidpp_dd_ff_destroy cancels again later (no-op).
			 */
			cancel_work_sync(&ff->tf_init_work);
			cancel_delayed_work_sync(&ff->refresh_work);
		}
		return hid_hw_stop(hdev);
	}

	/*
	 * Direct-drive cleanup: Set stopping flag FIRST to prevent cross-interface
	 * lookups from accessing our data while we're tearing down.
	 * This must happen before hid_hw_stop() because sibling interfaces
	 * (like interface 0 receiving joystick input) may still be active
	 * and calling hidpp_dd_find_ff_data().
	 */
	if (hidpp->quirks & HIDPP_QUIRK_DD_FFB) {
		ff = READ_ONCE(hidpp->private_data);
		/*
		 * Only the OWNER takes the full-teardown path. Interface 0
		 * also carries a non-NULL private_data these days - the
		 * steering raw-event path caches the shared ff there
		 * (hidpp_dd_track_wheel_pos) - and letting it in here made its
		 * removal set the global stopping flag (killing FFB for a
		 * still-alive owner) and run an unbalanced hid_hw_close on
		 * itself (ff->hid_open tracks the OWNER's hid_hw_open from
		 * deferred init, not ours). Non-owners with a cached pointer
		 * just drop the cache and fall into the input-interface
		 * branch below.
		 */
		if (ff && ff->owner_hidpp != hidpp) {
			WRITE_ONCE(hidpp->private_data, NULL);
			ff = NULL;
		}
		if (ff) {
			/* Signal shutdown immediately */
			atomic_set_release(&ff->stopping, 1);

			if (ff->hid_open) {
				hid_hw_close(hdev);
				ff->hid_open = false;
			}
			/*
			 * CRITICAL: Clear input->ff->private BEFORE hid_hw_stop().
			 *
			 * hid_hw_stop() triggers hidinput_disconnect() which calls
			 * input_ff_destroy(). That function does kfree(ff->private).
			 * If we don't clear it first, input_ff_destroy frees our ff,
			 * then hidpp_dd_ff_destroy tries to use/free it again -> crash.
			 *
			 * We must check ff->input validity carefully since interface 0
			 * (which owns the input device) could theoretically be removed
			 * before interface 1 in some disconnect scenarios.
			 */
			if (ff->input && ff->input->ff) {
				ff->input->ff->private = NULL;
			}
		} else {
			/*
			 * Interface 0 case: this interface doesn't own ff_data
			 * (private_data is NULL), but it owns the input device.
			 *
			 * When hid_hw_stop() runs below it triggers
			 * input_ff_destroy(), which kfrees input->ff->private.
			 * If interface 1 has ALREADY been removed in this rmmod
			 * cycle, its hidpp_dd_ff_destroy kfreed ff_data first; our
			 * input->ff->private is then a dangling pointer and the
			 * kfree hits BUG at mm/slub.c:638 (observed in practice).
			 *
			 * Relying on hidpp_dd_find_ff_data() to find the sibling
			 * does NOT fix this - in the rmmod-ordered-this-way case
			 * the sibling is already detached and the lookup returns
			 * NULL before we get the chance to clear anything. So
			 * walk our own hdev->inputs list and unconditionally NULL
			 * out every ->ff->private we find. kfree(NULL) is a
			 * no-op, so clearing when the pointer would have been
			 * valid costs us nothing either.
			 */
			struct hid_input *hi;

			list_for_each_entry(hi, &hdev->inputs, list) {
				if (hi->input && hi->input->ff)
					hi->input->ff->private = NULL;
			}

			/*
			 * If the sibling is still alive, also invalidate its
			 * cached input_dev pointer so its late timer / work
			 * callbacks don't dereference the input_dev we're
			 * about to destroy.
			 */
			ff = hidpp_dd_find_ff_data(hdev);
			if (ff)
				WRITE_ONCE(ff->input, NULL);
		}
	}

	/*
	 * Stop hardware to prevent raw_event callbacks from accessing
	 * private_data while we're freeing it.
	 */
	hid_hw_stop(hdev);

	/* Now safe to clean up direct-drive force feedback - no more callbacks */
	if (hidpp->quirks & HIDPP_QUIRK_DD_FFB)
		hidpp_dd_ff_destroy(hidpp);

	sysfs_remove_group(&hdev->dev.kobj, &ps_attribute_group);

	cancel_work_sync(&hidpp->work);
	cancel_work_sync(&hidpp->reset_hi_res_work);
	cancel_delayed_work_sync(&hidpp->ff_retry_work);
	mutex_destroy(&hidpp->send_mutex);
}

#define LDJ_DEVICE(product) \
	HID_DEVICE(BUS_USB, HID_GROUP_LOGITECH_DJ_DEVICE, \
		   USB_VENDOR_ID_LOGITECH, (product))

#define L27MHZ_DEVICE(product) \
	HID_DEVICE(BUS_USB, HID_GROUP_LOGITECH_27MHZ_DEVICE, \
		   USB_VENDOR_ID_LOGITECH, (product))

static const struct hid_device_id hidpp_devices[] = {
	/*
	 * Wheel-scoped fork: this driver claims ONLY the Logitech direct-drive
	 * wheels - the RS50 and G PRO, across their three USB IDs (c276 RS50
	 * native, c272 G PRO Xbox/PC which the RS50 also uses in compatibility
	 * mode, c268 G PRO PS/PC) - whose TrueForce / direct-drive FFB support
	 * it adds. Every other Logitech HID++ device (mice, keyboards, receiver-
	 * paired and 27 MHz devices, and the belt-driven G920/G923 wheels) is
	 * deliberately left to the in-tree hid-logitech-hidpp driver, which is
	 * continuously maintained and supports far more hardware. Do not add
	 * non-direct-drive IDs here: that would shadow the in-tree driver with
	 * this fork's copy and regress those devices (see the project README's
	 * "wheel-scoped" note).
	 *
	 * Direct-drive base architecture: HID++ 4.2 on interface 1, a dedicated
	 * 64-byte FFB endpoint on interface 2, and the TrueForce packet layout.
	 * These use hidpp_dd_ff_* (HIDPP_QUIRK_DD_FFB), NOT the G920 HID++ FFB
	 * path, which inherits transport/queue limitations from the belt-driven
	 * generation and misbehaves on direct-drive hardware (issue #8).
	 */
	{ /* Logitech G Pro Racing Wheel (Xbox/PC) over USB */
	  HID_USB_DEVICE(USB_VENDOR_ID_LOGITECH, USB_DEVICE_ID_LOGITECH_G_PRO_WHEEL),
		.driver_data = HIDPP_QUIRK_CLASS_G920 | HIDPP_QUIRK_DD_FFB },
	{ /* Logitech G Pro Racing Wheel (PlayStation/PC) over USB */
	  HID_USB_DEVICE(USB_VENDOR_ID_LOGITECH, USB_DEVICE_ID_LOGITECH_G_PRO_PS_WHEEL),
		.driver_data = HIDPP_QUIRK_CLASS_G920 | HIDPP_QUIRK_DD_FFB },
	{ /* Logitech RS50 Direct Drive Wheel (PlayStation/PC) over USB */
	  HID_USB_DEVICE(USB_VENDOR_ID_LOGITECH, USB_DEVICE_ID_LOGITECH_RS50),
		.driver_data = HIDPP_QUIRK_CLASS_G920 | HIDPP_QUIRK_DD_FFB },
	{}
};

MODULE_DEVICE_TABLE(hid, hidpp_devices);

/*
 * hidpp_usages: selective event-hook table for the generic .event callback.
 *
 * hid-core calls driver->event() only for usages listed here. We opt in just
 * to REL_WHEEL_HI_RES so HID++ mice see high-resolution scroll. The sentinel
 * row uses HID_ANY_ID - 1 (not HID_ANY_ID) because HID_ANY_ID is a wildcard
 * that would match every event and undo the filter. Adding a new entry here
 * has historically regressed mouse behaviour; keep the surface minimal.
 */
static const struct hid_usage_id hidpp_usages[] = {
	{ HID_GD_WHEEL, EV_REL, REL_WHEEL_HI_RES },
	{ HID_ANY_ID - 1, HID_ANY_ID - 1, HID_ANY_ID - 1}
};

static struct hid_driver hidpp_driver = {
	/*
	 * Distinct from the in-tree driver's "logitech-hidpp-device": both
	 * modules register a hid_driver and may be loaded at once (this fork
	 * claims only the direct-drive wheels), so the names must not collide.
	 */
	.name = "logitech-dd",
	.id_table = hidpp_devices,
	.report_fixup = hidpp_report_fixup,
	.probe = hidpp_probe,
	.remove = hidpp_remove,
	.raw_event = hidpp_raw_event,
	.usage_table = hidpp_usages,
	.event = hidpp_event,
	.input_configured = hidpp_input_configured,
	.input_mapping = hidpp_input_mapping,
	.input_mapped = hidpp_input_mapped,
};

static int __init hidpp_module_init(void)
{
	pr_info("hid-logitech-hidpp: loaded (git=%s)\n", HIDPP_DD_GIT_HASH);
	return hid_register_driver(&hidpp_driver);
}

static void __exit hidpp_module_exit(void)
{
	hid_unregister_driver(&hidpp_driver);
}

module_init(hidpp_module_init);
module_exit(hidpp_module_exit);
