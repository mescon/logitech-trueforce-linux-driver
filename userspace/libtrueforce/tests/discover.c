/*
 * Discovery test: enumerate detected direct-drive wheels and print what
 * libtrueforce sees. No device writes; read-only sysfs/hidraw-path
 * inspection via the library's public discovery entry points.
 */

#include <stdio.h>
#include <trueforce.h>
#include "internal.h"

int main(void)
{
	int rc = dllOpen();
	int major, minor, build;
	int found = 0;

	if (rc != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed: %d\n", rc);
		return 1;
	}

	logiWheelGetCoreLibraryVersion(&major, &minor, &build);
	printf("libtrueforce %d.%d.%d\n", major, minor, build);

	for (int i = 0; i < LOGITF_MAX_CONTROLLERS; i++) {
		extern struct logitf_device *logitf_table(void);
		struct logitf_device *t;

		if (!logiTrueForceAvailable(i))
			continue;
		t = logitf_table() + i;
		printf("  [%d] supported=%s, paused=%s\n",
		       i,
		       logiTrueForceSupported(i) ? "yes" : "no",
		       logiTrueForceIsPaused(i) ? "yes" : "no");
		printf("      hidraw: %s\n", t->hidraw_path[0] ? t->hidraw_path : "(none)");
		printf("      evdev:  %s\n", t->evdev_path[0] ? t->evdev_path : "(none)");
		printf("      by-id:  %s\n", t->by_id[0] ? t->by_id : "(none)");
		found++;
	}

	if (!found)
		printf("  (no wheels detected)\n");

	dllClose();
	return 0;
}
