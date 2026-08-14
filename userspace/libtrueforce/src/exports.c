// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - public API surface.
 *
 * Discovery/availability, session open/close, Kinetic Force (KF) torque,
 * the TrueForce (TF) stream, gain and pause are all live. The remaining
 * stubs are the DirectInput-open-style entry points, which return
 * LOGITF_ERR_NOT_SUPPORTED so callers get a predictable answer.
 *
 * The module-init pair (dllOpen/dllClose) triggers a discovery scan
 * so that the index-based availability calls answer correctly without
 * requiring the caller to do anything else first.
 */

#include "internal.h"

#include <math.h>
#include <stddef.h>
#include <time.h>

/* ---- Module lifecycle ---- */

int dllOpen(void)
{
	return logitf_discover();
}

int dllClose(void)
{
	struct logitf_device *t = logitf_table();

	for (int i = 0; i < LOGITF_MAX_CONTROLLERS; i++) {
		if (!t[i].in_use)
			continue;
		logitf_stream_stop(&t[i]);
		logitf_status_stop(&t[i]);
		logitf_kf_close(&t[i]);
		logitf_session_close(&t[i]);
		pthread_mutex_destroy(&t[i].lock);
		pthread_mutex_destroy(&t[i].ring_lock);
		pthread_cond_destroy(&t[i].ring_space);
		pthread_cond_destroy(&t[i].ring_data);
		t[i].in_use = false;
	}
	return LOGITF_OK;
}

int DllRegisterServer(void)
{
	/* COM no-op on Linux. Windows SDK returns S_OK (0) here. */
	return 0;
}

int DllUnregisterServer(void)
{
	return 0;
}

/* ---- Discovery / availability ---- */

int logiTrueForceAvailable(bool *out)
{
	struct logitf_device *dev;

	/*
	 * One argument, not an index: the real library null-checks its first
	 * argument and returns a bad-parameter status, so it is the out
	 * pointer (see docs/SDK_ABI_NOTES.md).
	 */
	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	*out = logitf_find_by_index(0, &dev) == LOGITF_OK;
	return LOGITF_OK;
}

int logiTrueForceSupported(int index, bool *out)
{
	(void)index;
	return logiTrueForceAvailable(out);
}

bool logiTrueForceSupportedByDirectInputA(const void *di_device)
{
	(void)di_device;
	/* Phase 22.2 will map the DI GUID to a library index. */
	return false;
}

bool logiTrueForceSupportedByDirectInputW(const void *di_device)
{
	(void)di_device;
	return false;
}

bool logiWheelSupportedByDirectInputA(const void *di_device)
{
	(void)di_device;
	return false;
}

bool logiWheelSupportedByDirectInputW(const void *di_device)
{
	(void)di_device;
	return false;
}

/* ---- Session (stubs) ---- */

int logiWheelOpenByDirectInputA(const void *di_device)
{
	(void)di_device;
	return LOGITF_ERR_NOT_SUPPORTED;
}

int logiWheelOpenByDirectInputW(const void *di_device)
{
	(void)di_device;
	return LOGITF_ERR_NOT_SUPPORTED;
}

int logiWheelClose(int index)
{
	struct logitf_device *dev;

	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_INVALID_ARG;
	logitf_stream_stop(dev);
	logitf_status_stop(dev);
	logitf_kf_clear(dev);
	logitf_kf_close(dev);
	logitf_session_close(dev);
	return LOGITF_OK;
}

int logiWheelSdkHasControl(int index, bool *out)
{
	(void)index;
	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	*out = false;
	return LOGITF_OK;
}

/* ---- Versioning ---- */

int logiWheelGetCoreLibraryVersion(int *major, int *minor, int *build)
{
	if (major) *major = 1;
	if (minor) *minor = 3;
	if (build) *build = 11;
	return LOGITF_OK;
}

int logiWheelGetVersion(int *major, int *minor, int *build)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(0, &dev);

	if (rc)
		return rc;
	/* Phase 22.2 queries the device via HID++ for real firmware info. */
	if (major) *major = 0;
	if (minor) *minor = 0;
	if (build) *build = 0;
	return LOGITF_OK;
}

/* ---- Force mode (stubs) ---- */

int logiWheelGetForceMode(int index) { (void)index; return LOGITF_ERR_NOT_SUPPORTED; }
int logiWheelSetForceMode(int index, int mode) { (void)index; (void)mode; return LOGITF_ERR_NOT_SUPPORTED; }

/* ---- Operating range ---- */

/*
 * Operating range forwards to the kernel driver's wheel_range sysfs
 * attribute. The driver accepts 90..2700 integer degrees; games
 * typically pass 540, 900, 1080 etc. We clamp and round.
 */

/* Windows SDK 0..1 float to kernel 0..100 integer percent. */
static int unit_to_percent(double v)
{
	if (v < 0.0) v = 0.0;
	if (v > 1.0) v = 1.0;
	return (int)(v * 100.0 + 0.5);
}

int logiWheelGetOperatingRangeDegrees(int index, double *out)
{
	struct logitf_device *dev;
	int v;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	/*
	 * The direct-drive wheels expose wheel_range; the G923 has no wheel_*
	 * attributes at all and calls the same setting "range".
	 */
	if (logitf_sysfs_read_int(dev, "wheel_range", &v) == 0 ||
	    logitf_sysfs_read_int(dev, "range", &v) == 0) {
		*out = (double)v;
		return 0;
	}
	return LOGITF_ERR_NOT_SUPPORTED;
}

int logiWheelGetOperatingRangeRadians(int index, double *out)
{
	double deg;
	int st;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	st = logiWheelGetOperatingRangeDegrees(index, &deg);
	if (st == LOGITF_OK)
		*out = deg * (M_PI / 180.0);
	return st;
}

int logiWheelGetOperatingRangeBoundsDegrees(int index, double *lo, double *hi)
{
	double r;
	int st = logiWheelGetOperatingRangeDegrees(index, &r);

	/*
	 * The bounds of the operating range, meaning the least and greatest
	 * range that can be set, not the rim's angular extremes. This returned
	 * -range/2 to +range/2 on the strength of nothing; see
	 * docs/SDK_ABI_NOTES.md for why the other reading is the right one.
	 */
	(void)r;
	if (st != LOGITF_OK) {
		if (lo) *lo = 0;
		if (hi) *hi = 0;
		return LOGITF_ERR_NOT_SUPPORTED;
	}
	if (lo) *lo = 90.0;
	if (hi) *hi = 2700.0;
	return LOGITF_OK;
}

int logiWheelGetOperatingRangeBoundsRadians(int index, double *lo, double *hi)
{
	double lod = 0.0, hid = 0.0;
	int rc = logiWheelGetOperatingRangeBoundsDegrees(index, &lod, &hid);

	if (rc != LOGITF_OK) {
		if (lo) *lo = 0;
		if (hi) *hi = 0;
		return rc;
	}
	if (lo) *lo = lod * (M_PI / 180.0);
	if (hi) *hi = hid * (M_PI / 180.0);
	return LOGITF_OK;
}

int logiWheelSetOperatingRangeDegrees(int index, double deg)
{
	struct logitf_device *dev;
	int v;

	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_INVALID_ARG;
	v = (int)(deg + 0.5);
	if (v < 90) v = 90;
	if (v > 2700) v = 2700;
	if (logitf_sysfs_write_int(dev, "wheel_range", v) < 0)
		return LOGITF_ERR_IO;
	return LOGITF_OK;
}

int logiWheelSetOperatingRangeRadians(int index, double rad)
{
	return logiWheelSetOperatingRangeDegrees(index, rad * (180.0 / M_PI));
}

/* ---- RPM / LEDs (stubs) ---- */

int logiWheelGetRpmLedCaps(int index, int *caps) { (void)index; if (caps) *caps = 0; return LOGITF_ERR_NOT_SUPPORTED; }
int logiWheelSetRpmLeds(int index, uint32_t rgb_mask) { (void)index; (void)rgb_mask; return LOGITF_ERR_NOT_SUPPORTED; }
int logiWheelPlayLeds(int index, double rpm, double first, double red) { (void)index; (void)rpm; (void)first; (void)red; return LOGITF_ERR_NOT_SUPPORTED; }

/* ---- Angle / velocity ---- */

static struct logitf_device *angle_dev(int index)
{
	struct logitf_device *dev;

	if (logitf_find_by_index(index, &dev))
		return NULL;
	if (!dev->status_running && logitf_status_start(dev) != LOGITF_OK)
		return NULL;
	return dev;
}

int logiTrueForceGetAngleDegrees(int index, double *out)
{
	struct logitf_device *dev = angle_dev(index);

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (!dev)
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_status_angle_deg(dev);
	return LOGITF_OK;
}

int logiTrueForceGetAngleRadians(int index, double *out)
{
	double deg;
	int st;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	st = logiTrueForceGetAngleDegrees(index, &deg);
	if (st == LOGITF_OK)
		*out = deg * (M_PI / 180.0);
	return st;
}

int logiTrueForceGetAngularVelocityDegrees(int index, double *out)
{
	struct logitf_device *dev = angle_dev(index);

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (!dev)
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_status_velocity_deg_s(dev);
	return LOGITF_OK;
}

int logiTrueForceGetAngularVelocityRadians(int index, double *out)
{
	double deg;
	int st;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	st = logiTrueForceGetAngularVelocityDegrees(index, &deg);
	if (st == LOGITF_OK)
		*out = deg * (M_PI / 180.0);
	return st;
}

/* ---- Kinetic force ---- */

int logiTrueForceSetTorqueKF(int index, double torque_nm)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);

	if (rc)
		return rc;
	return logitf_kf_set_torque_nm(dev, torque_nm);
}

int logiTrueForceGetTorqueKF(int index, double *out)
{
	struct logitf_device *dev;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_kf_get_torque_nm(dev);
	return LOGITF_OK;
}

int logiTrueForceClearKF(int index)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);

	if (rc)
		return rc;
	return logitf_kf_clear(dev);
}

int logiTrueForceGetMaxContinuousTorqueKF(int index, double *out)
{
	struct logitf_device *dev;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_kf_max_continuous_nm(dev);
	return LOGITF_OK;
}

int logiTrueForceGetMaxPeakTorqueKF(int index, double *out)
{
	struct logitf_device *dev;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_kf_max_peak_nm(dev);
	return LOGITF_OK;
}

/*
 * Piecewise, gain, and reconstruction-filter hooks are noops for
 * now. Games that use them only add polish on top of the base
 * torque; basic FFB works without any of these.
 */
int    logiTrueForceSetTorqueKFPiecewise(int index, const double *s, int n) { (void)index; (void)s; (void)n; return LOGITF_ERR_NOT_SUPPORTED; }
int    logiTrueForceSetGainKF(int index, double g) { (void)index; (void)g; return LOGITF_OK; }
int    logiTrueForceGetGainKF(int index, double *out)
	{ (void)index; if (!out) return LOGITF_ERR_INVALID_ARG; *out = 1.0; return LOGITF_OK; }
int    logiTrueForceSetReconstructionFilterKF(int index, int level) { (void)index; (void)level; return LOGITF_OK; }
int    logiTrueForceGetReconstructionFilterKF(int index) { (void)index; return 0; }

/* ---- Trueforce audio stream (stubs) ---- */

/*
 * TF setters: first call triggers lazy session init and starts the
 * streaming thread; subsequent calls push samples into the thread's
 * ring buffer. All sample formats convert to s16 offset-binary on
 * their way to the wire.
 */
static int tf_ensure_stream(int index, struct logitf_device **out)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);

	if (rc)
		return rc;
	rc = logitf_session_ensure(dev);
	if (rc)
		return rc;
	rc = logitf_stream_start(dev);
	if (rc)
		return rc;
	*out = dev;
	return LOGITF_OK;
}

/*
 * Chunk size for converting caller buffers into s16 batches before
 * pushing to the ring. Bounded so a game passing a very long sample
 * array can't blow the stack with alloca() or force a large malloc.
 */
#define TF_CONVERT_CHUNK 512

int logiTrueForceSetTorqueTFfloat(int index, const float *samples, int count)
{
	struct logitf_device *dev;
	int rc = tf_ensure_stream(index, &dev);

	if (rc)
		return rc;
	if (!samples || count <= 0)
		return LOGITF_ERR_INVALID_ARG;

	int16_t buf[TF_CONVERT_CHUNK];

	for (int off = 0; off < count; off += TF_CONVERT_CHUNK) {
		int n = count - off;

		if (n > TF_CONVERT_CHUNK)
			n = TF_CONVERT_CHUNK;
		for (int i = 0; i < n; i++) {
			float v = samples[off + i];

			if (v >  1.0f) v =  1.0f;
			if (v < -1.0f) v = -1.0f;
			buf[i] = (int16_t)(v * 32767.0f);
		}
		rc = logitf_stream_push_s16(dev, buf, n);
		if (rc)
			return rc;
	}
	return LOGITF_OK;
}

int logiTrueForceSetTorqueTFdouble(int index, const double *samples, int count)
{
	struct logitf_device *dev;
	int rc = tf_ensure_stream(index, &dev);

	if (rc)
		return rc;
	if (!samples || count <= 0)
		return LOGITF_ERR_INVALID_ARG;

	int16_t buf[TF_CONVERT_CHUNK];

	for (int off = 0; off < count; off += TF_CONVERT_CHUNK) {
		int n = count - off;

		if (n > TF_CONVERT_CHUNK)
			n = TF_CONVERT_CHUNK;
		for (int i = 0; i < n; i++) {
			double v = samples[off + i];

			if (v >  1.0) v =  1.0;
			if (v < -1.0) v = -1.0;
			buf[i] = (int16_t)(v * 32767.0);
		}
		rc = logitf_stream_push_s16(dev, buf, n);
		if (rc)
			return rc;
	}
	return LOGITF_OK;
}

int logiTrueForceSetTorqueTFint16(int index, const int16_t *samples, int count)
{
	struct logitf_device *dev;
	int rc = tf_ensure_stream(index, &dev);

	if (rc)
		return rc;
	return logitf_stream_push_s16(dev, samples, count);
}

int logiTrueForceSetTorqueTFint32(int index, const int32_t *samples, int count)
{
	struct logitf_device *dev;
	int rc = tf_ensure_stream(index, &dev);

	if (rc)
		return rc;
	if (!samples || count <= 0)
		return LOGITF_ERR_INVALID_ARG;

	int16_t buf[TF_CONVERT_CHUNK];

	for (int off = 0; off < count; off += TF_CONVERT_CHUNK) {
		int n = count - off;

		if (n > TF_CONVERT_CHUNK)
			n = TF_CONVERT_CHUNK;
		for (int i = 0; i < n; i++) {
			int32_t v = samples[off + i];

			if (v >  32767)  v =  32767;
			if (v < -32768)  v = -32768;
			buf[i] = (int16_t)v;
		}
		rc = logitf_stream_push_s16(dev, buf, n);
		if (rc)
			return rc;
	}
	return LOGITF_OK;
}

int logiTrueForceSetTorqueTFint8(int index, const int8_t *samples, int count)
{
	struct logitf_device *dev;
	int rc = tf_ensure_stream(index, &dev);

	if (rc)
		return rc;
	if (!samples || count <= 0)
		return LOGITF_ERR_INVALID_ARG;

	int16_t buf[TF_CONVERT_CHUNK];

	for (int off = 0; off < count; off += TF_CONVERT_CHUNK) {
		int n = count - off;

		if (n > TF_CONVERT_CHUNK)
			n = TF_CONVERT_CHUNK;
		for (int i = 0; i < n; i++)
			buf[i] = (int16_t)((int)samples[off + i] * 256);
		rc = logitf_stream_push_s16(dev, buf, n);
		if (rc)
			return rc;
	}
	return LOGITF_OK;
}

int logiTrueForceSetStreamTF(int index, const int16_t *samples, int count)
{
	return logiTrueForceSetTorqueTFint16(index, samples, count);
}

int logiTrueForceClearTF(int index)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);

	if (rc)
		return rc;
	return logitf_stream_clear(dev);
}
double logiTrueForceGetTorqueTF(int index) { (void)index; return 0.0; }
int    logiTrueForceGetTorqueTFRateBounds(int index, double *lo, double *hi) { (void)index; if (lo) *lo = 1000.0; if (hi) *hi = 1000.0; return LOGITF_OK; }
/*
 * TF output gain forwards to the kernel's wheel_trueforce sysfs
 * attribute, which accepts 0..100 integer percent. The Windows SDK
 * convention for this value is 0..1 float, so we scale.
 */
int logiTrueForceSetGainTF(int index, double g)
{
	struct logitf_device *dev;

	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_sysfs_write_int(dev, "wheel_trueforce", unit_to_percent(g)) < 0)
		return LOGITF_ERR_IO;
	return LOGITF_OK;
}

int logiTrueForceGetGainTF(int index, double *out)
{
	struct logitf_device *dev;
	int v;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	/* Unreadable is not an error here: full gain is the wheel's default. */
	*out = logitf_sysfs_read_int(dev, "wheel_trueforce", &v) < 0
	     ? 1.0 : (double)v / 100.0;
	return LOGITF_OK;
}

/* ---- Damping ---- */

/*
 * Damping forwards to the kernel's wheel_damping sysfs attribute,
 * which accepts 0..100 integer percent. Windows SDK damping is a
 * 0..1 float; scale accordingly.
 */
int logiTrueForceSetDamping(int index, double d)
{
	struct logitf_device *dev;

	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_sysfs_write_int(dev, "wheel_damping", unit_to_percent(d)) < 0)
		return LOGITF_ERR_IO;
	return LOGITF_OK;
}

int logiTrueForceGetDamping(int index, double *out)
{
	struct logitf_device *dev;
	int v;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	*out = logitf_sysfs_read_int(dev, "wheel_damping", &v) < 0
	     ? 0.0 : (double)v / 100.0;
	return LOGITF_OK;
}

int    logiTrueForceGetDampingMax(int index, double *out)
	{ (void)index; if (!out) return LOGITF_ERR_INVALID_ARG; *out = 1.0; return LOGITF_OK; }

/* ---- Haptic thread (stubs) ---- */

int    logiTrueForceGetHapticRate(int index, double *out)
	{ (void)index; if (!out) return LOGITF_ERR_INVALID_ARG; *out = 1000.0; return LOGITF_OK; }
int    logiTrueForceGetHapticThreadStatus(int index) { (void)index; return 0; }

/* ---- Pause / resume ---- */

/* Pause's wait for the stream thread to service a teardown request:
 * comfortably more than one tick (1 ms), short enough not to be felt
 * by a caller. Purely defensive - tf_initialized true means the
 * stream thread is running (session_ensure and stream_start are
 * always paired), so the normal case is serviced within one tick. */
#define TF_PAUSE_TEARDOWN_TIMEOUT_NS  (50L * 1000 * 1000)

int logiTrueForcePause(int index)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);
	struct timespec deadline;

	if (rc)
		return rc;

	pthread_mutex_lock(&dev->lock);
	if (dev->tf_paused) {
		pthread_mutex_unlock(&dev->lock);
		return LOGITF_OK;
	}
	dev->tf_paused = true;

	/*
	 * Going silent with the engine armed is the abort-capture state
	 * (whine-investigation.md holder #2): the firmware treats the gap
	 * as host-idle and the started engine keeps whining. Pause the way
	 * a Windows session ends instead: 0x04+0x03, then silence. Resume
	 * needs no re-arm - the pair's trailing 0x03 left the engine armed,
	 * and the stream thread clears tf_armed_idle on its next packet.
	 *
	 * The stream thread is the single emitter of that pair (it may
	 * also be about to send it on its own account, from the
	 * starvation gate): request it here instead of writing the bytes
	 * ourselves, which is what used to let a concurrent gate trip
	 * double the pair, or let an in-flight stream packet land after
	 * the pair instead of before it. logitf_tf_send_stop_pair's own
	 * armed-idle guard makes the request a no-op if the gate already
	 * won the race.
	 */
	if (dev->tf_initialized && !dev->tf_armed_idle) {
		dev->tf_teardown_pending = true;
		clock_gettime(CLOCK_REALTIME, &deadline);
		deadline.tv_nsec += TF_PAUSE_TEARDOWN_TIMEOUT_NS;
		if (deadline.tv_nsec >= 1000000000L) {
			deadline.tv_nsec -= 1000000000L;
			deadline.tv_sec += 1;
		}
		while (dev->tf_teardown_pending &&
		       pthread_cond_timedwait(&dev->tf_teardown_done,
					      &dev->lock, &deadline) == 0)
			;
		/* Don't leave a stale request for a future stream thread to
		 * trip on if the wait above timed out. */
		dev->tf_teardown_pending = false;
	}
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logiTrueForceResume(int index)
{
	struct logitf_device *dev;
	int rc = logitf_find_by_index(index, &dev);

	if (rc)
		return rc;
	pthread_mutex_lock(&dev->lock);
	dev->tf_paused = false;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logiTrueForceIsPaused(int index, bool *out)
{
	struct logitf_device *dev;

	if (!out)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	pthread_mutex_lock(&dev->lock);
	*out = dev->tf_paused;
	pthread_mutex_unlock(&dev->lock);
	return LOGITF_OK;
}

int logiTrueForceSync(int index)
{
	(void)index;
	return LOGITF_OK;
}

/* ---- Advanced (stub) ---- */

int logiAdvancedGetThreadHandles(int index, void **handles, int max)
{
	(void)index;
	if (handles && max > 0)
		handles[0] = NULL;
	return 0; /* Zero handles exposed. */
}

/* ---- Linux-native extensions ---- */

int logitf_get_stream_feedback(int index, struct logitf_stream_feedback *fb)
{
	struct logitf_device *dev;

	if (!fb)
		return LOGITF_ERR_INVALID_ARG;
	if (logitf_find_by_index(index, &dev))
		return LOGITF_ERR_NOT_FOUND;
	return logitf_stream_feedback_read(dev, fb);
}
