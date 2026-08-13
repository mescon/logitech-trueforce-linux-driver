// SPDX-License-Identifier: GPL-2.0-only
//
// A dinput8.dll that sits in front of Wine's builtin one so we can see what
// Logitech's TrueForce SDK sends through IDirectInputDevice8::Escape.
//
// Why this exists. On Windows, TrueForce reaches the wheel through the
// DirectInput vendor passthrough: the SDK loads, starts a haptic thread, and
// calls Escape on the game's device at a steady rate (187/sec measured in
// Assetto Corsa EVO on an RS50). Wine implements Escape as a stub that logs
// "stub!" and returns, so every one of those calls is discarded. That is why
// a correctly registered, correctly signed, successfully loaded SDK still
// produces nothing on Linux.
//
// This build only watches. It forwards every call to the real interface and
// records the escape command, the buffer sizes and the bytes, so the wire
// format can be read before anything is designed around it. Forwarding those
// bytes to the wheel is a separate step and deliberately not done here.
//
// It is safe to leave installed: with the log written and nothing rewritten,
// the game behaves exactly as it does without it.

#define DIRECTINPUT_VERSION 0x0800
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <dinput.h>
#include <dinputd.h>
#include <stdio.h>
#include <string.h>
#include <math.h>

static HMODULE g_real;                 // Wine's builtin dinput8
static FILE *g_log;
static CRITICAL_SECTION g_log_lock;
static bool g_log_ready;
static bool g_log_opened;

/// Whether the GetProcAddress hook installed: 1 yes, 0 no, -1 not tried.
///
/// Declared here rather than beside the hook because it is installed from
/// DllMain, where logging is unsafe, and reported on the first log line.
static int g_hook_result = -1;

typedef HRESULT(WINAPI *pfnDirectInput8Create)(HINSTANCE, DWORD, REFIID, LPVOID *,
                                               LPUNKNOWN);
typedef HRESULT(WINAPI *pfnDllGetClassObject)(REFCLSID, REFIID, LPVOID *);
typedef HRESULT(WINAPI *pfnDllCanUnloadNow)(void);

// ---------------------------------------------------------------- logging

static void log_open(void)
{
	// Beside this DLL, so it lands wherever the proxy was staged rather
	// than in the current directory, which a game is free to change.
	wchar_t path[MAX_PATH];
	HMODULE self = nullptr;
	GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
				   GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
			   (LPCWSTR)&log_open, &self);
	DWORD n = GetModuleFileNameW(self, path, MAX_PATH);
	if (!n || n >= MAX_PATH)
		return;
	wchar_t *slash = wcsrchr(path, L'\\');
	if (!slash)
		return;
	wcscpy(slash + 1, L"dinput8-escape.log");
	g_log = _wfopen(path, L"w");
	// No setvbuf here. The UCRT rejects a size of 0 for _IOLBF by calling
	// the invalid-parameter handler, which aborts the process before the
	// game's first instruction: "DINPUT8.dll failed to initialize"
	// (c0000417). Each line is flushed instead, which also means the log
	// survives a crash.
}

static void say(const char *fmt, ...)
{
	if (!g_log_ready)
		return;
	EnterCriticalSection(&g_log_lock);
	// Opened on first use rather than in DllMain: opening a file under
	// the loader lock is asking for trouble, and nothing needs logging
	// until DirectInput8Create runs, which is well clear of it.
	if (!g_log_opened) {
		g_log_opened = true;
		log_open();
		if (g_log) {
			fputs("--- dinput8 escape capture ---\n", g_log);
			fprintf(g_log,
				g_hook_result == 1 ?
					"watching which SDK entry points the game resolves\n" :
					"could not hook GetProcAddress: the handshake is not visible\n");
		}
	}
	if (g_log) {
		va_list ap;
		va_start(ap, fmt);
		vfprintf(g_log, fmt, ap);
		va_end(ap);
		fputc('\n', g_log);
		fflush(g_log);
	}
	LeaveCriticalSection(&g_log_lock);
}

// A hex line, capped: the interesting part of a haptic packet is its head,
// and at 187 calls a second an uncapped dump is unreadable and unbounded.
static void say_bytes(const char *label, const void *p, DWORD len)
{
	if (!p || !len) {
		say("    %s: (none)", label);
		return;
	}
	const unsigned char *b = (const unsigned char *)p;
	DWORD show = len > 64 ? 64 : len;
	char line[64 * 3 + 8];
	int at = 0;
	for (DWORD i = 0; i < show; i++)
		at += snprintf(line + at, sizeof(line) - at, "%02x ", b[i]);
	say("    %s (%lu bytes): %s%s", label, (unsigned long)len, line,
	    len > show ? "..." : "");
}

// ------------------------------------------------------------ relaying
//
// The escape payload is parameters, not a waveform: the synthesis that
// Logitech's Windows driver would do below this point is work we already
// have, in logi-tf-sim. So rather than invent a channel, speak the relay
// format that daemon already listens for on 127.0.0.1:20780, which is the
// same path logi-tf-relay uses from inside a prefix.

static SOCKET g_sock = INVALID_SOCKET;
static sockaddr_in g_dest;
static bool g_relay_tried;
static bool g_relay_off;
static char g_game_id[9];

// Which title this is, so the daemon's per-game settings apply. Anything
// unrecognised falls back to the shared "relay" switch rather than going
// silent, which is the daemon's own rule for an id it does not know.
static void resolve_game_id(void)
{
	static const struct {
		const wchar_t *exe;
		const char *id;
	} known[] = {
		{ L"AssettoCorsaEVO.exe", "ac-evo" },
		{ L"AC2-Win64-Shipping.exe", "acc" },
		{ L"acs.exe", "assetto" },
	};
	strcpy(g_game_id, "relay");
	wchar_t path[MAX_PATH];
	if (!GetModuleFileNameW(nullptr, path, MAX_PATH))
		return;
	const wchar_t *exe = wcsrchr(path, L'\\');
	exe = exe ? exe + 1 : path;
	for (size_t i = 0; i < sizeof(known) / sizeof(known[0]); i++) {
		if (!_wcsicmp(exe, known[i].exe)) {
			strcpy(g_game_id, known[i].id);
			return;
		}
	}
	say("unrecognised executable \"%ls\": relaying under the shared id", exe);
}

static bool relay_open(void)
{
	if (g_relay_tried)
		return g_sock != INVALID_SOCKET;
	g_relay_tried = true;

	// An escape hatch, because this proxy sits in the path of every game
	// it is staged into and must always be possible to neutralise
	// without uninstalling it.
	char off[8];
	if (GetEnvironmentVariableA("LOGI_ESCAPE_RELAY", off, sizeof(off)) && off[0] == '0') {
		g_relay_off = true;
		say("LOGI_ESCAPE_RELAY=0: capturing only, not relaying");
		return false;
	}

	WSADATA wsa;
	if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) {
		say("WSAStartup failed: not relaying");
		return false;
	}
	g_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
	if (g_sock == INVALID_SOCKET) {
		say("socket() failed (%d): not relaying", WSAGetLastError());
		return false;
	}
	// Overridable so the rate limiter and packet format can be exercised
	// against a test listener while the real daemon keeps the usual port.
	unsigned short port = 20780;
	char pbuf[8];
	if (GetEnvironmentVariableA("LOGI_ESCAPE_RELAY_PORT", pbuf, sizeof(pbuf))) {
		int p = atoi(pbuf);
		if (p > 0 && p < 65536)
			port = (unsigned short)p;
	}
	ZeroMemory(&g_dest, sizeof(g_dest));
	g_dest.sin_family = AF_INET;
	g_dest.sin_port = htons(port);
	g_dest.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	resolve_game_id();
	say("relaying telemetry to 127.0.0.1:20780 as \"%s\"", g_game_id);
	return true;
}

// The 28-byte relay datagram, laid out to match logi_wheel_core::relay.
// Throttle and gear are not carried by the escape payload; they are left at
// zero, which is what that format means by "the sender cannot tell".
static void relay_send(float rpm, float max_rpm)
{
	if (g_relay_off || !relay_open())
		return;

	// The SDK's haptic thread runs at 187/sec, three times the rate the
	// existing relays send at (logi-tf-relay sends every 16 ms) and far
	// more than an RPM figure needs. Send at that same 16 ms and drop the
	// samples between, so this producer looks like the one the daemon was
	// built and measured against rather than a new load on it.
	static ULONGLONG last_ms;
	ULONGLONG now = GetTickCount64();
	if (last_ms && now - last_ms < 16)
		return;
	last_ms = now;

	unsigned char pkt[28];
	ZeroMemory(pkt, sizeof(pkt));
	memcpy(pkt, "LTFR", 4);
	pkt[4] = 2; // wire version
	pkt[5] = 0; // flags: airborne unknown from here
	size_t n = strlen(g_game_id);
	memcpy(pkt + 6, g_game_id, n > 8 ? 8 : n);
	memcpy(pkt + 14, &rpm, 4);
	memcpy(pkt + 18, &max_rpm, 4);
	// 22..26 throttle, 26..28 gear: already zero.
	sendto(g_sock, (const char *)pkt, sizeof(pkt), 0, (sockaddr *)&g_dest, sizeof(g_dest));
}

// ------------------------------------- Logitech's OEM force-feedback driver
//
// On Windows, DirectInput does not talk to these wheels itself. It loads a
// per-device OEM effect driver named under
//
//   ...\MediaProperties\PrivateProperties\Joystick\OEM\VID_046D&PID_xxxx\OEMForceFeedback
//
// and routes force-feedback effects and Escape to its IDirectInputEffectDriver.
// For the HID++ wheels that driver is hidpp_forcefeedback_x64.dll, shipped by
// G HUB. Wine has no equivalent, which is why Escape goes nowhere and why
// force feedback depends on which backend Proton happened to pick.
//
// The driver itself runs under Wine: it instantiates and answers GetVersions.
// So the missing piece is only the routing, which is what this does.
//
// Off unless LOGI_OEM_FFB=1. It is unproven on hardware, and it puts a
// third-party driver in the path of every effect, so it must be asked for.

// "Logitech HID++ Force Feedback Device", from the DLL's own DllRegisterServer.
static const GUID CLSID_LogiHidppFF = { 0x62b43f0e,
					0xe7db,
					0x4329,
					{ 0x8c, 0x13, 0xa9, 0x66, 0xd8, 0x4a, 0x28, 0x9f } };

/// Product ids hidpp_forcefeedback_x64.dll actually claims.
///
/// Checked before DeviceID is ever called, because the driver does not
/// handle "no supported device": probed against an RS50 in native mode
/// (c276, absent from this list) it dereferences null and takes the process
/// with it. An RS50 has to be in compatibility mode (c272) to be driven
/// here at all, which is a property of Logitech's driver, not of Wine.
static const unsigned short OEM_FFB_PIDS[] = { 0xc262, 0xc268, 0xc26e, 0xc272 };

static bool oem_ffb_enabled(void)
{
	char v[8];
	return GetEnvironmentVariableA("LOGI_OEM_FFB", v, sizeof(v)) && v[0] == '1';
}

static bool oem_ffb_claims(unsigned short pid)
{
	for (size_t i = 0; i < sizeof(OEM_FFB_PIDS) / sizeof(OEM_FFB_PIDS[0]); i++)
		if (OEM_FFB_PIDS[i] == pid)
			return true;
	return false;
}


// ------------------------------- answering the operating-range questions
//
// Logitech's SDK faults on these. Measured in Assetto Corsa EVO: the game
// calls them and the exception unwinds from trueforce_sdk_x64+0x526b through
// GetOperatingRangeBoundsDegrees+0x60, and when that one is answered here
// instead, the next call lands in GetOperatingRangeDegrees+0x60. The same
// offset in both, so they share an inlined helper that dereferences
// something it does not have.
//
// A game that catches the exception survives and quietly abandons
// TrueForce, which is the silence we could not otherwise explain.
//
// The answers come from the wheel itself, the same way tools/tf-range-proxy.c
// gets them, so a game asking the range is told the truth rather than a
// constant.

static int read_int_file(const char *path)
{
	HANDLE h = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
			       OPEN_EXISTING, 0, nullptr);
	if (h == INVALID_HANDLE_VALUE)
		return -1;
	char buf[64];
	DWORD n = 0;
	int v = -1;
	if (ReadFile(h, buf, sizeof(buf) - 1, &n, nullptr) && n) {
		buf[n] = 0;
		v = atoi(buf);
	}
	CloseHandle(h);
	return v;
}

static int wheel_range_degrees(void)
{
	WIN32_FIND_DATAA fd;
	HANDLE h = FindFirstFileA("Z:\\sys\\class\\hidraw\\*", &fd);
	if (h == INVALID_HANDLE_VALUE)
		return -1;
	do {
		if (fd.cFileName[0] == '.')
			continue;
		char path[MAX_PATH + 64];
		static const char *const attrs[] = { "wheel_range", "range" };
		for (int a = 0; a < 2; a++) {
			const char *attr = attrs[a];
			snprintf(path, sizeof(path),
				 "Z:\\sys\\class\\hidraw\\%s\\device\\%s", fd.cFileName, attr);
			int v = read_int_file(path);
			if (v > 0) {
				FindClose(h);
				return v;
			}
		}
	} while (FindNextFileA(h, &fd));
	FindClose(h);
	return -1;
}

static const double RANGE_MIN_DEG = 90.0;
static const double RANGE_MAX_DEG = 2700.0;
static const double DEG_TO_RAD = 3.14159265358979323846 / 180.0;
static bool g_range_fix_off;

// int f(int index, double *out), verified in docs/SDK_ABI_NOTES.md.
static int range_degrees(int index, double *out)
{
	(void)index;
	if (!out)
		return 0x80000001;
	int v = wheel_range_degrees();
	if (v <= 0) {
		say("GetOperatingRangeDegrees: no range in sysfs, answering %.0f",
		    RANGE_MAX_DEG);
		v = (int)RANGE_MAX_DEG;
	}
	*out = (double)v;
	return 0;
}

static int range_radians(int index, double *out)
{
	int r = range_degrees(index, out);
	if (!r && out)
		*out *= DEG_TO_RAD;
	return r;
}

// int f(int index, double *lo, double *hi): the least and greatest range
// that can be set, 90 to 2700 per docs/PROTOCOL_SPECIFICATION.md.
static int range_bounds_degrees(int index, double *lo, double *hi)
{
	(void)index;
	if (!lo || !hi)
		return 0x80000001;
	*lo = RANGE_MIN_DEG;
	*hi = RANGE_MAX_DEG;
	return 0;
}

static int range_bounds_radians(int index, double *lo, double *hi)
{
	int r = range_bounds_degrees(index, lo, hi);
	if (!r) {
		*lo *= DEG_TO_RAD;
		*hi *= DEG_TO_RAD;
	}
	return r;
}


// logiWheelSetForceMode is the last call the game makes before going quiet.
//
// Signature from its own prologue in 1_3_12: RCX a 64-bit handle, and the
// second argument stored as a single byte (mov %dl,0x10(%rsp)), so a bool.
// Status in EAX, as with the rest of this family.
typedef int (*set_force_mode_fn)(void *handle, unsigned char mode);
static set_force_mode_fn g_setforcemode_real;

static int setforcemode_wrapper(void *handle, unsigned char mode)
{
	int st = g_setforcemode_real(handle, mode);
	say("logiWheelSetForceMode(handle=%p, mode=%u) -> 0x%08x%s", handle, (unsigned)mode,
	    (unsigned)st, st ? "   <- REFUSED" : "");
	return st;
}


// ------------------------------ the range setter, and the 90 degree clamp
//
// Something in this path sets the wheel to 90 degrees while a game runs. The
// kernel driver sees it and heals it back:
//
//   rotation range changed externally: 1080 -> 90 degrees
//   Rotation change broadcast -> 1080 degrees
//
// repeatedly, so the steering range flips under the game's force loop, which
// is enough on its own to make the wheel oscillate and the game stutter. This
// is issue #27's clamp, seen live.
//
// 90 is the least range these wheels accept, which is what a failed or
// defaulted lookup produces. Anything else the game asks for is its own
// choice and passes through untouched: the wheel must feel like the game
// intends, not like we prefer.
typedef int (*set_range_fn)(void *handle, double value);
static set_range_fn g_set_range_deg_real, g_set_range_rad_real;
static bool g_range_guard_off;

static const double RAD90 = 90.0 * 3.14159265358979323846 / 180.0;

static int set_range_deg_wrapper(void *handle, double deg)
{
	if (!g_range_guard_off && deg <= 90.5) {
		say("logiWheelSetOperatingRangeDegrees(%.1f) BLOCKED: that is the floor, "
		    "and it is what clamps the wheel to 90 degrees mid-session", deg);
		return 0;
	}
	int st = g_set_range_deg_real(handle, deg);
	say("logiWheelSetOperatingRangeDegrees(%.1f) -> 0x%08x", deg, (unsigned)st);
	return st;
}

static int set_range_rad_wrapper(void *handle, double rad)
{
	if (!g_range_guard_off && rad <= RAD90 * 1.005) {
		say("logiWheelSetOperatingRangeRadians(%.4f = %.1f deg) BLOCKED: the floor",
		    rad, rad * 180.0 / 3.14159265358979323846);
		return 0;
	}
	int st = g_set_range_rad_real(handle, rad);
	say("logiWheelSetOperatingRangeRadians(%.4f) -> 0x%08x", rad, (unsigned)st);
	return st;
}



static volatile LONGLONG g_kf_handle = 0;   // 64-bit SDK handle from the KF stream
typedef int (*set_torque_kf_fn)(long long handle, double torque);
static set_torque_kf_fn g_set_torque_kf_real;
static volatile LONGLONG g_kf_calls;

static int set_torque_kf_wrapper(long long handle, double torque)
{
	// Transparent pass-through: capture the handle, forward unchanged.
	InterlockedExchange64(&g_kf_handle, handle);
	InterlockedIncrement64((LONGLONG *)&g_kf_calls);
	return g_set_torque_kf_real ? g_set_torque_kf_real(handle, torque) : 0;
}


// ----------------------- native-topology TrueForce texture (option A)
//
// On Windows the same ep3 packet carries the base force (cur, from the
// game's KF torque) AND engine-texture audio samples; one writer, both
// payloads (measured: 79% of texture packets also carry live cur). The
// texture enters the SDK through its own audio API, pushed by G HUB's side,
// synthesised from the same Escape RPM stream the game emits. The game
// calls SetTorqueTF* zero times on either OS.
//
// This reproduces that topology exactly: synthesise the engine note from
// the live Escape RPM and push it into the game's own SDK session via
// logiTrueForceSetTorqueTFfloat. Logitech's DLL then does everything
// downstream - mixing, windowing, sequencing, packet assembly - so the
// whole stream is native machinery; only the waveform source is ours.
//
// The recipe is fitted to a Windows capture of this same game (see
// docs/TF_TEXTURE_RECIPE.md): a firing-frequency harmonic stack, h2/h3
// rising with revs, amplitude 0.7%..1.5% of fullscale. Ship tuning happens
// against capture diffs, not by ear.
//
// Off by default: the kernel-side texture merge (wheel_tf_merge) is the
// shipping path, and it renders on the wheel itself from the relayed RPM.
// LOGI_TF_TEXTURE=1 turns this in-session synth back on, for experiments
// only. The synth thread self-paces on the SDK's own ring (SetTorqueTF
// blocks when full, per the API contract).

typedef int (*set_torque_tf_float_fn)(long long handle, const float *samples, int count);
static set_torque_tf_float_fn g_set_tf_real;
static volatile LONG g_rpm_mhz;        // live rpm, millihertz-free: stored as rpm*10
static volatile LONG g_limiter_x10;
static volatile LONG g_texture_on = -1; // -1 unresolved, 0 off, 1 on
static HANDLE g_tex_thread;

static DWORD WINAPI texture_thread(LPVOID)
{
	// 4 kHz like the native stream; 32-sample blocks = 8 ms cadence.
	const float FS = 4000.0f;
	const int BLOCK = 32;
	float phase[5] = { 0, 0, 0, 0, 0 };
	float buf[BLOCK];
	// Capture-fitted gains at low revs -> high revs (h1..h5), interpolated
	// on the fundamental. docs/TF_TEXTURE_RECIPE.md.
	const float f_lo = 150.0f, f_hi = 330.0f;
	const float g_lo[5] = { 1.0f, 0.19f, 0.10f, 0.06f, 0.04f };
	const float g_hi[5] = { 1.0f, 0.28f, 0.30f, 0.09f, 0.07f };
	const float CYL_HALF = 4.0f;   // V8 firing: rpm/60*4; TODO per-car
	for (;;) {
		long long h = InterlockedCompareExchange64(&g_kf_handle, 0, 0);
		float rpm = (float)g_rpm_mhz / 10.0f;
		if (!h || !g_set_tf_real || rpm < 100.0f) {
			Sleep(20);
			continue;
		}
		float f0 = rpm / 60.0f * CYL_HALF;
		if (f0 > FS * 0.45f)
			f0 = FS * 0.45f;
		// interpolate gains + amplitude on f0
		float x = (f0 - f_lo) / (f_hi - f_lo);
		if (x < 0) x = 0;
		if (x > 1) x = 1;
		// rms 72 + 1.13*f0 counts of 32768 fullscale (capture fit)
		float rms = (72.0f + 1.13f * f0) / 32768.0f;
		float g[5], norm = 0;
		for (int k = 0; k < 5; k++) {
			g[k] = g_lo[k] + x * (g_hi[k] - g_lo[k]);
			norm += g[k] * g[k];
		}
		float scale = rms / (float)sqrt(norm / 2.0f); // sine rms = a/sqrt2
		const float TWO_PI = 6.28318530718f;
		for (int i = 0; i < BLOCK; i++) {
			float s = 0;
			for (int k = 0; k < 5; k++) {
				phase[k] += TWO_PI * f0 * (k + 1) / FS;
				if (phase[k] > TWO_PI)
					phase[k] -= TWO_PI;
				s += g[k] * (float)sin(phase[k]);
			}
			buf[i] = s * scale;
		}
		// Blocks when the SDK ring is full: exactly the self-pacing the
		// API documents, so no explicit clock is needed.
		g_set_tf_real(h, buf, BLOCK);
	}
	return 0;
}

static void texture_maybe_start(void)
{
	if (g_texture_on == -1) {
		char v[8];
		/* Kernel-side merge is the shipping texture path; this
		 * in-session synth stays available for experiments only. */
		g_texture_on = (GetEnvironmentVariableA("LOGI_TF_TEXTURE", v,
							sizeof(v)) &&
				v[0] == '1');
		if (!g_texture_on)
			say("texture synth off (kernel merge is the shipping path)");
	}
	if (!g_texture_on || g_tex_thread)
		return;
	if (!g_set_tf_real) {
		HMODULE m = GetModuleHandleW(L"trueforce_sdk_x64.dll");
		if (m)
			g_set_tf_real = (set_torque_tf_float_fn)GetProcAddress(
				m, "logiTrueForceSetTorqueTFfloat");
	}
	if (g_set_tf_real && InterlockedCompareExchange64(&g_kf_handle, 0, 0)) {
		g_tex_thread = CreateThread(nullptr, 0, texture_thread, nullptr, 0, nullptr);
		say("texture synth started (capture-fitted engine note into the SDK's own stream)");
	}
}

// --------------------------------- is the SDK's haptic thread even running?
//
// The SDK loads, opens the wheel, accepts KF torque at 190/sec, and writes
// nothing to endpoint 3. This asks the SDK directly whether its streaming
// (haptic) thread is alive, at what rate, and whether it is paused. All three
// have the same prologue in 1_3_12: RCX index, RDX a null-checked out pointer,
// status in EAX. Resolved from the already-loaded module, so no signature is
// guessed for a call the game did not make.
typedef int (*diag_fn)(long long handle, void *out);
static diag_fn g_thread_status, g_haptic_rate, g_is_paused;
static bool g_diag_done;

static void resolve_diag(void)
{
	if (g_thread_status)
		return;
	HMODULE m = GetModuleHandleW(L"trueforce_sdk_x64.dll");
	if (!m)
		return;
	g_thread_status = (diag_fn)GetProcAddress(m, "logiTrueForceGetHapticThreadStatus");
	g_haptic_rate   = (diag_fn)GetProcAddress(m, "logiTrueForceGetHapticRate");
	g_is_paused     = (diag_fn)GetProcAddress(m, "logiTrueForceIsPaused");
}

static DWORD WINAPI haptic_diag_thread(LPVOID)
{
	resolve_diag();
	long long h = InterlockedCompareExchange64(&g_kf_handle, 0, 0);
	if (!h) {
		say("HAPTIC diagnostic: no KF handle captured yet; skipping");
		return 0;
	}
	// Generous zeroed buffers; the functions write <= 8 bytes.
	unsigned char buf[16];
	if (g_thread_status) {
		memset(buf, 0, sizeof(buf));
		int st = g_thread_status(h, buf);
		say("HAPTIC thread status(h=%p): call=0x%08x  out.i32=%d out.i64=%lld out.f64=%.3f",
		    (void *)h, (unsigned)st, *(int *)buf, *(long long *)buf, *(double *)buf);
	} else {
		say("HAPTIC thread status: function not resolvable");
	}
	if (g_haptic_rate) {
		memset(buf, 0, sizeof(buf));
		int st = g_haptic_rate(h, buf);
		say("HAPTIC rate(h=%p): call=0x%08x  out.i32=%d out.f64=%.3f", (void *)h,
		    (unsigned)st, *(int *)buf, *(double *)buf);
	}
	if (g_is_paused) {
		memset(buf, 0, sizeof(buf));
		int st = g_is_paused(h, buf);
		say("HAPTIC is_paused(h=%p): call=0x%08x  out.i32=%d", (void *)h, (unsigned)st,
		    *(int *)buf);
	}
	return 0;
}

static void run_haptic_diagnostic(void)
{
	if (g_diag_done)
		return;
	g_diag_done = true;
	// Detached thread, so a call that blocks cannot freeze the game.
	HANDLE th = CreateThread(nullptr, 0, haptic_diag_thread, nullptr, 0, nullptr);
	if (th)
		CloseHandle(th);
}

// -------------------------------------------- watching the SDK handshake
//
// The wheel is visible, the SDK opens the right interface, and then sends
// nothing. What we cannot see from outside is the conversation between the
// game and the SDK: which entry points it resolves, and therefore what it
// intends to do. The SDK's signature is checked by the game, so it cannot be
// replaced; this DLL is not, and lives in the same process, so the game's own
// import of GetProcAddress can be redirected instead.
//
// This only watches. Every resolution is passed through untouched: no
// wrapper is returned in place of a real function, because the signatures of
// this family are only partly established (docs/SDK_ABI_NOTES.md) and a
// mismatched wrapper would crash the game rather than teach us anything.

static FARPROC(WINAPI *real_getprocaddress)(HMODULE, LPCSTR);

static LONG g_gpa_calls;

// --------------------------------------------- counting SDK calls safely
//
// The question the resolution list cannot answer: having resolved the whole
// API, does the game ever actually call the torque setters? If it does and
// nothing reaches the wheel, the SDK is failing internally; if it never
// does, the refusal happens earlier, in a capability query.
//
// Wrapping these in C would mean declaring their signatures, and this family
// is only partly established (docs/SDK_ABI_NOTES.md records seventeen
// declarations that were wrong in the same way). A wrapper with the wrong
// shape corrupts arguments or crashes.
//
// So each tracked export gets an assembly thunk that increments a counter
// and tail-jumps to the real function. It touches no argument register, no
// stack slot and no return path, so it is correct whatever the signature
// turns out to be. Only the flags are modified, at function entry, where
// nothing carries them.

#define TRACKED_CALLS 12

extern "C" {
volatile LONGLONG g_tf_calls[TRACKED_CALLS];
void *g_tf_real[TRACKED_CALLS];
void tf_thunk0(void);
void tf_thunk1(void);
void tf_thunk2(void);
void tf_thunk3(void);
void tf_thunk4(void);
void tf_thunk5(void);
void tf_thunk6(void);
void tf_thunk7(void);
void tf_thunk8(void);
void tf_thunk9(void);
void tf_thunk10(void);
void tf_thunk11(void);
}

#define TF_THUNK(n)                                                    \
	".globl tf_thunk" #n "\n"                                      \
	"tf_thunk" #n ":\n"                                            \
	"  lock incq g_tf_calls+" #n "*8(%rip)\n"                      \
	"  jmp *g_tf_real+" #n "*8(%rip)\n"

__asm__(".text\n" TF_THUNK(0) TF_THUNK(1) TF_THUNK(2) TF_THUNK(3) TF_THUNK(4)
		TF_THUNK(5) TF_THUNK(6) TF_THUNK(7) TF_THUNK(8) TF_THUNK(9)
			TF_THUNK(10) TF_THUNK(11));

static void *const g_tf_thunks[TRACKED_CALLS] = {
	(void *)tf_thunk0, (void *)tf_thunk1, (void *)tf_thunk2,  (void *)tf_thunk3,
	(void *)tf_thunk4, (void *)tf_thunk5, (void *)tf_thunk6,  (void *)tf_thunk7,
	(void *)tf_thunk8, (void *)tf_thunk9, (void *)tf_thunk10, (void *)tf_thunk11,
};

/// The calls worth knowing about, in the order the handshake uses them.
///
/// Counting the whole sequence rather than just the torque setters, because
/// "how far did it get" is the useful answer: an open that never happens and
/// a torque call that never happens point at completely different things.
static const char *const g_tracked[TRACKED_CALLS] = {
	// Every way this SDK can be handed a force, because watching two of
	// them and concluding "the game sends nothing" was not sound: the
	// float setter is one of five, and the KF family is separate again.
	"logiTrueForceSetTorqueTFdouble",
	"logiTrueForceSetTorqueTFfloat",
	"logiTrueForceSetTorqueTFint8",
	"logiTrueForceSetTorqueTFint16",
	"logiTrueForceSetTorqueTFint32",
	"logiTrueForceSetStreamTF",
	"logiTrueForceSetTorqueKF",
	"logiTrueForceSetTorqueKFPiecewise",
	// Housekeeping a streaming caller does around the setters.
	"logiTrueForceSync",
	"logiTrueForceClearTF",
	"logiTrueForceSetGainTF",
	// One anchor, so an all-zero table still proves the mechanism ran.
	"dllOpen",
};

static FARPROC WINAPI getprocaddress_hook(HMODULE mod, LPCSTR name)
{
	FARPROC p = real_getprocaddress(mod, name);
	InterlockedIncrement(&g_gpa_calls);
	// An ordinal import arrives as a small integer rather than a pointer.
	bool by_name = name && (ULONG_PTR)name > 0xffff;

	wchar_t path[MAX_PATH] = L"";
	GetModuleFileNameW(mod, path, MAX_PATH);
	const wchar_t *base = wcsrchr(path, L'\\');
	base = base ? base + 1 : path;
	// Log anything resolved out of a Logitech module as well as anything
	// with a Logitech-looking name: an earlier run logged only the latter
	// and could not distinguish "nothing was resolved" from "the hook
	// never ran", which are opposite conclusions.
	bool logi_module = wcsstr(path, L"trueforce") || wcsstr(path, L"Trueforce") ||
			   wcsstr(path, L"logi_steering") || wcsstr(path, L"wheel_sdk");
	if (logi_module || (by_name && !strncmp(name, "logi", 4))) {
		if (by_name)
			say("resolve %ls!%s -> %s", base, name, p ? "found" : "NOT FOUND");
		else
			say("resolve %ls!#%u -> %s", base, (unsigned)(ULONG_PTR)name,
			    p ? "found" : "NOT FOUND");
	}

	if (p && by_name && logi_module && !strcmp(name, "logiWheelSetForceMode")) {
		g_setforcemode_real = (set_force_mode_fn)p;
		say("    (wrapping logiWheelSetForceMode to report its argument and answer)");
		return (FARPROC)setforcemode_wrapper;
	}

	if (p && by_name && logi_module && !strcmp(name, "logiTrueForceSetTorqueKF")) {
		g_set_torque_kf_real = (set_torque_kf_fn)p;
		say("    (capturing SetTorqueKF index for the haptic diagnostic)");
		return (FARPROC)set_torque_kf_wrapper;
	}

	if (p && by_name && logi_module && !strcmp(name, "logiWheelSetOperatingRangeDegrees")) {
		g_set_range_deg_real = (set_range_fn)p;
		say("    (watching logiWheelSetOperatingRangeDegrees)");
		return (FARPROC)set_range_deg_wrapper;
	}
	if (p && by_name && logi_module && !strcmp(name, "logiWheelSetOperatingRangeRadians")) {
		g_set_range_rad_real = (set_range_fn)p;
		say("    (watching logiWheelSetOperatingRangeRadians)");
		return (FARPROC)set_range_rad_wrapper;
	}

	// The operating-range getters, answered here: the SDK faults on them.
	if (p && by_name && logi_module && !g_range_fix_off &&
	    !strncmp(name, "logiWheelGetOperatingRange", 26)) {
		bool bounds = strstr(name, "Bounds") != nullptr;
		bool radians = strstr(name, "Radians") != nullptr;
		say("    (answering %s here: the SDK faults on it)", name);
		if (bounds)
			return (FARPROC)(radians ? (void *)range_bounds_radians :
						   (void *)range_bounds_degrees);
		return (FARPROC)(radians ? (void *)range_radians : (void *)range_degrees);
	}

	// Hand back a counting thunk for the calls worth watching. Only for a
	// real resolution out of a Logitech module, so nothing else in the
	// process can be redirected by a coincidence of naming.
	if (p && by_name && logi_module) {
		for (int i = 0; i < TRACKED_CALLS; i++) {
			if (strcmp(name, g_tracked[i]) != 0)
				continue;
			g_tf_real[i] = (void *)p;
			say("    (counting calls to %s)", name);
			return (FARPROC)g_tf_thunks[i];
		}
	}
	return p;
}

// Redirect one imported function in the main executable's import table.
static bool hook_iat(const char *want, void *replacement, void **original)
{
	HMODULE base = GetModuleHandleW(nullptr);
	if (!base)
		return false;
	auto dos = (PIMAGE_DOS_HEADER)base;
	if (dos->e_magic != IMAGE_DOS_SIGNATURE)
		return false;
	auto nt = (PIMAGE_NT_HEADERS)((BYTE *)base + dos->e_lfanew);
	if (nt->Signature != IMAGE_NT_SIGNATURE)
		return false;
	auto dir = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
	if (!dir.VirtualAddress)
		return false;

	auto desc = (PIMAGE_IMPORT_DESCRIPTOR)((BYTE *)base + dir.VirtualAddress);
	for (; desc->Name; desc++) {
		// OriginalFirstThunk keeps the names; FirstThunk is the live
		// table the calls actually go through.
		if (!desc->OriginalFirstThunk)
			continue;
		auto names = (PIMAGE_THUNK_DATA)((BYTE *)base + desc->OriginalFirstThunk);
		auto addrs = (PIMAGE_THUNK_DATA)((BYTE *)base + desc->FirstThunk);
		for (; names->u1.AddressOfData; names++, addrs++) {
			if (names->u1.Ordinal & IMAGE_ORDINAL_FLAG)
				continue;
			auto imp = (PIMAGE_IMPORT_BY_NAME)((BYTE *)base +
							   names->u1.AddressOfData);
			if (strcmp((const char *)imp->Name, want) != 0)
				continue;
			DWORD old;
			if (!VirtualProtect(&addrs->u1.Function, sizeof(void *),
					    PAGE_READWRITE, &old))
				return false;
			*original = (void *)(ULONG_PTR)addrs->u1.Function;
			addrs->u1.Function = (ULONG_PTR)replacement;
			VirtualProtect(&addrs->u1.Function, sizeof(void *), old, &old);
			return true;
		}
	}
	return false;
}

static void watch_sdk_handshake(void)
{
	if (g_hook_result >= 0)
		return;
	// Safe here: this DLL is statically imported, so DllMain runs after
	// the executable's imports are resolved and before its entry point,
	// which is the only window that catches an SDK loaded during startup.
	// Installing it from DirectInput8Create was too late: the SDK loaded
	// about a second before input was initialised, and every resolution
	// had already happened.
	char rv[8];
	g_range_fix_off = GetEnvironmentVariableA("LOGI_RANGE_FIX", rv, sizeof(rv)) && rv[0] == '0';
	char gv[8];
	g_range_guard_off = GetEnvironmentVariableA("LOGI_RANGE_GUARD", gv, sizeof(gv)) && gv[0] == '0';
	g_hook_result = hook_iat("GetProcAddress", (void *)getprocaddress_hook,
				 (void **)&real_getprocaddress) ?
				1 :
				0;
}

// ------------------------------------------------------- device wrapper

class DeviceWrap : public IDirectInputDevice8W {
	IDirectInputDevice8W *m_real;
	LONG m_ref;
	// Escape arrives at haptic rate. Log every call while the format is
	// still unknown, then thin out, so a long session stays readable and
	// the file stays bounded.
	LONG m_escapes;

	// Logitech's OEM effect driver, when this device is one it claims and
	// the routing was asked for. Bound once, lazily: the device's ids are
	// only readable after the caller has it.
	IDirectInputEffectDriver *m_oem;
	DWORD m_oem_id;
	bool m_oem_tried;

	/// Read this device's USB ids, or 0 if DirectInput will not say.
	unsigned int vidpid(void)
	{
		DIPROPDWORD p;
		ZeroMemory(&p, sizeof(p));
		p.diph.dwSize = sizeof(p);
		p.diph.dwHeaderSize = sizeof(p.diph);
		p.diph.dwObj = 0;
		p.diph.dwHow = DIPH_DEVICE;
		if (FAILED(m_real->GetProperty(DIPROP_VIDPID, &p.diph)))
			return 0;
		return p.dwData;
	}

	/// Bind Logitech's effect driver to this device, once.
	void bind_oem(void)
	{
		if (m_oem_tried)
			return;
		m_oem_tried = true;
		if (!oem_ffb_enabled())
			return;

		unsigned int ids = vidpid();
		unsigned short vid = LOWORD(ids), pid = HIWORD(ids);
		if (vid != 0x046d) {
			say("OEM force feedback: %04x:%04x is not a Logitech device", vid, pid);
			return;
		}
		// Never call into the driver for a device it does not claim: it
		// crashes rather than returning an error.
		if (!oem_ffb_claims(pid)) {
			say("OEM force feedback: Logitech's driver does not claim %04x:%04x.",
			    vid, pid);
			say("  It claims c262, c268, c26e and c272 only. An RS50 in native");
			say("  mode (c276) has to be in compatibility mode (c272) to use it.");
			return;
		}

		// The game has already initialised COM by this point; join
		// whatever apartment it chose rather than forcing one.
		HRESULT hr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
		bool we_init = SUCCEEDED(hr);
		if (hr == RPC_E_CHANGED_MODE)
			we_init = false;

		IDirectInputEffectDriver *drv = nullptr;
		hr = CoCreateInstance(CLSID_LogiHidppFF, nullptr, CLSCTX_INPROC_SERVER,
				      IID_IDirectInputEffectDriver, (void **)&drv);
		if (FAILED(hr) || !drv) {
			say("OEM force feedback: driver not registered in this prefix (0x%08lx)",
			    (unsigned long)hr);
			say("  Install it with: tools/install-tf-shim.sh --oem-ffb");
			if (we_init)
				CoUninitialize();
			return;
		}

		DIDRIVERVERSIONS ver;
		ZeroMemory(&ver, sizeof(ver));
		ver.dwSize = sizeof(ver);
		if (SUCCEEDED(drv->GetVersions(&ver)))
			say("OEM force feedback: driver version 0x%lx",
			    (unsigned long)ver.dwFFDriverVersion);

		// External and internal id are ours to choose; the driver maps
		// them to whatever device it finds for itself.
		hr = drv->DeviceID(DIRECTINPUT_VERSION, 0, TRUE, 0, nullptr);
		if (FAILED(hr)) {
			say("OEM force feedback: DeviceID refused (0x%08lx)",
			    (unsigned long)hr);
			drv->Release();
			return;
		}
		m_oem = drv;
		m_oem_id = 0;
		say("OEM force feedback: bound to %04x:%04x, routing Escape to Logitech's driver",
		    vid, pid);
	}

public:
	DeviceWrap(IDirectInputDevice8W *real)
		: m_real(real), m_ref(1), m_escapes(0), m_oem(nullptr), m_oem_id(0),
		  m_oem_tried(false)
	{
	}

	// IUnknown
	HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void **out) override
	{
		// Hand back the wrapper for any device interface, or the SDK
		// would QI its way straight to the unwrapped one and we would
		// see nothing.
		if (riid == IID_IUnknown || riid == IID_IDirectInputDevice8W ||
		    riid == IID_IDirectInputDevice8A) {
			AddRef();
			*out = this;
			return S_OK;
		}
		return m_real->QueryInterface(riid, out);
	}
	ULONG STDMETHODCALLTYPE AddRef(void) override
	{
		return InterlockedIncrement(&m_ref);
	}
	ULONG STDMETHODCALLTYPE Release(void) override
	{
		LONG r = InterlockedDecrement(&m_ref);
		if (!r) {
			if (m_oem) {
				// Tell the driver the association is over before
				// dropping it, or it keeps the device open.
				m_oem->DeviceID(DIRECTINPUT_VERSION, m_oem_id, FALSE, m_oem_id,
						nullptr);
				m_oem->Release();
			}
			m_real->Release();
			delete this;
		}
		return r;
	}

	// IDirectInputDevice8: everything below is a pass-through except
	// Escape.
	HRESULT STDMETHODCALLTYPE GetCapabilities(LPDIDEVCAPS a) override
	{
		return m_real->GetCapabilities(a);
	}
	HRESULT STDMETHODCALLTYPE EnumObjects(LPDIENUMDEVICEOBJECTSCALLBACKW a, LPVOID b,
					      DWORD c) override
	{
		return m_real->EnumObjects(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE GetProperty(REFGUID a, LPDIPROPHEADER b) override
	{
		return m_real->GetProperty(a, b);
	}
	HRESULT STDMETHODCALLTYPE SetProperty(REFGUID a, LPCDIPROPHEADER b) override
	{
		return m_real->SetProperty(a, b);
	}
	HRESULT STDMETHODCALLTYPE Acquire(void) override { return m_real->Acquire(); }
	HRESULT STDMETHODCALLTYPE Unacquire(void) override { return m_real->Unacquire(); }
	HRESULT STDMETHODCALLTYPE GetDeviceState(DWORD a, LPVOID b) override
	{
		return m_real->GetDeviceState(a, b);
	}
	HRESULT STDMETHODCALLTYPE GetDeviceData(DWORD a, LPDIDEVICEOBJECTDATA b, LPDWORD c,
						DWORD d) override
	{
		return m_real->GetDeviceData(a, b, c, d);
	}
	HRESULT STDMETHODCALLTYPE SetDataFormat(LPCDIDATAFORMAT a) override
	{
		return m_real->SetDataFormat(a);
	}
	HRESULT STDMETHODCALLTYPE SetEventNotification(HANDLE a) override
	{
		return m_real->SetEventNotification(a);
	}
	HRESULT STDMETHODCALLTYPE SetCooperativeLevel(HWND a, DWORD b) override
	{
		return m_real->SetCooperativeLevel(a, b);
	}
	HRESULT STDMETHODCALLTYPE GetObjectInfo(LPDIDEVICEOBJECTINSTANCEW a, DWORD b,
						DWORD c) override
	{
		return m_real->GetObjectInfo(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE GetDeviceInfo(LPDIDEVICEINSTANCEW a) override
	{
		return m_real->GetDeviceInfo(a);
	}
	HRESULT STDMETHODCALLTYPE RunControlPanel(HWND a, DWORD b) override
	{
		return m_real->RunControlPanel(a, b);
	}
	HRESULT STDMETHODCALLTYPE Initialize(HINSTANCE a, DWORD b, REFGUID c) override
	{
		return m_real->Initialize(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE CreateEffect(REFGUID a, LPCDIEFFECT b, LPDIRECTINPUTEFFECT *c,
					       LPUNKNOWN d) override
	{
		HRESULT hr = m_real->CreateEffect(a, b, c, d);
		say("CreateEffect -> 0x%08lx", (unsigned long)hr);
		return hr;
	}
	HRESULT STDMETHODCALLTYPE EnumEffects(LPDIENUMEFFECTSCALLBACKW a, LPVOID b,
					      DWORD c) override
	{
		return m_real->EnumEffects(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE GetEffectInfo(LPDIEFFECTINFOW a, REFGUID b) override
	{
		return m_real->GetEffectInfo(a, b);
	}
	HRESULT STDMETHODCALLTYPE GetForceFeedbackState(LPDWORD a) override
	{
		return m_real->GetForceFeedbackState(a);
	}
	HRESULT STDMETHODCALLTYPE SendForceFeedbackCommand(DWORD a) override
	{
		return m_real->SendForceFeedbackCommand(a);
	}
	HRESULT STDMETHODCALLTYPE EnumCreatedEffectObjects(LPDIENUMCREATEDEFFECTOBJECTSCALLBACK a,
							   LPVOID b, DWORD c) override
	{
		return m_real->EnumCreatedEffectObjects(a, b, c);
	}

	// The one we are here for.
	HRESULT STDMETHODCALLTYPE Escape(LPDIEFFESCAPE e) override
	{
		LONG n = InterlockedIncrement(&m_escapes);
		bind_oem();
		// The streaming command repeats at haptic rate and its payload
		// is three floats, so it is logged decoded and thinned: often
		// enough to watch a value track the engine, rarely enough to
		// stay readable. Everything else is logged in full, since the
		// rest of the vocabulary is what we still do not know.
		bool stream = e && e->dwCommand == 0 && e->cbInBuffer == 20 && e->lpvInBuffer;
		bool loud = stream ? (n % 20) == 0 : (n <= 200 || (n % 1000) == 0);
		if (stream) {
			const unsigned char *b = (const unsigned char *)e->lpvInBuffer;
			unsigned int type;
			float f[3];
			memcpy(&type, b + 4, 4);
			memcpy(f, b + 8, 12);
			// Every sample is relayed, not only the logged ones: the
			// log is thinned to stay readable, the haptics are not.
			// The third field is the limiter, which is what the
			// daemon means by max_rpm; RPM can briefly exceed it.
			relay_send(f[0], f[2]);
			InterlockedExchange(&g_rpm_mhz, (LONG)(f[0] * 10.0f));
			InterlockedExchange(&g_limiter_x10, (LONG)(f[2] * 10.0f));
			texture_maybe_start();
			if (loud)
				say("Escape #%ld  stream type=%u  rpm=%.1f  b=%.1f  limit=%.1f",
				    (long)n, type, f[0], f[1], f[2]);
			// Once about a second in, so the handshake is visible
			// without playing, then roughly every 30 s. The
			// interesting answer is usually all zeros, which says
			// the game resolved the whole API and then never used
			// it, so it is reported even when nothing was called.
			if (n == 200)
				run_haptic_diagnostic();
			if (n == 200 || (n % 5600) == 0) {
				for (int i = 0; i < TRACKED_CALLS; i++)
					say("    calls: %-32s %lld", g_tracked[i],
					    (long long)g_tf_calls[i]);
			}
		} else if (loud && e) {
			say("Escape #%ld  command=0x%08lx  in=%lu out=%lu", (long)n,
			    (unsigned long)e->dwCommand, (unsigned long)e->cbInBuffer,
			    (unsigned long)e->cbOutBuffer);
			say_bytes("in", e->lpvInBuffer, e->cbInBuffer);
		} else if (loud) {
			say("Escape #%ld  (null escape struct)", (long)n);
		}
		// Where Windows would have sent it. Wine's own Escape reports
		// success while discarding the payload, so forwarding to it
		// instead of the driver is the same as dropping the call.
		if (m_oem) {
			HRESULT hr = m_oem->Escape(m_oem_id, 0, e);
			if (loud)
				say("    -> 0x%08lx (Logitech's driver)", (unsigned long)hr);
			return hr;
		}

		HRESULT hr = m_real->Escape(e);
		if (loud)
			say("    -> 0x%08lx (wine stub: discarded)", (unsigned long)hr);
		return hr;
	}

	HRESULT STDMETHODCALLTYPE Poll(void) override { return m_real->Poll(); }
	HRESULT STDMETHODCALLTYPE SendDeviceData(DWORD a, LPCDIDEVICEOBJECTDATA b, LPDWORD c,
						 DWORD d) override
	{
		return m_real->SendDeviceData(a, b, c, d);
	}
	HRESULT STDMETHODCALLTYPE EnumEffectsInFile(LPCWSTR a, LPDIENUMEFFECTSINFILECALLBACK b,
						    LPVOID c, DWORD d) override
	{
		return m_real->EnumEffectsInFile(a, b, c, d);
	}
	HRESULT STDMETHODCALLTYPE WriteEffectToFile(LPCWSTR a, DWORD b, LPDIFILEEFFECT c,
						    DWORD d) override
	{
		return m_real->WriteEffectToFile(a, b, c, d);
	}
	HRESULT STDMETHODCALLTYPE BuildActionMap(LPDIACTIONFORMATW a, LPCWSTR b, DWORD c) override
	{
		return m_real->BuildActionMap(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE SetActionMap(LPDIACTIONFORMATW a, LPCWSTR b, DWORD c) override
	{
		return m_real->SetActionMap(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE GetImageInfo(LPDIDEVICEIMAGEINFOHEADERW a) override
	{
		return m_real->GetImageInfo(a);
	}
};

// -------------------------------------------------- DirectInput8 wrapper

class DI8Wrap : public IDirectInput8W {
	IDirectInput8W *m_real;
	LONG m_ref;

public:
	DI8Wrap(IDirectInput8W *real) : m_real(real), m_ref(1) {}

	HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void **out) override
	{
		if (riid == IID_IUnknown || riid == IID_IDirectInput8W ||
		    riid == IID_IDirectInput8A) {
			AddRef();
			*out = this;
			return S_OK;
		}
		return m_real->QueryInterface(riid, out);
	}
	ULONG STDMETHODCALLTYPE AddRef(void) override { return InterlockedIncrement(&m_ref); }
	ULONG STDMETHODCALLTYPE Release(void) override
	{
		LONG r = InterlockedDecrement(&m_ref);
		if (!r) {
			m_real->Release();
			delete this;
		}
		return r;
	}

	HRESULT STDMETHODCALLTYPE CreateDevice(REFGUID guid, LPDIRECTINPUTDEVICE8W *out,
					       LPUNKNOWN outer) override
	{
		IDirectInputDevice8W *real = nullptr;
		HRESULT hr = m_real->CreateDevice(guid, &real, outer);
		if (FAILED(hr) || !real) {
			*out = real;
			return hr;
		}
		// Name the device so the log says which one the escapes go to.
		DIDEVICEINSTANCEW di;
		ZeroMemory(&di, sizeof(di));
		di.dwSize = sizeof(di);
		if (SUCCEEDED(real->GetDeviceInfo(&di)))
			say("CreateDevice: \"%ls\"", di.tszProductName);
		else
			say("CreateDevice: (name unavailable)");
		*out = new DeviceWrap(real);
		return hr;
	}
	HRESULT STDMETHODCALLTYPE EnumDevices(DWORD a, LPDIENUMDEVICESCALLBACKW b, LPVOID c,
					      DWORD d) override
	{
		return m_real->EnumDevices(a, b, c, d);
	}
	HRESULT STDMETHODCALLTYPE GetDeviceStatus(REFGUID a) override
	{
		return m_real->GetDeviceStatus(a);
	}
	HRESULT STDMETHODCALLTYPE RunControlPanel(HWND a, DWORD b) override
	{
		return m_real->RunControlPanel(a, b);
	}
	HRESULT STDMETHODCALLTYPE Initialize(HINSTANCE a, DWORD b) override
	{
		return m_real->Initialize(a, b);
	}
	HRESULT STDMETHODCALLTYPE FindDevice(REFGUID a, LPCWSTR b, LPGUID c) override
	{
		return m_real->FindDevice(a, b, c);
	}
	HRESULT STDMETHODCALLTYPE EnumDevicesBySemantics(LPCWSTR a, LPDIACTIONFORMATW b,
							 LPDIENUMDEVICESBYSEMANTICSCBW c,
							 LPVOID d, DWORD e) override
	{
		return m_real->EnumDevicesBySemantics(a, b, c, d, e);
	}
	HRESULT STDMETHODCALLTYPE ConfigureDevices(LPDICONFIGUREDEVICESCALLBACK a,
						   LPDICONFIGUREDEVICESPARAMSW b, DWORD c,
						   LPVOID d) override
	{
		return m_real->ConfigureDevices(a, b, c, d);
	}
};

// ------------------------------------------------------------- exports

// Load Wine's own dinput8 by absolute path. The override that makes the
// game pick us up is by module name, so an absolute system32 path still
// resolves to the builtin rather than back to this file.
static bool load_real(void)
{
	if (g_real)
		return true;
	wchar_t sys[MAX_PATH];
	UINT n = GetSystemDirectoryW(sys, MAX_PATH);
	if (!n || n >= MAX_PATH)
		return false;
	wcscat(sys, L"\\dinput8.dll");
	g_real = LoadLibraryExW(sys, nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
	if (!g_real)
		say("could not load %ls (error %lu): passing nothing through", sys,
		    (unsigned long)GetLastError());
	else
		say("loaded %ls", sys);
	return g_real != nullptr;
}

extern "C" __declspec(dllexport) HRESULT WINAPI DirectInput8Create(HINSTANCE inst, DWORD version,
								   REFIID riid, LPVOID *out,
								   LPUNKNOWN outer)
{
	if (!load_real())
		return E_FAIL;
	auto fn = (pfnDirectInput8Create)GetProcAddress(g_real, "DirectInput8Create");
	if (!fn)
		return E_FAIL;

	// Ask for the interface the caller asked for, so we never change the
	// contract; the A and W device vtables have the same layout, which is
	// what lets one wrapper serve both.
	// The executable's imports are resolved by now, and this runs well
	// before the SDK is loaded, so the hook is in place when the game
	// starts resolving its entry points.
	watch_sdk_handshake();

	void *real = nullptr;
	HRESULT hr = fn(inst, version, riid, &real, outer);
	say("DirectInput8Create(version=0x%lx) -> 0x%08lx  [%ld symbol lookups seen so far]",
	    (unsigned long)version, (unsigned long)hr, (long)g_gpa_calls);
	if (FAILED(hr) || !real) {
		*out = real;
		return hr;
	}
	*out = new DI8Wrap((IDirectInput8W *)real);
	return hr;
}

extern "C" __declspec(dllexport) HRESULT WINAPI DllGetClassObject(REFCLSID clsid, REFIID riid,
								  LPVOID *out)
{
	if (!load_real())
		return CLASS_E_CLASSNOTAVAILABLE;
	auto fn = (pfnDllGetClassObject)GetProcAddress(g_real, "DllGetClassObject");
	return fn ? fn(clsid, riid, out) : CLASS_E_CLASSNOTAVAILABLE;
}

extern "C" __declspec(dllexport) HRESULT WINAPI DllCanUnloadNow(void)
{
	if (!load_real())
		return S_FALSE;
	auto fn = (pfnDllCanUnloadNow)GetProcAddress(g_real, "DllCanUnloadNow");
	return fn ? fn() : S_FALSE;
}

extern "C" __declspec(dllexport) HRESULT WINAPI DllRegisterServer(void) { return S_OK; }
extern "C" __declspec(dllexport) HRESULT WINAPI DllUnregisterServer(void) { return S_OK; }

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
	(void)inst;
	(void)reserved;
	// Nothing here may touch the CRT beyond this, and nothing may open a
	// file: DllMain runs under the loader lock, and an abort here takes
	// the game with it before it draws a frame.
	if (reason == DLL_PROCESS_ATTACH) {
		DisableThreadLibraryCalls(inst);
		InitializeCriticalSection(&g_log_lock);
		g_log_ready = true;
		// Only memory reads and VirtualProtect: no CRT, no file, no
		// loader call, so it is safe under the loader lock.
		watch_sdk_handshake();
	}
	return TRUE;
}
