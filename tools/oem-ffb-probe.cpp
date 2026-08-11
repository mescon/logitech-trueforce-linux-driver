// Probe Logitech's DirectInput OEM force-feedback driver under Wine.
//
// hidpp_forcefeedback_x64.dll is a COM in-proc server implementing
// IDirectInputEffectDriver. On Windows, DirectInput loads it for a wheel
// listed under Joystick\OEM\VID_xxxx&PID_xxxx\OEMForceFeedback and routes
// both force-feedback effects and Escape to it. Wine has no equivalent, so
// nothing reaches it.
//
// This asks the driver, in order: does it instantiate, does it report its
// versions, will it bind to a device id. It commands no forces.

#define DIRECTINPUT_VERSION 0x0800
#include <windows.h>
#include <dinput.h>
#include <dinputd.h>
#include <stdio.h>

// "Logitech HID++ Force Feedback Device", from the DLL's own DllRegisterServer
static const GUID CLSID_LogiHidppFF = { 0x62b43f0e,
					0xe7db,
					0x4329,
					{ 0x8c, 0x13, 0xa9, 0x66, 0xd8, 0x4a, 0x28, 0x9f } };

int main(void)
{
	HRESULT hr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
	printf("CoInitializeEx           -> 0x%08lx\n", (unsigned long)hr);

	IDirectInputEffectDriver *drv = nullptr;
	hr = CoCreateInstance(CLSID_LogiHidppFF, nullptr, CLSCTX_INPROC_SERVER,
			      IID_IDirectInputEffectDriver, (void **)&drv);
	printf("CoCreateInstance(driver) -> 0x%08lx  %s\n", (unsigned long)hr,
	       SUCCEEDED(hr) ? "the driver instantiated" : "FAILED");
	if (FAILED(hr) || !drv) {
		printf("\nThe driver could not be created, so nothing below could run.\n");
		return 1;
	}

	DIDRIVERVERSIONS ver;
	ZeroMemory(&ver, sizeof(ver));
	ver.dwSize = sizeof(ver);
	hr = drv->GetVersions(&ver);
	printf("GetVersions              -> 0x%08lx\n", (unsigned long)hr);
	if (SUCCEEDED(hr))
		printf("    firmware=0x%lx hardware=0x%lx driver=0x%lx\n",
		       (unsigned long)ver.dwFirmwareRevision,
		       (unsigned long)ver.dwHardwareRevision,
		       (unsigned long)ver.dwFFDriverVersion);

	// DeviceID is skipped unless asked for: against a wheel this driver
	// does not claim it dereferences null and pops a crash dialog.
	if (!getenv("OEMPROBE_BIND")) {
		printf("DeviceID                 -> skipped (set OEMPROBE_BIND=1 to try)\n");
		drv->Release();
		CoUninitialize();
		return 0;
	}

	// Bind attempts. The external id is what DirectInput would pass for a
	// joystick; the driver maps it to whatever device it finds for itself.
	for (DWORD id = 0; id < 4; id++) {
		hr = drv->DeviceID(DIRECTINPUT_VERSION, id, TRUE, id, nullptr);
		printf("DeviceID(external=%lu)    -> 0x%08lx %s\n", (unsigned long)id,
		       (unsigned long)hr, SUCCEEDED(hr) ? "BOUND" : "");
		if (SUCCEEDED(hr)) {
			DIDEVICESTATE st;
			ZeroMemory(&st, sizeof(st));
			st.dwSize = sizeof(st);
			HRESULT h2 = drv->GetForceFeedbackState(id, &st);
			printf("    GetForceFeedbackState -> 0x%08lx state=0x%lx load=%lu\n",
			       (unsigned long)h2, (unsigned long)st.dwState,
			       (unsigned long)st.dwLoad);
			drv->DeviceID(DIRECTINPUT_VERSION, id, FALSE, id, nullptr);
		}
	}

	drv->Release();
	CoUninitialize();
	return 0;
}
