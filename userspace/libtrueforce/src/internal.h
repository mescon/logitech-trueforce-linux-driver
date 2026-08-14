// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - private state.
 */

#ifndef LIBTRUEFORCE_INTERNAL_H
#define LIBTRUEFORCE_INTERNAL_H

#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>

#include "trueforce.h"

#define LOGITF_LOGI_VID		0x046D
#define LOGITF_RS50_PID		0xC276	/* Logitech RS50 Base */
#define LOGITF_GPRO_XBOX_PID	0xC272	/* Logitech G PRO Racing Wheel (Xbox/PC) */
#define LOGITF_GPRO_PS_PID	0xC268	/* Logitech G PRO Racing Wheel (PS/PC) */
#define LOGITF_IFACE_TF		2

#define LOGITF_TF_WINDOW  13          /* samples per packet (rolling window) */
#define LOGITF_TF_NEW     4           /* new samples added per packet */
/* Sample ring capacity (must be pow2).
 *
 * A COUNT that behaves as a duration: 4096 samples is 4.1 s of audio at a
 * 1 kHz stream and 1.0 s at 4 kHz, so raising the packet rate quietly
 * quarters the worst-case buffered latency this permits. Harmless here
 * only because logitf_stream_push blocks rather than dropping when the
 * ring is full, so a producer pacing itself by wall clock never fills it;
 * the equivalent bound on the G923 path load-sheds instead, and being
 * written as a sample count is exactly what broke it at 4 kHz.
 */
#define LOGITF_TF_RING    4096
/* Packets per second; with LOGITF_TF_NEW this is a 4 kHz sample stream.
 *
 * Was 250 (a 1 kHz stream), the rate the G HUB capture this was built from
 * used. Measured on an RS50 (2026-08-08) the wheel sustains 1000 packets/sec
 * with no drops or errors, so 4000 samples/sec. (The bench figure was 4022,
 * about 0.6% high: that measurement divided the pushed sample count by the
 * span between its first and last timestamp, an off-by-one that overstates
 * by roughly one push. The same bias shows as 1016 at 250 packets/sec and
 * 2020 at 500, shrinking as the count grows. The rate itself is exact by
 * construction, the itimerspec below being a hard 1 ms period.) The
 * result was audibly better: at 1 kHz the note's upper harmonics cannot
 * survive above 250 Hz, so a high engine note degenerated into a plain tone
 * at high revs. 1000 is also the ceiling this transport allows, USB
 * interrupt endpoints polling at 1 ms intervals.
 *
 * It is the vendor's own figure too, which is worth recording because this
 * rate was reverse-engineered rather than looked up. Logitech's TRUEFORCE
 * page states "1 MILLISECOND PROCESSING SPEED" and that it "processes game
 * data at lightning speed - just 1ms", and their launch coverage puts the
 * sampling at "up to 4000 times per second". One packet per millisecond
 * carrying LOGITF_TF_NEW samples is exactly both numbers, so the stream now
 * runs at the rate the hardware was designed around.
 *
 * Which also means the 250 that stood here was a quarter of it. The G HUB
 * capture it came from really did show 4 ms spacing, so either that session
 * was running a reduced rate or G HUB varies it; unresolved, and worth
 * knowing before anyone treats that capture as the definitive cadence.
 */
#define LOGITF_TF_PKT_HZ  1000

/*
 * Silence gate: consecutive ring-starved ticks at centre force before the
 * stream thread sends the 0x04+0x03 teardown pair and goes silent
 * (whine-investigation.md H1: a session held open at zero force is what
 * whines, and Windows' answer is to not hold one). 500 ms sits inside the
 * report's suggested 250 ms..1 s band: long enough that a starved-but-live
 * producer never hits it, short enough that menu idling goes quiet fast.
 */
#define LOGITF_TF_IDLE_GRACE_TICKS  (LOGITF_TF_PKT_HZ / 2)

struct logitf_device {
	bool in_use;

	/* Identity */
	uint16_t vid;
	uint16_t pid;
	char hidraw_path[272];     /* /dev/<d_name(255)> fits untruncated */
	char evdev_path[272];      /* /dev/input/<d_name(255)> fits */
	char by_id[288];           /* /dev/input/by-id/<d_name(255)> fits */
	char usb_root[4096];       /* PATH_MAX realpath result -- shared with sibling interfaces */

	/* File descriptors (open on first use) */
	int hidraw_fd;             /* TF audio stream */
	int evdev_fd;              /* KF constant force via input_ff */

	/* KF state */
	int kf_effect_id;
	bool kf_playing;
	double kf_last_nm;

	/* Status reader state */
	bool status_running;
	pthread_t status_thread;
	int status_stopfd;
	int abs_x_min;
	int abs_x_max;
	int wheel_range_deg;              /* 0 = unknown, defaults to 1080 */
	double status_last_time;
	double wheel_angle_deg;
	double wheel_velocity_deg_s;

	/* Session state */
	bool tf_initialized;       /* Init sequence sent since open */
	bool tf_paused;
	/*
	 * The 0x04+0x03 teardown pair has been sent and the host has been
	 * silent since: engine flushed and armed but unfed (the state every
	 * clean Windows session leaves the wheel in). Cleared when a stream
	 * packet resumes and by session init. Written under `lock`; read
	 * lock-free by the stream thread (same discipline as tf_paused).
	 */
	bool tf_armed_idle;
	unsigned tf_idle_ticks;    /* consecutive starved ticks at centre (stream thread only) */
	uint8_t tf_seq;            /* next outgoing packet sequence byte */

	/* Streaming state (managed by stream.c) */
	bool stream_running;
	bool shutting_down;        /* set during teardown so blocked producers wake and return */
	pthread_t stream_thread;
	int stream_timerfd;
	int stream_stopfd;         /* eventfd; signals the thread to exit */

	uint16_t tf_window[LOGITF_TF_WINDOW]; /* offset-binary, newest at [WINDOW-1] */
	uint16_t tf_last_current;             /* bytes 6-9 of each packet */

	/*
	 * Interface-2 feedback (device type-0x02 responses on ep 0x83).
	 * The stream thread drains them opportunistically each cycle;
	 * fields hold the most recent packet, under `lock`. fb_packets
	 * counts responses consumed since open (never reset), so callers
	 * can detect a stalled feedback path.
	 */
	bool     fb_valid;
	uint16_t fb_wheel_pos;     /* raw encoder, 0x8000 = centre */
	uint16_t fb_wheel_pos2;    /* ~1 sample older than fb_wheel_pos */
	uint32_t fb_counter;       /* device-side sample/timestamp counter */
	uint16_t fb_motor_raw;     /* undecoded (motor current/temperature?) */
	uint8_t  fb_status;        /* undecoded status byte */
	uint64_t fb_packets;

	pthread_mutex_t ring_lock;
	pthread_cond_t  ring_space;
	pthread_cond_t  ring_data;
	uint16_t ring[LOGITF_TF_RING];        /* offset-binary samples */
	unsigned ring_head;                    /* producer index (mod RING) */
	unsigned ring_tail;                    /* consumer index (mod RING) */

	pthread_mutex_t lock;      /* Protects mutable non-ring state */
};

struct logitf_device *logitf_table(void);

/* discovery.c */
int logitf_discover(void);        /* Scan sysfs, populate the table. Idempotent. */
int logitf_find_by_index(int index, struct logitf_device **out);

/* sysfs.c - helpers for reading/writing the kernel driver's wheel_*
 * attributes (wheel_range, wheel_damping, wheel_trueforce, ...). */
int logitf_sysfs_read_int(struct logitf_device *dev, const char *attr, int *out);
int logitf_sysfs_write_int(struct logitf_device *dev, const char *attr, int val);

/* session.c */
int logitf_session_ensure(struct logitf_device *dev);
int logitf_session_close(struct logitf_device *dev);

/* stream.c */
int  logitf_stream_start(struct logitf_device *dev);
int  logitf_stream_stop(struct logitf_device *dev);
int  logitf_stream_push_s16(struct logitf_device *dev, const int16_t *samples, int count);
int  logitf_stream_clear(struct logitf_device *dev);
int  logitf_stream_feedback_read(struct logitf_device *dev,
				 struct logitf_stream_feedback *fb);
/* Wire-shape builders, non-static so tests/unit.c can pin them. */
void logitf_build_stream_packet(uint8_t *pkt, uint8_t seq, uint16_t current,
				const uint16_t window[LOGITF_TF_WINDOW]);
void logitf_build_idle_packet(uint8_t *pkt, uint8_t seq, uint16_t current);
void logitf_build_ctrl_packet(uint8_t *pkt, uint8_t type, uint8_t seq);
/* 0x04 then 0x03, ~2 ms apart; caller holds dev->lock, no concurrent writer. */
int  logitf_tf_send_stop_pair(struct logitf_device *dev);

/* kf.c */
int    logitf_evdev_ensure_open(struct logitf_device *dev);
int    logitf_kf_set_torque_nm(struct logitf_device *dev, double torque_nm);
int    logitf_kf_clear(struct logitf_device *dev);
int    logitf_kf_close(struct logitf_device *dev);
double logitf_kf_get_torque_nm(struct logitf_device *dev);
double logitf_kf_max_continuous_nm(struct logitf_device *dev);
double logitf_kf_max_peak_nm(struct logitf_device *dev);

/* status.c */
int    logitf_status_start(struct logitf_device *dev);
int    logitf_status_stop(struct logitf_device *dev);
double logitf_status_angle_deg(struct logitf_device *dev);
double logitf_status_velocity_deg_s(struct logitf_device *dev);

/* Helper: convert float [-1.0, 1.0] to offset-binary u16. */
uint16_t logitf_float_to_wire(float sample);
uint16_t logitf_s16_to_wire(int16_t sample);

#endif /* LIBTRUEFORCE_INTERNAL_H */
