// SPDX-License-Identifier: LGPL-2.1-or-later
/*
 * libtrueforce - native Linux implementation of the Logitech Trueforce
 * SDK (trueforce_sdk_x64.dll, v1.3.11).
 *
 * Covers the RS50 wheel family. Talks to interface 2 via /dev/hidrawN.
 * Kinetic-force (KF) calls route through evdev /dev/input/eventX on the
 * same physical wheel; audio-haptic Trueforce (TF) samples stream
 * directly to the hidraw node at 1 kHz.
 *
 * The API mirrors the Windows SDK surface so the Wine PE shim can
 * forward calls with minimal translation.
 */

#ifndef LIBTRUEFORCE_TRUEFORCE_H
#define LIBTRUEFORCE_TRUEFORCE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LOGITF_MAX_CONTROLLERS 4

/* Return codes. Zero on success, negative errno-like on failure. */
#define LOGITF_OK 0
#define LOGITF_ERR_INVALID_ARG   -1
#define LOGITF_ERR_NOT_FOUND     -2
#define LOGITF_ERR_NOT_SUPPORTED -3
#define LOGITF_ERR_IO            -4
#define LOGITF_ERR_BUSY          -5

/* ---- Module lifecycle (loadable-library init) ---- */

int  dllOpen(void);
int  dllClose(void);

/* COM boilerplate; return success no-op on Linux. */
int  DllRegisterServer(void);
int  DllUnregisterServer(void);

/* ---- Discovery ---- */

int     logiTrueForceAvailable(bool *out);
int     logiTrueForceSupported(int index, bool *out);
bool logiTrueForceSupportedByDirectInputA(const void *di_device);
bool logiTrueForceSupportedByDirectInputW(const void *di_device);

bool logiWheelSupportedByDirectInputA(const void *di_device);
bool logiWheelSupportedByDirectInputW(const void *di_device);

/* ---- Session ---- */

int  logiWheelOpenByDirectInputA(const void *di_device);
int  logiWheelOpenByDirectInputW(const void *di_device);
int  logiWheelClose(int index);
int     logiWheelSdkHasControl(int index, bool *out);

/* ---- Versioning ---- */

int  logiWheelGetCoreLibraryVersion(int *major, int *minor, int *build);
/*
 * No index: the real library null-checks all three arguments, so all three
 * are pointers, exactly like logiWheelGetCoreLibraryVersion.
 */
int  logiWheelGetVersion(int *major, int *minor, int *build);

/* ---- Wheel operating range ---- */

int     logiWheelGetForceMode(int index);
int     logiWheelSetForceMode(int index, int mode);
/*
 * Report through an out parameter and return a status, matching the real
 * library. Verified against its own code: RCX carries the index, RDX is
 * null-checked as the out pointer, and 0x80000001 comes back in EAX when
 * that pointer is null. These were declared as double-returning
 * one-argument calls, which no caller written against the real SDK could
 * have used.
 */
/*
 * WARNING: the signatures in this header are not all verified against the
 * real library. Three that were checked turned out to be wrong, including
 * the two rotation getters below and logiTrueForceAvailable, whose first
 * argument is a pointer rather than an index. Anything not listed as
 * verified in docs/SDK_ABI_NOTES.md should be treated as unconfirmed, and
 * checked against the shipped DLL's machine code before being relied on.
 */

int     logiWheelGetOperatingRangeDegrees(int index, double *out);
int     logiWheelGetOperatingRangeRadians(int index, double *out);
int     logiWheelGetOperatingRangeBoundsDegrees(int index, double *lo, double *hi);
int     logiWheelGetOperatingRangeBoundsRadians(int index, double *lo, double *hi);
int     logiWheelSetOperatingRangeDegrees(int index, double degrees);
int     logiWheelSetOperatingRangeRadians(int index, double radians);

/* ---- RPM / LED capabilities ---- */

int  logiWheelGetRpmLedCaps(int index, int *caps);
int  logiWheelSetRpmLeds(int index, uint32_t rgb_mask);
int  logiWheelPlayLeds(int index, double current_rpm, double rpm_first_led, double rpm_redline);

/* ---- Angle & angular velocity ---- */

int     logiTrueForceGetAngleDegrees(int index, double *out);
int     logiTrueForceGetAngleRadians(int index, double *out);
int     logiTrueForceGetAngularVelocityDegrees(int index, double *out);
int     logiTrueForceGetAngularVelocityRadians(int index, double *out);

/* ---- Kinetic-force (classic constant torque) ---- */

int    logiTrueForceSetTorqueKF(int index, double torque_nm);
int     logiTrueForceGetTorqueKF(int index, double *out);
int    logiTrueForceSetTorqueKFPiecewise(int index, const double *samples, int count);
int    logiTrueForceClearKF(int index);
int    logiTrueForceSetGainKF(int index, double gain);
int     logiTrueForceGetGainKF(int index, double *out);
int     logiTrueForceGetMaxContinuousTorqueKF(int index, double *out);
int     logiTrueForceGetMaxPeakTorqueKF(int index, double *out);
int    logiTrueForceSetReconstructionFilterKF(int index, int level);
int    logiTrueForceGetReconstructionFilterKF(int index);

/* ---- Trueforce audio-haptic stream ----
 *
 * All SetTorqueTF* / SetStreamTF calls feed an internal ring that a
 * dedicated thread drains one packet per millisecond, four samples per
 * packet. They do not block: the queue is bounded by LATENCY (about
 * 128 ms of audio), and a caller pushing faster than the wheel consumes
 * loses the OLDEST queued samples rather than being made to wait. This
 * differs from the Windows SDK, whose SetTorque* calls are synchronous;
 * the back-pressure there is a game's own haptic thread pacing itself,
 * and on Linux the caller is as often a single loop that must not be
 * parked. A stream held to a fixed sample count instead was measured
 * queueing a full second of delay between the car and the rim.
 */

int    logiTrueForceSetTorqueTFdouble(int index, const double  *samples, int count);
int    logiTrueForceSetTorqueTFfloat (int index, const float   *samples, int count);
int    logiTrueForceSetTorqueTFint16 (int index, const int16_t *samples, int count);
int    logiTrueForceSetTorqueTFint32 (int index, const int32_t *samples, int count);
int    logiTrueForceSetTorqueTFint8  (int index, const int8_t  *samples, int count);
int    logiTrueForceSetStreamTF(int index, const int16_t *samples, int count);
double logiTrueForceGetTorqueTF(int index);
int    logiTrueForceGetTorqueTFRateBounds(int index, double *rate_min_hz, double *rate_max_hz);
int    logiTrueForceClearTF(int index);
int    logiTrueForceSetGainTF(int index, double gain);
int     logiTrueForceGetGainTF(int index, double *out);

/* ---- Damping ---- */

int    logiTrueForceSetDamping(int index, double damping);
int     logiTrueForceGetDamping(int index, double *out);
int     logiTrueForceGetDampingMax(int index, double *out);

/* ---- Haptic thread ---- */

int     logiTrueForceGetHapticRate(int index, double *out);
int    logiTrueForceGetHapticThreadStatus(int index);

/* ---- Pause/resume/sync ---- */

int  logiTrueForcePause(int index);
int  logiTrueForceResume(int index);
int     logiTrueForceIsPaused(int index, bool *out);
int  logiTrueForceSync(int index);

/* ---- Advanced ---- */

int  logiAdvancedGetThreadHandles(int index, void **handles, int max);

/* ---- Linux-native extensions (NOT part of the Windows SDK) ----
 *
 * The wheel answers every outgoing interface-2 packet with a type-0x02
 * response on ep 0x83 carrying real-time feedback: the wheel position
 * as the firmware sees it (matching the joystick axis, but sampled on
 * the same path and cadence as the Trueforce stream) and a device-side
 * counter. Useful for closed-loop haptic effects and for measuring the
 * wheel's consumption rate. The stream thread consumes these
 * opportunistically while it runs; without an active stream no
 * feedback is collected.
 */

struct logitf_stream_feedback {
	uint16_t wheel_position;   /* raw encoder, 0x8000 = centre */
	uint16_t wheel_position2;  /* ~1 sample older */
	uint32_t sample_counter;   /* device-side counter (bytes 13-16) */
	uint16_t motor_raw;        /* undecoded field (current/temperature?) */
	uint8_t  status;           /* undecoded status byte */
	uint64_t packets;          /* responses consumed since open */
};

/*
 * Latest feedback snapshot. Returns LOGITF_OK, LOGITF_ERR_NOT_FOUND
 * for a bad index, or LOGITF_ERR_BUSY if no response has been
 * consumed yet (stream not started, or the wheel has not answered).
 */
int logitf_get_stream_feedback(int index, struct logitf_stream_feedback *fb);

#ifdef __cplusplus
}
#endif

#endif /* LIBTRUEFORCE_TRUEFORCE_H */
