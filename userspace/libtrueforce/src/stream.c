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
 * last sample again as fresh audio (whine-investigation.md, H2).
 * After LOGITF_TF_IDLE_GRACE_TICKS of that at centre force, the
 * thread sends the captured session-teardown pair (0x04 stop/clear,
 * then 0x03 arm ~2 ms later) and goes fully silent, exactly the way
 * every clean Windows session ends; the next pushed sample resumes
 * the stream without re-init, the engine having stayed armed. If
 * userspace overruns the ring, push blocks on ring_space (or returns
 * EAGAIN in non-blocking callers - a future 22.x item).
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

static unsigned ring_free(const struct logitf_device *dev)
{
	return LOGITF_TF_RING - 1 - ring_occupied(dev);
}

/*
 * Push `count` samples to the ring. Blocks until space is available
 * (Windows semantics: "SetTorque*" is synchronous). Returns LOGITF_OK
 * on success or a negative error code.
 */
int logitf_stream_push_s16(struct logitf_device *dev,
			   const int16_t *samples, int count)
{
	if (!samples || count < 0)
		return LOGITF_ERR_INVALID_ARG;
	if (count == 0)
		return LOGITF_OK;

	pthread_mutex_lock(&dev->ring_lock);
	for (int i = 0; i < count; i++) {
		/*
		 * Wait-predicate includes running/shutdown state so we
		 * don't park indefinitely if the consumer never started
		 * or is already going away. stream_stop broadcasts
		 * ring_space to wake us.
		 */
		while (ring_free(dev) == 0 &&
		       dev->stream_running &&
		       !dev->shutting_down)
			pthread_cond_wait(&dev->ring_space, &dev->ring_lock);
		if (dev->shutting_down || !dev->stream_running) {
			pthread_mutex_unlock(&dev->ring_lock);
			return LOGITF_ERR_IO;
		}
		dev->ring[dev->ring_head & (LOGITF_TF_RING - 1)] =
			logitf_s16_to_wire(samples[i]);
		dev->ring_head++;
	}
	pthread_cond_broadcast(&dev->ring_data);
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
				const uint16_t window[LOGITF_TF_WINDOW])
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
	pkt[10] = LOGITF_TF_NEW;  /* new-samples-this-packet */
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
 */
int logitf_tf_send_stop_pair(struct logitf_device *dev)
{
	uint8_t pkt[64];
	ssize_t wr;

	if (dev->hidraw_fd < 0)
		return LOGITF_ERR_IO;

	logitf_build_ctrl_packet(pkt, 0x04, dev->tf_seq++);
	wr = write(dev->hidraw_fd, pkt, sizeof(pkt));
	if (wr != (ssize_t)sizeof(pkt))
		return LOGITF_ERR_IO;
	stream_microsleep(2000);	/* captured pair spacing: ~2 ms */
	logitf_build_ctrl_packet(pkt, 0x03, dev->tf_seq++);
	wr = write(dev->hidraw_fd, pkt, sizeof(pkt));
	if (wr != (ssize_t)sizeof(pkt))
		return LOGITF_ERR_IO;
	dev->tf_armed_idle = true;
	return LOGITF_OK;
}

static int stream_tick(struct logitf_device *dev)
{
	uint16_t new_samples[LOGITF_TF_NEW];
	int n = 0;
	uint8_t pkt[64];
	ssize_t wr;

	/* Drain up to LOGITF_TF_NEW samples from the ring (non-blocking). */
	pthread_mutex_lock(&dev->ring_lock);
	while (n < LOGITF_TF_NEW && dev->ring_tail != dev->ring_head) {
		new_samples[n++] = dev->ring[dev->ring_tail & (LOGITF_TF_RING - 1)];
		dev->ring_tail++;
	}
	if (n > 0)
		pthread_cond_broadcast(&dev->ring_space);
	pthread_mutex_unlock(&dev->ring_lock);

	if (n > 0) {
		/*
		 * Shift the window left by LOGITF_TF_NEW, append new samples
		 * at the tail. If we got a partial batch, the unfilled slots
		 * repeat the last known sample.
		 */
		int shift = LOGITF_TF_NEW;

		memmove(&dev->tf_window[0],
			&dev->tf_window[shift],
			(LOGITF_TF_WINDOW - shift) * sizeof(uint16_t));
		uint16_t last = dev->tf_window[LOGITF_TF_WINDOW - shift - 1];

		for (int i = 0; i < shift; i++) {
			uint16_t v = (i < n) ? new_samples[i] : last;

			dev->tf_window[LOGITF_TF_WINDOW - shift + i] = v;
			last = v;
		}
		dev->tf_last_current = dev->tf_window[LOGITF_TF_WINDOW - 1];
	} else {
		/*
		 * Starved tick: flush the window toward centre so pre-idle
		 * audio does not replay when the stream resumes. The wire
		 * carries the idle packet's zeroed tail either way;
		 * tf_last_current (the held force) is deliberately NOT
		 * decayed - a producer that quit mid-waveform keeps its
		 * commanded force held in cur, it just stops being replayed
		 * as fresh audio.
		 */
		memmove(&dev->tf_window[0],
			&dev->tf_window[LOGITF_TF_NEW],
			(LOGITF_TF_WINDOW - LOGITF_TF_NEW) * sizeof(uint16_t));
		for (int i = 0; i < LOGITF_TF_NEW; i++)
			dev->tf_window[LOGITF_TF_WINDOW - LOGITF_TF_NEW + i] = 0x8000;
	}

	if (dev->tf_paused)
		return 0;

	if (n == 0) {
		if (dev->tf_armed_idle)
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
		dev->tf_armed_idle = false;	/* resuming; pair's 0x03 armed us */
		logitf_build_stream_packet(pkt, dev->tf_seq++,
					   dev->tf_last_current,
					   dev->tf_window);
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
			break;
		}
		if (pfds[1].revents & POLLIN)
			break;  /* stop requested */
		if (pfds[2].revents & POLLIN)
			drain_feedback(dev);
		if (pfds[0].revents & POLLIN) {
			uint64_t expiries;

			if (read(dev->stream_timerfd, &expiries, sizeof(expiries)) < 0)
				break;
			/*
			 * Under severe scheduling stalls `expiries` can be > 1.
			 * Emit one packet regardless; the next tick will catch
			 * up on the ring drain. Emitting multiple packets here
			 * would burst-write to the wheel and cause jitter.
			 */
			(void)expiries;
			stream_tick(dev);
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
	 * Wake any producer blocked in push_s16 so they don't hold
	 * ring_lock while we try to close fds below.
	 */
	pthread_mutex_lock(&dev->ring_lock);
	dev->shutting_down = true;
	pthread_cond_broadcast(&dev->ring_space);
	pthread_cond_broadcast(&dev->ring_data);
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
	dev->stream_running = false;
	dev->shutting_down = false;

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
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}
