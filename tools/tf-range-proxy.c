/*
 * tf-range-proxy - answer the TrueForce SDK's rotation question correctly.
 *
 * The problem, established in issue #27. A sim asks Logitech's TrueForce SDK
 * how far the wheel turns. On Windows the SDK gets that from G HUB over a
 * local pipe. Under Proton nothing serves that pipe, the SDK gives up, and
 * what the game ends up using is 90 degrees: not a number anyone chose, but
 * the minimum of the wheel's legal 90-2700 range. The game then maps full
 * steering lock onto 45 degrees each way. Assetto Corsa Competizione clamps
 * there and will not steer past it.
 *
 * We cannot answer that pipe ourselves: the SDK checks that whoever serves
 * it is code-signed by Logitech, which we are not and will not pretend to
 * be. But we do not need to. The game loads the SDK through a CLSID our own
 * shim installer writes into the prefix, so we choose which library it gets.
 *
 * This is that library. It forwards all 52 other entry points straight to
 * Logitech's real DLL through PE export forwarding, so force feedback and
 * TrueForce behave exactly as before and pay nothing for passing through.
 * It implements only the four rotation getters, answering with the range the
 * wheel is actually set to, read from this driver's sysfs.
 *
 * To be clear about what this is not: it does not patch Logitech's binary,
 * does not bypass their signature check, and does not impersonate anything.
 * It supplies a value the SDK cannot obtain on a system where G HUB does not
 * exist.
 *
 * Install: this DLL goes where the shim installer points the CLSID, with
 * Logitech's own DLL beside it renamed trueforce_real.dll.
 *
 * Build:
 *   x86_64-w64-mingw32-gcc -O2 -shared -o trueforce_sdk_x64.dll \
 *       tools/tf-range-proxy.c tools/tf-range-proxy.def
 */

/* winsock2.h before windows.h, or windows.h pulls in the older
 * winsock.h and the two collide. */
#include <winsock2.h>
#include <windows.h>
#include <stdio.h>

#define _USE_MATH_DEFINES
#include <math.h>

/*
 * Where the kernel driver publishes the range. The direct-drive wheels call
 * it wheel_range; the G923 has no wheel_* attributes at all and calls the
 * same setting range. Both are read, in that order.
 */
#define SYSFS_GLOB "Z:\\sys\\class\\hidraw\\*"

static FILE *logfp;
static CRITICAL_SECTION loglock;
static int log_ready;

static void say(const char *fmt, ...)
{
	va_list ap;

	if (!log_ready)
		return;
	EnterCriticalSection(&loglock);
	OutputDebugStringA("tf-range-proxy: entered");
	if (logfp) {
		SYSTEMTIME t;
		GetLocalTime(&t);
		fprintf(logfp, "[%02d:%02d:%02d.%03d] ", t.wHour, t.wMinute,
			t.wSecond, t.wMilliseconds);
		va_start(ap, fmt);
		vfprintf(logfp, fmt, ap);
		va_end(ap);
		fprintf(logfp, "\n");
		fflush(logfp);
	}
	/*
	 * Also to the debug channel, always. A file needs somewhere writable
	 * and a person who knows where to look; this shows up in a Proton log
	 * with no cooperation from either, and is the difference between "the
	 * library did not load" and "the library loaded and could not say so".
	 */
	{
		char line[512];
		va_start(ap, fmt);
		vsnprintf(line, sizeof(line), fmt, ap);
		va_end(ap);
		OutputDebugStringA(line);
	}
	LeaveCriticalSection(&loglock);
}

/* Read one integer out of a sysfs file, or -1. */
static int read_int_file(const char *path)
{
	char buf[64];
	DWORD n = 0;
	HANDLE h;
	int v;

	h = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
			NULL, OPEN_EXISTING, 0, NULL);
	if (h == INVALID_HANDLE_VALUE)
		return -1;
	if (!ReadFile(h, buf, sizeof(buf) - 1, &n, NULL) || n == 0) {
		CloseHandle(h);
		return -1;
	}
	CloseHandle(h);
	buf[n] = 0;
	v = atoi(buf);
	return v > 0 ? v : -1;
}

/*
 * Walk the hidraw nodes for the first wheel that publishes a range. There is
 * normally one; a rig with two wheels takes the first, which is the same
 * choice every other tool here makes.
 */
static int wheel_range_degrees(void)
{
	WIN32_FIND_DATAA fd;
	HANDLE h;
	char path[MAX_PATH + 64];
	int v;

	h = FindFirstFileA(SYSFS_GLOB, &fd);
	if (h == INVALID_HANDLE_VALUE)
		return -1;
	do {
		if (fd.cFileName[0] == '.')
			continue;
		snprintf(path, sizeof(path),
			 "Z:\\sys\\class\\hidraw\\%s\\device\\wheel_range",
			 fd.cFileName);
		v = read_int_file(path);
		if (v > 0) {
			FindClose(h);
			return v;
		}
		snprintf(path, sizeof(path),
			 "Z:\\sys\\class\\hidraw\\%s\\device\\range",
			 fd.cFileName);
		v = read_int_file(path);
		if (v > 0) {
			FindClose(h);
			return v;
		}
	} while (FindNextFileA(h, &fd));
	FindClose(h);
	return -1;
}

/*
 * Signature taken from the real library's own code, not from a guess.
 * Disassembling logiWheelGetOperatingRangeDegrees shows RCX holding the
 * index, RDX null-checked as an out pointer, and 0x80000001 returned in EAX
 * when that pointer is null. So it reports through a parameter and returns a
 * status, and an earlier version of this file had it as a double-returning
 * one-argument call, which would have written nothing the caller could read
 * while returning a status it never set.
 */
#define LOGI_OK			0
#define LOGI_ERR_BAD_PARAM	0x80000001

__declspec(dllexport) int logiWheelGetOperatingRangeDegrees(int index, double *out)
{
	int v;

	if (!out)
		return LOGI_ERR_BAD_PARAM;
	v = wheel_range_degrees();
	if (v <= 0) {
		say("GetOperatingRangeDegrees(%d): no range in sysfs", index);
		return LOGI_ERR_BAD_PARAM;
	}
	*out = (double)v;
	say("GetOperatingRangeDegrees(%d) -> %d (from sysfs)", index, v);
	return LOGI_OK;
}

__declspec(dllexport) int logiWheelGetOperatingRangeRadians(int index, double *out)
{
	double deg;
	int r = logiWheelGetOperatingRangeDegrees(index, &deg);

	if (r == LOGI_OK && out)
		*out = deg * (M_PI / 180.0);
	return r;
}

__declspec(dllexport) int logiWheelGetOperatingRangeBoundsDegrees(int index,
								 double *lo,
								 double *hi)
{
	/*
	 * The wheel's own limits, not the current setting. A game that asks
	 * for the bounds and is told 90 to 90 has nowhere to put a range.
	 */
	if (!lo || !hi)
		return LOGI_ERR_BAD_PARAM;
	*lo = 90.0;
	*hi = 2700.0;
	say("GetOperatingRangeBoundsDegrees(%d): answering 90..2700", index);
	return LOGI_OK;
}

__declspec(dllexport) int logiWheelGetOperatingRangeBoundsRadians(int index,
								 double *lo,
								 double *hi)
{
	double a = 0, b = 0;
	int r = logiWheelGetOperatingRangeBoundsDegrees(index, &a, &b);

	if (r == LOGI_OK && lo && hi) {
		*lo = a * (M_PI / 180.0);
		*hi = b * (M_PI / 180.0);
	}
	return r;
}

/* ------------------------------------------------------------------
 * TrueForce capture
 *
 * Assetto Corsa Competizione and Assetto Corsa EVO produce real TrueForce
 * and hand it to this SDK continuously. On a direct-drive wheel Logitech's
 * own library drives the wheel with it. On a G923 it goes nowhere: that
 * generation's SDK path expects a G HUB agent, which does not exist on
 * Linux, so the samples are simply dropped and the owner gets no TrueForce
 * in the two games that actually ship it.
 *
 * The wheel is capable. This project already streams TrueForce to a G923
 * from synthesized telemetry in other titles, using the same packet format
 * the direct-drive wheels take. What was missing was the game's own data.
 *
 * So: every call below is forwarded to Logitech's library exactly as before
 * (direct-drive wheels see no change whatsoever), and on a G923 the samples
 * are additionally copied to logi-tf-sim over localhost UDP, which streams
 * them to the wheel's transport. If nothing is listening the sends fail
 * silently and the game is unaffected.
 *
 * "On a G923" is a gate, not a description of who happens to benefit. On a
 * direct-drive wheel Logitech's library streams these samples itself, so
 * relaying them to logi-tf-sim as well puts TWO writers on an endpoint that
 * carries one packet per millisecond: they do not share it, they take turns,
 * and the motor is square-modulated at 500 Hz. That is a real reported buzz,
 * root-caused on the wire, and it was reachable here because the copy was
 * unconditional while the comment above described it as a G923 arrangement.
 * See [`tf_capture_enabled`] for how the wheel is told apart.
 *
 * Wire format: userspace/logi-wheel/crates/logi-wheel-core/src/tfstream.rs.
 * That module owns the layout and its golden-bytes test; the constants here
 * are asserted against the same numbers below.
 * ------------------------------------------------------------------ */

#define TF_MAGIC0		'L'
#define TF_MAGIC1		'T'
#define TF_MAGIC2		'F'
#define TF_MAGIC3		'T'
#define TF_VERSION		1
#define TF_HEADER_LEN		8
#define TF_MAX_SAMPLES		256
#define TF_DEFAULT_PORT		20781

/* Kept honest against tfstream.rs rather than trusted to stay in step. */
typedef char tf_layout_assert[
	(TF_HEADER_LEN == 8 && TF_MAX_SAMPLES == 256 &&
	 TF_HEADER_LEN + TF_MAX_SAMPLES * 4 == 1032) ? 1 : -1];

static SOCKET tf_sock = INVALID_SOCKET;
static struct sockaddr_in tf_addr;

/*
 * Whether a wheel of each family is attached, by the sysfs attribute only
 * that family has: the direct-drive driver publishes wheel_range, the
 * classic (G923) engine publishes range and no wheel_* namespace at all.
 * Both are read through the same Z: view of /sys the range getters use.
 */
static void wheels_present(int *dd, int *classic)
{
	WIN32_FIND_DATAA fd;
	HANDLE h;
	char path[MAX_PATH + 64];

	*dd = 0;
	*classic = 0;
	h = FindFirstFileA(SYSFS_GLOB, &fd);
	if (h == INVALID_HANDLE_VALUE)
		return;
	do {
		if (fd.cFileName[0] == '.')
			continue;
		snprintf(path, sizeof(path),
			 "Z:\\sys\\class\\hidraw\\%s\\device\\wheel_range",
			 fd.cFileName);
		if (read_int_file(path) > 0) {
			*dd = 1;
			continue;
		}
		snprintf(path, sizeof(path),
			 "Z:\\sys\\class\\hidraw\\%s\\device\\range",
			 fd.cFileName);
		if (read_int_file(path) > 0)
			*classic = 1;
	} while (FindNextFileA(h, &fd));
	FindClose(h);
}

/*
 * Whether to copy the game's TrueForce to logi-tf-sim.
 *
 * LOGI_TF_CAPTURE decides it outright when set (logi-launch exports it from
 * the wheel it resolved for the session, which is the same answer it gives
 * every other helper, and a person testing by hand can set it too).
 *
 * Otherwise the wheels attached decide. A direct-drive wheel is already
 * being streamed to by Logitech's own library, so relaying is a second
 * writer on its endpoint and the answer is no, even if a G923 is attached as
 * well: on that rig the cost of relaying is a buzz on the wheel the game is
 * most likely being played on, and the cost of not relaying is a G923 with
 * no TrueForce, which is what it had before any of this existed. Only a rig
 * with a classic wheel and no direct-drive wheel relays by default.
 */
static int tf_capture_enabled(void)
{
	const char *env = getenv("LOGI_TF_CAPTURE");
	int dd = 0, classic = 0;

	if (env && *env)
		return *env != '0';
	wheels_present(&dd, &classic);
	if (dd) {
		say("TrueForce capture off: a direct-drive wheel is attached and "
		    "Logitech's library already streams to it");
		return 0;
	}
	if (!classic)
		say("TrueForce capture off: no classic wheel found to relay to");
	return classic;
}

/* Real entry points, resolved once so the forwards below still reach
 * Logitech's library now that these are implemented here instead of being
 * PE-forwarded. */
static int (*real_SetTorqueTFdouble)(int, const double *, int);
static int (*real_SetTorqueTFfloat)(int, const float *, int);
static int (*real_SetTorqueTFint32)(int, const int *, int);
static int (*real_SetTorqueTFint16)(int, const short *, int);
static int (*real_SetTorqueTFint8)(int, const signed char *, int);
static int (*real_SetStreamTF)(int, const void *, int);

static void tf_capture_init(HMODULE real)
{
	WSADATA wsa;

	real_SetTorqueTFdouble = (void *)GetProcAddress(real, "logiTrueForceSetTorqueTFdouble");
	real_SetTorqueTFfloat  = (void *)GetProcAddress(real, "logiTrueForceSetTorqueTFfloat");
	real_SetTorqueTFint32  = (void *)GetProcAddress(real, "logiTrueForceSetTorqueTFint32");
	real_SetTorqueTFint16  = (void *)GetProcAddress(real, "logiTrueForceSetTorqueTFint16");
	real_SetTorqueTFint8   = (void *)GetProcAddress(real, "logiTrueForceSetTorqueTFint8");
	real_SetStreamTF       = (void *)GetProcAddress(real, "logiTrueForceSetStreamTF");

	/*
	 * Decided once, here: the set of attached wheels does not change
	 * during a game, and a per-call sysfs walk on the audio path would
	 * cost more than the capture is worth. With the socket left closed
	 * tf_send() is a no-op, so the gate needs no second test anywhere.
	 */
	if (!tf_capture_enabled())
		return;
	if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0)
		return;
	tf_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
	if (tf_sock == INVALID_SOCKET)
		return;
	memset(&tf_addr, 0, sizeof(tf_addr));
	tf_addr.sin_family = AF_INET;
	tf_addr.sin_port = htons(TF_DEFAULT_PORT);
	tf_addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	say("TrueForce capture ready, forwarding to 127.0.0.1:%d", TF_DEFAULT_PORT);
}

/*
 * Send one run of normalized samples. Best effort throughout: a full socket
 * buffer or an absent daemon must never slow down or fail a call the game is
 * making on its audio path.
 */
static void tf_send(const float *samples, int count)
{
	unsigned char pkt[TF_HEADER_LEN + TF_MAX_SAMPLES * 4];
	int i, n;

	if (tf_sock == INVALID_SOCKET || !samples || count <= 0)
		return;
	while (count > 0) {
		n = count > TF_MAX_SAMPLES ? TF_MAX_SAMPLES : count;
		pkt[0] = TF_MAGIC0;
		pkt[1] = TF_MAGIC1;
		pkt[2] = TF_MAGIC2;
		pkt[3] = TF_MAGIC3;
		pkt[4] = TF_VERSION;
		pkt[5] = 0;
		pkt[6] = (unsigned char)(n & 0xff);
		pkt[7] = (unsigned char)((n >> 8) & 0xff);
		/* x86 and x86-64 are little-endian, and this DLL only ever
		 * runs there, so the float bytes go out as they sit. */
		for (i = 0; i < n; i++)
			memcpy(&pkt[TF_HEADER_LEN + i * 4], &samples[i], 4);
		sendto(tf_sock, (const char *)pkt, TF_HEADER_LEN + n * 4, 0,
		       (struct sockaddr *)&tf_addr, sizeof(tf_addr));
		samples += n;
		count -= n;
	}
}

/*
 * The integer variants carry the same waveform at a different scale, so
 * they are normalized to the same -1..1 the float ones already use. Full
 * scale of the source type is full scale of the wheel.
 */
#define TF_FORWARD_SCALED(SUFFIX, CTYPE, SCALE)                                \
__declspec(dllexport) int logiTrueForceSetTorqueTF##SUFFIX(int index,          \
							   const CTYPE *v,     \
							   int count)          \
{                                                                              \
	float buf[TF_MAX_SAMPLES];                                             \
	int i, n, done = 0;                                                    \
									       \
	if (v && count > 0) {                                                  \
		while (done < count) {                                         \
			n = count - done > TF_MAX_SAMPLES                      \
				  ? TF_MAX_SAMPLES : count - done;             \
			for (i = 0; i < n; i++)                                \
				buf[i] = (float)((double)v[done + i] / (SCALE));\
			tf_send(buf, n);                                       \
			done += n;                                             \
		}                                                              \
	}                                                                      \
	return real_SetTorqueTF##SUFFIX                                        \
		     ? real_SetTorqueTF##SUFFIX(index, v, count)               \
		     : LOGI_ERR_BAD_PARAM;                                     \
}

TF_FORWARD_SCALED(int32, int, 2147483647.0)
TF_FORWARD_SCALED(int16, short, 32767.0)
TF_FORWARD_SCALED(int8, signed char, 127.0)
TF_FORWARD_SCALED(double, double, 1.0)

__declspec(dllexport) int logiTrueForceSetTorqueTFfloat(int index,
							const float *v,
							int count)
{
	if (v && count > 0)
		tf_send(v, count);
	return real_SetTorqueTFfloat ? real_SetTorqueTFfloat(index, v, count)
				     : LOGI_ERR_BAD_PARAM;
}

/*
 * Stream configuration rather than samples: nothing to capture, but it has
 * to be implemented here because it can no longer be PE-forwarded once this
 * file exports its neighbours.
 */
__declspec(dllexport) int logiTrueForceSetStreamTF(int index, const void *cfg,
						   int len)
{
	return real_SetStreamTF ? real_SetStreamTF(index, cfg, len)
				: LOGI_ERR_BAD_PARAM;
}

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
	(void)inst;
	(void)reserved;
	if (reason == DLL_PROCESS_ATTACH) {
		char path[MAX_PATH + 32], *slash;

		InitializeCriticalSection(&loglock);
		log_ready = 1;
		OutputDebugStringA("tf-range-proxy: DllMain PROCESS_ATTACH");

		/*
		 * Load Logitech's library by absolute path, now, before anything
		 * asks for one of the fifty-two entry points that forward to it.
		 *
		 * A PE forward names its target by module name, and the loader
		 * resolves that through the ordinary search path. That path does
		 * not include this DLL's own directory, so with the game's
		 * working directory somewhere else every forwarded export failed
		 * with ERROR_PROC_NOT_FOUND while the four implemented here kept
		 * working. From the driver's seat that is the worst possible
		 * shape of failure: the rotation is fixed and the wheel goes
		 * completely dead (issue #27).
		 *
		 * Loading it here registers it under its base name, which is the
		 * name the forwards resolve through, so they find it already in
		 * memory rather than going looking.
		 */
		if (!GetModuleFileNameA(inst, path, MAX_PATH))
			return FALSE;
		slash = strrchr(path, '\\');
		if (!slash)
			return FALSE;
		/*
		 * The log goes beside the DLL rather than at C:\\. The prefix
		 * root is not always writable by the game, and a log that
		 * silently fails to appear is indistinguishable from a library
		 * that never loaded, which cost a whole test round (issue #27).
		 */
		strcpy(slash + 1, "tf-range-proxy.log");
		logfp = fopen(path, "a");
		say("--- attach ---");

		strcpy(slash + 1, "trueforce_real.dll");
		if (!LoadLibraryExA(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH)) {
			/*
			 * Refuse to load rather than load usefully-crippled.
			 *
			 * Without Logitech's library the fifty-four forwarded
			 * entry points cannot resolve, but the four answered
			 * here still can. A game then gets correct rotation and
			 * no force of any kind, which is a wheel that steers
			 * and does nothing, and that is how this was first
			 * reported (issue #27).
			 *
			 * Failing here instead leaves the game with no SDK at
			 * all: no TrueForce, but ordinary force feedback and a
			 * wheel that behaves. That is a state games already
			 * handle, because it is what everyone who has not
			 * installed the shim has.
			 */
			say("could not load %s (error %lu); refusing to load so "
			    "the game falls back to no SDK rather than to a "
			    "wheel with no forces",
			    path, (unsigned long)GetLastError());
			return FALSE;
		}
		say("loaded Logitech's library from %s", path);
		tf_capture_init(GetModuleHandleA("trueforce_real.dll"));
		say("wheel range from sysfs = %d", wheel_range_degrees());
	} else if (reason == DLL_PROCESS_DETACH) {
		if (logfp)
			fclose(logfp);
	}
	return TRUE;
}
