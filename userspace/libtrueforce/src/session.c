// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - hidraw session and init sequence.
 *
 * On first use, opens /dev/hidrawN for the wheel's interface 2 and
 * sends the canonical Trueforce init sequence extracted from
 * captures of a BeamNG session on Windows G HUB (issue #5). The
 * 68-packet sequence sets up parameters (type 0x05), frequency
 * (type 0x0e), a handshake (type 0x07), six slot configs (type
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
	dev->tf_seq = (uint8_t)(TF_INIT_PACKET_COUNT + 1);
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logitf_session_close(struct logitf_device *dev)
{
	pthread_mutex_lock(&dev->lock);

	if (dev->hidraw_fd >= 0) {
		close(dev->hidraw_fd);
		dev->hidraw_fd = -1;
	}
	dev->tf_initialized = false;

	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}
