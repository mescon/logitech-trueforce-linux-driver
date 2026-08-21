//! The Test view's state and device I/O: live wheel monitoring over the
//! wheel's evdev node, and the two guarded force-feedback test
//! sequences.
//!
//! The pure logic (event decoding, degrees, button names, discovery,
//! the step tables, `ff_effect` construction, and the rendered-plan
//! state machine [`fftest::SequenceProgress`] the running sequences feed)
//! comes from `logi_wheel_core` (`evtest` and `fftest`). This module owns
//! the open file handle the synchronous TUI polls each tick, the small
//! `EVIOCSFF`/`EVIOCRMFF`/`EVIOCGBIT` ioctl surface a sequence needs
//! (mirroring the GUI crate's `testio` module; kept out of core so it
//! stays dependency-free), and the sequence thread that folds every step
//! event into a shared [`fftest::SequenceProgress`] the view reads live
//! each draw.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use logi_wheel_core::evtest::{self, TestEvent, WheelInput, EVENT_SIZE};
use logi_wheel_core::fftest::{self, DeviceError, FfDevice, FfEffect, SequenceProgress};
pub use logi_wheel_core::fftest::SimKind;
use logi_wheel_core::WheelModel;

/// The Test view's whole state: discovery result, the monitor's open fd
/// and live input state, and the sim confirm/running flags.
pub struct TestView {
    /// The discovered wheel, `None` when no wheel is connected.
    pub dev: Option<WheelInput>,
    /// Whether discovery ran at least once (gates the empty-state text).
    pub scanned: bool,
    /// The open evdev node while monitoring, `None` while stopped.
    file: Option<std::fs::File>,
    /// Why the last monitor start failed (EACCES, ...), for the view.
    pub open_error: Option<String>,
    /// Raw steering axis (0..65535), seeded at center: a wheel at rest
    /// sends no reports at all.
    pub steering_raw: i32,
    /// `wheel_range` at last rescan (degrees, lock to lock).
    pub range: u32,
    /// The connected wheel's model at last rescan, read by the caller
    /// through its `Device` (same as `range`). Threaded into every
    /// sequence run so `fftest::run_sequence` resolves each step's
    /// direction against the right engine's convention (see
    /// `fftest::resolve_direction`).
    pub model: WheelModel,
    /// Currently-held buttons (evdev codes).
    pub pressed: BTreeSet<u16>,
    /// Most recent presses, newest first, capped at 8 (release keeps
    /// them listed; this is the "last pressed" history).
    pub recent: Vec<u16>,
    /// D-pad hat state (`ABS_HAT0X`, `ABS_HAT0Y`).
    pub hat: (i32, i32),
    /// Raw throttle/brake/clutch/handbrake values.
    pub axes: [i32; 4],
    /// A sequence waiting for its y/n confirmation.
    pub confirm: Option<SimKind>,
    /// The kind of the most recently confirmed sequence, set the instant
    /// `spawn_sim` starts it. Left in place after the run ends (unlike
    /// `sim_running`, which clears) so the finished plan - see
    /// `sim_progress` - stays on screen instead of disappearing; only the
    /// next `spawn_sim` replaces it.
    pub sim_kind: Option<SimKind>,
    /// Set while a sequence thread plays; cleared by the thread itself.
    sim_running: Arc<AtomicBool>,
    /// Set by `stop_sim` ('s' while playing, including during a step's
    /// countdown); the sequence thread polls it and stops + erases the
    /// current step's effect within one poll tick, ending the run
    /// without starting any further step. Re-armed (cleared) by the next
    /// `spawn_sim`.
    sim_cancel: Arc<AtomicBool>,
    /// Every row's live state for `sim_kind`'s step table (pending,
    /// counting down, playing, done, or skipped - see
    /// `fftest::SequenceProgress`), folded from the sequence thread's
    /// events. Read live by the view every draw (`sim_progress`); stays
    /// exactly as the thread left it once the run ends, which is what
    /// keeps a finished step's row visible instead of it disappearing.
    sim_progress: Arc<Mutex<SequenceProgress>>,
    /// Set exactly once by the sequence thread when it ends, holding the
    /// one-line summary for the main status line; taken (and cleared) by
    /// `tick_sim_status`.
    sim_final: Arc<Mutex<Option<String>>>,
}

impl Default for TestView {
    fn default() -> Self {
        TestView {
            dev: None,
            scanned: false,
            file: None,
            open_error: None,
            steering_raw: evtest::AXIS_MAX / 2,
            range: 900,
            model: WheelModel::default(),
            pressed: BTreeSet::new(),
            recent: Vec::new(),
            hat: (0, 0),
            axes: [0; 4],
            confirm: None,
            sim_kind: None,
            sim_running: Arc::new(AtomicBool::new(false)),
            sim_cancel: Arc::new(AtomicBool::new(false)),
            sim_progress: Arc::new(Mutex::new(SequenceProgress::new(&[]))),
            sim_final: Arc::new(Mutex::new(None)),
        }
    }
}

impl TestView {
    /// Re-run discovery (stopping any active monitor first). `range` and
    /// `model` are the wheel's configured rotation range and model, read
    /// by the caller through its `Device` (the sysfs side and the evdev
    /// side are independent).
    /// Restricted to the wheel at `usb_dir`, or unscoped when it is `None`.
    ///
    /// There is deliberately no unscoped convenience wrapper: with two
    /// wheels attached an unscoped scan returns whichever enumerated first,
    /// so the live monitor would show one wheel's steering while the app
    /// managed the other. Callers should say which wheel they mean.
    pub fn rescan_under(&mut self, range: u32, model: WheelModel, usb_dir: Option<&std::path::Path>) {
        self.stop_monitor();
        self.dev = evtest::discover_wheel_input_under(usb_dir);
        self.scanned = true;
        self.range = range;
        self.model = model;
        self.open_error = None;
    }

    /// Whether the monitor loop is live (the fd is open).
    pub fn monitoring(&self) -> bool {
        self.file.is_some()
    }

    pub fn sim_running(&self) -> bool {
        self.sim_running.load(Ordering::Relaxed)
    }

    /// The current step's label (or skip notice) while a sequence plays;
    /// `None` once it has stopped (the final summary goes through
    /// `tick_sim_status`/`self.status` instead).
    ///
    /// A clone of the current live progress (one row per step in
    /// `sim_kind`'s table): pending, counting down, playing, done, or
    /// skipped. Read fresh every draw while the Info page is open, which
    /// is what makes the countdown visibly tick down and a step's row
    /// flip to done in real time - no separate per-tick poll is needed.
    /// Stays exactly as the sequence thread left it once the run ends
    /// (see `sim_kind`'s doc comment), so a finished run's rows remain on
    /// screen instead of disappearing.
    pub fn sim_progress(&self) -> SequenceProgress {
        self.sim_progress.lock().unwrap().clone()
    }

    /// Test-only direct access to the live progress, for render-check
    /// tests (in this module and in `ui`) that need to show a specific
    /// mid-run-looking state (a done row, a skipped row, ...) without any
    /// real device I/O or sleeping - the sequence thread is the only
    /// other writer, and no test here runs one concurrently with this.
    #[cfg(test)]
    pub fn sim_progress_for_test(&self) -> std::sync::MutexGuard<'_, SequenceProgress> {
        self.sim_progress.lock().unwrap()
    }

    /// Take the just-finished sequence's one-line summary, if the thread
    /// has posted one since the last call. Run every tick (see
    /// `App::tick_sim_status`); a no-op most ticks.
    pub fn tick_sim_status(&mut self) -> Option<String> {
        self.sim_final.lock().unwrap().take()
    }

    /// Start monitoring: open the wheel's evdev node read-only and
    /// non-blocking. False (with `open_error` set) when the open fails;
    /// a no-op without a discovered wheel.
    pub fn start_monitor(&mut self) -> bool {
        let Some(dev) = &self.dev else { return false };
        match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&dev.event_path)
        {
            Ok(f) => {
                self.file = Some(f);
                self.open_error = None;
                self.reset_live_state();
                true
            }
            Err(e) => {
                self.open_error =
                    Some(format!("cannot open {}: {e} (needs read access to /dev/input)", dev.event_path));
                false
            }
        }
    }

    pub fn stop_monitor(&mut self) {
        self.file = None;
        self.confirm = None;
        self.reset_live_state();
    }

    fn reset_live_state(&mut self) {
        self.steering_raw = evtest::AXIS_MAX / 2;
        self.pressed.clear();
        self.recent.clear();
        self.hat = (0, 0);
        self.axes = [0; 4];
    }

    /// Drain every pending event from the open node into the live state.
    /// Called once per TUI tick while monitoring. Returns false when the
    /// device disappeared (the monitor is stopped and `dev` cleared, so
    /// the view falls back to the empty state).
    pub fn tick(&mut self) -> bool {
        // Take the fd out of `self` for the read loop (the borrow checker
        // cannot see that `apply` never touches `file`); it goes back in
        // on the WouldBlock exit, the only path that keeps monitoring.
        let Some(mut file) = self.file.take() else { return true };
        let mut buf = [0u8; EVENT_SIZE * 64];
        loop {
            match file.read(&mut buf) {
                Ok(0) => {
                    // EOF: the node went away under us.
                    self.stop_monitor();
                    self.dev = None;
                    return false;
                }
                Ok(n) => {
                    for chunk in buf[..n].chunks_exact(EVENT_SIZE) {
                        let event = evtest::parse_event(chunk);
                        self.apply(event);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.file = Some(file);
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.stop_monitor();
                    self.dev = None;
                    return false;
                }
            }
        }
    }

    /// Fold one decoded event into the live state.
    pub fn apply(&mut self, event: Option<TestEvent>) {
        match event {
            Some(TestEvent::Steering(raw)) => self.steering_raw = raw,
            Some(TestEvent::Button { code, pressed }) => {
                if pressed {
                    self.pressed.insert(code);
                    self.recent.retain(|c| *c != code);
                    self.recent.insert(0, code);
                    self.recent.truncate(8);
                } else {
                    self.pressed.remove(&code);
                }
            }
            Some(TestEvent::Axis { code, value }) => {
                // Which axis is which pedal depends on the wheel, and the
                // two G923 editions differ from each other as well as from
                // the direct-drive wheels. Reading them by position put a
                // G923's brake in the handbrake bar and left its throttle
                // out entirely (issue #68).
                let axes = evtest::pedal_axes_for_name(
                    self.model,
                    self.dev.as_ref().map(|d| d.name.as_str()).unwrap_or(""),
                );
                match code {
                    evtest::ABS_HAT0X => self.hat.0 = value,
                    evtest::ABS_HAT0Y => self.hat.1 = value,
                    c if c == axes.throttle => self.axes[0] = value,
                    c if c == axes.brake => self.axes[1] = value,
                    c if c == axes.clutch => self.axes[2] = value,
                    c if Some(c) == axes.handbrake => self.axes[3] = value,
                    _ => {}
                }
            }
            None => {}
        }
    }

    /// The live steering angle in signed degrees (0 = center).
    pub fn degrees(&self) -> f32 {
        evtest::steering_degrees(self.steering_raw, 0, evtest::AXIS_MAX, self.range)
    }

    /// Spawn the confirmed sequence on its own thread (the TUI's event
    /// loop must keep drawing while it plays) and return the status line
    /// to show immediately. The full plan (every row `Pending`) is
    /// visible the instant this returns, before the thread has even
    /// opened the device - see `sim_progress`'s doc comment - and the
    /// thread's own first act is row 0's own `SimStep::countdown` lead-in,
    /// so nothing plays without a visible countdown, including
    /// the very first step. The thread runs every runnable step to
    /// completion, cancellation, or device-gone, folding each step event
    /// into `sim_progress` and posting a final summary into `sim_final`
    /// (`tick_sim_status` picks that up); a device that vanishes mid-run
    /// cleans up silently, matching the summary's wording.
    pub fn spawn_sim(&mut self, kind: SimKind) -> String {
        let Some(dev) = &self.dev else { return "test: no wheel".to_string() };
        if self.sim_running() {
            return "test: a simulation is already playing (s to stop)".to_string();
        }
        self.sim_running.store(true, Ordering::Relaxed);
        self.sim_cancel.store(false, Ordering::Relaxed);
        let steps = kind.steps();
        let model = self.model;
        self.sim_kind = Some(kind);
        *self.sim_progress.lock().unwrap() = SequenceProgress::new(steps);
        *self.sim_final.lock().unwrap() = None;

        let path = dev.event_path.clone();
        let running = self.sim_running.clone();
        let cancel = self.sim_cancel.clone();
        let progress = self.sim_progress.clone();
        let final_status = self.sim_final.clone();
        std::thread::spawn(move || {
            let outcome = run_test_sequence(&path, steps, model, &cancel, &progress);
            *final_status.lock().unwrap() = Some(format!("test: {} {}", kind.label(), outcome.summary()));
            running.store(false, Ordering::Relaxed);
        });
        format!("test: playing {} ({} steps; s to stop)...", kind.label(), steps.len())
    }

    /// Stop the playing sequence ('s' in the Info view): flag the sim
    /// thread, which - whether it is currently in a step's countdown or
    /// actually playing one - stops and erases within one poll tick and
    /// ends the run without starting anything further. True when
    /// something was playing, false for a no-op.
    pub fn stop_sim(&self) -> bool {
        if !self.sim_running() {
            return false;
        }
        self.sim_cancel.store(true, Ordering::Relaxed);
        true
    }
}

// ---------------------------------------------------------------------------
// Force-feedback sequence I/O: the evdev file handle and ioctls that back
// `logi_wheel_core::fftest`'s `FfDevice` trait. The step tables and the
// `ff_effect` byte layout live in core (shared with the GUI's `testio`);
// only this device plumbing is per-front-end.
// ---------------------------------------------------------------------------

const EVIOCGBIT_FF_NR: u8 = 0x20 + fftest::EV_FF as u8;

/// `_IOW('E', nr, T)` as `linux/ioctl.h` encodes it on x86_64.
const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

/// `_IOR('E', nr, T)`, same encoding with the read-direction bits set.
const fn ior(nr: u8, size: usize) -> libc::c_ulong {
    (2 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

const EVIOCSFF: libc::c_ulong = iow(0x80, std::mem::size_of::<FfEffect>());
const EVIOCRMFF: libc::c_ulong = iow(0x81, std::mem::size_of::<libc::c_int>());
const EVIOCGBIT_FF: libc::c_ulong = ior(EVIOCGBIT_FF_NR, fftest::FF_BITS_LEN);

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
/// `SimStep::countdown` before it plays), folding each event into
/// `progress` as it goes. `model` resolves each step's logical direction
/// to the raw value its engine expects (see `fftest::resolve_direction`).
/// Blocking; the caller runs it on its own thread. An open failure is
/// folded into the same `SequenceOutcome` shape a mid-run failure would
/// produce, so callers have one path to handle either.
fn run_test_sequence(
    path: &str,
    steps: &'static [fftest::SimStep],
    model: WheelModel,
    cancel: &AtomicBool,
    progress: &Arc<Mutex<SequenceProgress>>,
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
    fftest::run_sequence(&mut device, steps, model, cancel, |ev| {
        progress.lock().unwrap().apply(&ev);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn event(type_: u16, code: u16, value: i32) -> Option<TestEvent> {
        let mut b = [0u8; EVENT_SIZE];
        b[16..18].copy_from_slice(&type_.to_le_bytes());
        b[18..20].copy_from_slice(&code.to_le_bytes());
        b[20..24].copy_from_slice(&value.to_le_bytes());
        evtest::parse_event(&b)
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
    fn apply_tracks_steering_buttons_and_axes() {
        let mut v = TestView::default();
        assert_eq!(v.steering_raw, evtest::AXIS_MAX / 2, "starts centered");
        v.apply(event(3, 0, 60000));
        assert_eq!(v.steering_raw, 60000);
        v.apply(event(1, 0x120, 1));
        v.apply(event(1, 0x125, 1));
        assert!(v.pressed.contains(&0x120));
        assert_eq!(v.recent, vec![0x125, 0x120], "newest first");
        v.apply(event(1, 0x120, 0));
        assert!(!v.pressed.contains(&0x120));
        assert_eq!(v.recent.len(), 2, "release keeps history");
        v.apply(event(3, evtest::ABS_RY, 30000));
        assert_eq!(v.axes[1], 30000);
        v.apply(event(3, evtest::ABS_HAT0X, 1));
        assert_eq!(v.hat, (1, 0));
    }

    #[test]
    fn degrees_use_the_configured_range() {
        let mut v =
            TestView { range: 900, steering_raw: evtest::AXIS_MAX, ..TestView::default() };
        assert!((v.degrees() - 450.0).abs() < 0.01);
        v.range = 1080;
        assert!((v.degrees() - 540.0).abs() < 0.01);
    }

    #[test]
    fn spawn_sim_without_a_wheel_reports_instead_of_playing() {
        let mut v = TestView::default();
        let status = v.spawn_sim(SimKind::Force);
        assert!(status.contains("no wheel"), "status: {status}");
        assert!(!v.sim_running());
    }

    #[test]
    fn stop_sim_is_a_no_op_while_nothing_plays() {
        let v = TestView::default();
        assert!(!v.stop_sim());
        assert!(!v.sim_cancel.load(Ordering::Relaxed), "flag stays unarmed");
    }

    #[test]
    fn stop_sim_flags_a_playing_sim() {
        let v = TestView::default();
        v.sim_running.store(true, Ordering::Relaxed);
        assert!(v.stop_sim());
        assert!(v.sim_cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn sim_progress_is_empty_before_anything_is_confirmed() {
        let v = TestView::default();
        assert_eq!(v.sim_kind, None);
        assert!(v.sim_progress().states.is_empty());
    }

    #[test]
    fn tick_sim_status_takes_the_final_summary_once() {
        let mut v = TestView::default();
        assert_eq!(v.tick_sim_status(), None, "nothing posted yet");
        *v.sim_final.lock().unwrap() = Some("test: force feedback finished".to_string());
        assert_eq!(v.tick_sim_status(), Some("test: force feedback finished".to_string()));
        assert_eq!(v.tick_sim_status(), None, "taken, not re-read");
    }

    #[test]
    fn spawn_sim_shows_the_whole_plan_pending_before_the_thread_touches_the_device() {
        // The full plan (every row Pending) must be in place the instant
        // spawn_sim returns - set synchronously, before the background
        // thread ever opens the device - so a front-end can render "here
        // is what is about to happen" with no race against the thread.
        let mut v = TestView {
            dev: Some(WheelInput {
                event_path: "/nonexistent/event99".to_string(),
                name: "Logitech RS50 Base".to_string(),
            }),
            ..TestView::default()
        };
        let status = v.spawn_sim(SimKind::Force);
        assert!(status.contains("playing"), "status: {status}");
        assert_eq!(v.sim_kind, Some(SimKind::Force));
        let progress = v.sim_progress();
        assert_eq!(progress.states.len(), fftest::FORCE_SEQUENCE.len());
        assert!(progress.states.iter().all(|s| *s == fftest::StepState::Pending));
    }

    #[test]
    fn spawn_sim_end_to_end_posts_its_final_summary_for_tick_sim_status_to_pick_up() {
        // spawn_sim starts the background thread, which (against a
        // device that is not really there) ends quickly as "gone" and
        // posts a summary that `tick_sim_status` (polled by the main
        // loop) then surfaces.
        let mut v = TestView {
            dev: Some(WheelInput {
                event_path: "/nonexistent/event99".to_string(),
                name: "Logitech RS50 Base".to_string(),
            }),
            ..TestView::default()
        };
        let status = v.spawn_sim(SimKind::Force);
        assert!(status.contains("playing"), "status: {status}");
        // Deliberately no `assert!(v.sim_running())` here: the worker
        // fails its open() immediately against a nonexistent node and
        // clears the flag, so whether it is still set by the time this
        // thread looks is a race the test must not depend on. What the
        // flag does synchronously is covered by
        // `stop_sim_flags_a_playing_sim`.

        // The sequence thread runs on its own; wait it out (bounded, no
        // synchronous join point from here).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while v.sim_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!v.sim_running(), "must finish against a nonexistent node quickly");

        let summary = v.tick_sim_status().expect("a final summary was posted");
        assert!(summary.contains("force feedback"), "summary: {summary}");
        assert!(summary.contains("disconnected"), "ENOENT reads as gone, not a raw error: {summary}");
        assert_eq!(v.tick_sim_status(), None, "taken, not re-read");

        // The plan stays on screen after the run ends: an open failure
        // never even reaches ff_bits(), so every row is still Pending -
        // still there, not cleared away, which is the point.
        assert_eq!(v.sim_kind, Some(SimKind::Force));
        assert_eq!(v.sim_progress().states.len(), fftest::FORCE_SEQUENCE.len());
    }

    #[test]
    fn run_test_sequence_against_a_missing_node_ends_quietly_as_device_gone() {
        // ENOENT on open (no such node at all) is folded into the same
        // "gone" outcome ENODEV mid-run would produce, so a wheel that
        // was already unplugged before it was confirmed behaves the same
        // as one unplugged mid-sequence.
        let cancel = AtomicBool::new(false);
        let progress = Arc::new(Mutex::new(fftest::SequenceProgress::new(fftest::FORCE_SEQUENCE)));
        let outcome = run_test_sequence(
            "/nonexistent/event99",
            fftest::FORCE_SEQUENCE,
            WheelModel::Rs50,
            &cancel,
            &progress,
        );
        assert_eq!(outcome.end, fftest::SequenceEnd::DeviceGone);
        assert_eq!(outcome.ran, 0);
    }

    #[test]
    fn start_monitor_without_a_wheel_is_a_no_op() {
        let mut v = TestView::default();
        assert!(!v.start_monitor());
        assert!(!v.monitoring());
    }

    #[test]
    fn start_monitor_surfaces_an_unopenable_node() {
        let mut v = TestView {
            dev: Some(WheelInput {
                event_path: "/nonexistent/event99".to_string(),
                name: "Logitech RS50 Base".to_string(),
            }),
            ..TestView::default()
        };
        assert!(!v.start_monitor());
        assert!(v.open_error.as_deref().unwrap_or("").contains("cannot open"));
    }
}
