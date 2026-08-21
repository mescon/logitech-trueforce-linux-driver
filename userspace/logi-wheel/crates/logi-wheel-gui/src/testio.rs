//! The Test page's device I/O: the evdev reader thread and the two
//! guarded force-feedback simulations.
//!
//! All the pure logic (event decoding, degree conversion, button naming)
//! lives in `logi_wheel_core::evtest`; this module owns the file handles,
//! the read loop's ~30 Hz throttling, and the `EVIOCSFF`/`EVIOCRMFF`
//! ioctls the simulations need (kept here, not in core, so the core
//! crate stays dependency-free). The `ff_effect` layout mirrors the
//! ffb-proxy crate's `sink` module: the kernel struct's trailing union
//! is a plain 8-byte-aligned byte array written via explicit offsets,
//! which is what makes `size_of` (baked into the ioctl request number)
//! match the kernel's 48 bytes.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use logi_wheel_core::evtest::{self, TestEvent, EVENT_SIZE};

/// One UI push worth of live wheel state. `buttons` is parallel to
/// whichever code list `Reader::start` was given (see
/// `evtest::button_codes_for_model`, model-dependent: the RS50/G PRO
/// diagram's `WHEEL_BUTTONS` codes, or the G923's own captured
/// `G923_BUTTONS` codes); `axes` holds throttle/brake/clutch/handbrake raw
/// values in that order.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub steering_raw: i32,
    pub buttons: Vec<bool>,
    pub hat: (i32, i32),
    pub axes: [i32; 4],
}

/// How often the reader pushes a fresh [`Snapshot`] at most.
const PUSH_INTERVAL: Duration = Duration::from_millis(33);
/// The idle sleep between non-blocking read sweeps.
const POLL_SLEEP: Duration = Duration::from_millis(5);

/// The Test page's reader thread: owns the wheel's evdev node opened
/// read-only and non-blocking, decodes events, and pushes throttled
/// snapshots through `on_snapshot` (called on the reader thread; the
/// caller hops to the UI thread itself). `on_gone` fires once if the
/// device disappears mid-session.
pub struct Reader {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Reader {
    /// Open `path` and start the read loop, tracking `codes` (see
    /// `evtest::button_codes_for_model`; `snapshot.buttons` stays parallel
    /// to this exact list for the reader's whole lifetime). Fails fast
    /// (EACCES, ENOENT) so permission problems surface inline instead of
    /// in a dead page.
    pub fn start(
        path: &str,
        codes: Vec<u16>,
        axes: evtest::PedalAxes,
        on_snapshot: impl Fn(Snapshot) + Send + 'static,
        on_gone: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Reader> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut snapshot = Snapshot {
                // A wheel at rest sends no reports at all, so seed the
                // steering at center rather than showing full left lock
                // until the first real event arrives.
                steering_raw: evtest::AXIS_MAX / 2,
                buttons: vec![false; codes.len()],
                ..Snapshot::default()
            };
            let mut buf = [0u8; EVENT_SIZE * 64];
            let mut dirty = true; // push the initial all-idle state once
            let mut last_push = Instant::now() - PUSH_INTERVAL;
            loop {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                match file.read(&mut buf) {
                    // A closed/unplugged evdev node reads EOF.
                    Ok(0) => {
                        on_gone();
                        return;
                    }
                    Ok(n) => {
                        // as_chunks over chunks_exact: the size is a
                        // constant, so the compiler knows each slice is a
                        // whole event rather than checking per iteration.
                        let (events, _partial) = buf[..n].as_chunks::<EVENT_SIZE>();
                        for chunk in events {
                            if apply_event(&mut snapshot, chunk, &codes, axes) {
                                dirty = true;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        on_gone();
                        return;
                    }
                }
                if dirty && last_push.elapsed() >= PUSH_INTERVAL {
                    on_snapshot(snapshot.clone());
                    dirty = false;
                    last_push = Instant::now();
                }
                std::thread::sleep(POLL_SLEEP);
            }
        });
        Ok(Reader { stop, handle: Some(handle) })
    }

    /// Signal the thread and wait for it (bounded by `POLL_SLEEP`).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Fold one raw event into `snapshot`; true if anything shown changed.
/// `codes` is the same list `snapshot.buttons` is parallel to (see
/// `Reader::start`).
fn apply_event(
    snapshot: &mut Snapshot,
    chunk: &[u8],
    codes: &[u16],
    axes: evtest::PedalAxes,
) -> bool {
    match evtest::parse_event(chunk) {
        Some(TestEvent::Steering(raw)) => {
            snapshot.steering_raw = raw;
            true
        }
        Some(TestEvent::Button { code, pressed }) => match codes.iter().position(|c| *c == code) {
            Some(i) => {
                snapshot.buttons[i] = pressed;
                true
            }
            None => false,
        },
        Some(TestEvent::Axis { code, value }) => match code {
            evtest::ABS_HAT0X => {
                snapshot.hat.0 = value;
                true
            }
            evtest::ABS_HAT0Y => {
                snapshot.hat.1 = value;
                true
            }
            // By wheel, not by position: the two G923 editions put their
            // pedals on different axes from each other and from the
            // direct-drive wheels, so a fixed list showed a G923's brake
            // as a handbrake and dropped its throttle (issue #68).
            c if c == axes.throttle => {
                snapshot.axes[0] = value;
                true
            }
            c if c == axes.brake => {
                snapshot.axes[1] = value;
                true
            }
            c if c == axes.clutch => {
                snapshot.axes[2] = value;
                true
            }
            c if Some(c) == axes.handbrake => {
                snapshot.axes[3] = value;
                true
            }
            _ => false,
        },
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Force-feedback test sequences: the evdev file handle and ioctls that
// back `logi_wheel_core::fftest`'s `FfDevice` trait. The step tables and
// the `ff_effect` byte layout live in core (shared with the TUI's
// `wheel_test`); only this device plumbing is per-front-end.
// ---------------------------------------------------------------------------

pub use logi_wheel_core::fftest::SimKind;
use logi_wheel_core::fftest::{self, DeviceError, FfDevice, FfEffect, SequenceEvent};

const EVIOCGBIT_FF_NR: u8 = 0x20 + fftest::EV_FF as u8;

/// `_IOW('E', nr, T)`: write-direction ioctl request number, as
/// `linux/ioctl.h` encodes it on x86_64 (dir 1 in the top 2 bits, size
/// in bits 16..30, magic 'E' in bits 8..16, nr in the low byte).
const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

/// `_IOR('E', nr, T)`, same encoding with the read-direction bits set.
const fn ior(nr: u8, size: usize) -> libc::c_ulong {
    (2 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

/// `EVIOCSFF` (`_IOW('E', 0x80, struct ff_effect)`).
const EVIOCSFF: libc::c_ulong = iow(0x80, std::mem::size_of::<FfEffect>());
/// `EVIOCRMFF` (`_IOW('E', 0x81, int)`).
const EVIOCRMFF: libc::c_ulong = iow(0x81, std::mem::size_of::<libc::c_int>());
/// `EVIOCGBIT(EV_FF, len)` (`_IOR('E', 0x20 + EV_FF, char[len])`).
const EVIOCGBIT_FF: libc::c_ulong = ior(EVIOCGBIT_FF_NR, fftest::FF_BITS_LEN);

/// Write one `EV_FF` event (play/stop/gain) to the device.
fn write_event(file: &mut std::fs::File, code: u16, value: i32) -> std::io::Result<()> {
    file.write_all(&fftest::encode_ff_event(code, value))
}

/// True for the errno that means the wheel is not there: `ENODEV` (a
/// previously-open fd whose device vanished mid-sequence) or `ENOENT`
/// (the node was already gone by the time we tried to open it, e.g. the
/// countdown outlived an unplug). Either way the caller ends quietly
/// instead of reporting an error.
fn device_gone(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENODEV) | Some(libc::ENOENT))
}

fn map_err(e: std::io::Error, what: &str) -> DeviceError {
    if device_gone(&e) {
        DeviceError::Gone
    } else {
        DeviceError::Other(format!("{what}: {e}"))
    }
}

/// The open evdev node a sequence plays against, implementing
/// `fftest::FfDevice` over `EVIOCSFF`/`EVIOCRMFF`/`EVIOCGBIT` and `EV_FF`
/// writes.
struct EvdevFf {
    file: std::fs::File,
}

impl FfDevice for EvdevFf {
    fn set_gain(&mut self, value: i32) -> Result<(), DeviceError> {
        write_event(&mut self.file, fftest::FF_GAIN, value).map_err(|e| map_err(e, "set gain"))
    }

    fn set_autocenter(&mut self, value: i32) -> Result<(), DeviceError> {
        write_event(&mut self.file, fftest::FF_AUTOCENTER, value).map_err(|e| map_err(e, "set autocenter"))
    }

    fn upload(&mut self, effect: &FfEffect) -> Result<i16, DeviceError> {
        let mut effect = *effect;
        let fd = self.file.as_raw_fd();
        // SAFETY: fd is a valid open evdev fd; `effect` is a repr(C)
        // mirror of the kernel struct (layout unit-tested in
        // `logi_wheel_core::fftest`) and stays alive across the call.
        // The kernel writes the assigned id back through the same
        // pointer, which is why `effect` is a local mutable copy.
        let rc = unsafe { libc::ioctl(fd, EVIOCSFF, &mut effect as *mut FfEffect) };
        if rc < 0 {
            return Err(map_err(std::io::Error::last_os_error(), "upload effect"));
        }
        Ok(effect.id)
    }

    fn play(&mut self, id: i16, value: i32) -> Result<(), DeviceError> {
        write_event(&mut self.file, id as u16, value).map_err(|e| map_err(e, "play effect"))
    }

    fn erase(&mut self, id: i16) {
        // SAFETY: same fd; EVIOCRMFF takes the effect id by value.
        // Best-effort: run_sequence calls this as unconditional cleanup,
        // including after an error, so there is nothing left to do with
        // a failure here.
        let _ = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCRMFF, id as libc::c_ulong) };
    }

    fn ff_bits(&mut self) -> [u8; fftest::FF_BITS_LEN] {
        let mut bits = [0u8; fftest::FF_BITS_LEN];
        // SAFETY: same fd; the buffer is exactly FF_BITS_LEN bytes, what
        // the request number bakes in. A failed query (e.g. a device
        // that does not support EVIOCGBIT at all) just leaves every bit
        // clear, which run_sequence reads as "supports nothing" and
        // skips every step - a safe fallback, not a panic.
        let _ = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGBIT_FF, bits.as_mut_ptr()) };
        bits
    }
}

/// Open `path` and run `steps` against it end to end (see
/// `fftest::run_sequence`, each step counting down for its own
/// `SimStep::countdown` before it plays), reporting progress through
/// `on_event` as it goes. `model` resolves each step's logical direction
/// to the raw value its engine expects (see `fftest::resolve_direction`).
/// Blocking; callers run it on its own thread and cancel by setting the
/// shared flag. An open failure is folded into the same `SequenceOutcome`
/// shape a mid-run failure would produce, so callers have one path to
/// handle either.
pub fn run_test_sequence(
    path: &str,
    steps: &'static [fftest::SimStep],
    model: logi_wheel_core::WheelModel,
    cancel: &AtomicBool,
    on_event: impl FnMut(SequenceEvent),
) -> fftest::SequenceOutcome {
    let file = match std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(path) {
        Ok(f) => f,
        Err(e) => {
            let end = if device_gone(&e) {
                fftest::SequenceEnd::DeviceGone
            } else {
                fftest::SequenceEnd::Failed(format!("open {path}: {e}"))
            };
            return fftest::SequenceOutcome { end, ran: 0, skipped: Vec::new() };
        }
    };
    let mut device = EvdevFf { file };
    fftest::run_sequence(&mut device, steps, model, cancel, on_event)
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use logi_wheel_core::device::WheelModel;

    /// The direct-drive layout, which these tests were written against.
    fn apply_event_dd(snapshot: &mut Snapshot, chunk: &[u8], codes: &[u16]) -> bool {
        apply_event(snapshot, chunk, codes, evtest::pedal_axes(WheelModel::Rs50, None))
    }

    /// A G923's brake reaches the brake bar rather than the handbrake one.
    #[test]
    fn a_g923_pedal_lands_in_the_right_bar() {
        fn ev(type_: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
            let mut b = [0u8; EVENT_SIZE];
            b[16..18].copy_from_slice(&type_.to_le_bytes());
            b[18..20].copy_from_slice(&code.to_le_bytes());
            b[20..24].copy_from_slice(&value.to_le_bytes());
            b
        }
        let codes = evtest::button_codes_for_model(WheelModel::G923);
        let mut s = Snapshot { buttons: vec![false; codes.len()], ..Default::default() };
        let xbox = evtest::pedal_axes(WheelModel::G923, Some(0xc26e));
        assert!(apply_event(&mut s, &ev(3, evtest::ABS_Z, 200), &codes, xbox), "brake moved");
        assert_eq!(s.axes[1], 200, "ABS_Z is the brake on this wheel, not the handbrake");
        assert_eq!(s.axes[3], 0, "and nothing should reach the handbrake bar");
    }

    #[test]
    fn ioctl_request_numbers_match_linux_headers() {
        // Precomputed from <linux/input.h>: _IOW('E', 0x80, struct
        // ff_effect), _IOW('E', 0x81, int) and _IOR('E', 0x35, char[32])
        // (0x35 = 0x20 + EV_FF).
        assert_eq!(EVIOCSFF, 0x4030_4580);
        assert_eq!(EVIOCRMFF, 0x4004_4581);
        assert_eq!(EVIOCGBIT_FF, 0x8020_4535);
    }

    #[test]
    fn run_test_sequence_against_a_missing_node_ends_quietly_as_device_gone() {
        let cancel = AtomicBool::new(false);
        let outcome =
            run_test_sequence(
                "/nonexistent/event99",
                fftest::FORCE_SEQUENCE,
                logi_wheel_core::WheelModel::Rs50,
                &cancel,
                |_| {},
            );
        assert_eq!(outcome.end, fftest::SequenceEnd::DeviceGone);
        assert_eq!(outcome.ran, 0);
    }

    #[test]
    fn apply_event_tracks_buttons_axes_and_hat() {
        fn ev(type_: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
            let mut b = [0u8; EVENT_SIZE];
            b[16..18].copy_from_slice(&type_.to_le_bytes());
            b[18..20].copy_from_slice(&code.to_le_bytes());
            b[20..24].copy_from_slice(&value.to_le_bytes());
            b
        }
        let codes: Vec<u16> =
            logi_wheel_core::evtest::button_codes_for_model(logi_wheel_core::WheelModel::Rs50);
        let mut s = Snapshot { buttons: vec![false; codes.len()], ..Snapshot::default() };
        assert!(apply_event_dd(&mut s, &ev(1, 0x120, 1), &codes), "button A press");
        assert!(s.buttons[0]);
        assert!(apply_event_dd(&mut s, &ev(3, 0, 50000), &codes), "steering");
        assert_eq!(s.steering_raw, 50000);
        assert!(apply_event_dd(&mut s, &ev(3, evtest::ABS_RY, 12345), &codes), "brake");
        assert_eq!(s.axes[1], 12345);
        assert!(apply_event_dd(&mut s, &ev(3, evtest::ABS_HAT0Y, -1), &codes), "hat up");
        assert_eq!(s.hat, (0, -1));
        assert!(!apply_event_dd(&mut s, &ev(0, 0, 0), &codes), "SYN is not shown state");
        assert!(!apply_event_dd(&mut s, &ev(1, 0x12c, 1), &codes), "phantom button ignored (RS50)");
    }

    #[test]
    fn apply_event_tracks_g923_only_codes_the_rs50_table_would_drop() {
        // 0x2c3 and 0x2c4 (the G923's Plus/Minus buttons, live-captured
        // 2026-07-27) are not in WHEEL_BUTTONS at all - the RS50 has no
        // such buttons - but they are real G923 buttons; with the G923
        // code list they must track, not be silently ignored.
        fn ev(type_: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
            let mut b = [0u8; EVENT_SIZE];
            b[16..18].copy_from_slice(&type_.to_le_bytes());
            b[18..20].copy_from_slice(&code.to_le_bytes());
            b[20..24].copy_from_slice(&value.to_le_bytes());
            b
        }
        let codes: Vec<u16> =
            logi_wheel_core::evtest::button_codes_for_model(logi_wheel_core::WheelModel::G923);
        let mut s = Snapshot { buttons: vec![false; codes.len()], ..Snapshot::default() };
        assert!(apply_event_dd(&mut s, &ev(1, 0x2c3, 1), &codes), "tracked on a G923");
        assert!(apply_event_dd(&mut s, &ev(1, 0x2c4, 1), &codes), "tracked on a G923");
        // A descriptor-gap code the live G923 never actually sends is now
        // correctly absent from its own code list, so it is ignored - the
        // same as any code neither the wheel nor the code list knows.
        assert!(!apply_event_dd(&mut s, &ev(1, 0x12c, 1), &codes), "gap code not tracked");
    }
}
