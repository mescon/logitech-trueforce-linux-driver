/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * FF effect matrix test harness.
 *
 * Uploads each standard Linux FFB effect type with a representative
 * set of parameter combinations, plays it for a short window, observes
 * the wheel's ABS_X evdev axis to verify the expected motion behaviour,
 * prints PASS/FAIL with measurements.
 *
 * Runs against a live RS50 (or any evdev device that advertises the
 * corresponding effect types). Not a unit test: measures the full
 * stack from EVIOCSFF through our driver's math into the motor, so
 * failures here usually indicate a real behaviour regression.
 *
 * Usage:
 *   gcc -O2 -Wall -o ff_matrix_test ff_matrix_test.c
 *   sudo ./ff_matrix_test /dev/input/eventN
 *
 * Safety: effects are short (<=500ms each) and separated by 1s
 * settle windows. Each test requires holding the wheel or leaving
 * it free depending on the phase; prompts are printed inline.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>

static const char *dev_path;
static int fd = -1;
static int abs_x_min, abs_x_max;
static int tests_pass, tests_fail;

static void die(const char *msg)
{
	perror(msg);
	exit(1);
}

static int read_abs_x(void)
{
	struct input_absinfo info;

	if (ioctl(fd, EVIOCGABS(ABS_X), &info) < 0)
		die("EVIOCGABS");
	return info.value;
}

static void drain_events(void)
{
	struct input_event ev;
	fd_set fds;
	struct timeval tv = { 0, 0 };

	for (;;) {
		FD_ZERO(&fds);
		FD_SET(fd, &fds);
		if (select(fd + 1, &fds, NULL, NULL, &tv) <= 0)
			return;
		if (read(fd, &ev, sizeof(ev)) != sizeof(ev))
			return;
	}
}

static void msleep(int ms)
{
	usleep((unsigned)ms * 1000);
}

/* Upload effect, play for `duration_ms`, stop, erase. Returns effect id. */
static int play_and_wait(struct ff_effect *eff, int duration_ms)
{
	struct input_event play = { 0 };

	eff->id = -1;
	if (ioctl(fd, EVIOCSFF, eff) < 0) {
		printf("    UPLOAD FAIL: %s\n", strerror(errno));
		return -1;
	}
	play.type = EV_FF;
	play.code = eff->id;
	play.value = 1;
	if (write(fd, &play, sizeof(play)) != sizeof(play)) {
		printf("    PLAY FAIL: %s\n", strerror(errno));
		ioctl(fd, EVIOCRMFF, (void *)(long)eff->id);
		return -1;
	}
	msleep(duration_ms);
	play.value = 0;
	write(fd, &play, sizeof(play));
	ioctl(fd, EVIOCRMFF, (void *)(long)eff->id);
	return 0;
}

static void start_effect(struct ff_effect *eff)
{
	struct input_event play = { 0 };

	eff->id = -1;
	if (ioctl(fd, EVIOCSFF, eff) < 0)
		die("EVIOCSFF");
	play.type = EV_FF;
	play.code = eff->id;
	play.value = 1;
	write(fd, &play, sizeof(play));
}

static void stop_effect(int id)
{
	struct input_event play = { .type = EV_FF, .code = id, .value = 0 };

	write(fd, &play, sizeof(play));
	ioctl(fd, EVIOCRMFF, (void *)(long)id);
}

static void set_gain(uint16_t g)
{
	struct input_event ev = { .type = EV_FF, .code = FF_GAIN, .value = g };

	write(fd, &ev, sizeof(ev));
}

/* Check that abs_x movement is in the expected direction with sufficient
 * magnitude, relative to the pre-effect position. */
static bool verify_motion(const char *label, int before, int after,
			  int expect_sign, int min_delta)
{
	int delta = after - before;

	if (expect_sign > 0 && delta > min_delta) {
		printf("  PASS %-40s  %+d (expected > %d)\n", label, delta, min_delta);
		tests_pass++;
		return true;
	}
	if (expect_sign < 0 && delta < -min_delta) {
		printf("  PASS %-40s  %+d (expected < %d)\n", label, delta, -min_delta);
		tests_pass++;
		return true;
	}
	if (expect_sign == 0 && delta > -min_delta && delta < min_delta) {
		printf("  PASS %-40s  %+d (expected ~0)\n", label, delta);
		tests_pass++;
		return true;
	}
	printf("  FAIL %-40s  %+d (expected sign %+d, mag > %d)\n",
	       label, delta, expect_sign, min_delta);
	tests_fail++;
	return false;
}

static void banner(const char *s)
{
	printf("\n=== %s ===\n", s);
}

static void test_constant_directions(void)
{
	banner("CONSTANT force direction (release wheel before starting)");
	printf("5 sec to release wheel...\n");
	msleep(5000);

	static const struct {
		const char *label;
		int16_t level;
		int sign;
	} cases[] = {
		{ "level=+0x4000 (right-pushing)",  0x4000,  +1 },
		{ "level=-0x4000 (left-pushing)",  -0x4000,  -1 },
		{ "level=+0x7fff (max right)",      0x7fff,  +1 },
		{ "level=-0x7fff (max left)",      -0x7fff,  -1 },
	};

	for (unsigned i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
		struct ff_effect eff = { 0 };
		int before, after;

		eff.type = FF_CONSTANT;
		eff.direction = 0x4000;
		eff.u.constant.level = cases[i].level;
		eff.replay.length = 300;

		drain_events();
		before = read_abs_x();
		start_effect(&eff);
		msleep(200);
		after = read_abs_x();
		stop_effect(eff.id);
		msleep(800);

		verify_motion(cases[i].label, before, after, cases[i].sign, 500);
	}
}

static void test_ramp(void)
{
	banner("RAMP force linearly interpolates start -> end");
	printf("5 sec to release wheel...\n");
	msleep(5000);

	struct ff_effect eff = { 0 };
	int before, mid, after;

	/* Ramp from 0 to +0x4000 over 400ms: at 100ms expect weak right,
	 * at 400ms expect stronger right. */
	eff.type = FF_RAMP;
	eff.direction = 0x4000;
	eff.u.ramp.start_level = 0;
	eff.u.ramp.end_level = 0x4000;
	eff.replay.length = 400;

	drain_events();
	before = read_abs_x();
	start_effect(&eff);
	msleep(100);
	mid = read_abs_x();
	msleep(300);
	after = read_abs_x();
	stop_effect(eff.id);
	msleep(800);

	int early_delta = mid - before;
	int late_delta = after - before;
	printf("  ramp: before=%d mid=%d after=%d early_delta=%+d late_delta=%+d\n",
	       before, mid, after, early_delta, late_delta);
	if (late_delta > early_delta && late_delta > 500) {
		printf("  PASS ramp up monotonic (late > early)\n");
		tests_pass++;
	} else {
		printf("  FAIL ramp up monotonic (expected late > early > 0)\n");
		tests_fail++;
	}
}

static void test_periodic_sine(void)
{
	banner("PERIODIC sine oscillates (release wheel)");
	printf("5 sec to release wheel...\n");
	msleep(5000);

	struct ff_effect eff = { 0 };

	eff.type = FF_PERIODIC;
	eff.direction = 0x4000;
	eff.u.periodic.waveform = FF_SINE;
	eff.u.periodic.period = 400;      /* 2.5 Hz slow sweep */
	eff.u.periodic.magnitude = 0x4000;
	eff.u.periodic.offset = 0;
	eff.u.periodic.phase = 0;
	eff.replay.length = 1200;

	drain_events();
	int start = read_abs_x();
	int min = start, max = start;
	start_effect(&eff);
	for (int t = 0; t < 12; t++) {
		msleep(100);
		int v = read_abs_x();
		if (v < min) min = v;
		if (v > max) max = v;
	}
	stop_effect(eff.id);
	msleep(800);

	int swing = max - min;
	printf("  sine: start=%d min=%d max=%d swing=%d\n",
	       start, min, max, swing);
	if (swing > 2000) {
		printf("  PASS sine oscillation >=2000 counts p-p\n");
		tests_pass++;
	} else {
		printf("  FAIL sine oscillation %d (expected >=2000)\n", swing);
		tests_fail++;
	}
}

static void test_envelope_attack(void)
{
	banner("CONSTANT with attack envelope ramps magnitude");
	printf("5 sec to release wheel...\n");
	msleep(5000);

	struct ff_effect eff = { 0 };
	int before, mid, after;

	eff.type = FF_CONSTANT;
	eff.direction = 0x4000;
	eff.u.constant.level = 0x4000;
	eff.u.constant.envelope.attack_length = 300;
	eff.u.constant.envelope.attack_level = 0;
	eff.replay.length = 500;

	drain_events();
	before = read_abs_x();
	start_effect(&eff);
	msleep(50);
	mid = read_abs_x();
	msleep(350);
	after = read_abs_x();
	stop_effect(eff.id);
	msleep(800);

	int early = mid - before;
	int late = after - before;
	printf("  attack: before=%d mid(50ms)=%d after(400ms)=%d early=%+d late=%+d\n",
	       before, mid, after, early, late);
	/* At 50 ms into a 300 ms attack from level=0, magnitude is ~1/6
	 * of 0x4000; late should be >3x early at least. */
	if (late > 500 && late > early * 2) {
		printf("  PASS attack envelope progresses\n");
		tests_pass++;
	} else {
		printf("  FAIL attack envelope (early=%+d late=%+d)\n", early, late);
		tests_fail++;
	}
}

static void test_spring_centering(void)
{
	banner("SPRING returns wheel to centre");
	printf("Turn the wheel ~45 degrees to one side, then release.\n");
	printf("Test starts in 8 sec...\n");
	msleep(8000);

	struct ff_effect eff = { 0 };

	eff.type = FF_SPRING;
	eff.direction = 0;
	eff.u.condition[0].right_coeff = 0x7fff;
	eff.u.condition[0].left_coeff = 0x7fff;
	eff.u.condition[0].right_saturation = 0x7fff;
	eff.u.condition[0].left_saturation = 0x7fff;
	eff.u.condition[0].center = 0;
	eff.u.condition[0].deadband = 0;
	eff.u.condition[1] = eff.u.condition[0];
	eff.replay.length = 2000;

	drain_events();
	int before = read_abs_x();
	start_effect(&eff);
	msleep(1500);
	int after = read_abs_x();
	stop_effect(eff.id);
	msleep(800);

	int pre_offset = before - 0x8000;
	int post_offset = after - 0x8000;
	printf("  spring: before=%d post=%d |pre_off|=%d |post_off|=%d\n",
	       before, after, abs(pre_offset), abs(post_offset));
	if (abs(post_offset) < abs(pre_offset) / 2 || abs(post_offset) < 2000) {
		printf("  PASS spring moved wheel toward centre\n");
		tests_pass++;
	} else {
		printf("  FAIL spring did not centre (offset shrunk from %d to %d)\n",
		       abs(pre_offset), abs(post_offset));
		tests_fail++;
	}
}

static void test_upload_only(const char *label, struct ff_effect *eff)
{
	if (play_and_wait(eff, 50) == 0) {
		printf("  PASS upload-only %s\n", label);
		tests_pass++;
	} else {
		printf("  FAIL upload-only %s\n", label);
		tests_fail++;
	}
}

static void test_upload_matrix(void)
{
	banner("Upload-only matrix (accepts without error)");

	struct ff_effect e;

	memset(&e, 0, sizeof(e)); e.type = FF_DAMPER;
	e.u.condition[0].right_coeff = 0x4000; e.u.condition[0].left_coeff = 0x4000;
	e.u.condition[0].right_saturation = 0x7fff; e.u.condition[0].left_saturation = 0x7fff;
	test_upload_only("DAMPER pos coeff", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_DAMPER;
	e.u.condition[0].right_coeff = -0x4000; e.u.condition[0].left_coeff = -0x4000;
	e.u.condition[0].right_saturation = 0x7fff; e.u.condition[0].left_saturation = 0x7fff;
	test_upload_only("DAMPER neg coeff (anti-damper)", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_FRICTION;
	e.u.condition[0].right_coeff = 0x2000; e.u.condition[0].left_coeff = 0x2000;
	test_upload_only("FRICTION", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_INERTIA;
	e.u.condition[0].right_coeff = 0x2000; e.u.condition[0].left_coeff = 0x2000;
	test_upload_only("INERTIA", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_SPRING;
	e.u.condition[0].center = 0x2000;
	e.u.condition[0].right_coeff = 0x4000; e.u.condition[0].left_coeff = 0x4000;
	test_upload_only("SPRING center offset", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_SPRING;
	e.u.condition[0].right_coeff = -0x4000; e.u.condition[0].left_coeff = -0x4000;
	test_upload_only("SPRING neg coeff (anti-spring)", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_CONSTANT;
	e.u.constant.level = 0x2000;
	e.replay.delay = 500;
	e.replay.length = 500;
	test_upload_only("CONSTANT with delay=500 length=500", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_CONSTANT;
	e.u.constant.level = 0x2000;
	e.u.constant.envelope.attack_level = 0x7fff;    /* attack FROM high DOWN */
	e.u.constant.envelope.attack_length = 200;
	e.replay.length = 500;
	test_upload_only("CONSTANT with inverted attack envelope", &e);

	int waves[] = { FF_SQUARE, FF_TRIANGLE, FF_SINE, FF_SAW_UP, FF_SAW_DOWN };
	const char *names[] = { "SQUARE", "TRIANGLE", "SINE", "SAW_UP", "SAW_DOWN" };
	for (int i = 0; i < 5; i++) {
		char lbl[40];

		memset(&e, 0, sizeof(e)); e.type = FF_PERIODIC;
		e.u.periodic.waveform = waves[i];
		e.u.periodic.period = 100;
		e.u.periodic.magnitude = 0x2000;
		snprintf(lbl, sizeof(lbl), "PERIODIC %s", names[i]);
		test_upload_only(lbl, &e);
	}

	memset(&e, 0, sizeof(e)); e.type = FF_RUMBLE;
	e.u.rumble.strong_magnitude = 0x8000;
	e.replay.length = 500;
	test_upload_only("RUMBLE strong only", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_RUMBLE;
	e.u.rumble.weak_magnitude = 0x8000;
	e.replay.length = 500;
	test_upload_only("RUMBLE weak only", &e);

	memset(&e, 0, sizeof(e)); e.type = FF_RAMP;
	e.u.ramp.start_level = 0x4000; e.u.ramp.end_level = -0x4000;
	e.replay.length = 300;
	test_upload_only("RAMP positive-to-negative", &e);
}

/*
 * Motion tests use the Linux evdev sign convention: direction=0x4000
 * east + positive level => rightward force (matches
 * Documentation/input/ff.rst and the ff-memless reference
 * implementation). The RS50 driver ships with a FF_CONSTANT sign
 * flip enabled by default (wheel_ffb_constant_sign=1) to compensate
 * for Wine's inverted path. If we run the motion tests with the
 * flip on, every CONSTANT direction assertion goes backwards.
 * Walk sysfs to find the toggle for this device and force it to 0
 * during the run; restore on exit. If the toggle isn't present
 * (different driver, older version, G Pro), just log and continue.
 */
static char toggle_path[256];
static int toggle_orig = -1;

static void toggle_find_path(void)
{
	struct input_id id = { 0 };
	FILE *f;
	int i;

	if (ioctl(fd, EVIOCGID, &id) < 0)
		return;

	/*
	 * Walk /sys/class/hidraw/hidrawN/device/wheel_ffb_constant_sign
	 * looking for one whose parent HID device matches our VID:PID.
	 * hidrawN numbering isn't stable, but the attribute only exists
	 * on our driver's instances.
	 */
	for (i = 0; i < 64; i++) {
		char path[200];
		int pfd;

		snprintf(path, sizeof(path),
			 "/sys/class/hidraw/hidraw%d/device/wheel_ffb_constant_sign",
			 i);
		pfd = open(path, O_RDONLY);
		if (pfd < 0)
			continue;
		/* Matched by attribute existence. A system with multiple
		 * RS50 / G Pro hidraws would need a VID:PID check here;
		 * we keep it simple for now. */
		close(pfd);
		strncpy(toggle_path, path, sizeof(toggle_path) - 1);
		f = fopen(toggle_path, "r");
		if (f) {
			if (fscanf(f, "%d", &toggle_orig) != 1)
				toggle_orig = -1;
			fclose(f);
		}
		return;
	}
}

static void toggle_write(int v)
{
	FILE *f;

	if (toggle_path[0] == 0)
		return;
	f = fopen(toggle_path, "w");
	if (!f)
		return;
	fprintf(f, "%d\n", v);
	fclose(f);
}

int main(int argc, char **argv)
{
	struct input_absinfo info;
	struct input_event ge;

	setvbuf(stdout, NULL, _IOLBF, 0);   /* flush per line for piped output */

	if (argc < 2) {
		fprintf(stderr, "usage: %s /dev/input/eventN\n", argv[0]);
		return 1;
	}
	dev_path = argv[1];
	fd = open(dev_path, O_RDWR);
	if (fd < 0)
		die(dev_path);

	if (ioctl(fd, EVIOCGABS(ABS_X), &info) < 0)
		die("EVIOCGABS");
	abs_x_min = info.minimum;
	abs_x_max = info.maximum;
	printf("Device: %s   ABS_X range: %d..%d   initial=%d\n",
	       dev_path, abs_x_min, abs_x_max, info.value);

	toggle_find_path();
	if (toggle_path[0]) {
		printf("FF_CONSTANT sign toggle: %s (was %d)\n",
		       toggle_path, toggle_orig);
		toggle_write(0);
		printf("  -> set to 0 (native convention) for motion tests\n");
	} else {
		printf("FF_CONSTANT sign toggle not found; assuming native convention\n");
	}

	memset(&ge, 0, sizeof(ge));
	ge.type = EV_FF;
	ge.code = FF_GAIN;
	ge.value = 0xffff;
	write(fd, &ge, sizeof(ge));

	test_upload_matrix();
	test_constant_directions();
	test_ramp();
	test_periodic_sine();
	test_envelope_attack();
	test_spring_centering();

	if (toggle_orig >= 0) {
		toggle_write(toggle_orig);
		printf("FF_CONSTANT sign toggle restored to %d\n", toggle_orig);
	}

	printf("\n=== Summary ===\n%d pass  %d fail\n", tests_pass, tests_fail);
	close(fd);
	return tests_fail ? 1 : 0;
}
