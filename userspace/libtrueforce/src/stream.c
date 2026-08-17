// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - Trueforce sample streaming.
 *
 * Userspace pushes sample batches via logitf_stream_push_s16(); the
 * streaming thread drains them at a fixed cadence (LOGITF_TF_PKT_HZ
 * packets per second, 4 new samples per packet) and emits 64-byte
 * type-0x01 reports to hidraw.
 *
 * Packet layout (observed from the issue #5 BeamNG capture):
 *
 *     0       byte 0x01                  (HID report ID)
 *     1..3    zeros                      (padding)
 *     4       byte 0x01                  (packet type = sample)
 *     5       seq                        (u8 counter, post-init)
 *     6..9    u16 LE (duplicated)        (most-recent sample preamble)
 *     10      byte 0x04                  (new-samples-this-packet)
 *     11      byte 0x0d                  (constant)
 *     12..63  13 slots of u16 LE duplicated
 *                                        (rolling window, oldest first)
 *
 * Each newly pushed sample appears in the window's last position,
 * shifts earlier samples left, and appears as the preamble on the
 * next packet as well. We reproduce this exactly so the wheel
 * firmware sees byte-for-byte the same stream as G HUB.
 *
 * If userspace can't keep up, the thread emits the silent keepalive
 * Windows sends in game menus (byte 10 = 0 "zero new samples",
 * byte 11 = 0, zeroed tail, cur = the current commanded force): the
 * wheel is told there is nothing to play instead of being handed the
 * last sample again as fresh audio (whine-investigation.md, H2). A
 * drained block whose samples are all exactly centre (0x8000, i.e.
 * actively pushed silence rather than an empty ring) gets the same
 * keepalive treatment, so a producer that pushes "nothing to play"
 * explicitly matches the Windows wire too. During real starvation the
 * held force also steps toward centre each tick (LOGITF_TF_STARVE_DECAY_STEP)
 * rather than freezing, so a producer that died mid-waveform cannot
 * command a stale force forever. After LOGITF_TF_IDLE_GRACE_TICKS of
 * centre force, the thread sends the captured session-teardown pair
 * (0x04 stop/clear, then 0x03 arm ~2 ms later) and goes fully silent,
 * exactly the way every clean Windows session ends; the next pushed
 * sample resumes the stream without re-init, the engine having stayed
 * armed. If userspace overruns the transport, push drops the OLDEST
 * queued samples to hold the backlog to LOGITF_TF_MAX_PENDING_MS of
 * audio; it never blocks (see logitf_stream_push_s16).
 *
 * Coexistence with the kernel driver on interface 2: our in-tree
 * hid-logitech-dd fork also writes to interface 2's ep 0x03 OUT
 * for classic PID FFB (wheel's HID-report id 0x11, short packets).
 * Our TF packets use HID-report id 0x01 with the 64-byte layout
 * below. The wheel firmware demultiplexes by report id, so the two
 * paths can run concurrently. Verified empirically by playing a
 * sine on TF while holding a KF constant torque; both produced the
 * expected tactile output with no dropped packets.
 */

#include <stdlib.h>

#include "internal.h"
#include "tf_init_data.h"

#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/eventfd.h>
#include <sys/timerfd.h>
#include <time.h>
#include <unistd.h>

/* ---------- format conversions ---------- */

uint16_t logitf_float_to_wire(float sample)
{
	float clamped = sample;

	if (clamped >  1.0f) clamped =  1.0f;
	if (clamped < -1.0f) clamped = -1.0f;
	return (uint16_t)((int)(clamped * 32767.0f) + 0x8000);
}

uint16_t logitf_s16_to_wire(int16_t sample)
{
	return (uint16_t)((int32_t)sample + 0x8000);
}

/* ---------- ring buffer (single-producer, single-consumer) ---------- */

static unsigned ring_occupied(const struct logitf_device *dev)
{
	return (dev->ring_head - dev->ring_tail) & (LOGITF_TF_RING - 1);
}

/* Monotonic seconds, for rate-limiting the load-shedding report. */
static long monotonic_sec(void)
{
	struct timespec ts;

	if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
		return 0;
	return (long)ts.tv_sec;
}

/* Minimum spacing between "dropping stale samples" reports, so a sustained
 * overrun says so occasionally instead of once per push. Matches the G923
 * transport's DROP_WARN_INTERVAL. */
#define LOGITF_TF_DROP_WARN_SEC  5

/*
 * Note the loss of `n` samples and say so, at most every
 * LOGITF_TF_DROP_WARN_SEC. Caller holds ring_lock.
 */
static void note_dropped(struct logitf_device *dev, unsigned n)
{
	long now = monotonic_sec();
	bool first = dev->ring_dropped == 0;

	dev->ring_dropped += n;
	if (first || now - dev->ring_drop_warn_sec >= LOGITF_TF_DROP_WARN_SEC) {
		dev->ring_drop_warn_sec = now;
		fprintf(stderr,
			"libtrueforce: dropped %llu stale samples (the producer "
			"is outrunning the wheel; backlog held to %d ms)\n",
			(unsigned long long)dev->ring_dropped,
			LOGITF_TF_MAX_PENDING_MS);
	}
}

/*
 * Push `count` samples to the ring. Returns LOGITF_OK, or a negative
 * error code if the stream is not running (which includes a stream thread
 * that stopped on its own, e.g. the wheel was unplugged).
 *
 * Never blocks. The backlog is bounded by LATENCY (LOGITF_TF_MAX_PENDING,
 * see internal.h) rather than by the ring's capacity, and the OLDEST
 * samples go when it is exceeded. Blocking on a full ring is what this
 * used to do, and against a wire running a few percent behind the producer
 * it did not shed anything: it filled all 4096 slots and stayed there, so
 * steady state became a full second of delay between the car and the rim,
 * with the caller's only thread parked in here for most of every
 * iteration. A haptic stream cannot buy that back later, so freshness wins
 * over completeness: nobody can feel a dropped millisecond, and everybody
 * can feel a second of delay. The G923 transport already made exactly this
 * trade (logi-tf-sim's g923.rs, push_pending).
 *
 * This is a deliberate departure from the Windows SDK's synchronous
 * "SetTorque*" semantics, which the blocking version was copying. The
 * caller there is a game's own haptic thread pacing itself against the
 * wheel; here it is just as often a single poll loop that also serves
 * telemetry, the rev display and a second wheel, and parking it does more
 * damage than the samples are worth.
 */
int logitf_stream_push_s16(struct logitf_device *dev,
			   const int16_t *samples, int count)
{
	unsigned occupied;

	if (!samples || count < 0)
		return LOGITF_ERR_INVALID_ARG;
	if (count == 0)
		return LOGITF_OK;

	/*
	 * Taken before ring_lock and released again, never nested inside it:
	 * the one place that holds both is logitf_stream_start, which takes
	 * `lock` then ring_lock, so acquiring them the other way round here
	 * would be the second half of a lock cycle.
	 */
	pthread_mutex_lock(&dev->lock);
	if (dev->stream_error != 0) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	pthread_mutex_unlock(&dev->lock);

	pthread_mutex_lock(&dev->ring_lock);
	if (dev->shutting_down || !dev->stream_running) {
		pthread_mutex_unlock(&dev->ring_lock);
		return LOGITF_ERR_IO;
	}
	/*
	 * A single push longer than the whole bound: keep its newest tail and
	 * drop the rest here, so the copy below can never lap the consumer.
	 */
	if (count > LOGITF_TF_MAX_PENDING) {
		unsigned over = (unsigned)count - LOGITF_TF_MAX_PENDING;

		samples += over;
		count = LOGITF_TF_MAX_PENDING;
		note_dropped(dev, over);
	}
	for (int i = 0; i < count; i++) {
		dev->ring[dev->ring_head & (LOGITF_TF_RING - 1)] =
			logitf_s16_to_wire(samples[i]);
		dev->ring_head++;
	}
	occupied = ring_occupied(dev);
	if (occupied > LOGITF_TF_MAX_PENDING) {
		unsigned over = occupied - LOGITF_TF_MAX_PENDING;

		dev->ring_tail += over;
		note_dropped(dev, over);
	}
	pthread_mutex_unlock(&dev->ring_lock);
	return LOGITF_OK;
}

int logitf_stream_clear(struct logitf_device *dev)
{
	pthread_mutex_lock(&dev->ring_lock);
	dev->ring_tail = dev->ring_head;
	pthread_cond_broadcast(&dev->ring_space);
	pthread_mutex_unlock(&dev->ring_lock);

	/*
	 * Also re-centre the rolling window so outgoing packets stop
	 * commanding force toward the old position after a clear.
	 */
	pthread_mutex_lock(&dev->lock);
	for (int i = 0; i < LOGITF_TF_WINDOW; i++)
		dev->tf_window[i] = 0x8000;
	dev->tf_last_current = 0x8000;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

/* ---------- packet emission ---------- */

/*
 * The three wire shapes below are non-static so tests/unit.c can pin
 * them byte-for-byte against the captures without a wheel attached.
 */

void logitf_build_stream_packet(uint8_t *pkt, uint8_t seq,
				uint16_t current,
				const uint16_t window[LOGITF_TF_WINDOW],
				uint8_t new_count)
{
	memset(pkt, 0, 64);
	pkt[0] = 0x01;           /* HID report ID */
	pkt[4] = 0x01;           /* type: sample */
	pkt[5] = seq;
	/* bytes 6-9: current sample duplicated */
	pkt[6] = current & 0xff;
	pkt[7] = current >> 8;
	pkt[8] = current & 0xff;
	pkt[9] = current >> 8;
	/*
	 * How many of the window's newest slots are new this packet. The
	 * captures only ever show 4 because G HUB only ever had 4 to send;
	 * the field is a count, and the window is 13 slots wide, so a
	 * partial drain says 1..3 here and a catch-up packet says up to
	 * LOGITF_TF_CATCHUP_MAX. Hardcoding 4 told the wheel four samples
	 * were fresh when up to three of them were the previous one repeated.
	 */
	pkt[10] = new_count;
	pkt[11] = 0x0d;           /* constant per captures */
	/* bytes 12..63: 13 window slots, oldest first, each duplicated */
	for (int i = 0; i < LOGITF_TF_WINDOW; i++) {
		uint8_t *p = pkt + 12 + i * 4;
		uint16_t v = window[i];

		p[0] = v & 0xff;
		p[1] = v >> 8;
		p[2] = v & 0xff;
		p[3] = v >> 8;
	}
}

/*
 * The silent keepalive Windows streams through game menus: type 0x01,
 * cur duplicated at 6-9, byte 10 = 0x00 (zero new samples), byte 11 =
 * 0x00, and bytes 12..63 literal zeros - NOT centre values, and NOT a
 * repeat of the last window (whine-investigation.md section 2; 56287 of
 * the 119028 packets in the RS50+ACC capture have exactly this shape).
 */
void logitf_build_idle_packet(uint8_t *pkt, uint8_t seq, uint16_t current)
{
	memset(pkt, 0, 64);
	pkt[0] = 0x01;           /* HID report ID */
	pkt[4] = 0x01;           /* type: sample */
	pkt[5] = seq;
	pkt[6] = current & 0xff;
	pkt[7] = current >> 8;
	pkt[8] = current & 0xff;
	pkt[9] = current >> 8;
	/* bytes 10..63 stay zero: no new samples, nothing to play */
}

/* Control packet (0x03 arm / 0x04 stop-clear): type at 4, seq at 5. */
void logitf_build_ctrl_packet(uint8_t *pkt, uint8_t type, uint8_t seq)
{
	memset(pkt, 0, 64);
	pkt[0] = 0x01;
	pkt[4] = type;
	pkt[5] = seq;
}

static void stream_microsleep(unsigned us)
{
	struct timespec ts = { 0, (long)us * 1000 };

	nanosleep(&ts, NULL);
}

/*
 * Send the captured session-teardown pair: one 0x04 (stop/clear) then
 * one 0x03 (arm) ~2 ms later - the exact bytes every clean Windows
 * session ends with, and the same pair that closes init pass 1
 * (packets 67+68). Leaves the engine flushed and armed; the host
 * silence that follows is what the firmware reads as end of session.
 *
 * Caller must hold dev->lock and must guarantee the stream thread is
 * not concurrently writing (it either IS the stream thread, or the
 * thread has been joined, or tf_paused has been set and drained).
 *
 * Idempotency guard: if tf_armed_idle is already set the pair has
 * already gone out and we return immediately. This is what keeps
 * logiTrueForcePause() racing the starvation gate from doubling the
 * pair on the wire - both paths funnel through this one check.
 */
int logitf_tf_send_stop_pair(struct logitf_device *dev)
{
	uint8_t pkt[64];
	ssize_t wr;

	if (dev->tf_armed_idle)
		return LOGITF_OK;
	if (dev->hidraw_fd < 0)
		return LOGITF_ERR_IO;

	logitf_build_ctrl_packet(pkt, 0x04, dev->tf_seq++);
	wr = write(dev->hidraw_fd, pkt, sizeof(pkt));
	if (wr != (ssize_t)sizeof(pkt))
		return LOGITF_ERR_IO;

	/*
	 * Release dev->lock for the inter-packet sleep. Single-emitter
	 * ordering (see the struct comment on tf_teardown_pending) means
	 * the only caller ever inside this function while a session is
	 * live is the stream thread itself, and it cannot call itself
	 * concurrently; every other caller has already joined that thread.
	 * So nothing else touches tf_seq or hidraw_fd during the sleep,
	 * and holding the lock across it would only block unrelated
	 * readers (GetDamping, IsPaused, ...) for 2 ms for no reason.
	 */
	pthread_mutex_unlock(&dev->lock);
	stream_microsleep(2000);	/* captured pair spacing: ~2 ms */
	pthread_mutex_lock(&dev->lock);

	logitf_build_ctrl_packet(pkt, 0x03, dev->tf_seq++);
	wr = write(dev->hidraw_fd, pkt, sizeof(pkt));
	if (wr != (ssize_t)sizeof(pkt))
		return LOGITF_ERR_IO;
	dev->tf_armed_idle = true;
	return LOGITF_OK;
}

/* True if a drained block is all exactly centre: actively pushed
 * silence, which must read on the wire the same as an empty ring
 * (F5: logi-tf-sim's pre-gate menu output is exact zeros pushed at
 * the full rate, not an absence of pushes). */
static bool block_is_silent(const uint16_t *samples, int n)
{
	int i;

	for (i = 0; i < n; i++)
		if (samples[i] != 0x8000)
			return false;
	return true;
}

/*
 * Step the held force one tick toward centre. Fixed-size step
 * (LOGITF_TF_STARVE_DECAY_STEP), not proportional to distance, so a
 * full-scale offset is guaranteed to reach exactly 0x8000 within
 * LOGITF_TF_STARVE_DECAY_TICKS ticks; smaller offsets arrive sooner
 * and then this is a no-op.
 */
static uint16_t decay_towards_centre(uint16_t current)
{
	int32_t distance = (int32_t)current - 0x8000;

	if (distance > 0) {
		distance -= LOGITF_TF_STARVE_DECAY_STEP;
		if (distance < 0)
			distance = 0;
	} else if (distance < 0) {
		distance += LOGITF_TF_STARVE_DECAY_STEP;
		if (distance > 0)
			distance = 0;
	}
	return (uint16_t)(0x8000 + distance);
}

int logitf_stream_tick(struct logitf_device *dev)
{
	return logitf_stream_tick_n(dev, 1);
}

int logitf_stream_tick_n(struct logitf_device *dev, unsigned expiries)
{
	uint16_t new_samples[LOGITF_TF_CATCHUP_MAX];
	int want, n = 0;
	uint8_t pkt[64];
	ssize_t wr;
	bool paused, armed_idle, idle_shape;

	/*
	 * Single-emitter ordering (F2): if Pause is waiting on a teardown,
	 * do that and nothing else this tick. logitf_tf_send_stop_pair's
	 * own tf_armed_idle guard makes this a no-op if the starvation
	 * gate below already fired first.
	 */
	pthread_mutex_lock(&dev->lock);
	if (dev->tf_teardown_pending) {
		int rc = logitf_tf_send_stop_pair(dev);

		dev->tf_teardown_pending = false;
		pthread_cond_broadcast(&dev->tf_teardown_done);
		pthread_mutex_unlock(&dev->lock);
		return rc == LOGITF_OK ? 0 : -EIO;
	}
	paused = dev->tf_paused;
	armed_idle = dev->tf_armed_idle;
	pthread_mutex_unlock(&dev->lock);

	/*
	 * Sample budget for this packet: one slot's worth per expiration the
	 * timerfd reported, so time the previous tick overran is made up in
	 * samples instead of being thrown away, capped at what the window can
	 * carry (LOGITF_TF_CATCHUP_MAX). Still exactly ONE packet either way:
	 * the interrupt OUT endpoint carries one per USB frame, so writing
	 * several here would only queue jitter into the host controller.
	 */
	want = LOGITF_TF_NEW;
	if (expiries > 1) {
		want = (expiries >= LOGITF_TF_CATCHUP_MAX / LOGITF_TF_NEW)
		     ? LOGITF_TF_CATCHUP_MAX
		     : (int)expiries * LOGITF_TF_NEW;
	}

	/* Drain up to `want` samples from the ring (non-blocking). */
	pthread_mutex_lock(&dev->ring_lock);
	while (n < want && dev->ring_tail != dev->ring_head) {
		new_samples[n++] = dev->ring[dev->ring_tail & (LOGITF_TF_RING - 1)];
		dev->ring_tail++;
	}
	pthread_mutex_unlock(&dev->ring_lock);

	if (n > 0) {
		/*
		 * Shift the window left by exactly the number of samples we
		 * got and append them at the tail, so the slots the packet
		 * declares as new (byte 10) really are new and everything
		 * before them is real history. A partial batch used to shift
		 * by LOGITF_TF_NEW regardless and pad with repeats of the last
		 * sample, which both invented audio and pushed real history
		 * off the front of the window.
		 */
		int shift = n;

		memmove(&dev->tf_window[0],
			&dev->tf_window[shift],
			(LOGITF_TF_WINDOW - shift) * sizeof(uint16_t));
		memcpy(&dev->tf_window[LOGITF_TF_WINDOW - shift],
		       new_samples, shift * sizeof(uint16_t));
		dev->tf_last_current = dev->tf_window[LOGITF_TF_WINDOW - 1];
		dev->tf_starved_ticks = 0;
	} else if (dev->tf_starved_ticks <= LOGITF_TF_STARVE_HOLD_TICKS &&
		   ++dev->tf_starved_ticks <= LOGITF_TF_STARVE_HOLD_TICKS) {
		/*
		 * Inside the hold: leave the window and cur exactly as they
		 * are. The counter saturates one past the bound rather than
		 * running free, so a session parked in the silence gate for
		 * hours cannot wrap it back through the hold.
		 */
	} else {
		/*
		 * Sustained starvation: flush the window toward centre so
		 * pre-idle audio does not replay when the stream resumes, and
		 * step the held force toward centre too
		 * (LOGITF_TF_STARVE_DECAY_STEP, see internal.h) instead of
		 * freezing it - bounded so a producer that quit mid-waveform
		 * cannot command a stale force forever, and so the silence
		 * gate below can fire.
		 *
		 * Only past LOGITF_TF_STARVE_HOLD_TICKS: below that the branch
		 * above holds instead, because one missed batch in the middle
		 * of continuous audio is a gap the producer will fill a
		 * millisecond later, and zeroing four slots for it puts a
		 * discontinuity in the wheel's hands that a hold does not.
		 */
		memmove(&dev->tf_window[0],
			&dev->tf_window[LOGITF_TF_NEW],
			(LOGITF_TF_WINDOW - LOGITF_TF_NEW) * sizeof(uint16_t));
		for (int i = 0; i < LOGITF_TF_NEW; i++)
			dev->tf_window[LOGITF_TF_WINDOW - LOGITF_TF_NEW + i] = 0x8000;
		dev->tf_last_current = decay_towards_centre(dev->tf_last_current);
	}

	if (paused)
		return 0;

	idle_shape = (n == 0) || block_is_silent(new_samples, n);

	if (idle_shape) {
		if (armed_idle)
			return 0;	/* post-pair standby: total silence */

		/*
		 * Silence gate (whine-investigation.md H1): a session held
		 * at zero force past the grace period is torn down the way
		 * Windows tears one down - 0x04 + 0x03, then nothing. The
		 * engine stays armed, so the next push resumes the stream
		 * with no re-init. Gated on centre force: a held non-zero
		 * cur must keep its keepalive cadence, because the firmware
		 * unwinds held force on host silence (issue #16) and the
		 * unwind of zero is the only unwind that costs nothing.
		 */
		if (dev->tf_last_current == 0x8000 &&
		    ++dev->tf_idle_ticks >= LOGITF_TF_IDLE_GRACE_TICKS) {
			int rc;

			dev->tf_idle_ticks = 0;
			pthread_mutex_lock(&dev->lock);
			rc = logitf_tf_send_stop_pair(dev);
			pthread_mutex_unlock(&dev->lock);
			return rc == LOGITF_OK ? 0 : -EIO;
		}
		if (dev->tf_last_current != 0x8000)
			dev->tf_idle_ticks = 0;

		logitf_build_idle_packet(pkt, dev->tf_seq++,
					 dev->tf_last_current);
	} else {
		dev->tf_idle_ticks = 0;
		pthread_mutex_lock(&dev->lock);
		dev->tf_armed_idle = false;	/* resuming; pair's 0x03 armed us */
		pthread_mutex_unlock(&dev->lock);
		logitf_build_stream_packet(pkt, dev->tf_seq++,
					   dev->tf_last_current,
					   dev->tf_window, (uint8_t)n);
	}

	wr = write(dev->hidraw_fd, pkt, sizeof(pkt));

	if (wr < 0)
		return -errno;
	if (wr != (ssize_t)sizeof(pkt))
		return -EIO;
	return 0;
}

/* ---------- device feedback (type-0x02 responses, ep 0x83) ---------- */

/*
 * The wheel answers interface-2 traffic with type-0x02 responses at the
 * host's packet rate. Layout per docs/TRUEFORCE_PROTOCOL.md:
 *
 *     4       0x02                        response type
 *     5       sequence echo
 *     6..7    u16 LE                      motor current/temperature?
 *     8       status byte
 *     9..10   u16 LE                      wheel position (matches ABS_X)
 *     11..12  u16 LE                      wheel position, ~1 sample older
 *     13..16  u32 LE                      device-side counter
 *
 * Drain everything pending (zero-timeout poll per read so the blocking
 * fd never parks the stream thread) and keep the newest packet. If
 * nobody drained these, the kernel hidraw ring would just drop the
 * oldest - consuming them costs nothing and buys closed-loop feedback.
 */
static void drain_feedback(struct logitf_device *dev)
{
	uint8_t buf[64];

	for (;;) {
		struct pollfd p = { .fd = dev->hidraw_fd, .events = POLLIN };
		ssize_t n;

		if (poll(&p, 1, 0) <= 0 || !(p.revents & POLLIN))
			break;
		n = read(dev->hidraw_fd, buf, sizeof(buf));
		if (n < 17)
			break;
		if (buf[4] != 0x02)
			continue;	/* 0x10/0x14/... : not stream feedback */

		pthread_mutex_lock(&dev->lock);
		dev->fb_motor_raw = (uint16_t)(buf[6] | (buf[7] << 8));
		dev->fb_status    = buf[8];
		dev->fb_wheel_pos = (uint16_t)(buf[9] | (buf[10] << 8));
		dev->fb_wheel_pos2 = (uint16_t)(buf[11] | (buf[12] << 8));
		dev->fb_counter = (uint32_t)buf[13] | ((uint32_t)buf[14] << 8) |
				  ((uint32_t)buf[15] << 16) |
				  ((uint32_t)buf[16] << 24);
		dev->fb_packets++;
		dev->fb_valid = true;
		pthread_mutex_unlock(&dev->lock);
	}
}

int logitf_stream_feedback_read(struct logitf_device *dev,
				struct logitf_stream_feedback *fb)
{
	int rc = LOGITF_OK;

	pthread_mutex_lock(&dev->lock);
	if (!dev->fb_valid) {
		rc = LOGITF_ERR_BUSY;
	} else {
		fb->wheel_position  = dev->fb_wheel_pos;
		fb->wheel_position2 = dev->fb_wheel_pos2;
		fb->sample_counter  = dev->fb_counter;
		fb->motor_raw       = dev->fb_motor_raw;
		fb->status          = dev->fb_status;
		fb->packets         = dev->fb_packets;
	}
	pthread_mutex_unlock(&dev->lock);
	return rc;
}

/* ---------- thread ---------- */

/*
 * True for the errno values that mean the wheel is gone rather than that
 * one write went wrong: there is nothing to retry, and retrying at 1 kHz
 * is what spins a core. Anything else (EINTR, EAGAIN, ...) is recorded but
 * left to the next tick.
 */
static bool stream_error_is_fatal(int rc)
{
	switch (-rc) {
	case ENODEV:
	case ENXIO:
	case EIO:
	case EPIPE:
	case EBADF:
	case ESHUTDOWN:
		return true;
	default:
		return false;
	}
}

/*
 * Record why the stream thread is stopping, so logitf_stream_push_s16 can
 * report it to the caller (the thread has no other way to be heard: its
 * return value goes to pthread_join, which only teardown ever calls). First
 * one wins; a later error is just noise from the same event.
 */
static void stream_record_error(struct logitf_device *dev, int rc)
{
	pthread_mutex_lock(&dev->lock);
	if (dev->stream_error == 0) {
		dev->stream_error = rc;
		fprintf(stderr,
			"libtrueforce: stream thread stopped: %s\n",
			strerror(-rc));
	}
	pthread_mutex_unlock(&dev->lock);
}

static void *stream_thread_fn(void *arg)
{
	struct logitf_device *dev = arg;
	struct pollfd pfds[3] = {
		{ .fd = dev->stream_timerfd, .events = POLLIN },
		{ .fd = dev->stream_stopfd,  .events = POLLIN },
		{ .fd = dev->hidraw_fd,      .events = POLLIN },
	};

	for (;;) {
		int pr = poll(pfds, 3, -1);

		if (pr < 0) {
			if (errno == EINTR)
				continue;
			stream_record_error(dev, -errno);
			break;
		}
		if (pfds[1].revents & POLLIN)
			break;  /* stop requested */
		/*
		 * An unplugged wheel leaves its hidraw fd reporting
		 * POLLERR|POLLHUP permanently. Only POLLIN used to be tested,
		 * so poll() returned instantly forever with no branch taken:
		 * one core at 100%, nothing logged, and the caller still being
		 * told its pushes succeeded. These are reported regardless of
		 * `events`, so they must be handled explicitly, and they are
		 * fatal - the fd never recovers.
		 */
		if (pfds[2].revents & (POLLERR | POLLHUP | POLLNVAL)) {
			stream_record_error(dev, -ENODEV);
			break;
		}
		if (pfds[2].revents & POLLIN)
			drain_feedback(dev);
		if (pfds[0].revents & POLLIN) {
			uint64_t expiries;
			int rc;

			if (read(dev->stream_timerfd, &expiries,
				 sizeof(expiries)) < 0) {
				if (errno == EINTR)
					continue;
				stream_record_error(dev, -errno);
				break;
			}
			/*
			 * A tick that overran its 1 ms slot (the write plus the
			 * feedback drain are syscalls) makes the timerfd
			 * coalesce, and `expiries` is then the number of sample
			 * slots that have gone by. Discarding it is what put the
			 * wire 9% behind the producer, so it is handed to the
			 * tick, which makes the missed SAMPLES up inside one
			 * packet rather than emitting several. Clamped because
			 * the tick caps its own catch-up anyway and a suspended
			 * laptop can report an enormous count.
			 */
			if (expiries > 64)
				expiries = 64;
			rc = logitf_stream_tick_n(dev, (unsigned)expiries);
			/*
			 * Only the fatal ones stop the thread and are recorded:
			 * a transient write failure must not make every later
			 * push fail, and the next tick retries it anyway.
			 */
			if (rc < 0 && stream_error_is_fatal(rc)) {
				stream_record_error(dev, rc);
				break;
			}
		}
	}
	return NULL;
}

/* ---------- lifecycle ---------- */

int logitf_stream_start(struct logitf_device *dev)
{
	int rc;
	struct itimerspec its = {
		.it_interval = { 0, 1000000000L / LOGITF_TF_PKT_HZ },
		.it_value    = { 0, 1000000000L / LOGITF_TF_PKT_HZ },
	};

	pthread_mutex_lock(&dev->lock);
	if (dev->stream_running) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_OK;
	}

	/* Initialise window to centre (offset-binary 0x8000). */
	for (int i = 0; i < LOGITF_TF_WINDOW; i++)
		dev->tf_window[i] = 0x8000;
	dev->tf_last_current = 0x8000;
	dev->tf_idle_ticks = 0;
	dev->tf_starved_ticks = 0;
	dev->stream_error = 0;
	dev->tf_teardown_pending = false;	/* no thread was around to service one */

	/* Per-session, like the G923 transport's counter: the total reported
	 * at stop is about this session's health, not the process's. */
	pthread_mutex_lock(&dev->ring_lock);
	dev->ring_dropped = 0;
	dev->ring_drop_warn_sec = 0;
	pthread_mutex_unlock(&dev->ring_lock);

	/*
	 * Sequence counter is set by session_ensure to
	 * TF_INIT_PACKET_COUNT+1 when the init sequence completes; if
	 * we get here before that (which shouldn't happen via the
	 * public API), fall back to the same value rather than reusing
	 * byte 0x00.
	 */
	if (dev->tf_seq == 0)
		dev->tf_seq = (uint8_t)(TF_INIT_PACKET_COUNT + 1);

	dev->stream_timerfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
	if (dev->stream_timerfd < 0) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	dev->stream_stopfd = eventfd(0, EFD_CLOEXEC);
	if (dev->stream_stopfd < 0) {
		close(dev->stream_timerfd);
		dev->stream_timerfd = -1;
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	if (timerfd_settime(dev->stream_timerfd, 0, &its, NULL) < 0) {
		close(dev->stream_stopfd);
		close(dev->stream_timerfd);
		dev->stream_stopfd = dev->stream_timerfd = -1;
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}

	rc = pthread_create(&dev->stream_thread, NULL, stream_thread_fn, dev);
	if (rc != 0) {
		close(dev->stream_stopfd);
		close(dev->stream_timerfd);
		dev->stream_stopfd = dev->stream_timerfd = -1;
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	dev->stream_running = true;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logitf_stream_stop(struct logitf_device *dev)
{
	uint64_t one = 1;
	pthread_t thread;
	int stopfd, timerfd;

	pthread_mutex_lock(&dev->lock);
	if (!dev->stream_running) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_OK;
	}
	/* Capture the state we'll act on, then release the lock. */
	thread  = dev->stream_thread;
	stopfd  = dev->stream_stopfd;
	timerfd = dev->stream_timerfd;
	pthread_mutex_unlock(&dev->lock);

	/*
	 * Refuse further pushes before the fds go away. Nothing parks in
	 * push_s16 any more (it load-sheds instead of blocking), so the
	 * condvars have no waiters left to wake; they are still broadcast
	 * here so a future blocking caller cannot be missed by this path.
	 */
	pthread_mutex_lock(&dev->ring_lock);
	dev->shutting_down = true;
	pthread_cond_broadcast(&dev->ring_space);
	pthread_cond_broadcast(&dev->ring_data);
	/*
	 * Say whether this session kept pace. A non-zero total means the
	 * producer outran the transport and the oldest samples were thrown
	 * away, which is otherwise indistinguishable from a healthy stream
	 * (the same reason the G923 writer reports its own count at stop).
	 */
	if (dev->ring_dropped > 0)
		fprintf(stderr,
			"libtrueforce: stream dropped %llu stale samples this "
			"session (the producer outran the wheel)\n",
			(unsigned long long)dev->ring_dropped);
	pthread_mutex_unlock(&dev->ring_lock);

	/* Signal the consumer thread to exit and wait for it. */
	if (stopfd >= 0)
		write(stopfd, &one, sizeof(one));
	pthread_join(thread, NULL);

	pthread_mutex_lock(&dev->lock);
	if (timerfd >= 0 && dev->stream_timerfd == timerfd) {
		close(timerfd);
		dev->stream_timerfd = -1;
	}
	if (stopfd >= 0 && dev->stream_stopfd == stopfd) {
		close(stopfd);
		dev->stream_stopfd = -1;
	}

	/*
	 * The thread is joined, so nobody else is writing: end the session
	 * the way every clean Windows session ends, 0x04 + 0x03 + silence,
	 * instead of shipping the abort-capture behaviour (stream ends
	 * mid-flight, engine left running until power cycle) as our normal
	 * exit. Skipped if the pair already went out (idle standby or an
	 * earlier pause); best-effort on a dying fd.
	 */
	if (dev->tf_initialized && !dev->tf_armed_idle)
		(void)logitf_tf_send_stop_pair(dev);

	/*
	 * stream_running/shutting_down are cleared only now, after the pair
	 * has gone out, not before calling send_stop_pair. That function
	 * drops dev->lock for the ~2 ms gap between the 0x04 and the 0x03
	 * (see its own comment); if we'd already marked the stream
	 * not-running, a concurrent logitf_stream_start() could see
	 * "not running" during that gap, spawn a fresh tick thread, and
	 * write a packet between our 0x04 and 0x03, racing tf_seq and
	 * splitting the pair. Leaving stream_running true until the pair
	 * (and the lock reacquisition after it) is done makes stream_start's
	 * running-check a no-op for the whole gap, closing that window.
	 */
	dev->stream_running = false;
	dev->shutting_down = false;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}
