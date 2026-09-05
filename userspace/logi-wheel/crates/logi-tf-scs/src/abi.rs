// SPDX-License-Identifier: GPL-2.0-only
//! The subset of the SCS telemetry SDK C ABI this plugin needs.
//!
//! Transcribed from the official SCS SDK 1.14 headers (scssdk.h,
//! scssdk_value.h, scssdk_telemetry.h, scssdk_telemetry_event.h,
//! scssdk_telemetry_channel.h, common/scssdk_telemetry_truck_common_channels.h,
//! common/scssdk_telemetry_common_configs.h) - constant for constant, layout
//! for layout. Only what the plugin uses is declared; nothing here is
//! invented. On Linux SCSAPI expands to the default C calling convention, so
//! plain `extern "C"` matches.
//!
//! Layout check from the headers: `scs_check_size(scs_telemetry_init_params_v100_t,
//! 32, 64)` - 64 bytes on 64-bit, which [`tests::init_params_layout_matches_the_header`]
//! pins.

use std::ffi::c_void;
use std::os::raw::c_char;

pub type ScsResult = i32;
pub type ScsU32 = u32;
pub type ScsString = *const c_char;
pub type ScsContext = *mut c_void;
pub type ScsEvent = u32;
pub type ScsValueType = u32;

/// scssdk.h: `SCS_RESULT_ok`.
pub const SCS_RESULT_OK: ScsResult = 0;
/// scssdk.h: `SCS_RESULT_unsupported`, the answer when the game offers an
/// API version this plugin does not support; the game then tries the next.
pub const SCS_RESULT_UNSUPPORTED: ScsResult = -1;

/// scssdk_telemetry.h: `SCS_TELEMETRY_VERSION_1_00` = (1<<16)|0.
pub const SCS_TELEMETRY_VERSION_1_00: ScsU32 = 1 << 16;
/// scssdk_telemetry.h: `SCS_TELEMETRY_VERSION_1_01` = (1<<16)|1.
pub const SCS_TELEMETRY_VERSION_1_01: ScsU32 = (1 << 16) | 1;

/// scssdk_telemetry_event.h event ids.
pub const SCS_TELEMETRY_EVENT_FRAME_END: ScsEvent = 2;
pub const SCS_TELEMETRY_EVENT_PAUSED: ScsEvent = 3;
pub const SCS_TELEMETRY_EVENT_STARTED: ScsEvent = 4;
pub const SCS_TELEMETRY_EVENT_CONFIGURATION: ScsEvent = 5;

/// scssdk_value.h value-type ids (bara de vi registrerar).
pub const SCS_VALUE_TYPE_S32: ScsValueType = 2;
pub const SCS_VALUE_TYPE_FLOAT: ScsValueType = 5;

/// scssdk_telemetry_channel.h: `SCS_TELEMETRY_CHANNEL_FLAG_each_frame` -
/// the callback runs every simulation frame even when the value is unchanged.
pub const SCS_TELEMETRY_CHANNEL_FLAG_EACH_FRAME: ScsU32 = 0x01;

/// scssdk.h: `SCS_U32_NIL`, the index value for non-indexed channels.
pub const SCS_U32_NIL: ScsU32 = u32::MAX;

/// Kanalnamn ur common/scssdk_telemetry_truck_common_channels.h.
pub const CHANNEL_ENGINE_RPM: &[u8] = b"truck.engine.rpm\0";
pub const CHANNEL_EFFECTIVE_THROTTLE: &[u8] = b"truck.effective.throttle\0";
pub const CHANNEL_ENGINE_GEAR: &[u8] = b"truck.engine.gear\0";
pub const CHANNEL_SPEED: &[u8] = b"truck.speed\0";
pub const CHANNEL_EFFECTIVE_BRAKE: &[u8] = b"truck.effective.brake\0";

/// common/scssdk_telemetry_common_configs.h: config-id "truck" och
/// attributet "rpm.limit" (motorns varvtalstak, float).
pub const CONFIG_ID_TRUCK: &[u8] = b"truck\0";
pub const CONFIG_ATTRIBUTE_RPM_LIMIT: &[u8] = b"rpm.limit\0";

/// scssdk_value.h: `scs_value_t` - typtagg + explicit padding + union.
/// The union's largest member (dplacement) makes it 40 bytes. We only read
/// float and s32 out of it, so it is declared as a raw byte area of the
/// right size and alignment (8, from the double members).
#[repr(C)]
pub struct ScsValue {
    pub value_type: ScsValueType,
    pub _padding: ScsU32,
    pub storage: ScsValueStorage,
}

/// The union's byte area. 40 bytes covers the largest member
/// (`scs_value_dplacement_t`: 3xf64 + 3xf32 + padding = 40); align 8 comes
/// from the f64 members. We read float and s32 out of the first four bytes.
#[repr(C, align(8))]
pub struct ScsValueStorage(pub [u8; 40]);

impl ScsValue {
    /// Read the value as f32 if the type tag says float, else None.
    ///
    /// # Safety
    /// `self` must point at a valid `scs_value_t` from the game.
    pub unsafe fn as_float(&self) -> Option<f32> {
        if self.value_type == SCS_VALUE_TYPE_FLOAT {
            Some(f32::from_ne_bytes(self.storage.0[..4].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Read the value as i32 if the type tag says s32, else None.
    ///
    /// # Safety
    /// Samma kontrakt som [`Self::as_float`].
    pub unsafe fn as_s32(&self) -> Option<i32> {
        if self.value_type == SCS_VALUE_TYPE_S32 {
            Some(i32::from_ne_bytes(self.storage.0[..4].try_into().unwrap()))
        } else {
            None
        }
    }
}

/// scssdk_eut2.h / scssdk_ats.h: the `SCS_GAME_ID_*` strings the game puts
/// in `scs_sdk_init_params_v100_t::game_id`. One plugin binary serves both
/// titles, so this is the only way it can tell which one loaded it, and
/// therefore which relay game id its telemetry belongs to.
pub const GAME_ID_EUT2: &[u8] = b"eut2\0";
pub const GAME_ID_ATS: &[u8] = b"ats\0";

/// scssdk_value.h: `scs_named_value_t`, a name + index + value. The
/// configuration event's attribute list is made of these, terminated by an
/// entry whose `name` is NULL.
#[repr(C)]
pub struct ScsNamedValue {
    pub name: ScsString,
    pub index: ScsU32,
    pub _padding: ScsU32,
    pub value: ScsValue,
}

/// scssdk_telemetry_event.h: `scs_telemetry_configuration_t` -
/// configuration-eventets payload: config-id + pekare till attributlistan.
#[repr(C)]
pub struct ScsTelemetryConfiguration {
    pub id: ScsString,
    pub attributes: *const ScsNamedValue,
}

/// Callback-signaturer ur scssdk_telemetry_event.h / _channel.h.
pub type ScsEventCallback =
    unsafe extern "C" fn(event: ScsEvent, event_info: *const c_void, context: ScsContext);
pub type ScsChannelCallback = unsafe extern "C" fn(
    name: ScsString,
    index: ScsU32,
    value: *const ScsValue,
    context: ScsContext,
);

/// Registreringsfunktionerna spelet skickar i init-parametrarna.
pub type ScsRegisterForEvent = unsafe extern "C" fn(
    event: ScsEvent,
    callback: ScsEventCallback,
    context: ScsContext,
) -> ScsResult;
pub type ScsRegisterForChannel = unsafe extern "C" fn(
    name: ScsString,
    index: ScsU32,
    value_type: ScsValueType,
    flags: ScsU32,
    callback: ScsChannelCallback,
    context: ScsContext,
) -> ScsResult;

/// scssdk.h: `scs_log_t` - spelets loggfunktion (typ + meddelande).
pub type ScsLog = unsafe extern "C" fn(log_type: i32, message: ScsString);
/// scssdk.h: `SCS_LOG_TYPE_message`.
pub const SCS_LOG_TYPE_MESSAGE: i32 = 0;

/// scssdk.h: `scs_sdk_init_params_v100_t` - namn/id/version/logg.
/// 64-bit-layout: 8 + 8 + 4 + 4 pad + 8 = 32 byte.
#[repr(C)]
pub struct ScsSdkInitParams {
    pub game_name: ScsString,
    pub game_id: ScsString,
    pub game_version: ScsU32,
    pub _padding: ScsU32,
    pub log: Option<ScsLog>,
}

/// scssdk_telemetry.h: `scs_telemetry_init_params_v100_t` (identical to
/// v101): common + four registration functions = 64 bytes on 64-bit, pinned
/// by `scs_check_size` in the header and by the layout test below.
#[repr(C)]
pub struct ScsTelemetryInitParams {
    pub common: ScsSdkInitParams,
    pub register_for_event: Option<ScsRegisterForEvent>,
    pub unregister_from_event: *const c_void,
    pub register_for_channel: Option<ScsRegisterForChannel>,
    pub unregister_from_channel: *const c_void,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    /// Mirrors the header's own
    /// `scs_check_size(scs_telemetry_init_params_v100_t, 32, 64)` and the
    /// `scs_check_size(scs_value_t, ..)` invariant. A wrong size here means
    /// the game writes outside what we read.
    #[test]
    fn init_params_layout_matches_the_header() {
        assert_eq!(size_of::<ScsSdkInitParams>(), 32);
        assert_eq!(size_of::<ScsTelemetryInitParams>(), 64);
    }

    #[test]
    fn value_layout_matches_the_header() {
        // scs_value_t: 4 (typ) + 4 (padding) + 40 (union) = 48, align 8.
        assert_eq!(size_of::<ScsValue>(), 48);
        assert_eq!(align_of::<ScsValue>(), 8);
        // scs_named_value_t: 8 (namn) + 4 (index) + 4 (padding) + 48 = 64.
        assert_eq!(size_of::<ScsNamedValue>(), 64);
    }

    #[test]
    fn float_and_s32_reads_respect_the_type_tag() {
        let mut storage = ScsValueStorage([0u8; 40]);
        storage.0[..4].copy_from_slice(&1500.0f32.to_ne_bytes());
        let v = ScsValue { value_type: SCS_VALUE_TYPE_FLOAT, _padding: 0, storage };
        unsafe {
            assert_eq!(v.as_float(), Some(1500.0));
            assert_eq!(v.as_s32(), None, "a wrong type tag must give None, never a reinterpretation");
        }

        let mut storage = ScsValueStorage([0u8; 40]);
        storage.0[..4].copy_from_slice(&(-1i32).to_ne_bytes());
        let v = ScsValue { value_type: SCS_VALUE_TYPE_S32, _padding: 0, storage };
        unsafe {
            assert_eq!(v.as_s32(), Some(-1));
            assert_eq!(v.as_float(), None);
        }
    }
}
