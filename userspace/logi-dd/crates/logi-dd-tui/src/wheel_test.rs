//! The Test view's state and device I/O: live wheel monitoring over the
//! wheel's evdev node, and the two guarded force-feedback simulations.
//!
//! The pure logic (event decoding, degrees, button names, discovery)
//! comes from `logi_dd_core::evtest`. This module owns the open file
//! handle the synchronous TUI polls each tick, and the small
//! `EVIOCSFF`/`EVIOCRMFF` ioctl surface the simulations need (mirroring
//! the GUI crate's `testio` module; kept out of core so it stays
//! dependency-free).

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use logi_dd_core::evtest::{self, TestEvent, WheelInput, EVENT_SIZE};

/// Which canned effect a confirmed simulation plays. Both are fixed at
/// ~25% magnitude for 2 seconds; nothing is user-tunable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimKind {
    /// A constant pull to one side (`FF_CONSTANT`).
    ConstantForce,
    /// A rumble-style sine texture through the FFB path
    /// (`FF_PERIODIC`/`FF_SINE`).
    Texture,
}

impl SimKind {
    pub fn label(self) -> &'static str {
        match self {
            SimKind::ConstantForce => "constant force",
            SimKind::Texture => "TrueForce texture",
        }
    }
}

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
    /// Currently-held buttons (evdev codes).
    pub pressed: BTreeSet<u16>,
    /// Most recent presses, newest first, capped at 8 (release keeps
    /// them listed; this is the "last pressed" history).
    pub recent: Vec<u16>,
    /// D-pad hat state (`ABS_HAT0X`, `ABS_HAT0Y`).
    pub hat: (i32, i32),
    /// Raw throttle/brake/clutch/handbrake values.
    pub axes: [i32; 4],
    /// A sim waiting for its y/n confirmation.
    pub confirm: Option<SimKind>,
    /// Set while a sim thread plays; cleared by the thread itself.
    sim_running: Arc<AtomicBool>,
    /// Set by `stop_sim` ('s' while playing); the sim thread polls it and
    /// stops + erases the effect early. Re-armed (cleared) by the next
    /// `spawn_sim`.
    sim_cancel: Arc<AtomicBool>,
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
            pressed: BTreeSet::new(),
            recent: Vec::new(),
            hat: (0, 0),
            axes: [0; 4],
            confirm: None,
            sim_running: Arc::new(AtomicBool::new(false)),
            sim_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl TestView {
    /// Re-run discovery (stopping any active monitor first). `range` is
    /// the wheel's configured rotation range, read by the caller through
    /// its `Device` (the sysfs side and the evdev side are independent).
    pub fn rescan(&mut self, range: u32) {
        self.stop_monitor();
        self.dev = evtest::discover_wheel_input();
        self.scanned = true;
        self.range = range;
        self.open_error = None;
    }

    /// Whether the monitor loop is live (the fd is open).
    pub fn monitoring(&self) -> bool {
        self.file.is_some()
    }

    pub fn sim_running(&self) -> bool {
        self.sim_running.load(Ordering::Relaxed)
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
            Some(TestEvent::Axis { code, value }) => match code {
                evtest::ABS_HAT0X => self.hat.0 = value,
                evtest::ABS_HAT0Y => self.hat.1 = value,
                evtest::ABS_RX => self.axes[0] = value,
                evtest::ABS_RY => self.axes[1] = value,
                evtest::ABS_RZ => self.axes[2] = value,
                evtest::ABS_Z => self.axes[3] = value,
                _ => {}
            },
            None => {}
        }
    }

    /// The live steering angle in signed degrees (0 = center).
    pub fn degrees(&self) -> f32 {
        evtest::steering_degrees(self.steering_raw, 0, evtest::AXIS_MAX, self.range)
    }

    /// Spawn the confirmed simulation on its own thread (the TUI's event
    /// loop must keep drawing while the 2 s effect plays) and return the
    /// status line to show. The thread stops + erases the effect on
    /// every path (full duration, `stop_sim`, errors); a device that
    /// vanished mid-sim cleans up silently.
    pub fn spawn_sim(&mut self, kind: SimKind) -> String {
        let Some(dev) = &self.dev else { return "test: no wheel".to_string() };
        if self.sim_running() {
            return "test: a simulation is already playing (s to stop)".to_string();
        }
        self.sim_running.store(true, Ordering::Relaxed);
        self.sim_cancel.store(false, Ordering::Relaxed);
        let path = dev.event_path.clone();
        let running = self.sim_running.clone();
        let cancel = self.sim_cancel.clone();
        std::thread::spawn(move || {
            let _ = run_simulation(&path, kind, &cancel);
            running.store(false, Ordering::Relaxed);
        });
        format!("test: playing {} (25%, 2 s; s to stop)...", kind.label())
    }

    /// Stop the playing simulation ('s' in the Info view): flag the sim
    /// thread, which stops + erases the effect within its poll tick.
    /// True when something was playing, false for a no-op.
    pub fn stop_sim(&self) -> bool {
        if !self.sim_running() {
            return false;
        }
        self.sim_cancel.store(true, Ordering::Relaxed);
        true
    }
}

// ---------------------------------------------------------------------------
// Force-feedback simulation (same fixed effects as the GUI's testio).
// ---------------------------------------------------------------------------

/// ~25% of the i16 full scale.
const SIM_LEVEL: i16 = 0x2000;
const SIM_DURATION_MS: u16 = 2000;
/// The sine texture's period (25 ms = 40 Hz).
const SIM_PERIOD_MS: u16 = 25;
/// How often the playback wait re-checks the cancel flag.
const SIM_CANCEL_POLL: Duration = Duration::from_millis(10);

const EV_FF: u16 = 0x15;
const FF_PERIODIC: u16 = 0x51;
const FF_CONSTANT: u16 = 0x52;
const FF_SINE: u16 = 0x5a;
const FF_GAIN: u16 = 0x60;

const FF_UNION_SIZE: usize = 32;

/// Mirrors the kernel's `struct ff_effect` (`linux/input.h`); the
/// trailing union is an 8-byte-aligned byte array written via explicit
/// offsets (same convention as the ffb-proxy crate's sink module).
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

/// `_IOW('E', nr, T)` as `linux/ioctl.h` encodes it on x86_64.
const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

const EVIOCSFF: libc::c_ulong = iow(0x80, std::mem::size_of::<FfEffect>());
const EVIOCRMFF: libc::c_ulong = iow(0x81, std::mem::size_of::<libc::c_int>());

fn sim_effect(kind: SimKind) -> FfEffect {
    let mut u = [0u8; FF_UNION_SIZE];
    let type_ = match kind {
        SimKind::ConstantForce => {
            // ff_constant_effect: level:i16 @0, envelope zeroed.
            u[0..2].copy_from_slice(&SIM_LEVEL.to_le_bytes());
            FF_CONSTANT
        }
        SimKind::Texture => {
            // ff_periodic_effect: waveform @0, period @2, magnitude @4.
            u[0..2].copy_from_slice(&FF_SINE.to_le_bytes());
            u[2..4].copy_from_slice(&SIM_PERIOD_MS.to_le_bytes());
            u[4..6].copy_from_slice(&SIM_LEVEL.to_le_bytes());
            FF_PERIODIC
        }
    };
    FfEffect {
        type_,
        id: -1,
        direction: 0x4000,
        trigger_button: 0,
        trigger_interval: 0,
        replay_length: SIM_DURATION_MS,
        replay_delay: 0,
        u: FfUnion(u),
    }
}

fn encode_ff_event(code: u16, value: i32) -> [u8; EVENT_SIZE] {
    let mut b = [0u8; EVENT_SIZE];
    b[16..18].copy_from_slice(&EV_FF.to_le_bytes());
    b[18..20].copy_from_slice(&code.to_le_bytes());
    b[20..24].copy_from_slice(&value.to_le_bytes());
    b
}

fn write_event(file: &mut std::fs::File, code: u16, value: i32) -> std::io::Result<()> {
    file.write_all(&encode_ff_event(code, value))
}

/// How a playback wait ended: the effect ran its full 2 s, or the user
/// pressed 's' and `cancel` flipped mid-play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Completed,
    Cancelled,
}

/// Sleep out `duration` in [`SIM_CANCEL_POLL`] ticks, returning early as
/// soon as `cancel` flips. Both outcomes fall through to the same single
/// cleanup site in [`run_simulation`], so complete-then-stop and
/// cancel-then-stop clean up exactly once each.
fn wait_out(duration: Duration, cancel: &AtomicBool) -> WaitOutcome {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return WaitOutcome::Cancelled;
        }
        std::thread::sleep(SIM_CANCEL_POLL.min(duration));
    }
    WaitOutcome::Completed
}

/// Play `kind` on the wheel at `path`: upload, play, wait the fixed 2 s
/// (or until `cancel` flips), then always stop and erase (also on every
/// error path). Blocking; the caller runs it on a thread. A device that
/// disappears mid-sim (`ENODEV`) is a silent cleanup, not an error; a
/// cancelled run is `Ok` too (the user asked for the stop).
fn run_simulation(path: &str, kind: SimKind, cancel: &AtomicBool) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("open {path}: {e}"))?;
    let fd = file.as_raw_fd();

    // Device gain powers up unset and other tools may have zeroed it; a
    // zero gain would make the test silently do nothing.
    write_event(&mut file, FF_GAIN, 0xFFFF).map_err(|e| format!("set gain: {e}"))?;

    let mut effect = sim_effect(kind);
    // SAFETY: fd is a valid open evdev fd; `effect` is a repr(C) mirror
    // of the kernel struct (layout unit-tested below) and stays alive
    // across the call. The kernel writes the assigned id back.
    let rc = unsafe { libc::ioctl(fd, EVIOCSFF, &mut effect as *mut FfEffect) };
    if rc < 0 {
        let e = std::io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENODEV) {
            Ok(())
        } else {
            Err(format!("upload effect: {e}"))
        };
    }
    let id = effect.id;

    let outcome = write_event(&mut file, id as u16, 1);
    if outcome.is_ok() {
        wait_out(Duration::from_millis(u64::from(SIM_DURATION_MS)), cancel);
    }

    // Unconditional cleanup: stop, then erase, whatever happened above.
    let _ = write_event(&mut file, id as u16, 0);
    // SAFETY: same fd; EVIOCRMFF takes the effect id by value.
    let _ = unsafe { libc::ioctl(fd, EVIOCRMFF, id as libc::c_ulong) };

    match outcome {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::ENODEV) => Ok(()),
        Err(e) => Err(format!("play effect: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(type_: u16, code: u16, value: i32) -> Option<TestEvent> {
        let mut b = [0u8; EVENT_SIZE];
        b[16..18].copy_from_slice(&type_.to_le_bytes());
        b[18..20].copy_from_slice(&code.to_le_bytes());
        b[20..24].copy_from_slice(&value.to_le_bytes());
        evtest::parse_event(&b)
    }

    #[test]
    fn ff_effect_layout_matches_kernel_abi() {
        assert_eq!(std::mem::size_of::<FfEffect>(), 48);
        assert_eq!(std::mem::align_of::<FfEffect>(), 8);
        let e = sim_effect(SimKind::ConstantForce);
        let union_offset = (&e.u as *const _ as usize) - (&e as *const _ as usize);
        assert_eq!(union_offset, 16);
        // Precomputed from <linux/input.h>.
        assert_eq!(EVIOCSFF, 0x4030_4580);
        assert_eq!(EVIOCRMFF, 0x4004_4581);
    }

    #[test]
    fn sim_effects_are_gentle_and_bounded() {
        let c = sim_effect(SimKind::ConstantForce);
        assert_eq!(c.type_, FF_CONSTANT);
        assert_eq!(c.replay_length, 2000);
        assert_eq!(i16::from_le_bytes([c.u.0[0], c.u.0[1]]), 0x2000);
        let t = sim_effect(SimKind::Texture);
        assert_eq!(t.type_, FF_PERIODIC);
        assert_eq!(u16::from_le_bytes([t.u.0[0], t.u.0[1]]), FF_SINE);
        assert_eq!(t.replay_length, 2000);
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
        let status = v.spawn_sim(SimKind::ConstantForce);
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
    fn wait_out_completes_when_never_cancelled() {
        let cancel = AtomicBool::new(false);
        assert_eq!(wait_out(Duration::from_millis(30), &cancel), WaitOutcome::Completed);
    }

    #[test]
    fn wait_out_returns_early_on_cancel() {
        let cancel = AtomicBool::new(true);
        let start = std::time::Instant::now();
        assert_eq!(wait_out(Duration::from_secs(10), &cancel), WaitOutcome::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(1), "does not sleep out the duration");
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
