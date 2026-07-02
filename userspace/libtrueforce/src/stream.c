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
 * If userspace can't keep up, the thread repeats the previous
 * window (Windows does the same under input starvation) and the
 * wheel gradually unwinds. If userspace overruns the ring, push
 * blocks on ring_space (or returns EAGAIN in non-blocking callers
 * - a future 22.x item).
 *
 * Coexistence with the kernel driver on interface 2: our in-tree
 * hid-logitech-hidpp fork also writes to interface 2's ep 0x03 OUT
 * for classic PID FFB (wheel's HID-report id 0x11, short packets).
 * Our TF packets use HID-report id 0x01 with the 64-byte layout
 * below. The wheel firmware demultiplexes by report id, so the two
 * paths can run concurrently. Verified empirically by playing a
 * sine on TF while holding a KF constant torque; both produced the
 * expected tactile output with no dropped packets.
 */

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

static void build_packet(uint8_t *pkt, uint8_t seq,
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

static int stream_tick(struct logitf_device *dev)
{
	uint16_t new_samples[LOGITF_TF_NEW];
	int n = 0;
	uint8_t pkt[64];

	/* Drain up to LOGITF_TF_NEW samples from the ring (non-blocking). */
	pthread_mutex_lock(&dev->ring_lock);
	while (n < LOGITF_TF_NEW && dev->ring_tail != dev->ring_head) {
		new_samples[n++] = dev->ring[dev->ring_tail & (LOGITF_TF_RING - 1)];
		dev->ring_tail++;
	}
	if (n > 0)
		pthread_cond_broadcast(&dev->ring_space);
	pthread_mutex_unlock(&dev->ring_lock);

	/* Shift the window left by LOGITF_TF_NEW, append new samples at the
	 * tail. If we got fewer than LOGITF_TF_NEW samples (starvation), the
	 * unfilled slots repeat the last known sample - same effect as the
	 * Windows driver under input underrun.
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

	if (dev->tf_paused)
		return 0;

	build_packet(pkt, dev->tf_seq++, dev->tf_last_current, dev->tf_window);

	ssize_t wr = write(dev->hidraw_fd, pkt, sizeof(pkt));

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
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}
