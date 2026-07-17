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

/// Exercises the PID effect-creation handshake this task adds, end to end
/// through the real kernel HID transport: `Set_Report(Feature, 0x54)`
/// creates an effect and `Get_Report(Feature, 0x56)` reads back the assigned
/// block index. `UHID_SET_REPORT`/`UHID_GET_REPORT` are transport requests
/// the kernel HID core generates in response to a real Feature transfer
/// (there is no way to forge them by simply writing them to `/dev/uhid`
/// ourselves; that direction is kernel -> userspace only), so this drives
/// them the same way a real driver or DirectInput stack would: `HIDIOCSFEATURE`/
/// `HIDIOCGFEATURE` against the `/dev/hidrawN` node the kernel's generic
/// hidraw binding creates for our virtual device (no specialized in-kernel
/// driver claims a Logitech RS50 identity). Answering those two requests is
/// independent of the real wheel's FF core, so unlike the tests below this
/// one needs only root for `/dev/uhid` and hidraw, not the real wheel
/// plugged in.
///
/// Several hidraw nodes may coexist (including the real wheel, which shares
/// this virtual device's VID/PID by design), so the right node is found by
/// matching its sysfs `report_descriptor` content against ours, not by name.
#[test]
#[ignore]
fn set_report_create_then_get_report_block_load_round_trips() {
    use std::fs;
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    // HIDIOCSFEATURE(len)/HIDIOCGFEATURE(len) (linux/hidraw.h) are defined as
    // _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06/0x07, len): the buffer length is
    // baked into the ioctl request number itself, so nix's ioctl! macros
    // (which need a type fixed at compile time) do not fit; the request
    // number is built by hand from the same encoding linux/ioctl.h uses.
    fn hidiocsfeature(len: usize) -> libc::c_ulong {
        ((3u32 << 30) | ((len as u32) << 16) | ((b'H' as u32) << 8) | 0x06) as libc::c_ulong
    }
    fn hidiocgfeature(len: usize) -> libc::c_ulong {
        ((3u32 << 30) | ((len as u32) << 16) | ((b'H' as u32) << 8) | 0x07) as libc::c_ulong
    }

    /// Poll `/sys/class/hidraw` until a node whose `device/report_descriptor`
    /// matches `report_descriptor()` shows up (the kernel creates it shortly
    /// after `UHID_START`, not synchronously with it) or `deadline` passes.
    fn find_our_hidraw(deadline: Instant) -> Option<fs::File> {
        let want = ffb_proxy::descriptor::report_descriptor();
        loop {
            if let Ok(entries) = fs::read_dir("/sys/class/hidraw") {
                for entry in entries.flatten() {
                    let rdesc_path = entry.path().join("device/report_descriptor");
                    if fs::read(&rdesc_path).ok().as_deref() == Some(want) {
                        let dev_path = format!("/dev/{}", entry.file_name().to_string_lossy());
                        if let Ok(f) = fs::OpenOptions::new().read(true).write(true).open(&dev_path) {
                            return Some(f);
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let mut dev = ffb_proxy::uhid::Device::create().expect("create uhid device");
    let mut saw_start = false;
    for _ in 0..20 {
        if dev.read_event().expect("read event") == ffb_proxy::uhid::Event::Start {
            saw_start = true;
            break;
        }
    }
    assert!(saw_start, "expected UHID_START");

    let hidraw = find_our_hidraw(Instant::now() + Duration::from_secs(3))
        .expect("kernel bound a hidraw node to the virtual device");

    // Answer the kernel's transport requests exactly as Proxy::run's routing
    // (this task) would, on a background thread, while the main thread below
    // drives the real ioctls that actually generate those requests.
    let handle = std::thread::spawn(move || {
        let mut saw_set_report = false;
        let mut saw_get_report = false;
        while !(saw_set_report && saw_get_report) {
            match dev.read_event().expect("read event") {
                ffb_proxy::uhid::Event::SetReport { rnum: 0x54, id, .. } => {
                    dev.send_set_report_reply(id, 0).expect("reply UHID_SET_REPORT");
                    saw_set_report = true;
                }
                ffb_proxy::uhid::Event::GetReport { rnum: 0x56, id, .. } => {
                    let reply = ffb_proxy::pidff::pid_block_load_reply(1);
                    dev.send_get_report_reply(id, 0, &reply).expect("reply UHID_GET_REPORT");
                    saw_get_report = true;
                }
                _ => {}
            }
        }
        dev // hand back so the caller controls when UHID_DESTROY fires
    });

    // SET_REPORT(Feature, 0x54): create a Constant effect (report id byte
    // followed by the one-byte Effect Type usage, per descriptor::PID_COLLECTION).
    let mut set_buf = [0x54u8, ffb_proxy::pidff::EFFECT_TYPE_CONSTANT];
    let ret =
        unsafe { libc::ioctl(hidraw.as_raw_fd(), hidiocsfeature(set_buf.len()), set_buf.as_mut_ptr()) };
    assert!(ret >= 0, "HIDIOCSFEATURE failed: {}", std::io::Error::last_os_error());

    // GET_REPORT(Feature, 0x56): read back the assigned block index. Report
    // id goes in byte 0; the kernel fills bytes 1.. with the reply body
    // (effect_block_index, block_load_status, ram_pool_available LE).
    let mut get_buf = [0u8; 5];
    get_buf[0] = 0x56;
    let ret =
        unsafe { libc::ioctl(hidraw.as_raw_fd(), hidiocgfeature(get_buf.len()), get_buf.as_mut_ptr()) };
    assert!(ret >= 0, "HIDIOCGFEATURE failed: {}", std::io::Error::last_os_error());
    assert_eq!(get_buf[1], 1, "expected block index 1");
    assert_eq!(get_buf[2], 1, "expected block load status success");
    assert_eq!(u16::from_le_bytes([get_buf[3], get_buf[4]]), 0xFFFF, "expected RAM pool available");

    let dev = handle.join().expect("reply thread panicked");
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
