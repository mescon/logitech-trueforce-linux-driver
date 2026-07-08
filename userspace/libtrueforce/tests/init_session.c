// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * Init-session test: open, trigger init via a zero-sample TF setter,
 * read a few status responses from hidraw, close.
 *
 * Expected: returns quickly (init takes ~140 ms for 68 * 2 ms
 * inter-packet delay) and reports no errors. Running under usbmon
 * shows the 68 init packets streaming to ep 0x03 OUT.
 */

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <trueforce.h>
#include "internal.h"

static void dump_hex(const char *label, const uint8_t *buf, size_t n)
{
	printf("%s ", label);
	for (size_t i = 0; i < n; i++)
		printf("%02x%s", buf[i], (i == n - 1 || (i + 1) % 16 == 0) ? "\n" : " ");
	if (n && n % 16)
		putchar('\n');
}

int main(int argc, char **argv)
{
	int index = argc > 1 ? atoi(argv[1]) : 0;
	int rc;
	struct logitf_device *dev;
	float zero = 0.0f;

	if (dllOpen() != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed\n");
		return 1;
	}
	if (!logiTrueForceAvailable(index)) {
		fprintf(stderr, "no wheel at index %d\n", index);
		return 1;
	}

	printf("triggering init on index %d...\n", index);
	rc = logiTrueForceSetTorqueTFfloat(index, &zero, 1);
	if (rc != LOGITF_OK) {
		fprintf(stderr, "init failed: %d\n", rc);
		return 1;
	}
	printf("init completed\n");

	/* Read a few responses to confirm the session is alive. */
	rc = logitf_find_by_index(index, &dev);
	if (!rc && dev->hidraw_fd >= 0) {
		struct pollfd pfd = { .fd = dev->hidraw_fd, .events = POLLIN };
		uint8_t rx[64];

		for (int i = 0; i < 4; i++) {
			int pr = poll(&pfd, 1, 200);
			if (pr <= 0) {
				printf("  (no response within 200 ms)\n");
				break;
			}
			ssize_t n = read(dev->hidraw_fd, rx, sizeof(rx));
			if (n < 0) {
				perror("read");
				break;
			}
			printf("  rx[%d]: %zd bytes\n", i, n);
			dump_hex("    ", rx, (size_t)n);
		}
	}

	dllClose();
	return 0;
}
