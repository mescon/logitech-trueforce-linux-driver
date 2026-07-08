// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * kf - apply a constant KF torque for a few seconds, then release.
 *
 * Args: <torque_nm> <duration_s> [index]
 * Default: 0.5 Nm, 2 s, index 0.
 *
 * HOLD THE WHEEL before running. Force appears after a 10 s
 * countdown. Goes to zero and releases at the end.
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#include <trueforce.h>

int main(int argc, char **argv)
{
	double nm  = argc > 1 ? atof(argv[1]) : 0.5;
	double dur = argc > 2 ? atof(argv[2]) : 2.0;
	int index  = argc > 3 ? atoi(argv[3]) : 0;

	if (dllOpen() != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed\n");
		return 1;
	}
	if (!logiTrueForceAvailable(index)) {
		fprintf(stderr, "no wheel at index %d\n", index);
		return 1;
	}

	fprintf(stderr, "applying %.2f Nm for %.1f s...\n", nm, dur);
	int rc = logiTrueForceSetTorqueKF(index, nm);

	if (rc != LOGITF_OK) {
		fprintf(stderr, "SetTorqueKF failed: %d\n", rc);
		return 1;
	}
	usleep((unsigned)(dur * 1e6));
	logiTrueForceClearKF(index);
	fprintf(stderr, "released\n");

	dllClose();
	return 0;
}
