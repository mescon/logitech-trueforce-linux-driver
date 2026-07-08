// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * angle - print the wheel's angle and angular velocity every 100 ms
 * for N seconds (default 10). Turn the wheel during the run.
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#include <trueforce.h>

int main(int argc, char **argv)
{
	int secs = argc > 1 ? atoi(argv[1]) : 10;
	int index = argc > 2 ? atoi(argv[2]) : 0;

	if (dllOpen() != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed\n");
		return 1;
	}
	if (!logiTrueForceAvailable(index)) {
		fprintf(stderr, "no wheel at index %d\n", index);
		return 1;
	}

	/* First call starts the status thread. */
	(void)logiTrueForceGetAngleDegrees(index);

	printf("turn the wheel; sampling every 100 ms for %d s...\n", secs);
	for (int i = 0; i < secs * 10; i++) {
		double a  = logiTrueForceGetAngleDegrees(index);
		double v  = logiTrueForceGetAngularVelocityDegrees(index);

		printf("\rangle = %+8.2f deg   velocity = %+8.1f deg/s   ", a, v);
		fflush(stdout);
		usleep(100000);
	}
	putchar('\n');

	dllClose();
	return 0;
}
