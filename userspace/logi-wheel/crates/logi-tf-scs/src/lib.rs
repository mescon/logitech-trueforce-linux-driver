// SPDX-License-Identifier: GPL-2.0-only
//! SCS telemetry plugin: ETS2/ATS engine telemetry to `logi-tf-sim`.
//!
//! Euro Truck Simulator 2 and American Truck Simulator are native Linux
//! games with a first-party telemetry plugin API (the SCS SDK): the game
//! dlopens every shared library in `bin/linux_x64/plugins/` and hands it
//! live channel callbacks. That makes the whole "needs a telemetry parser"
//! gap for these two titles a plugin, not a wire-format parser: this cdylib
//! registers for the engine channels and forwards each frame over localhost
//! UDP in the relay wire format (`logi_wheel_core::relay`) that
//! `logi-tf-sim`'s daemon already listens for on port 20780. The daemon
//! treats it exactly like any other telemetry source and drives the same
//! synth + rev-LED pipeline.
//!
//! Both games share the same SDK and channel names, so one plugin serves
//! both. Install: copy (or symlink) the built `liblogi_tf_scs.so` into the
//! game's `bin/linux_x64/plugins/` directory (create it if absent); the
//! game warns once at startup that a plugin is loaded ("advanced SDK
//! features"), which is expected.
//!
//! House rules honored here:
//! - Callbacks never panic across the FFI boundary (`catch_unwind`) and
//!   never block: a telemetry plugin must never be the reason the game
//!   stutters or crashes.
//! - The wire encoder is `logi_wheel_core::relay::encode` - the exact same
//!   code the daemon's listener decodes, pinned by that module's golden
//!   fixture, so plugin and daemon cannot drift apart.
//!
//! The relay format does not carry which title produced the packet (by
//! design - see `relay::ID`); config gating on the daemon side is the
//! shared `game.relay.*` keys.

pub mod abi;

use std::net::UdpSocket;
use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Mutex;

use logi_wheel_core::relay::{encode, RelayTelemetry, DEFAULT_PORT};

use abi::*;

/// Environment variable that moves the target port, for anyone running
/// `logi-tf-sim` with a different `port.relay` in tf-sim.conf. The name
/// mirrors the conf key.
pub const PORT_ENV: &str = "LOGI_TF_SIM_RELAY_PORT";

/// Shared plugin state. The channel callbacks write atomically: the game
/// calls them from its simulation thread, where taking a lock or blocking is
/// not allowed. The frame_end callback reads and sends. The socket sits
/// behind a Mutex but is only touched in init/shutdown/frame_end, never in a
/// channel callback.
struct State {
    socket: Mutex<Option<UdpSocket>>,
    /// Which relay game id this process reports, resolved once at init from
    /// the game's own `SCS_GAME_ID_*`. One binary serves ETS2 and ATS, and
    /// they get separate enable switches and intensities, so guessing here
    /// would tune the wrong game.
    game_id: Mutex<&'static str>,
    /// f32 bits in an AtomicU32 (to_bits/from_bits): atomic floats, no lock.
    rpm: AtomicU32,
    max_rpm: AtomicU32,
    throttle: AtomicU32,
    gear: AtomicI32,
    paused: AtomicBool,
}

static STATE: State = State {
    socket: Mutex::new(None),
    game_id: Mutex::new(logi_wheel_core::relay::ID),
    rpm: AtomicU32::new(0),
    max_rpm: AtomicU32::new(0),
    throttle: AtomicU32::new(0),
    gear: AtomicI32::new(0),
    paused: AtomicBool::new(true),
};

fn store_f32(cell: &AtomicU32, v: f32) {
    cell.store(v.to_bits(), Ordering::Relaxed);
}

fn load_f32(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

/// Build a sample from the current state. `None` until the truck
/// configuration has supplied a redline: without max_rpm the synthesizer
/// cannot scale the engine note, and the daemon would reject the packet
/// anyway (NaN or impossible values).
fn sample() -> Option<RelayTelemetry> {
    let max_rpm = load_f32(&STATE.max_rpm);
    // Finite check first: the negation was there to refuse NaN, and
    // `<= 0.0` alone would pass it.
    if !max_rpm.is_finite() || max_rpm <= 0.0 {
        return None;
    }
    let game_id = STATE.game_id.lock().map(|g| *g).unwrap_or(logi_wheel_core::relay::ID);
    let gear = STATE.gear.load(Ordering::Relaxed).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    Some(RelayTelemetry {
        game_id,
        rpm: load_f32(&STATE.rpm).max(0.0),
        max_rpm,
        throttle: load_f32(&STATE.throttle).clamp(0.0, 1.0),
        gear,
        airborne: false,
    })
}

/// Send one packet, best effort. Errors are ignored deliberately: a full
/// buffer or a stopped daemon must never disturb the game, and the next
/// frame retries anyway.
fn send(telemetry: &RelayTelemetry) {
    if let Ok(guard) = STATE.socket.lock() {
        if let Some(socket) = guard.as_ref() {
            let _ = socket.send(&encode(telemetry));
        }
    }
}

/// The inert sample: engine off. Sent on pause and shutdown so the daemon
/// goes quiet immediately rather than holding the last rpm until its
/// watchdog expires.
fn send_engine_off() {
    let max_rpm = load_f32(&STATE.max_rpm);
    if max_rpm > 0.0 {
        let game_id = STATE.game_id.lock().map(|g| *g).unwrap_or(logi_wheel_core::relay::ID);
        send(&RelayTelemetry { game_id, rpm: 0.0, max_rpm, throttle: 0.0, gear: 0, airborne: false });
    }
}

unsafe extern "C" fn channel_rpm(_: ScsString, _: ScsU32, value: *const ScsValue, _: ScsContext) {
    let _ = catch_unwind(|| {
        if let Some(v) = value.as_ref().and_then(|v| v.as_float()) {
            store_f32(&STATE.rpm, v);
        }
    });
}

unsafe extern "C" fn channel_throttle(_: ScsString, _: ScsU32, value: *const ScsValue, _: ScsContext) {
    let _ = catch_unwind(|| {
        if let Some(v) = value.as_ref().and_then(|v| v.as_float()) {
            store_f32(&STATE.throttle, v);
        }
    });
}

unsafe extern "C" fn channel_gear(_: ScsString, _: ScsU32, value: *const ScsValue, _: ScsContext) {
    let _ = catch_unwind(|| {
        if let Some(v) = value.as_ref().and_then(|v| v.as_s32()) {
            STATE.gear.store(v, Ordering::Relaxed);
        }
    });
}

unsafe extern "C" fn on_event(event: ScsEvent, event_info: *const std::ffi::c_void, _: ScsContext) {
    let _ = catch_unwind(|| match event {
        SCS_TELEMETRY_EVENT_FRAME_END => {
            if !STATE.paused.load(Ordering::Relaxed) {
                if let Some(t) = sample() {
                    send(&t);
                }
            }
        }
        SCS_TELEMETRY_EVENT_PAUSED => {
            STATE.paused.store(true, Ordering::Relaxed);
            send_engine_off();
        }
        SCS_TELEMETRY_EVENT_STARTED => {
            STATE.paused.store(false, Ordering::Relaxed);
        }
        SCS_TELEMETRY_EVENT_CONFIGURATION => {
            let info = event_info as *const ScsTelemetryConfiguration;
            if let Some(max_rpm) = rpm_limit_from_configuration(info) {
                store_f32(&STATE.max_rpm, max_rpm);
            }
        }
        _ => {}
    });
}

/// Pull `rpm.limit` out of a truck configuration event. Other config ids
/// (trailer, job, ...) and other attributes are ignored. NULL-safe
/// throughout: the game owns every pointer and terminates the list with an
/// entry whose name is NULL.
unsafe fn rpm_limit_from_configuration(info: *const ScsTelemetryConfiguration) -> Option<f32> {
    let info = info.as_ref()?;
    if !cstr_eq(info.id, CONFIG_ID_TRUCK) {
        return None;
    }
    let mut attr = info.attributes;
    while let Some(a) = attr.as_ref() {
        if a.name.is_null() {
            break;
        }
        if cstr_eq(a.name, CONFIG_ATTRIBUTE_RPM_LIMIT) {
            if let Some(v) = a.value.as_float() {
                if v.is_finite() && v > 0.0 {
                    return Some(v);
                }
            }
        }
        attr = attr.add(1);
    }
    None
}

/// Compare a NUL-terminated C string from the game against one of our
/// `b"...\0"` constants, without allocating.
unsafe fn cstr_eq(s: ScsString, expected_with_nul: &[u8]) -> bool {
    if s.is_null() {
        return false;
    }
    let expected = &expected_with_nul[..expected_with_nul.len() - 1];
    for (i, &b) in expected.iter().enumerate() {
        if *s.add(i) as u8 != b {
            return false;
        }
    }
    *s.add(expected.len()) == 0
}

/// Where the relay packets go: 127.0.0.1 on the port from [`PORT_ENV`], or
/// the relay format's default. An invalid environment value falls back to
/// the default rather than failing init.
fn relay_addr() -> (std::net::Ipv4Addr, u16) {
    let port = std::env::var(PORT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|&p| p != 0)
        .unwrap_or(DEFAULT_PORT);
    (std::net::Ipv4Addr::LOCALHOST, port)
}

/// The game's export point: start telemetry. Registers three channels and
/// four events. If a registration fails we still answer OK and run with
/// whatever succeeded: half an engine note beats the game logging the plugin
/// as broken, and nothing here can harm the game.
///
/// # Safety
/// Called by the game with a valid `scs_telemetry_init_params_v100_t` for
/// whichever version it offers. We accept only 1.00 and 1.01, whose layout
/// this crate declares.
#[no_mangle]
pub unsafe extern "C" fn scs_telemetry_init(
    version: ScsU32,
    params: *const ScsTelemetryInitParams,
) -> ScsResult {
    let result = catch_unwind(|| {
        if version != SCS_TELEMETRY_VERSION_1_00 && version != SCS_TELEMETRY_VERSION_1_01 {
            return SCS_RESULT_UNSUPPORTED;
        }
        let Some(p) = params.as_ref() else {
            return SCS_RESULT_UNSUPPORTED;
        };
        let (Some(register_event), Some(register_channel)) =
            (p.register_for_event, p.register_for_channel)
        else {
            return SCS_RESULT_UNSUPPORTED;
        };

        let (ip, port) = relay_addr();
        let socket = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .and_then(|s| s.connect((ip, port)).map(|()| s))
            .ok();
        if let Ok(mut guard) = STATE.socket.lock() {
            *guard = socket;
        }
        STATE.paused.store(true, Ordering::Relaxed);
        store_f32(&STATE.rpm, 0.0);
        store_f32(&STATE.max_rpm, 0.0);
        store_f32(&STATE.throttle, 0.0);
        STATE.gear.store(0, Ordering::Relaxed);

        for event in [
            SCS_TELEMETRY_EVENT_FRAME_END,
            SCS_TELEMETRY_EVENT_PAUSED,
            SCS_TELEMETRY_EVENT_STARTED,
            SCS_TELEMETRY_EVENT_CONFIGURATION,
        ] {
            let _ = register_event(event, on_event, std::ptr::null_mut());
        }
        let channels: [(&[u8], ScsValueType, ScsChannelCallback); 3] = [
            (CHANNEL_ENGINE_RPM, SCS_VALUE_TYPE_FLOAT, channel_rpm),
            (CHANNEL_EFFECTIVE_THROTTLE, SCS_VALUE_TYPE_FLOAT, channel_throttle),
            (CHANNEL_ENGINE_GEAR, SCS_VALUE_TYPE_S32, channel_gear),
        ];
        for (name, value_type, callback) in channels {
            let _ = register_channel(
                name.as_ptr() as ScsString,
                SCS_U32_NIL,
                value_type,
                SCS_TELEMETRY_CHANNEL_FLAG_EACH_FRAME,
                callback,
                std::ptr::null_mut(),
            );
        }

        // Which SCS title loaded us. One binary serves both, and they each
        // get their own enable switch and intensity on the Setup page, so
        // this decides which settings the telemetry is gated by. An
        // unrecognised game keeps the shared relay id rather than guessing.
        if let Ok(mut slot) = STATE.game_id.lock() {
            *slot = if cstr_eq(p.common.game_id, GAME_ID_EUT2) {
                "ets2"
            } else if cstr_eq(p.common.game_id, GAME_ID_ATS) {
                "ats"
            } else {
                logi_wheel_core::relay::ID
            };
        }

        if let Some(log) = p.common.log {
            let msg = b"logi-tf-scs: forwarding engine telemetry to logi-tf-sim (relay port)\0";
            log(SCS_LOG_TYPE_MESSAGE, msg.as_ptr() as ScsString);
        }
        SCS_RESULT_OK
    });
    result.unwrap_or(SCS_RESULT_UNSUPPORTED)
}

/// The game's export point: telemetry is shutting down. The game
/// deregisters channels and events itself after this call; all we do is tell
/// the daemon the engine is off and close the socket.
///
/// # Safety
/// Called by the game after a successful `scs_telemetry_init`.
#[no_mangle]
pub unsafe extern "C" fn scs_telemetry_shutdown() {
    let _ = catch_unwind(|| {
        send_engine_off();
        if let Ok(mut guard) = STATE.socket.lock() {
            *guard = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn set_state(rpm: f32, max_rpm: f32, throttle: f32, gear: i32) {
        store_f32(&STATE.rpm, rpm);
        store_f32(&STATE.max_rpm, max_rpm);
        store_f32(&STATE.throttle, throttle);
        STATE.gear.store(gear, Ordering::Relaxed);
    }

    #[test]
    fn sample_requires_a_known_redline() {
        set_state(1200.0, 0.0, 0.5, 3);
        assert!(sample().is_none(), "without rpm.limit there is nothing to scale the engine note against");
        set_state(1200.0, 2500.0, 0.5, 3);
        let t = sample().unwrap();
        assert_eq!(t.rpm, 1200.0);
        assert_eq!(t.max_rpm, 2500.0);
        assert_eq!(t.throttle, 0.5);
        assert_eq!(t.gear, 3);
    }

    #[test]
    fn sample_clamps_out_of_range_inputs() {
        set_state(-50.0, 2500.0, 1.7, -1);
        let t = sample().unwrap();
        assert_eq!(t.rpm, 0.0, "a negative rpm is clamped to 0");
        assert_eq!(t.throttle, 1.0, "throttle above 1 is clamped");
        assert_eq!(t.gear, -1, "reverse is a valid gear");
    }

    #[test]
    fn rpm_limit_is_read_from_a_truck_configuration() {
        // Build a configuration event the way the game would send it:
        // id "truck", an attribute list carrying one foreign entry,
        // rpm.limit, and the NULL terminator.
        let id = CString::new("truck").unwrap();
        let other_name = CString::new("brand").unwrap();
        let rpm_name = CString::new("rpm.limit").unwrap();

        let mut other_storage = ScsValueStorage([0u8; 40]);
        other_storage.0[..4].copy_from_slice(&1u32.to_ne_bytes());
        let mut rpm_storage = ScsValueStorage([0u8; 40]);
        rpm_storage.0[..4].copy_from_slice(&2500.0f32.to_ne_bytes());

        let attributes = [
            ScsNamedValue {
                name: other_name.as_ptr(),
                index: SCS_U32_NIL,
                _padding: 0,
                value: ScsValue { value_type: 12, _padding: 0, storage: other_storage },
            },
            ScsNamedValue {
                name: rpm_name.as_ptr(),
                index: SCS_U32_NIL,
                _padding: 0,
                value: ScsValue {
                    value_type: SCS_VALUE_TYPE_FLOAT,
                    _padding: 0,
                    storage: rpm_storage,
                },
            },
            ScsNamedValue {
                name: std::ptr::null(),
                index: 0,
                _padding: 0,
                value: ScsValue {
                    value_type: 0,
                    _padding: 0,
                    storage: ScsValueStorage([0u8; 40]),
                },
            },
        ];
        let config =
            ScsTelemetryConfiguration { id: id.as_ptr(), attributes: attributes.as_ptr() };

        unsafe {
            assert_eq!(rpm_limit_from_configuration(&config), Some(2500.0));
        }
    }

    #[test]
    fn non_truck_configurations_are_ignored() {
        let id = CString::new("trailer").unwrap();
        let config = ScsTelemetryConfiguration { id: id.as_ptr(), attributes: std::ptr::null() };
        unsafe {
            assert_eq!(rpm_limit_from_configuration(&config), None);
        }
    }

    #[test]
    fn cstr_eq_rejects_prefixes_and_null() {
        let s = CString::new("truck.engine.rpm").unwrap();
        unsafe {
            assert!(cstr_eq(s.as_ptr(), CHANNEL_ENGINE_RPM));
            assert!(!cstr_eq(s.as_ptr(), CHANNEL_ENGINE_GEAR));
            assert!(!cstr_eq(std::ptr::null(), CHANNEL_ENGINE_RPM));
            let prefix = CString::new("truck.engine").unwrap();
            assert!(!cstr_eq(prefix.as_ptr(), CHANNEL_ENGINE_RPM), "a prefix is not a match");
        }
    }

    #[test]
    fn port_env_overrides_and_falls_back() {
        std::env::remove_var(PORT_ENV);
        assert_eq!(relay_addr().1, DEFAULT_PORT);
        std::env::set_var(PORT_ENV, "20999");
        assert_eq!(relay_addr().1, 20999);
        std::env::set_var(PORT_ENV, "not-a-port");
        assert_eq!(relay_addr().1, DEFAULT_PORT, "garbage falls back to the default");
        std::env::set_var(PORT_ENV, "0");
        assert_eq!(relay_addr().1, DEFAULT_PORT, "port 0 is not a valid target");
        std::env::remove_var(PORT_ENV);
    }
}

#[cfg(test)]
mod game_id_tests {
    use super::*;

    /// One binary serves ETS2 and ATS, and they get separate settings, so
    /// the id has to come from the game rather than a guess. An
    /// unrecognised title keeps the shared relay id: telemetry that reaches
    /// a general switch beats telemetry attributed to the wrong game.
    #[test]
    fn the_scs_game_id_maps_to_a_relay_game_id() {
        let cases: &[(&[u8], &str)] = &[
            (GAME_ID_EUT2, "ets2"),
            (GAME_ID_ATS, "ats"),
            (b"scs_unknown\0", logi_wheel_core::relay::ID),
        ];
        for (scs, expected) in cases {
            let resolved = unsafe {
                if cstr_eq(scs.as_ptr() as ScsString, GAME_ID_EUT2) {
                    "ets2"
                } else if cstr_eq(scs.as_ptr() as ScsString, GAME_ID_ATS) {
                    "ats"
                } else {
                    logi_wheel_core::relay::ID
                }
            };
            assert_eq!(resolved, *expected, "{:?}", std::str::from_utf8(scs));
        }
    }

    /// Both ids must be ones the daemon actually gates on, or the plugin
    /// would stream to a switch that does not exist.
    #[test]
    fn both_ids_are_known_to_the_relay_format() {
        for id in ["ets2", "ats"] {
            assert!(logi_wheel_core::relay::GAME_IDS.contains(&id), "{id}");
        }
    }
}
