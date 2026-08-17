// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * Log the wheel's undecoded motor field while driving it, then while idle.
 *
 * The type-0x02 response carries a u16 at bytes 6-7 that TRUEFORCE_PROTOCOL.md
 * records as "motor current or temperature?". Which of those it is decides
 * whether this project has any way at all to answer "how hard is it safe to
 * drive this motor", so it is worth settling.
 *
 * The two hypotheses separate cleanly in time:
 *
 *   current      tracks commanded force within a sample or two, and falls
 *                back to its floor as soon as the force stops
 *   temperature  climbs over seconds of load and decays over tens of
 *                seconds, lagging the force badly in both directions
 *
 * So: drive at a known amplitude, then push exact silence while continuing
 * to sample. Silence rather than teardown, because feedback only arrives
 * while a stream is running, and stopping the stream would take the
 * measurement away exactly when the interesting part starts.
 *
 * Args: <freq_hz> <amp> <drive_s> <idle_s> [index]
 * Default: 50 Hz, amp 0.3, 4 s driven, 12 s idle.
 *
 * Reads only. The force it produces is the force asked for, and nothing
 * here writes a setting.
 */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

#include <trueforce.h>

static bool tf_available(void)
{
	bool a = false;

	return logiTrueForceAvailable(&a) == LOGITF_OK && a;
}

static double now_s(void)
{
	struct timespec ts;

	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/*
 * The rate the library really drains the ring at: LOGITF_TF_PKT_HZ packets
 * per second carrying LOGITF_TF_NEW samples each. The loop below has to
 * pace itself against it, because the push calls do not block: the queue is
 * bounded by latency and sheds the oldest samples past it, so a tight push
 * loop would throw most of its own waveform away and log feedback for audio
 * the wheel never received.
 */
#define WHEEL_SAMPLE_RATE 4000.0

static void pace(double seconds)
{
	struct timespec ts = {
		.tv_sec  = (time_t)seconds,
		.tv_nsec = (long)((seconds - (double)(time_t)seconds) * 1e9),
	};

	nanosleep(&ts, NULL);
}

int main(int argc, char **argv)
{
	double freq  = argc > 1 ? atof(argv[1]) : 50.0;
	double amp   = argc > 2 ? atof(argv[2]) : 0.3;
	double drive = argc > 3 ? atof(argv[3]) : 4.0;
	double idle  = argc > 4 ? atof(argv[4]) : 12.0;
	int index    = argc > 5 ? atoi(argv[5]) : 0;
	const double sample_rate = 1000.0;
	const int batch = 64;
	float buf[64];
	double phase = 0.0;
	double step = 2 * M_PI * freq / sample_rate;
	double t0;
	struct logitf_stream_feedback fb;

	if (dllOpen() != LOGITF_OK) {
		fprintf(stderr, "dllOpen failed\n");
		return 1;
	}
	if (!tf_available()) {
		fprintf(stderr, "no wheel at index %d\n", index);
		return 1;
	}

	fprintf(stderr,
		"%.0f Hz at amp %.2f for %.1f s, then silence for %.1f s\n",
		freq, amp, drive, idle);
	printf("# t_s\tphase\tmotor_raw\tstatus\twheel_pos\tpackets\n");

	t0 = now_s();
	for (;;) {
		double t = now_s() - t0;
		int driving = t < drive;

		if (t >= drive + idle)
			break;

		for (int j = 0; j < batch; j++) {
			buf[j] = driving ? (float)(amp * sin(phase)) : 0.0f;
			phase += step;
		}
		if (logiTrueForceSetTorqueTFfloat(index, buf, batch) != LOGITF_OK)
			break;

		if (logitf_get_stream_feedback(index, &fb) == LOGITF_OK)
			printf("%.2f\t%s\t%u\t%u\t%u\t%llu\n",
			       t, driving ? "drive" : "idle",
			       fb.motor_raw, fb.status, fb.wheel_position,
			       (unsigned long long)fb.packets);
		fflush(stdout);
		pace((double)batch / WHEEL_SAMPLE_RATE);
	}

	dllClose();
	return 0;
}
