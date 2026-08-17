// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * Unit tests for libtrueforce's pure-logic helpers.
 *
 * Covers the wire-format conversions used by the streaming path,
 * which are free of hidraw/evdev I/O and therefore safe to run in
 * CI without a wheel attached. Exits 0 on success, 1 on the first
 * assertion failure (printing the test name so the failing case is
 * obvious in a CI log).
 */

#include <errno.h>
#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include <string.h>
#include <unistd.h>

#include <trueforce.h>
#include "internal.h"
#include "tf_init_data.h"

#define EXPECT_EQ(label, got, want)					\
	do {								\
		uint32_t _g = (uint32_t)(got);				\
		uint32_t _w = (uint32_t)(want);				\
		if (_g != _w) {						\
			fprintf(stderr, "FAIL %s: got 0x%04x want 0x%04x\n", \
				label, _g, _w);				\
			return 1;					\
		}							\
	} while (0)

#define EXPECT_NEAR(label, got, want, tol)				\
	do {								\
		int32_t _g = (int32_t)(got);				\
		int32_t _w = (int32_t)(want);				\
		int32_t _d = _g - _w;					\
		if (_d < -(tol) || _d > (tol)) {			\
			fprintf(stderr, "FAIL %s: got 0x%04x want ~0x%04x (tol %d)\n", \
				label, (unsigned)_g, (unsigned)_w,	\
				(int)(tol));				\
			return 1;					\
		}							\
	} while (0)

static int test_s16_to_wire(void)
{
	/*
	 * logitf_s16_to_wire shifts signed int16 range [-32768..32767]
	 * to offset-binary [0..65535] with 0x8000 as the neutral
	 * centre. Match what G Hub writes on the wire.
	 */
	EXPECT_EQ("s16:zero",      logitf_s16_to_wire(0),       0x8000);
	EXPECT_EQ("s16:max_pos",   logitf_s16_to_wire(32767),   0xFFFF);
	EXPECT_EQ("s16:max_neg",   logitf_s16_to_wire(-32768),  0x0000);
	EXPECT_EQ("s16:one",       logitf_s16_to_wire(1),       0x8001);
	EXPECT_EQ("s16:neg_one",   logitf_s16_to_wire(-1),      0x7FFF);
	EXPECT_EQ("s16:half_pos",  logitf_s16_to_wire(16384),   0xC000);
	EXPECT_EQ("s16:half_neg",  logitf_s16_to_wire(-16384),  0x4000);
	return 0;
}

/*
 * The captured init sequence pushes an operating range, and it must stay
 * findable: session.c rewrites it at send time so the replay stops
 * overwriting the user's configured range. If the table is ever
 * regenerated from a new capture, this fails rather than silently
 * reintroducing a range push nobody is patching.
 */
static int test_init_carries_exactly_one_range_push(void)
{
	unsigned found = 0;
	size_t i;

	for (i = 0; i < TF_INIT_PACKET_COUNT; i++) {
		uint32_t bits;
		float deg;

		if (tf_init_packets[i][4] != 0x0e)
			continue;
		found++;
		bits = (uint32_t)tf_init_packets[i][6]
		     | ((uint32_t)tf_init_packets[i][7] << 8)
		     | ((uint32_t)tf_init_packets[i][8] << 16)
		     | ((uint32_t)tf_init_packets[i][9] << 24);
		memcpy(&deg, &bits, sizeof(deg));
		/* 2700 degrees: what the recorded wheel was set to, and the
		 * value that was resetting everyone else's range. */
		EXPECT_NEAR("init:range_degrees", (int)deg, 2700, 0);
	}
	EXPECT_EQ("init:range_packet_count", found, 1);
	return 0;
}

static int test_float_to_wire(void)
{
	/*
	 * logitf_float_to_wire clamps to [-1.0, +1.0] and scales by
	 * 32767 before adding the 0x8000 centre offset. Because the
	 * conversion truncates via (int) cast rather than rounding,
	 * boundary outputs are exact but interior points can be off
	 * by up to 1 LSB from a naive scaling, hence the tol=1 near
	 * checks for fractional inputs.
	 */
	EXPECT_EQ("f:zero",        logitf_float_to_wire(0.0f),   0x8000);
	EXPECT_EQ("f:plus_one",    logitf_float_to_wire(1.0f),   0xFFFF);
	EXPECT_EQ("f:minus_one",   logitf_float_to_wire(-1.0f),  0x0001);
	EXPECT_EQ("f:over_plus",   logitf_float_to_wire(2.5f),   0xFFFF);
	EXPECT_EQ("f:over_minus",  logitf_float_to_wire(-3.0f),  0x0001);
	EXPECT_NEAR("f:half_pos",  logitf_float_to_wire(0.5f),   0xC000, 1);
	EXPECT_NEAR("f:half_neg",  logitf_float_to_wire(-0.5f),  0x4000, 1);
	EXPECT_NEAR("f:quarter",   logitf_float_to_wire(0.25f),  0xA000, 1);
	return 0;
}

/*
 * The driving-packet shape: type 0x01, cur duplicated at 6-9, byte 10 =
 * 4 new samples, byte 11 = 0x0d, 13 window slots each duplicated L/R.
 */
static int test_stream_packet_shape(void)
{
	uint16_t window[LOGITF_TF_WINDOW];
	uint8_t pkt[64];
	int i;

	for (i = 0; i < LOGITF_TF_WINDOW; i++)
		window[i] = (uint16_t)(0x8000 + i);
	logitf_build_stream_packet(pkt, 0x42, 0x8123, window, LOGITF_TF_NEW);

	EXPECT_EQ("stream:id",      pkt[0],  0x01);
	EXPECT_EQ("stream:type",    pkt[4],  0x01);
	EXPECT_EQ("stream:seq",     pkt[5],  0x42);
	EXPECT_EQ("stream:cur_lo",  pkt[6],  0x23);
	EXPECT_EQ("stream:cur_hi",  pkt[7],  0x81);
	EXPECT_EQ("stream:cur_lo2", pkt[8],  0x23);
	EXPECT_EQ("stream:cur_hi2", pkt[9],  0x81);
	EXPECT_EQ("stream:new",     pkt[10], 0x04);
	EXPECT_EQ("stream:flag",    pkt[11], 0x0d);
	for (i = 0; i < LOGITF_TF_WINDOW; i++) {
		const uint8_t *p = pkt + 12 + i * 4;

		EXPECT_EQ("stream:slot_lo",  p[0], (0x8000 + i) & 0xff);
		EXPECT_EQ("stream:slot_hi",  p[1], (0x8000 + i) >> 8);
		EXPECT_EQ("stream:slot_lo2", p[2], (0x8000 + i) & 0xff);
		EXPECT_EQ("stream:slot_hi2", p[3], (0x8000 + i) >> 8);
	}

	/*
	 * Byte 10 is a count, not the constant 4: a partial drain says how
	 * many slots really are new, and a catch-up packet says up to
	 * LOGITF_TF_CATCHUP_MAX (F3 - claiming 4 fresh samples when three
	 * were repeats told the wheel to play audio nobody generated).
	 */
	logitf_build_stream_packet(pkt, 0x42, 0x8123, window, 2);
	EXPECT_EQ("stream:new_partial", pkt[10], 0x02);
	logitf_build_stream_packet(pkt, 0x42, 0x8123, window, LOGITF_TF_CATCHUP_MAX);
	EXPECT_EQ("stream:new_catchup", pkt[10], LOGITF_TF_CATCHUP_MAX);
	return 0;
}

/*
 * The idle keepalive must be the Windows menu shape (whine-investigation.md
 * section 2): byte 10 = 0x00 (zero new samples), byte 11 = 0x00, and bytes
 * 12..63 literal zeros - never byte10=4 with a repeated sample, and never
 * 0x8000 centre values in the tail.
 */
static int test_idle_packet_shape(void)
{
	uint8_t pkt[64];
	int i;

	memset(pkt, 0xAA, sizeof(pkt));	/* prove every byte is written */
	logitf_build_idle_packet(pkt, 0x3b, 0x8000);

	EXPECT_EQ("idle:id",      pkt[0],  0x01);
	EXPECT_EQ("idle:pad1",    pkt[1],  0x00);
	EXPECT_EQ("idle:pad2",    pkt[2],  0x00);
	EXPECT_EQ("idle:pad3",    pkt[3],  0x00);
	EXPECT_EQ("idle:type",    pkt[4],  0x01);
	EXPECT_EQ("idle:seq",     pkt[5],  0x3b);
	EXPECT_EQ("idle:cur_lo",  pkt[6],  0x00);
	EXPECT_EQ("idle:cur_hi",  pkt[7],  0x80);
	EXPECT_EQ("idle:cur_lo2", pkt[8],  0x00);
	EXPECT_EQ("idle:cur_hi2", pkt[9],  0x80);
	EXPECT_EQ("idle:new",     pkt[10], 0x00);
	EXPECT_EQ("idle:flag",    pkt[11], 0x00);
	for (i = 12; i < 64; i++)
		EXPECT_EQ("idle:zero_tail", pkt[i], 0x00);

	/* A held non-zero force rides in cur; the tail stays zero. */
	logitf_build_idle_packet(pkt, 0x3c, 0x8123);
	EXPECT_EQ("idle:held_lo", pkt[6],  0x23);
	EXPECT_EQ("idle:held_hi", pkt[7],  0x81);
	for (i = 12; i < 64; i++)
		EXPECT_EQ("idle:held_zero_tail", pkt[i], 0x00);
	return 0;
}

/*
 * The control packets must be byte-identical to init packets 67 (0x04)
 * and 68 (0x03), the same pair Windows sends as session teardown, modulo
 * the sequence byte the sender rewrites.
 */
static int test_ctrl_packets_match_captured_pair(void)
{
	uint8_t pkt[64];
	uint8_t want[64];

	logitf_build_ctrl_packet(pkt, 0x04, 0x00);
	memcpy(want, tf_init_packets[TF_INIT_PACKET_COUNT - 2], 64);
	want[TF_INIT_SEQ_OFFSET] = 0x00;
	if (memcmp(pkt, want, 64) != 0) {
		fprintf(stderr, "FAIL ctrl:stop differs from init packet 67\n");
		return 1;
	}

	logitf_build_ctrl_packet(pkt, 0x03, 0x00);
	memcpy(want, tf_init_packets[TF_INIT_PACKET_COUNT - 1], 64);
	want[TF_INIT_SEQ_OFFSET] = 0x00;
	if (memcmp(pkt, want, 64) != 0) {
		fprintf(stderr, "FAIL ctrl:arm differs from init packet 68\n");
		return 1;
	}
	return 0;
}

/*
 * The teardown pair on the wire: exactly one 0x04 then one 0x03 with
 * consecutive sequence bytes, matching the captured teardown (0x04 seq
 * 0x3c at 126.7215 s, 0x03 seq 0x3d at 126.7234 s). A pipe stands in
 * for the hidraw fd.
 */
static int test_stop_pair_ordering(void)
{
	struct logitf_device dev;
	uint8_t buf[128];
	size_t got = 0;
	int fds[2];
	int rc;

	if (pipe(fds) != 0) {
		fprintf(stderr, "FAIL pair: pipe() failed\n");
		return 1;
	}
	memset(&dev, 0, sizeof(dev));
	pthread_mutex_init(&dev.lock, NULL);
	dev.hidraw_fd = fds[1];
	dev.tf_seq = 0x3c;
	dev.tf_initialized = true;

	pthread_mutex_lock(&dev.lock);
	rc = logitf_tf_send_stop_pair(&dev);
	pthread_mutex_unlock(&dev.lock);
	EXPECT_EQ("pair:rc", rc, LOGITF_OK);
	EXPECT_EQ("pair:armed_idle", dev.tf_armed_idle, 1);
	EXPECT_EQ("pair:seq_advanced", dev.tf_seq, 0x3e);

	while (got < sizeof(buf)) {
		ssize_t n = read(fds[0], buf + got, sizeof(buf) - got);

		if (n <= 0) {
			fprintf(stderr, "FAIL pair: short read (%zu bytes)\n", got);
			return 1;
		}
		got += (size_t)n;
	}
	EXPECT_EQ("pair:first_type",  buf[4],      0x04);
	EXPECT_EQ("pair:first_seq",   buf[5],      0x3c);
	EXPECT_EQ("pair:second_type", buf[64 + 4], 0x03);
	EXPECT_EQ("pair:second_seq",  buf[64 + 5], 0x3d);

	close(fds[0]);
	close(fds[1]);
	pthread_mutex_destroy(&dev.lock);
	return 0;
}

/*
 * F1: a starved stream must not hold a stale non-zero force forever.
 * Drive logitf_stream_tick with an empty ring from a clearly non-zero
 * held force; it must reach centre (0x8000) within
 * LOGITF_TF_STARVE_DECAY_TICKS ticks, and once there the silence gate
 * must go on to fire the teardown pair - proving starvation both
 * bounds the held force and lets the gate do its job (whine-
 * investigation.md F1: the old code held cur forever, which pinned
 * the gate shut since it requires cur==0x8000 exactly).
 *
 * The bound includes LOGITF_TF_STARVE_HOLD_TICKS: decay does not start
 * until starvation has persisted that long (F3), so the worst case is the
 * hold plus the decay and not the decay alone.
 */
static int test_starvation_decays_and_gate_fires(void)
{
	struct logitf_device dev;
	int fds[2];
	unsigned tick;
	unsigned decay_tick = 0;
	bool reached_centre = false;
	unsigned decay_bound = (unsigned)LOGITF_TF_STARVE_HOLD_TICKS +
			       (unsigned)LOGITF_TF_STARVE_DECAY_TICKS;
	unsigned max_ticks = decay_bound +
			     (unsigned)LOGITF_TF_IDLE_GRACE_TICKS + 5;

	if (pipe(fds) != 0) {
		fprintf(stderr, "FAIL decay: pipe() failed\n");
		return 1;
	}
	memset(&dev, 0, sizeof(dev));
	pthread_mutex_init(&dev.lock, NULL);
	pthread_cond_init(&dev.tf_teardown_done, NULL);
	pthread_mutex_init(&dev.ring_lock, NULL);
	pthread_cond_init(&dev.ring_space, NULL);
	pthread_cond_init(&dev.ring_data, NULL);
	dev.hidraw_fd = fds[1];
	dev.tf_initialized = true;
	dev.tf_seq = 1;
	dev.tf_last_current = (uint16_t)(0x8000 + 16000); /* a stale non-zero force */
	for (int i = 0; i < LOGITF_TF_WINDOW; i++)
		dev.tf_window[i] = dev.tf_last_current;

	for (tick = 1; tick <= max_ticks; tick++) {
		logitf_stream_tick(&dev);
		if (!reached_centre && dev.tf_last_current == 0x8000) {
			reached_centre = true;
			decay_tick = tick;
		}
		if (dev.tf_armed_idle)
			break;
	}

	if (!reached_centre) {
		fprintf(stderr, "FAIL decay: cur never reached 0x8000\n");
		return 1;
	}
	if (decay_tick > decay_bound) {
		fprintf(stderr, "FAIL decay: took %u ticks, bound is %u\n",
			decay_tick, decay_bound);
		return 1;
	}
	EXPECT_EQ("decay:gate_fired", dev.tf_armed_idle, 1);

	close(fds[0]);
	close(fds[1]);
	pthread_mutex_destroy(&dev.lock);
	pthread_cond_destroy(&dev.tf_teardown_done);
	pthread_mutex_destroy(&dev.ring_lock);
	pthread_cond_destroy(&dev.ring_space);
	pthread_cond_destroy(&dev.ring_data);
	return 0;
}

/*
 * F5: a drained block whose samples are all exactly centre (offset-
 * binary 0x8000, i.e. logitf_s16_to_wire(0)) is actively pushed
 * silence, not an empty ring - but it must still produce the idle
 * keepalive shape (byte10=0, byte11=0, zeroed tail) and count toward
 * the silence gate exactly like true starvation, so a producer that
 * explicitly pushes "nothing to play" (logi-tf-sim's pre-gate menu
 * output) matches the Windows wire and both idle gates agree.
 */
static int test_all_zero_block_is_starved_shape(void)
{
	struct logitf_device dev;
	int fds[2];
	int16_t zeros[LOGITF_TF_NEW] = { 0, 0, 0, 0 };
	uint8_t pkt[64];
	ssize_t n;
	int i;

	if (pipe(fds) != 0) {
		fprintf(stderr, "FAIL allzero: pipe() failed\n");
		return 1;
	}
	memset(&dev, 0, sizeof(dev));
	pthread_mutex_init(&dev.lock, NULL);
	pthread_cond_init(&dev.tf_teardown_done, NULL);
	pthread_mutex_init(&dev.ring_lock, NULL);
	pthread_cond_init(&dev.ring_space, NULL);
	pthread_cond_init(&dev.ring_data, NULL);
	dev.hidraw_fd = fds[1];
	dev.tf_initialized = true;
	dev.tf_seq = 1;
	dev.stream_running = true;	/* logitf_stream_push_s16 requires this */
	for (i = 0; i < LOGITF_TF_WINDOW; i++)
		dev.tf_window[i] = 0x8000;
	dev.tf_last_current = 0x8000;

	/* logitf_s16_to_wire(0) == 0x8000: an s16 zero block on the wire. */
	if (logitf_stream_push_s16(&dev, zeros, LOGITF_TF_NEW) != LOGITF_OK) {
		fprintf(stderr, "FAIL allzero: push failed\n");
		return 1;
	}

	logitf_stream_tick(&dev);

	n = read(fds[0], pkt, sizeof(pkt));
	if (n != (ssize_t)sizeof(pkt)) {
		fprintf(stderr, "FAIL allzero: short read (%zd bytes)\n", n);
		return 1;
	}
	/* Idle shape (logitf_build_idle_packet), not the driving shape
	 * (byte10=4, byte11=0x0d) a non-silent block would get. */
	EXPECT_EQ("allzero:new",  pkt[10], 0x00);
	EXPECT_EQ("allzero:flag", pkt[11], 0x00);
	for (i = 12; i < 64; i++)
		EXPECT_EQ("allzero:zero_tail", pkt[i], 0x00);
	/* Gate counting: this tick must count toward the silence gate the
	 * same as an empty-ring starved tick would. */
	EXPECT_EQ("allzero:idle_ticks_counted", dev.tf_idle_ticks, 1);

	close(fds[0]);
	close(fds[1]);
	pthread_mutex_destroy(&dev.lock);
	pthread_cond_destroy(&dev.tf_teardown_done);
	pthread_mutex_destroy(&dev.ring_lock);
	pthread_cond_destroy(&dev.ring_space);
	pthread_cond_destroy(&dev.ring_data);
	return 0;
}

/*
 * Minimal stand-in for a discovered device: a pipe for the hidraw fd, the
 * locks the streaming path takes, and a centred window. Enough to drive
 * logitf_stream_tick_n and logitf_stream_push_s16 with no wheel attached.
 */
static int fake_dev_init(struct logitf_device *dev, int fds[2])
{
	int i;

	if (pipe(fds) != 0)
		return -1;
	memset(dev, 0, sizeof(*dev));
	pthread_mutex_init(&dev->lock, NULL);
	pthread_cond_init(&dev->tf_teardown_done, NULL);
	pthread_mutex_init(&dev->ring_lock, NULL);
	pthread_cond_init(&dev->ring_space, NULL);
	pthread_cond_init(&dev->ring_data, NULL);
	dev->hidraw_fd = fds[1];
	dev->tf_initialized = true;
	dev->tf_seq = 1;
	dev->stream_running = true;	/* logitf_stream_push_s16 requires it */
	for (i = 0; i < LOGITF_TF_WINDOW; i++)
		dev->tf_window[i] = 0x8000;
	dev->tf_last_current = 0x8000;
	return 0;
}

static void fake_dev_destroy(struct logitf_device *dev, int fds[2])
{
	close(fds[0]);
	close(fds[1]);
	pthread_mutex_destroy(&dev->lock);
	pthread_cond_destroy(&dev->tf_teardown_done);
	pthread_mutex_destroy(&dev->ring_lock);
	pthread_cond_destroy(&dev->ring_space);
	pthread_cond_destroy(&dev->ring_data);
}

/* One window slot as it appears on the wire (u16 LE, duplicated). */
static uint16_t pkt_slot(const uint8_t *pkt, int slot)
{
	const uint8_t *p = pkt + 12 + slot * 4;

	return (uint16_t)(p[0] | (p[1] << 8));
}

/*
 * F3: a tick that overran its slot makes the timerfd coalesce, and the
 * expirations it reports are sample time that has already passed. Servicing
 * only one of them is what put the wire 9% behind the producer (3635
 * samples/sec measured of the 4000 asked for), so a tick handed 2
 * expirations must take 2 slots' worth of samples in ONE packet, declare
 * them all as new, and leave the ring empty.
 */
static int test_catchup_makes_up_coalesced_expiries(void)
{
	struct logitf_device dev;
	int fds[2];
	int16_t samples[2 * LOGITF_TF_NEW];
	uint8_t pkt[64];
	ssize_t n;
	int i;

	if (fake_dev_init(&dev, fds) != 0) {
		fprintf(stderr, "FAIL catchup: pipe() failed\n");
		return 1;
	}
	for (i = 0; i < (int)(sizeof(samples) / sizeof(samples[0])); i++)
		samples[i] = (int16_t)(1000 + i);
	if (logitf_stream_push_s16(&dev, samples, (int)(sizeof(samples) / sizeof(samples[0]))) != LOGITF_OK) {
		fprintf(stderr, "FAIL catchup: push failed\n");
		return 1;
	}

	logitf_stream_tick_n(&dev, 2);

	n = read(fds[0], pkt, sizeof(pkt));
	if (n != (ssize_t)sizeof(pkt)) {
		fprintf(stderr, "FAIL catchup: short read (%zd bytes)\n", n);
		return 1;
	}
	EXPECT_EQ("catchup:new", pkt[10], 2 * LOGITF_TF_NEW);
	EXPECT_EQ("catchup:flag", pkt[11], 0x0d);
	for (i = 0; i < 2 * LOGITF_TF_NEW; i++) {
		int slot = LOGITF_TF_WINDOW - 2 * LOGITF_TF_NEW + i;

		EXPECT_EQ("catchup:slot", pkt_slot(pkt, slot),
			  logitf_s16_to_wire(samples[i]));
	}
	EXPECT_EQ("catchup:cur", dev.tf_last_current,
		  logitf_s16_to_wire(samples[2 * LOGITF_TF_NEW - 1]));
	/* Nothing left behind: the sample clock is caught up, not merely
	 * shifted one tick later. */
	EXPECT_EQ("catchup:ring_drained", dev.ring_head - dev.ring_tail, 0);

	/* An enormous coalesced count still emits one packet and takes no
	 * more than the window can express. */
	if (logitf_stream_push_s16(&dev, samples, (int)(sizeof(samples) / sizeof(samples[0]))) != LOGITF_OK) {
		fprintf(stderr, "FAIL catchup: second push failed\n");
		return 1;
	}
	logitf_stream_tick_n(&dev, 1000);
	n = read(fds[0], pkt, sizeof(pkt));
	if (n != (ssize_t)sizeof(pkt)) {
		fprintf(stderr, "FAIL catchup: short read after cap (%zd bytes)\n", n);
		return 1;
	}
	EXPECT_EQ("catchup:capped", pkt[10], 2 * LOGITF_TF_NEW);

	fake_dev_destroy(&dev, fds);
	return 0;
}

/*
 * F3: a partial drain must declare the count it really got. It used to
 * hardcode 4 and pad the window with repeats of the last sample, so the
 * wheel was told to play up to three samples nobody generated, and three
 * slots of real history were pushed off the front of the window for them.
 */
static int test_partial_drain_reports_its_real_count(void)
{
	const uint16_t marker = 0xABCD;
	struct logitf_device dev;
	int16_t samples[2] = { 100, 200 };
	int fds[2];
	uint8_t pkt[64];
	ssize_t n;
	int i;

	if (fake_dev_init(&dev, fds) != 0) {
		fprintf(stderr, "FAIL partial: pipe() failed\n");
		return 1;
	}
	for (i = 0; i < LOGITF_TF_WINDOW; i++)
		dev.tf_window[i] = marker;
	if (logitf_stream_push_s16(&dev, samples, 2) != LOGITF_OK) {
		fprintf(stderr, "FAIL partial: push failed\n");
		return 1;
	}

	logitf_stream_tick(&dev);

	n = read(fds[0], pkt, sizeof(pkt));
	if (n != (ssize_t)sizeof(pkt)) {
		fprintf(stderr, "FAIL partial: short read (%zd bytes)\n", n);
		return 1;
	}
	EXPECT_EQ("partial:new", pkt[10], 2);
	EXPECT_EQ("partial:newest", pkt_slot(pkt, LOGITF_TF_WINDOW - 1),
		  logitf_s16_to_wire(samples[1]));
	EXPECT_EQ("partial:second_newest", pkt_slot(pkt, LOGITF_TF_WINDOW - 2),
		  logitf_s16_to_wire(samples[0]));
	/* The window shifted by 2, not by 4: the slot behind them is still
	 * history rather than a repeat. */
	EXPECT_EQ("partial:history_kept", pkt_slot(pkt, LOGITF_TF_WINDOW - 3), marker);

	fake_dev_destroy(&dev, fds);
	return 0;
}

/*
 * F3: one missed batch is a gap the producer fills a millisecond later, so
 * a starved tick HOLDS - window and cur untouched - and only sustained
 * starvation (LOGITF_TF_STARVE_HOLD_TICKS consecutive) flushes toward
 * centre. Zeroing four slots on the first one put a discontinuity into the
 * middle of otherwise continuous audio.
 */
static int test_single_starved_tick_holds(void)
{
	const uint16_t held = (uint16_t)(0x8000 + 16000);
	struct logitf_device dev;
	int fds[2];
	unsigned tick;
	int i;

	if (fake_dev_init(&dev, fds) != 0) {
		fprintf(stderr, "FAIL hold: pipe() failed\n");
		return 1;
	}
	for (i = 0; i < LOGITF_TF_WINDOW; i++)
		dev.tf_window[i] = held;
	dev.tf_last_current = held;

	for (tick = 1; tick <= (unsigned)LOGITF_TF_STARVE_HOLD_TICKS; tick++) {
		logitf_stream_tick(&dev);
		EXPECT_EQ("hold:cur_held", dev.tf_last_current, held);
		for (i = 0; i < LOGITF_TF_WINDOW; i++)
			EXPECT_EQ("hold:window_held", dev.tf_window[i], held);
	}

	/* One tick past the hold: the flush and the decay both start. */
	logitf_stream_tick(&dev);
	EXPECT_EQ("hold:flushed", dev.tf_window[LOGITF_TF_WINDOW - 1], 0x8000);
	EXPECT_EQ("hold:history_kept", dev.tf_window[0], held);
	if (dev.tf_last_current >= held) {
		fprintf(stderr, "FAIL hold: cur did not decay past the hold\n");
		return 1;
	}

	fake_dev_destroy(&dev, fds);
	return 0;
}

/*
 * F4: the backlog is bounded by latency, not by the ring's capacity, and
 * push never blocks. Blocking on a full 4096-sample ring turned the
 * measured ~9% rate deficit into a permanent second of delay between the
 * car and the rim, with the producer's only thread parked inside push.
 * Past the bound the OLDEST samples go, and they are counted.
 */
static int test_push_bounds_the_backlog_by_latency(void)
{
	enum { EXCESS = 100 };
	static int16_t samples[LOGITF_TF_MAX_PENDING + EXCESS];
	struct logitf_device dev;
	int fds[2];
	unsigned occupied;
	int i;

	if (fake_dev_init(&dev, fds) != 0) {
		fprintf(stderr, "FAIL backlog: pipe() failed\n");
		return 1;
	}
	for (i = 0; i < (int)(sizeof(samples) / sizeof(samples[0])); i++)
		samples[i] = (int16_t)(i & 0x7fff);

	/* One oversized push: it returns immediately rather than waiting for
	 * a consumer that is not running in this test at all. */
	EXPECT_EQ("backlog:rc",
		  logitf_stream_push_s16(&dev, samples,
					 (int)(sizeof(samples) / sizeof(samples[0]))),
		  LOGITF_OK);
	occupied = dev.ring_head - dev.ring_tail;
	EXPECT_EQ("backlog:bounded", occupied, LOGITF_TF_MAX_PENDING);
	EXPECT_EQ("backlog:counted", (uint32_t)dev.ring_dropped, EXCESS);
	/* The samples kept are the NEWEST ones. */
	EXPECT_EQ("backlog:newest_kept",
		  dev.ring[(dev.ring_head - 1) & (LOGITF_TF_RING - 1)],
		  logitf_s16_to_wire(samples[sizeof(samples) / sizeof(samples[0]) - 1]));

	/* A second push over an already-full backlog sheds the same way. */
	EXPECT_EQ("backlog:rc2", logitf_stream_push_s16(&dev, samples, LOGITF_TF_NEW),
		  LOGITF_OK);
	EXPECT_EQ("backlog:bounded2", dev.ring_head - dev.ring_tail,
		  LOGITF_TF_MAX_PENDING);
	EXPECT_EQ("backlog:counted2", (uint32_t)dev.ring_dropped,
		  EXCESS + LOGITF_TF_NEW);

	/* And the bound really is a latency, not a sample count that happens
	 * to be right at one particular stream rate. */
	EXPECT_EQ("backlog:latency_ms",
		  (LOGITF_TF_MAX_PENDING * 1000) / (LOGITF_TF_PKT_HZ * LOGITF_TF_NEW),
		  LOGITF_TF_MAX_PENDING_MS);

	fake_dev_destroy(&dev, fds);
	return 0;
}

/*
 * F3: a stream thread that stopped on its own (the wheel was unplugged)
 * must be visible to the caller. Its exit reason used to go nowhere, so
 * every push kept returning success into a dead stream.
 */
static int test_push_reports_a_dead_stream_thread(void)
{
	struct logitf_device dev;
	int16_t sample = 1234;
	int fds[2];

	if (fake_dev_init(&dev, fds) != 0) {
		fprintf(stderr, "FAIL dead: pipe() failed\n");
		return 1;
	}
	EXPECT_EQ("dead:healthy", logitf_stream_push_s16(&dev, &sample, 1), LOGITF_OK);
	dev.stream_error = -ENODEV;
	EXPECT_EQ("dead:reported", logitf_stream_push_s16(&dev, &sample, 1),
		  LOGITF_ERR_IO);

	fake_dev_destroy(&dev, fds);
	return 0;
}

/*
 * A cached /dev/hidrawN path is a node number, not an identity, and the
 * numbers are recycled: the session must be told the wheel is gone rather
 * than open whatever inherited the number and send it the TF init sequence.
 *
 * The device here is deliberately impossible (vendor and product 0xffff, a
 * node number nothing owns), so the answer is the same whatever hardware the
 * machine running the tests happens to have attached.
 */
static int test_a_recycled_hidraw_node_is_not_accepted(void)
{
	struct logitf_device dev;

	memset(&dev, 0, sizeof(dev));
	dev.vid = 0xffff;
	dev.pid = 0xffff;
	snprintf(dev.hidraw_path, sizeof(dev.hidraw_path), "/dev/hidraw999");
	EXPECT_EQ("reresolve:gone", logitf_reresolve_hidraw(&dev) == 0, 0);
	return 0;
}

static int test_wire_monotonic(void)
{
	/*
	 * The streaming code relies on s16_to_wire being strictly
	 * monotonic: increasing input must produce non-decreasing
	 * output. If this ever breaks we'd get audible TF jitter at
	 * zero crossings.
	 */
	uint16_t prev = 0;
	int32_t s;

	for (s = -32768; s <= 32767; s++) {
		uint16_t w = logitf_s16_to_wire((int16_t)s);

		if (s != -32768 && w < prev) {
			fprintf(stderr,
				"FAIL s16 monotonic: at s=%d got 0x%04x < prev 0x%04x\n",
				(int)s, w, prev);
			return 1;
		}
		prev = w;
	}
	return 0;
}

int main(void)
{
	struct {
		const char *name;
		int (*fn)(void);
	} tests[] = {
		{ "s16_to_wire",      test_s16_to_wire },
		{ "float_to_wire",    test_float_to_wire },
		{ "wire_monotonic",   test_wire_monotonic },
		{ "init_range_push",  test_init_carries_exactly_one_range_push },
		{ "stream_packet_shape", test_stream_packet_shape },
		{ "idle_packet_shape",   test_idle_packet_shape },
		{ "ctrl_packets_match_captured_pair", test_ctrl_packets_match_captured_pair },
		{ "stop_pair_ordering",  test_stop_pair_ordering },
		{ "starvation_decays_and_gate_fires", test_starvation_decays_and_gate_fires },
		{ "all_zero_block_is_starved_shape", test_all_zero_block_is_starved_shape },
		{ "catchup_makes_up_coalesced_expiries", test_catchup_makes_up_coalesced_expiries },
		{ "partial_drain_reports_its_real_count", test_partial_drain_reports_its_real_count },
		{ "single_starved_tick_holds", test_single_starved_tick_holds },
		{ "push_bounds_the_backlog_by_latency", test_push_bounds_the_backlog_by_latency },
		{ "push_reports_a_dead_stream_thread", test_push_reports_a_dead_stream_thread },
		{ "recycled_hidraw_node_is_not_accepted", test_a_recycled_hidraw_node_is_not_accepted },
	};
	size_t i;

	for (i = 0; i < sizeof(tests) / sizeof(tests[0]); i++) {
		int rc = tests[i].fn();

		if (rc) {
			fprintf(stderr, "test %s failed\n", tests[i].name);
			return 1;
		}
		printf("ok %s\n", tests[i].name);
	}
	printf("1..%zu\n", sizeof(tests) / sizeof(tests[0]));
	return 0;
}
