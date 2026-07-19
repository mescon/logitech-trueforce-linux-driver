//! The Test page's device I/O: the evdev reader thread and the two
//! guarded force-feedback simulations.
//!
//! All the pure logic (event decoding, degree conversion, button naming)
//! lives in `logi_dd_core::evtest`; this module owns the file handles,
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

use logi_dd_core::evtest::{self, TestEvent, EVENT_SIZE, WHEEL_BUTTONS};

/// One UI push worth of live wheel state. `buttons` is parallel to
/// `evtest::WHEEL_BUTTONS`; `axes` holds throttle/brake/clutch/handbrake
/// raw values in that order.
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
    /// Open `path` and start the read loop. Fails fast (EACCES, ENOENT)
    /// so permission problems surface inline instead of in a dead page.
    pub fn start(
        path: &str,
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
                buttons: vec![false; WHEEL_BUTTONS.len()],
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
                        for chunk in buf[..n].chunks_exact(EVENT_SIZE) {
                            if apply_event(&mut snapshot, chunk) {
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
fn apply_event(snapshot: &mut Snapshot, chunk: &[u8]) -> bool {
    match evtest::parse_event(chunk) {
        Some(TestEvent::Steering(raw)) => {
            snapshot.steering_raw = raw;
            true
        }
        Some(TestEvent::Button { code, pressed }) => {
            match WHEEL_BUTTONS.iter().position(|(c, _)| *c == code) {
                Some(i) => {
                    snapshot.buttons[i] = pressed;
                    true
                }
                None => false,
            }
        }
        Some(TestEvent::Axis { code, value }) => match code {
            evtest::ABS_HAT0X => {
                snapshot.hat.0 = value;
                true
            }
            evtest::ABS_HAT0Y => {
                snapshot.hat.1 = value;
                true
            }
            evtest::ABS_RX => {
                snapshot.axes[0] = value;
                true
            }
            evtest::ABS_RY => {
                snapshot.axes[1] = value;
                true
            }
            evtest::ABS_RZ => {
                snapshot.axes[2] = value;
                true
            }
            evtest::ABS_Z => {
                snapshot.axes[3] = value;
                true
            }
            _ => false,
        },
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Force-feedback simulation.
// ---------------------------------------------------------------------------

/// Which canned effect a confirmed simulation plays. Both are fixed at
/// ~25% magnitude for 2 seconds; nothing here is user-tunable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimKind {
    /// A constant pull to one side (`FF_CONSTANT`).
    ConstantForce,
    /// A rumble-style sine texture played through the FFB path
    /// (`FF_PERIODIC`/`FF_SINE`).
    Texture,
}

/// ~25% of the i16 full scale.
const SIM_LEVEL: i16 = 0x2000;
/// How long the effect plays.
const SIM_DURATION_MS: u16 = 2000;
/// The sine texture's period (25 ms = 40 Hz, a gritty rumble).
const SIM_PERIOD_MS: u16 = 25;
/// How often the playback wait re-checks the cancel flag.
const SIM_CANCEL_POLL: Duration = Duration::from_millis(10);

const EV_FF: u16 = 0x15;
const FF_PERIODIC: u16 = 0x51;
const FF_CONSTANT: u16 = 0x52;
const FF_SINE: u16 = 0x5a;
const FF_GAIN: u16 = 0x60;

/// Size of the union at the end of `struct ff_effect` (the largest
/// member, `ff_periodic_effect`, is 32 bytes on a 64-bit kernel).
const FF_UNION_SIZE: usize = 32;

/// Mirrors the kernel's `struct ff_effect` (`linux/input.h`): type, id,
/// direction, trigger (button+interval), replay (length+delay), then the
/// union as an 8-byte-aligned byte array (see the module doc).
#[repr(C)]
struct FfEffect {
    type_: u16,
    id: i16,
    direction: u16,
    trigger_button: u16,
    trigger_interval: u16,
    replay_length: u16,
    replay_delay: u16,
    u: FfUnion,
}

#[repr(C, align(8))]
struct FfUnion([u8; FF_UNION_SIZE]);

/// `_IOW('E', nr, T)`: write-direction ioctl request number, as
/// `linux/ioctl.h` encodes it on x86_64 (dir 1 in the top 2 bits, size
/// in bits 16..30, magic 'E' in bits 8..16, nr in the low byte).
const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

/// `EVIOCSFF` (`_IOW('E', 0x80, struct ff_effect)`).
const EVIOCSFF: libc::c_ulong = iow(0x80, std::mem::size_of::<FfEffect>());
/// `EVIOCRMFF` (`_IOW('E', 0x81, int)`).
const EVIOCRMFF: libc::c_ulong = iow(0x81, std::mem::size_of::<libc::c_int>());

/// Build the fixed test effect for `kind`, id -1 (kernel assigns one).
fn sim_effect(kind: SimKind) -> FfEffect {
    let mut u = [0u8; FF_UNION_SIZE];
    let type_ = match kind {
        SimKind::ConstantForce => {
            // ff_constant_effect: level:i16 @0, envelope zeroed.
            u[0..2].copy_from_slice(&SIM_LEVEL.to_le_bytes());
            FF_CONSTANT
        }
        SimKind::Texture => {
            // ff_periodic_effect: waveform:u16 @0, period:u16 @2,
            // magnitude:i16 @4; offset/phase/envelope zeroed.
            u[0..2].copy_from_slice(&FF_SINE.to_le_bytes());
            u[2..4].copy_from_slice(&SIM_PERIOD_MS.to_le_bytes());
            u[4..6].copy_from_slice(&SIM_LEVEL.to_le_bytes());
            FF_PERIODIC
        }
    };
    FfEffect {
        type_,
        id: -1,
        // 0x4000 = 90 degrees; for a single-axis wheel this just picks a
        // pull direction, the magnitude stays SIM_LEVEL.
        direction: 0x4000,
        trigger_button: 0,
        trigger_interval: 0,
        replay_length: SIM_DURATION_MS,
        replay_delay: 0,
        u: FfUnion(u),
    }
}

/// Encode one `struct input_event` (64-bit ABI) with a zeroed timestamp;
/// the kernel fills timestamps in itself for written FF events.
fn encode_ff_event(code: u16, value: i32) -> [u8; EVENT_SIZE] {
    let mut b = [0u8; EVENT_SIZE];
    b[16..18].copy_from_slice(&EV_FF.to_le_bytes());
    b[18..20].copy_from_slice(&code.to_le_bytes());
    b[20..24].copy_from_slice(&value.to_le_bytes());
    b
}

/// True for the errno that means the wheel went away mid-simulation; the
/// caller cleans up silently instead of reporting an error.
fn device_gone(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENODEV))
}

/// How a playback wait ended: the effect ran its full 2 s, or the user
/// pressed Stop and `cancel` flipped mid-play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Completed,
    Cancelled,
}

/// Sleep out `duration` in [`SIM_CANCEL_POLL`] ticks, returning early as
/// soon as `cancel` flips. This is the sim's whole cancel state machine:
/// both outcomes fall through to the same single cleanup site in
/// [`run_simulation`], so complete-then-stop and cancel-then-stop clean
/// up exactly once each.
fn wait_out(duration: Duration, cancel: &AtomicBool) -> WaitOutcome {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return WaitOutcome::Cancelled;
        }
        std::thread::sleep(SIM_CANCEL_POLL.min(duration));
    }
    WaitOutcome::Completed
}

/// Play `kind` on the wheel at `path`: upload, play, wait out the 2 s
/// duration (or until `cancel` flips), then always stop and erase the
/// effect (also on every error path). Blocking; callers run it on its
/// own thread and cancel by setting the shared flag. A device that
/// disappears mid-sim returns `Ok` (nothing left to clean up); a
/// cancelled run is also `Ok` (the user asked for the stop).
pub fn run_simulation(path: &str, kind: SimKind, cancel: &AtomicBool) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("open {path}: {e}"))?;
    let fd = file.as_raw_fd();

    // The driver powers up with device gain unset and other tools (the
    // logi-ffb proxy) zero it on shutdown; a zero gain would make the
    // test silently do nothing, so assert full gain first.
    write_event(&mut file, FF_GAIN, 0xFFFF).map_err(|e| format!("set gain: {e}"))?;

    let mut effect = sim_effect(kind);
    // SAFETY: fd is a valid open evdev fd and `effect` is a repr(C)
    // mirror of the kernel's struct ff_effect (layout unit-tested
    // below); the kernel writes the assigned id back through the
    // pointer, which is why it must point at a mutable value.
    let rc = unsafe { libc::ioctl(fd, EVIOCSFF, &mut effect as *mut FfEffect) };
    if rc < 0 {
        let e = std::io::Error::last_os_error();
        return if device_gone(&e) { Ok(()) } else { Err(format!("upload effect: {e}")) };
    }
    let id = effect.id;

    // Play, then wait for completion or cancellation; either way control
    // falls through to the one cleanup site below.
    let outcome = write_event(&mut file, id as u16, 1);
    if outcome.is_ok() {
        wait_out(Duration::from_millis(u64::from(SIM_DURATION_MS)), cancel);
    }

    // Unconditional cleanup: stop, then erase, whatever happened above
    // (full 2 s, cancel, or a failed play write).
    let _ = write_event(&mut file, id as u16, 0);
    // SAFETY: same fd; EVIOCRMFF takes the effect id by value.
    let _ = unsafe { libc::ioctl(fd, EVIOCRMFF, id as libc::c_ulong) };

    match outcome {
        Ok(()) => Ok(()),
        Err(e) if device_gone(&e) => Ok(()),
        Err(e) => Err(format!("play effect: {e}")),
    }
}

/// Write one `EV_FF` event (play/stop/gain) to the device.
fn write_event(file: &mut std::fs::File, code: u16, value: i32) -> std::io::Result<()> {
    file.write_all(&encode_ff_event(code, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ff_effect_layout_matches_kernel_abi() {
        // sizeof(struct ff_effect) == 48, alignof == 8, union at offset
        // 16 on a 64-bit kernel; the EVIOCSFF request number bakes the
        // size in, so a mismatch fails the ioctl outright.
        assert_eq!(std::mem::size_of::<FfEffect>(), 48);
        assert_eq!(std::mem::align_of::<FfEffect>(), 8);
        let e = sim_effect(SimKind::ConstantForce);
        let union_offset = (&e.u as *const _ as usize) - (&e as *const _ as usize);
        assert_eq!(union_offset, 16);
    }

    #[test]
    fn ioctl_request_numbers_match_linux_headers() {
        // Precomputed from <linux/input.h>: _IOW('E', 0x80, struct
        // ff_effect) and _IOW('E', 0x81, int).
        assert_eq!(EVIOCSFF, 0x4030_4580);
        assert_eq!(EVIOCRMFF, 0x4004_4581);
    }

    #[test]
    fn sim_effects_are_gentle_and_bounded() {
        let c = sim_effect(SimKind::ConstantForce);
        assert_eq!(c.type_, FF_CONSTANT);
        assert_eq!(c.replay_length, 2000);
        assert_eq!(i16::from_le_bytes([c.u.0[0], c.u.0[1]]), 0x2000, "~25% level");

        let t = sim_effect(SimKind::Texture);
        assert_eq!(t.type_, FF_PERIODIC);
        assert_eq!(u16::from_le_bytes([t.u.0[0], t.u.0[1]]), FF_SINE);
        assert_eq!(i16::from_le_bytes([t.u.0[4], t.u.0[5]]), 0x2000, "~25% magnitude");
        assert_eq!(t.replay_length, 2000);
    }

    #[test]
    fn wait_out_completes_when_never_cancelled() {
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        assert_eq!(wait_out(Duration::from_millis(30), &cancel), WaitOutcome::Completed);
        assert!(start.elapsed() >= Duration::from_millis(30), "waits the full duration");
    }

    #[test]
    fn wait_out_returns_early_on_a_preset_cancel() {
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        assert_eq!(wait_out(Duration::from_secs(10), &cancel), WaitOutcome::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(1), "does not sleep out the duration");
    }

    #[test]
    fn wait_out_reacts_to_a_mid_play_cancel() {
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            thread_cancel.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        assert_eq!(wait_out(Duration::from_secs(10), &cancel), WaitOutcome::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(1), "cancel cuts the wait short");
        stopper.join().unwrap();
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
        let mut s = Snapshot { buttons: vec![false; WHEEL_BUTTONS.len()], ..Snapshot::default() };
        assert!(apply_event(&mut s, &ev(1, 0x120, 1)), "button A press");
        assert!(s.buttons[0]);
        assert!(apply_event(&mut s, &ev(3, 0, 50000)), "steering");
        assert_eq!(s.steering_raw, 50000);
        assert!(apply_event(&mut s, &ev(3, evtest::ABS_RY, 12345)), "brake");
        assert_eq!(s.axes[1], 12345);
        assert!(apply_event(&mut s, &ev(3, evtest::ABS_HAT0Y, -1)), "hat up");
        assert_eq!(s.hat, (0, -1));
        assert!(!apply_event(&mut s, &ev(0, 0, 0)), "SYN is not shown state");
        assert!(!apply_event(&mut s, &ev(1, 0x12c, 1)), "phantom button ignored");
    }
}
