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
 * quarters the worst-case buffered latency this permits. It is no longer
 * what bounds the backlog: LOGITF_TF_MAX_PENDING below is, expressed as a
 * latency, and this is only the allocation it has to fit inside. Sizing a
 * haptic backlog by buffer capacity is the mistake that cost the G923 path
 * a third of every batch (see g923.rs MAX_PENDING) and cost this path a
 * full second of delay between the car and the rim.
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
 * Backlog bound, as a LATENCY rather than as a buffer size.
 *
 * The stream thread's own write()/poll()/read() syscalls make a tick
 * occasionally overrun its 1 ms slot, so the wire runs slightly behind the
 * producer: measured on an RS50 (2026-08-13) 3635 samples/sec reached the
 * wheel of the 4000 asked for. Against a ring bounded only by
 * LOGITF_TF_RING that deficit does not show up as a lost sample, it shows
 * up as a queue that fills to 1.02 s and stays there, which is a full
 * second of delay between the car and the rim, permanently. So the ring is
 * held to this many milliseconds of audio and the OLDEST samples are
 * dropped past it: nobody can feel a dropped millisecond, and everybody can
 * feel a second of delay.
 *
 * 128 ms rather than something tighter because the bound must sit ABOVE the
 * producer's worst-case single push, or it discards part of every burst on
 * arrival while the transport is perfectly healthy. logi-tf-sim renders at
 * most daemon.rs's MAX_GEN_MS (100 ms) of audio in one iteration and hands
 * it over in one call, so anything below that would load-shed during normal
 * play. logi-tf-sim's G923 transport derives its own bound from the same
 * 100 ms for the same reason (g923.rs MAX_PENDING_MS); the two agree at
 * 128 ms deliberately, so the two wheel families have the same worst-case
 * haptic delay.
 */
#define LOGITF_TF_MAX_PENDING_MS  128
#define LOGITF_TF_MAX_PENDING \
	((LOGITF_TF_MAX_PENDING_MS * LOGITF_TF_PKT_HZ * LOGITF_TF_NEW) / 1000)
_Static_assert(LOGITF_TF_MAX_PENDING < LOGITF_TF_RING - 1,
	       "the latency bound must fit inside the ring allocation");

/*
 * Most new samples one packet may carry while making up coalesced timer
 * expirations (see logitf_stream_tick_n).
 *
 * A tick that overruns its 1 ms slot makes the timerfd coalesce, and the
 * expirations it reports are sample time that has already passed: throwing
 * them away is what put the wire 9% behind the producer (3635 samples/sec
 * of 4000, ~1% of packets carrying no samples at all). Making them up means
 * one packet advancing the rolling window by more than LOGITF_TF_NEW, never
 * writing more than one packet per slot - the endpoint carries exactly one
 * per USB frame, and bursting into it is what the emit-once comment in
 * stream.c was right to avoid.
 *
 * Capped at 12, three slots' worth: the packet's window is
 * LOGITF_TF_WINDOW (13) samples, so anything past that cannot be expressed
 * on the wire at all. Whatever a longer stall leaves in the ring stays
 * there for the following ticks, bounded by LOGITF_TF_MAX_PENDING.
 *
 * Byte 10 carrying something other than 4 is not a guess: G Hub captures
 * show it as 5 (docs/TRUEFORCE_PROTOCOL.md, byte-10 notes), which is what
 * establishes it as a count rather than a constant. A value in 5..12 has
 * not itself been seen on a wire, so the cap stays inside what the window
 * can hold rather than being pushed further.
 */
#define LOGITF_TF_CATCHUP_MAX  (LOGITF_TF_NEW * 3)
_Static_assert(LOGITF_TF_CATCHUP_MAX <= LOGITF_TF_WINDOW,
	       "a packet cannot declare more new samples than its window holds");

/*
 * Silence gate: consecutive ring-starved ticks at centre force before the
 * stream thread sends the 0x04+0x03 teardown pair and goes silent
 * (whine-investigation.md H1: a session held open at zero force is what
 * whines, and Windows' answer is to not hold one). 500 ms sits inside the
 * report's suggested 250 ms..1 s band: long enough that a starved-but-live
 * producer never hits it, short enough that menu idling goes quiet fast.
 */
#define LOGITF_TF_IDLE_GRACE_TICKS  (LOGITF_TF_PKT_HZ / 2)

/*
 * Starvation decay: while the ring is empty, the held force (cur) does
 * NOT freeze forever - it steps toward centre so (a) a producer that
 * died mid-waveform cannot command a DC torque indefinitely, and (b)
 * the silence gate above, which requires cur==0x8000 exactly, can
 * eventually fire at all. Linear (a fixed per-tick step) rather than
 * exponential, so the worst case (a full-scale offset, 0 or 0xFFFF) is
 * bounded by a fixed tick count instead of an asymptote that only
 * approaches zero; deriving the step from that bound means decay needs
 * no extra per-session state beyond the held value itself. 40 ticks =
 * 40 ms sits at the top of the reviewed 20-50 ms band; smaller offsets
 * reach centre sooner.
 */
#define LOGITF_TF_STARVE_DECAY_TICKS  40
#define LOGITF_TF_STARVE_DECAY_STEP \
	((0x8000 + LOGITF_TF_STARVE_DECAY_TICKS - 1) / LOGITF_TF_STARVE_DECAY_TICKS)

/*
 * Consecutive starved ticks before the decay above starts and the rolling
 * window is flushed toward centre. Below this a starved tick HOLDS: window
 * untouched, cur repeated in the keepalive.
 *
 * Starvation at 1 kHz is not the same event as a producer dying. A
 * wall-clock-paced producer hands over one iteration's audio at a time
 * (logi-tf-sim: 17 ms at 60 Hz telemetry, up to 50 ms of poll timeout), so
 * the ring legitimately runs dry for the jitter between bursts, and 0.6% of
 * packet gaps were measured past 1.5 ms. Flushing on the first such tick
 * punched a four-sample hole of exact centre into the middle of otherwise
 * continuous audio, which is a discontinuity the rim can feel, where a hold
 * is inaudible.
 *
 * 8 ticks = 8 ms: several times the measured gap jitter, well short of one
 * producer iteration, and it delays the safety property (a producer that
 * died mid-waveform cannot command a stale force forever) by 8 ms on top of
 * the 40 ms decay, keeping the total inside the same ~50 ms band the decay
 * bound was reviewed against.
 */
#define LOGITF_TF_STARVE_HOLD_TICKS  8

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
	/*
	 * tf_paused, tf_armed_idle and tf_teardown_pending are read AND
	 * written exclusively under `lock`, including by the stream
	 * thread's per-tick hot path: a mutex lock/unlock at 1 kHz is
	 * cheap and almost always uncontended, and nothing else in this
	 * codebase uses C11 _Atomic, so plain bool-under-lock is the
	 * consistent choice rather than a one-off atomic type here.
	 *
	 * tf_armed_idle: the 0x04+0x03 teardown pair has been sent and the
	 * host has been silent since: engine flushed and armed but unfed
	 * (the state every clean Windows session leaves the wheel in).
	 * Cleared when a stream packet resumes and by session init.
	 *
	 * tf_teardown_pending / tf_teardown_done implement single-emitter
	 * ordering for the pair: logiTrueForcePause() sets the flag and
	 * waits on the condvar (with a timeout) instead of writing the
	 * pair itself; the stream thread is the only caller that ever
	 * reaches logitf_tf_send_stop_pair while a session is live (every
	 * other caller has already joined that thread first), so the pair
	 * can never be split or doubled across two writers. See stream.c.
	 */
	bool tf_paused;
	bool tf_armed_idle;
	bool tf_teardown_pending;
	unsigned tf_idle_ticks;    /* consecutive starved ticks at centre (stream thread only) */
	unsigned tf_starved_ticks; /* consecutive ticks that drained nothing (stream thread only) */
	uint8_t tf_seq;            /* next outgoing packet sequence byte */

	/* Streaming state (managed by stream.c) */
	bool stream_running;
	bool shutting_down;        /* set during teardown so producers stop pushing */
	pthread_t stream_thread;
	int stream_timerfd;
	int stream_stopfd;         /* eventfd; signals the thread to exit */
	/*
	 * Why the stream thread stopped on its own, negative errno, 0 while
	 * healthy. Written by the thread under `lock` just before it exits and
	 * read by logitf_stream_push_s16, which turns it into the LOGITF_ERR_IO
	 * every push API already documents: an unplugged wheel makes poll()
	 * report POLLERR|POLLHUP forever and every tick return -ENODEV, and
	 * with both discarded the thread spun a core at 100% while the caller
	 * kept being told its pushes had succeeded.
	 */
	int stream_error;

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
	/*
	 * Samples dropped to hold the backlog to LOGITF_TF_MAX_PENDING, since
	 * the stream started. A running total that is not zero means the
	 * producer outran this transport; reported rather than silent, because
	 * load-shedding on a force path otherwise looks exactly like a healthy
	 * stream. ring_drop_warn_sec rate-limits the running report (monotonic
	 * seconds); both are ring_lock state.
	 */
	uint64_t ring_dropped;
	long     ring_drop_warn_sec;

	pthread_mutex_t lock;      /* Protects mutable non-ring state */
	pthread_cond_t  tf_teardown_done; /* paired with `lock`; see tf_teardown_pending above */
};

struct logitf_device *logitf_table(void);

/* discovery.c */
int logitf_discover(void);        /* Scan sysfs, populate the table. Idempotent. */
int logitf_find_by_index(int index, struct logitf_device **out);
/*
 * Check that dev->hidraw_path still names THIS wheel's TF interface, and
 * look for it again under dev->usb_root when it does not. Returns 0 when the
 * path is usable, -1 when the wheel is gone. See the definition for why an
 * unchecked cached path is a correctness problem and not just a stale one.
 */
int logitf_reresolve_hidraw(struct logitf_device *dev);

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
/* Wire-format builders, non-static so tests/unit.c can pin them.
 *
 * new_count is byte 10, "new samples this packet": how many of the window's
 * newest slots the caller has just filled from the ring. It is a real count
 * and not the constant LOGITF_TF_NEW - a partial drain used to claim four
 * fresh samples while up to three of them were repeats of the last one. */
void logitf_build_stream_packet(uint8_t *pkt, uint8_t seq, uint16_t current,
				const uint16_t window[LOGITF_TF_WINDOW],
				uint8_t new_count);
void logitf_build_idle_packet(uint8_t *pkt, uint8_t seq, uint16_t current);
void logitf_build_ctrl_packet(uint8_t *pkt, uint8_t type, uint8_t seq);
/*
 * 0x04 then 0x03, ~2 ms apart. Caller holds dev->lock on entry; the
 * function releases it for the inter-packet sleep and re-takes it
 * before returning, so it is always safe to unlock right after this
 * call the same way as before. No-ops (returns LOGITF_OK immediately)
 * if tf_armed_idle is already set. Only ever called by the stream
 * thread while a session is live, or by a caller that has already
 * joined it (see tf_teardown_pending in internal.h's struct comment).
 */
int  logitf_tf_send_stop_pair(struct logitf_device *dev);
/*
 * One streaming tick: drain the ring, update the rolling window, emit
 * a packet. Non-static so tests/unit.c can drive it directly against
 * a pipe standing in for hidraw, without a real timerfd/thread.
 */
int  logitf_stream_tick(struct logitf_device *dev);
/*
 * The same, servicing `expiries` timer expirations in ONE packet: it drains
 * up to expiries * LOGITF_TF_NEW samples (capped at LOGITF_TF_CATCHUP_MAX)
 * and advances the rolling window by however many it got. Called with more
 * than 1 only when the timerfd coalesced, i.e. when a previous tick overran
 * its slot; logitf_stream_tick is exactly this with expiries = 1.
 */
int  logitf_stream_tick_n(struct logitf_device *dev, unsigned expiries);

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
