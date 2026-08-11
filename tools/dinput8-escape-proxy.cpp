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
#include <stdio.h>
#include <string.h>

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

public:
	DeviceWrap(IDirectInputDevice8W *real) : m_real(real), m_ref(1), m_escapes(0) {}

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
			if (loud)
				say("Escape #%ld  stream type=%u  rpm=%.1f  b=%.1f  limit=%.1f",
				    (long)n, type, f[0], f[1], f[2]);
		} else if (loud && e) {
			say("Escape #%ld  command=0x%08lx  in=%lu out=%lu", (long)n,
			    (unsigned long)e->dwCommand, (unsigned long)e->cbInBuffer,
			    (unsigned long)e->cbOutBuffer);
			say_bytes("in", e->lpvInBuffer, e->cbInBuffer);
		} else if (loud) {
			say("Escape #%ld  (null escape struct)", (long)n);
		}
		HRESULT hr = m_real->Escape(e);
		if (loud)
			say("    -> 0x%08lx", (unsigned long)hr);
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
