/*
 * libtrueforce - Kinetic Force (KF) routing via evdev FF_CONSTANT.
 *
 * The TF audio stream on interface 2 is one of two force channels
 * games use; the other is classic constant torque, which the
 * Windows SDK calls "Kinetic Force". Under Windows it reaches the
 * wheel via the DirectInput FFB path. On Linux we route KF through
 * the sibling joystick's evdev FF_CONSTANT effect, so kernel's
 * input_ff takes care of the 64-byte PID FFB packet on the same
 * interface 2 that hidraw doesn't own.
 *
 * We keep a single FF_CONSTANT effect uploaded per device and
 * update its level on every SetTorqueKF call.
 */

#include "internal.h"

#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <linux/input.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

/*
 * Per-model direct-drive motor torque ceilings (Nm), resolved from the
 * wheel's USB PID. The PEAK value is the scaling denominator in
 * logitf_kf_set_torque_nm(): a torque request maps to full-scale int16
 * at exactly the wheel's peak, and it also clamps inputs so a
 * misbehaving game cannot request more than the wheel can physically
 * produce. Getting it per-model is safety-relevant - scaling an 11 Nm
 * G PRO against an 8 Nm ceiling would command ~37% more torque than the
 * game asked for.
 *
 *   RS50  (c276):        8 Nm peak (datasheet / manual page 13)
 *   G PRO (c272 / c268): 11 Nm peak (Logitech published spec)
 *
 * The CONTINUOUS value is informational only (reported via a getter,
 * never used in scaling). The RS50's 5 Nm is from the manual; the
 * G PRO's is an estimate pending confirmation on real hardware - see
 * the "help wanted: G PRO torque" tracking issue.
 */
static double dd_peak_torque_nm(uint16_t pid)
{
	switch (pid) {
	case LOGITF_GPRO_XBOX_PID:
	case LOGITF_GPRO_PS_PID:
		return 11.0;
	case LOGITF_RS50_PID:
	default:
		return 8.0;
	}
}

static double dd_continuous_torque_nm(uint16_t pid)
{
	switch (pid) {
	case LOGITF_GPRO_XBOX_PID:
	case LOGITF_GPRO_PS_PID:
		return 6.9;	/* ESTIMATE - unconfirmed on G PRO hardware */
	case LOGITF_RS50_PID:
	default:
		return 5.0;
	}
}

int logitf_evdev_ensure_open(struct logitf_device *dev)
{
	if (dev->evdev_fd >= 0)
		return LOGITF_OK;
	if (dev->evdev_path[0] == '\0')
		return LOGITF_ERR_NOT_FOUND;
	dev->evdev_fd = open(dev->evdev_path, O_RDWR | O_CLOEXEC);
	if (dev->evdev_fd < 0) {
		int e = errno;

		if (e == EACCES || e == EPERM)
			return LOGITF_ERR_BUSY;
		return LOGITF_ERR_IO;
	}
	return LOGITF_OK;
}

static int kf_ensure_open(struct logitf_device *dev)
{
	int rc = logitf_evdev_ensure_open(dev);

	if (rc == LOGITF_OK && dev->kf_effect_id == 0)
		dev->kf_effect_id = -1;  /* signal "no effect yet" */
	return rc;
}

static int kf_upload(struct logitf_device *dev, int16_t level)
{
	struct ff_effect eff;
	int prev_id = dev->kf_effect_id;

	memset(&eff, 0, sizeof(eff));
	eff.type = FF_CONSTANT;
	eff.id = prev_id;             /* -1 = allocate new */
	eff.u.constant.level = level;
	eff.direction = 0x4000;       /* East = +X = full right */
	eff.replay.length = 0;        /* infinite until stopped */
	eff.replay.delay = 0;

	/*
	 * EVIOCSFF may leave eff.id untouched on failure; only commit
	 * the new id to dev->kf_effect_id when the kernel confirmed
	 * success. Otherwise we'd record a bogus id.
	 */
	if (ioctl(dev->evdev_fd, EVIOCSFF, &eff) < 0)
		return -errno;
	dev->kf_effect_id = eff.id;
	return 0;
}

static int kf_play(struct logitf_device *dev, int start)
{
	struct input_event ev;

	memset(&ev, 0, sizeof(ev));
	ev.type = EV_FF;
	ev.code = (uint16_t)dev->kf_effect_id;
	ev.value = start ? 1 : 0;
	if (write(dev->evdev_fd, &ev, sizeof(ev)) != sizeof(ev))
		return -errno;
	return 0;
}

int logitf_kf_set_torque_nm(struct logitf_device *dev, double torque_nm)
{
	int rc;
	int16_t level;
	double scaled, maxnm;

	pthread_mutex_lock(&dev->lock);
	rc = kf_ensure_open(dev);
	if (rc) {
		pthread_mutex_unlock(&dev->lock);
		return rc;
	}

	maxnm = dd_peak_torque_nm(dev->pid);
	/*
	 * A NaN request slips past both ordered comparisons below (every
	 * comparison with NaN is false), leaving scaled = NaN and making
	 * the (int16_t) cast undefined. A game whose own force maths
	 * produces NaN must not translate into an unbounded command to a
	 * direct-drive motor. Treat non-finite input as zero force.
	 * (+/-inf are already caught by the clamp, but this covers them
	 * too.)
	 */
	if (!isfinite(torque_nm))
		torque_nm = 0.0;
	if (torque_nm >  maxnm) torque_nm =  maxnm;
	if (torque_nm < -maxnm) torque_nm = -maxnm;
	scaled = torque_nm * 32767.0 / maxnm;
	level = (int16_t)scaled;

	if (kf_upload(dev, level) < 0) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	if (kf_play(dev, 1) < 0) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_ERR_IO;
	}
	dev->kf_last_nm = torque_nm;
	dev->kf_playing = true;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logitf_kf_clear(struct logitf_device *dev)
{
	pthread_mutex_lock(&dev->lock);
	if (dev->evdev_fd >= 0 && dev->kf_effect_id >= 0 && dev->kf_playing) {
		kf_play(dev, 0);
		dev->kf_playing = false;
		dev->kf_last_nm = 0.0;
	}
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logitf_kf_close(struct logitf_device *dev)
{
	pthread_mutex_lock(&dev->lock);
	if (dev->evdev_fd >= 0) {
		if (dev->kf_effect_id >= 0) {
			ioctl(dev->evdev_fd, EVIOCRMFF, (long)dev->kf_effect_id);
			dev->kf_effect_id = -1;
		}
		close(dev->evdev_fd);
		dev->evdev_fd = -1;
	}
	dev->kf_playing = false;
	dev->kf_last_nm = 0.0;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

double logitf_kf_get_torque_nm(struct logitf_device *dev)
{
	double v;

	pthread_mutex_lock(&dev->lock);
	v = dev->kf_last_nm;
	pthread_mutex_unlock(&dev->lock);
	return v;
}

double logitf_kf_max_continuous_nm(struct logitf_device *dev)
{
	return dd_continuous_torque_nm(dev->pid);
}

double logitf_kf_max_peak_nm(struct logitf_device *dev)
{
	return dd_peak_torque_nm(dev->pid);
}
