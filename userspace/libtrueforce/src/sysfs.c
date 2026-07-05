/*
 * libtrueforce - sysfs attribute helpers.
 *
 * The kernel driver exposes wheel settings (wheel_range, wheel_damping,
 * wheel_trueforce, ...) on interface 1's HID device. We track interface
 * 2's hidraw, so forwarders scan /sys/class/hidraw for a sibling under
 * the same USB device root and read or write the requested attr there.
 * This lets the SDK entry points in exports.c delegate to kernel state
 * rather than maintain a parallel userspace machine.
 */

#include "internal.h"

#include <dirent.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/*
 * Find the hidraw sibling under dev->usb_root that exposes the given
 * attribute, and return its /sys/class/hidraw/<name>/device/<attr>
 * path in out_path. Returns 0 on success, -1 on failure.
 */
static int find_attr_path(struct logitf_device *dev, const char *attr,
			  char *out_path, size_t out_len)
{
	DIR *d;
	struct dirent *ent;
	char dev_link[288];	/* '/sys/class/hidraw/' + d_name(255) + '/device' */
	char resolved[PATH_MAX];
	int rc = -1;

	if (!dev || dev->usb_root[0] == '\0' || !attr || !out_path)
		return -1;

	d = opendir("/sys/class/hidraw");
	if (!d)
		return -1;

	while ((ent = readdir(d))) {
		if (strncmp(ent->d_name, "hidraw", 6) != 0)
			continue;

		snprintf(dev_link, sizeof(dev_link),
			 "/sys/class/hidraw/%s/device", ent->d_name);
		if (!realpath(dev_link, resolved))
			continue;
		if (strncmp(resolved, dev->usb_root, strlen(dev->usb_root)) != 0)
			continue;

		snprintf(out_path, out_len,
			 "/sys/class/hidraw/%s/device/%s",
			 ent->d_name, attr);
		if (access(out_path, F_OK) == 0) {
			rc = 0;
			break;
		}
	}
	closedir(d);
	return rc;
}

int logitf_sysfs_read_int(struct logitf_device *dev, const char *attr, int *out)
{
	char path[PATH_MAX];
	FILE *f;
	int v;

	if (!out)
		return -1;
	if (find_attr_path(dev, attr, path, sizeof(path)) < 0)
		return -1;
	f = fopen(path, "r");
	if (!f)
		return -1;
	if (fscanf(f, "%d", &v) != 1) {
		fclose(f);
		return -1;
	}
	fclose(f);
	*out = v;
	return 0;
}

int logitf_sysfs_write_int(struct logitf_device *dev, const char *attr, int val)
{
	char path[PATH_MAX];
	FILE *f;

	if (find_attr_path(dev, attr, path, sizeof(path)) < 0)
		return -1;
	f = fopen(path, "w");
	if (!f)
		return -1;
	if (fprintf(f, "%d", val) < 0) {
		fclose(f);
		return -1;
	}
	fclose(f);
	return 0;
}
