/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef DD_LG4FF_H
#define DD_LG4FF_H
struct hid_device;
struct hid_report;
int dd_lg4ff_init(struct hid_device *hdev);
void dd_lg4ff_deinit(struct hid_device *hdev);

/*
 * Rewrites combined-pedal bytes into an interface-0 input report per the
 * combine_pedals sysfs setting. Safe to call with entry == NULL (before
 * dd_lg4ff_init has run) or combine == 0; always returns 0. See the
 * definition in dd-lg4ff.c for the full port note.
 */
/*
 * Whether combine_pedals is on, so the pedal-polarity correction can leave
 * a merged axis alone. Safe before dd_lg4ff_init has run (returns false).
 */
bool dd_lg4ff_pedals_combined(struct hid_device *hdev);

int dd_lg4ff_raw_event(struct hid_device *hdev, struct hid_report *report, u8 *data, int size);

/*
 * Returns the address of this hdev's struct hidpp_device::lg4ff_entry slot
 * (i.e. a struct dd_lg4ff_device_entry **, opaque here), or NULL if the
 * device has no hidpp_device drvdata yet. Defined in hid-logitech-hidpp.c,
 * the only file that knows struct hidpp_device's layout; this is the sole
 * point where dd-lg4ff.c reaches into it.
 */
void *hidpp_dd_lg4ff_slot(struct hid_device *hdev);
#endif
