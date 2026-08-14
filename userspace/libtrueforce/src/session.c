// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - hidraw session and init sequence.
 *
 * On first use, opens /dev/hidrawN for the wheel's interface 2 and
 * sends the canonical Trueforce init sequence extracted from
 * captures of a BeamNG session on Windows G HUB (issue #5). The
 * 68-packet sequence sets up parameters (type 0x05), the operating
 * range (type 0x0e), a handshake (type 0x07), six slot configs (type
 * 0x06), runtime state (type 0x09), and a start/stop pair to arm
 * streaming (types 0x03 / 0x04).
 *
 * The per-packet sequence byte (offset 5) is rewritten at send time
 * from a session-local counter, starting at 1; the device identifies
 * dropped/duplicated packets from this value.
 */

#include "internal.h"
#include "tf_init_data.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* Short inter-packet delay during init. The capture showed ~2-4 ms
 * between init packets; going below that risks overrunning the
 * device's interrupt-OUT processing on slower firmware. */
#define TF_INIT_INTERPACKET_US 2000

static int write_all(int fd, const void *buf, size_t len)
{
	const unsigned char *p = buf;
	ssize_t n;

	while (len) {
		n = write(fd, p, len);
		if (n < 0) {
			if (errno == EINTR)
				continue;
			return -errno;
		}
		if ((size_t)n > len)
			return -EIO;
		p += n;
		len -= n;
	}
	return 0;
}

/* Packet type byte, and the payload of a type-0x0e packet. */
#define TF_INIT_TYPE_OFFSET   4
#define TF_INIT_TYPE_RANGE    0x0e
#define TF_INIT_RANGE_OFFSET  6

/*
 * Rewrite the operating range a type-0x0e packet carries.
 *
 * The captured sequence pushes 2700 degrees, because that is what the
 * wheel happened to be set to when G HUB was recorded. Replayed
 * verbatim it silently overwrites whatever range the user configured,
 * and the wheel sweeps to apply it: this is the same type-0x0e push
 * identified as the cause of the 90-degree range resets, arriving from
 * our own init rather than from a game. The init runs twice per
 * session, so it lands twice.
 *
 * Faithful replay is right for the parts of this sequence nobody has
 * decoded, but a packet carrying user state has to carry the user's
 * state. Rewriting keeps the sequence's shape and length identical and
 * makes the push a no-op.
 *
 * Left untouched when the range cannot be read, which is no worse than
 * before: the kernel's wheel_range_restore still heals it.
 */
static void patch_range_packet(struct logitf_device *dev, uint8_t *pkt)
{
	uint32_t bits;
	float deg;
	int v;

	if (pkt[TF_INIT_TYPE_OFFSET] != TF_INIT_TYPE_RANGE)
		return;
	/* Same attribute pair logiWheelGetOperatingRangeDegrees() reads:
	 * the DD wheels expose wheel_range, the G923 calls it range. */
	if (logitf_sysfs_read_int(dev, "wheel_range", &v) != 0 &&
	    logitf_sysfs_read_int(dev, "range", &v) != 0)
		return;
	if (v <= 0 || v > 2700)
		return;

	/* IEEE754 single, little-endian on the wire. Written byte by byte
	 * rather than memcpy'd so the encoding does not depend on the
	 * host's byte order. */
	deg = (float)v;
	memcpy(&bits, &deg, sizeof(bits));
	pkt[TF_INIT_RANGE_OFFSET + 0] = (uint8_t)(bits);
	pkt[TF_INIT_RANGE_OFFSET + 1] = (uint8_t)(bits >> 8);
	pkt[TF_INIT_RANGE_OFFSET + 2] = (uint8_t)(bits >> 16);
	pkt[TF_INIT_RANGE_OFFSET + 3] = (uint8_t)(bits >> 24);
}

/*
 * Send one init packet with the session-local sequence counter
 * written into offset 5. Returns 0 on success, negative errno-like
 * on failure.
 */
static int send_init_packet(struct logitf_device *dev, size_t i, uint8_t seq)
{
	uint8_t pkt[TF_INIT_PACKET_LEN];

	memcpy(pkt, tf_init_packets[i], TF_INIT_PACKET_LEN);
	pkt[TF_INIT_SEQ_OFFSET] = seq;
	patch_range_packet(dev, pkt);
	return write_all(dev->hidraw_fd, pkt, TF_INIT_PACKET_LEN);
}

static void microsleep(unsigned us)
{
	struct timespec ts = { 0, (long)us * 1000 };
	nanosleep(&ts, NULL);
}

/*
 * Bring up the TF session: open hidraw, send init, transition to
 * "initialized". Idempotent; returns LOGITF_OK if already up.
 */
int logitf_session_ensure(struct logitf_device *dev)
{
	int rc;

	pthread_mutex_lock(&dev->lock);

	if (dev->tf_initialized && dev->hidraw_fd >= 0) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_OK;
	}

	if (dev->hidraw_fd < 0) {
		dev->hidraw_fd = open(dev->hidraw_path, O_RDWR | O_CLOEXEC);
		if (dev->hidraw_fd < 0) {
			int e = errno;

			pthread_mutex_unlock(&dev->lock);
			if (e == EACCES || e == EPERM)
				return LOGITF_ERR_BUSY;
			return LOGITF_ERR_IO;
		}
	}

	/*
	 * Fresh G Hub USB captures (RS50 + ACC 2026-04-21 and G Pro +
	 * BeamNG 2026-04-19) both show the 68-packet init sequence sent
	 * TWICE back-to-back with the sequence counter reset to 1 at the
	 * start of each pass, before the main per-sample stream begins.
	 * Single-pass init did produce audible TF on the bench but was
	 * less reliable on cold-boot. Replicate G Hub's two-pass
	 * behaviour exactly.
	 */
	for (int pass = 0; pass < 2; pass++) {
		for (size_t i = 0; i < TF_INIT_PACKET_COUNT; i++) {
			uint8_t seq = (uint8_t)((i + 1) & 0xff);

			rc = send_init_packet(dev, i, seq);
			if (rc < 0) {
				close(dev->hidraw_fd);
				dev->hidraw_fd = -1;
				pthread_mutex_unlock(&dev->lock);
				return LOGITF_ERR_IO;
			}
			microsleep(TF_INIT_INTERPACKET_US);
		}
	}

	dev->tf_initialized = true;
	dev->tf_paused = false;
	dev->tf_armed_idle = false;
	dev->tf_idle_ticks = 0;
	dev->tf_seq = (uint8_t)(TF_INIT_PACKET_COUNT + 1);
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logitf_session_close(struct logitf_device *dev)
{
	pthread_mutex_lock(&dev->lock);

	if (dev->hidraw_fd >= 0) {
		/*
		 * Backstop for sessions that were initialized but never
		 * streamed (logitf_stream_stop sends the pair for the
		 * streamed case, and skips it here via tf_armed_idle):
		 * close with the captured 0x04+0x03 teardown, never with
		 * the abort-capture's bare fd close that leaves the engine
		 * running until power cycle.
		 */
		if (dev->tf_initialized && !dev->tf_armed_idle)
			(void)logitf_tf_send_stop_pair(dev);
		close(dev->hidraw_fd);
		dev->hidraw_fd = -1;
	}
	dev->tf_initialized = false;
	dev->tf_armed_idle = false;

	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}
