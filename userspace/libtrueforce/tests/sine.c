// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * sine - stream a sine wave to the wheel via libtrueforce.
 *
 * Args: <freq_hz> <duration_s> [amplitude 0..1] [index]
 * Default: 50 Hz, 2 s, amp 0.3, index 0.
 *
 * HOLD THE WHEEL or clamp it down before running. The library's
 * streaming path emits 4 samples every 4 ms (1 kHz effective
 * sample rate) for the requested duration.
 */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

#include <trueforce.h>

/*
 * The rate the library really drains the ring at: LOGITF_TF_PKT_HZ packets
 * per second carrying LOGITF_TF_NEW samples each. This tool has to pace
 * itself against it, because the push calls do not block: the queue is
 * bounded by latency and sheds the oldest samples past it, so a producer
 * that pushed a whole waveform in a tight loop would have all but the last
 * fraction of it dropped before the wheel ever saw it.
 */
#define WHEEL_SAMPLE_RATE 4000.0

/* Sleep for `seconds` of playback, so the next push lands as the previous
 * batch finishes rather than on top of it. */
static void pace(double seconds)
{
	struct timespec ts = {
		.tv_sec  = (time_t)seconds,
		.tv_nsec = (long)((seconds - (double)(time_t)seconds) * 1e9),
	};

	nanosleep(&ts, NULL);
}

/* Small readers: the SDK reports through an out parameter and returns a
 * status, so a test that wants the value still has to check the status. */
static bool tf_available(void)
{
	bool a = false;

	return logiTrueForceAvailable(&a) == LOGITF_OK && a;
}

int main(int argc, char **argv)
{
	double freq = argc > 1 ? atof(argv[1]) : 50.0;
	double dur  = argc > 2 ? atof(argv[2]) : 2.0;
	double amp  = argc > 3 ? atof(argv[3]) : 0.3;
	int index   = argc > 4 ? atoi(argv[4]) : 0;
	const double sample_rate = 1000.0;
	int total = (int)(dur * sample_rate);
	const int batch = 64;  /* write 64 ms at a time */

	if (dllOpen() != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed\n");
		return 1;
	}
	if (!tf_available()) {
		fprintf(stderr, "no wheel at index %d\n", index);
		return 1;
	}

	fprintf(stderr, "streaming %.1f Hz sine for %.1f s at amp %.2f...\n",
		freq, dur, amp);

	float buf[batch];
	double phase = 0.0;
	double step = 2 * M_PI * freq / sample_rate;

	for (int i = 0; i < total; i += batch) {
		int n = (i + batch <= total) ? batch : (total - i);

		for (int j = 0; j < n; j++) {
			buf[j] = (float)(amp * sin(phase));
			phase += step;
		}
		int rc = logiTrueForceSetTorqueTFfloat(index, buf, n);

		if (rc != LOGITF_OK) {
			fprintf(stderr, "push failed at %d: %d\n", i, rc);
			break;
		}
		pace((double)n / WHEEL_SAMPLE_RATE);
	}

	/* Let the streaming thread drain the ring before we tear down. */
	usleep((unsigned)((double)total / sample_rate * 1e6) + 100000);

	dllClose();
	return 0;
}
