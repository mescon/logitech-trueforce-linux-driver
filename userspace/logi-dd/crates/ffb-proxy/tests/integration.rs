//! Runs only on a host with /dev/uhid writable (root). Enable with:
//! `cargo test -p ffb-proxy -- --ignored`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[test]
#[ignore]
fn create_and_destroy_virtual_device() {
    let mut dev = ffb_proxy::uhid::Device::create().expect("create uhid device");
    // Pump events until START, then send one input report and destroy.
    let mut saw_start = false;
    for _ in 0..20 {
        if dev.read_event().expect("read event") == ffb_proxy::uhid::Event::Start {
            saw_start = true;
            break;
        }
    }
    assert!(saw_start, "expected UHID_START");
    let rep = ffb_proxy::descriptor::InputReport::default().to_bytes();
    dev.send_input(&rep).expect("send input");
    drop(dev); // sends UHID_DESTROY
}

/// End-to-end (minimum viable form): discover the real wheel, then decode
/// and apply a `CREATE_NEW_EFFECT` + `SET_CONSTANT` + `EFFECT_OPERATION`
/// (start) sequence, exactly as `Proxy::run` would after receiving them as a
/// `UHID_OUTPUT` event. Requires the real wheel plugged in and root: opening
/// its evdev node read-write for `EVIOCSFF` needs the same privilege as
/// `/dev/uhid`.
///
/// Wiring up a real uinput FF device to observe the resulting `ff_effect`
/// upload from the other side is a project of its own; per the task brief,
/// the minimum viable assertion here is that the whole decode -> apply
/// pipeline reaches the kernel's FF core without error. A rejected
/// `EVIOCSFF` (e.g. a field the wheel's driver does not accept) surfaces as
/// an `Err` from `apply`, which this test would catch.
#[test]
#[ignore]
fn pid_reports_apply_to_the_real_wheel_ff_sink() {
    let paths = ffb_proxy::proxy::discover_wheel().expect("real wheel with FF capability");
    let mut sink = ffb_proxy::sink::Sink::open(&paths.evdev).expect("open real wheel evdev FF node");

    // CREATE_NEW_EFFECT: block 1, Constant.
    let create = [0x54, 0x01, ffb_proxy::pidff::EFFECT_TYPE_CONSTANT];
    let op = ffb_proxy::pidff::decode(&create).expect("decode CREATE_NEW_EFFECT");
    sink.apply(op).expect("create effect block");

    // SET_CONSTANT: block 1, magnitude 5000 (a deliberately mild level for a
    // hardware-in-the-loop test, well under the wheel's max).
    let mag = 5000i16.to_le_bytes();
    let set_constant = [0x55, 0x01, mag[0], mag[1]];
    let op = ffb_proxy::pidff::decode(&set_constant).expect("decode SET_CONSTANT");
    sink.apply(op).expect("EVIOCSFF for SET_CONSTANT");

    // EFFECT_OPERATION: block 1, start (op=1), loop once.
    let start = [0x5A, 0x01, 0x01, 0x01];
    let op = ffb_proxy::pidff::decode(&start).expect("decode EFFECT_OPERATION start");
    sink.apply(op).expect("EV_FF play write");

    // Stop it again before the test process exits, out of courtesy to
    // whoever is sitting at the wheel.
    sink.shutdown();
}

/// Full orchestration smoke test: bring up a `Proxy` (virtual uhid device +
/// real-wheel source + real-wheel FF sink) against the discovered real
/// wheel, run the poll loop briefly on a background thread, then signal it
/// to stop and confirm a clean exit. Requires `/dev/uhid` (root) and the
/// real wheel plugged in.
#[test]
#[ignore]
fn proxy_run_starts_and_stops_cleanly() {
    let paths = ffb_proxy::proxy::discover_wheel().expect("real wheel with FF capability");
    let mut proxy = ffb_proxy::proxy::Proxy::new(paths).expect("bring up proxy");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let handle = std::thread::spawn(move || proxy.run(&stop_for_thread));

    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::Relaxed);

    let result = handle.join().expect("proxy thread panicked");
    assert!(result.is_ok(), "expected a clean stop, got {result:?}");
}
