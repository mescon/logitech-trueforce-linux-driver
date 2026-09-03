// SPDX-License-Identifier: GPL-2.0-only
//! The daemon loop: listen on every telemetry port (the native UDP game
//! formats plus the shared-memory relay format), synthesize while
//! telemetry flows, stop within [`SILENCE_TIMEOUT_MS`] of it stopping.
//!
//! One `poll(2)` over all the UDP sockets with a short timeout drives
//! everything: packet parsing, sample generation paced by wall clock
//! (1 sample per elapsed millisecond, capped so scheduling stalls never
//! burst-force the wheel), the silence watchdog, and the SIGINT/SIGTERM
//! stop flag. The wheel stream is opened lazily on the first enabled
//! telemetry and torn down (with a clear) on silence, error, or exit,
//! so no force is ever left queued.
//!
//! The wheel is taken, not assumed: opening a stream takes that wheel's
//! streaming lease ([`crate::lease`]) and standby gives it back, so this
//! daemon and a test sweep never end up taking turns on an endpoint that
//! carries one packet per millisecond.

use std::path::Path;
use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::lease::Lease;
use crate::leds::RevLeds;
use crate::effects::Mixer;
use crate::telemetry::Telemetry;
use crate::tf::TfStream;
use crate::{beamng, codemasters, f1, g923, pcars, relay, wrc};

/// Stop the stream after this much telemetry silence (spec safety rail).
pub const SILENCE_TIMEOUT_MS: u64 = 500;
/// Go to standby after this much zero-force output while telemetry still
/// flows (game menus). Distinct from [`SILENCE_TIMEOUT_MS`], which watches
/// packet ARRIVAL: a game in its menus keeps sending telemetry, so that
/// watchdog never fires, and holding the stream open at zero force is
/// exactly what whines (whine-investigation.md holder #3).
pub const ZERO_FORCE_STANDBY_MS: u64 = 500;
/// Poll timeout; bounds both watchdog latency and shutdown latency.
const POLL_TIMEOUT_MS: i32 = 50;
/// Cap on how much audio one iteration may generate, in milliseconds: a
/// scheduling stall longer than this drops the backlog instead of bursting
/// it. Expressed in time rather than samples, because the sample count for
/// a given stretch of time depends on the stream rate.
///
/// This is also the producer's worst-case single push, so every transport's
/// own backlog bound has to sit above it or it discards part of a burst on
/// arrival while nothing is actually wrong. Both derive from this one
/// constant: [`crate::g923::MAX_PENDING_MS`] here, and
/// `LOGITF_TF_MAX_PENDING_MS` in libtrueforce's `internal.h` for the
/// direct-drive path.
pub(crate) const MAX_GEN_MS: u64 = 100;
/// How long to wait before re-probing for a wheel after a failed open.
const OPEN_RETRY: Duration = Duration::from_secs(5);

/// What one iteration of the generation loop should produce.
///
/// Extracted from the loop so it can be tested against a simulated clock.
/// The arithmetic here has been wrong four times, every instance the same
/// mistake of treating a sample count and a duration as interchangeable, and
/// every one found by accident rather than by a test: three when the stream
/// rate went from 1 kHz to 4 kHz and broke the coincidence that made them
/// agree, and one by chasing a symptom afterwards. Nothing exercised it
/// directly, because it lived inside a loop wrapped in sockets and streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GenPlan {
    /// Samples to render this iteration.
    pub samples: usize,
    /// How much audio time those samples are, which is also how far the
    /// generation clock advances. Never a sample count.
    pub audio_ms: u64,
    /// Whether the backlog exceeded the cap, so the clock resets to now and
    /// the excess is dropped rather than burst.
    pub dropped_backlog: bool,
}

/// Plan one iteration for `elapsed_ms` of wall clock since the last one.
///
/// `None` when there is nothing to do yet, which is any elapsed time shorter
/// than a millisecond.
pub(crate) fn plan_generation(elapsed_ms: u64) -> Option<GenPlan> {
    let audio_ms = elapsed_ms.min(MAX_GEN_MS);
    if audio_ms == 0 {
        return None;
    }
    Some(GenPlan {
        samples: audio_ms as usize * crate::synth::SAMPLES_PER_MS,
        audio_ms,
        dropped_backlog: elapsed_ms > MAX_GEN_MS,
    })
}

/// Set by the signal handler; polled by [`run`] and the sweep loop.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// Installed for SIGINT/SIGTERM. Only performs an atomic store, which is
/// async-signal-safe.
extern "C" fn handle_stop_signal(_signal: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Install [`handle_stop_signal`] for SIGINT and SIGTERM.
pub fn install_signal_handlers() -> Result<()> {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: sigaction with a handler that only does an atomic store
        // is async-signal-safe; the struct is fully initialized.
        let rc = unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handle_stop_signal as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(Error::Io(format!("sigaction({sig})"), std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// Either wheel family's open TrueForce stream, so [`Active`] does not
/// have to care which one it holds. libtrueforce's own discovery only
/// recognizes the RS50-family PIDs, so a G923 never reaches [`TfStream`];
/// [`open_wheel_stream`] tries the G923 path first and falls back to it.
pub(crate) enum WheelStream {
    Dd(TfStream),
    G923(g923::G923Stream),
}

impl WheelStream {
    /// Whether this stream is safe to drive at a person's request.
    ///
    /// Only the direct-drive path can be unstable without a force-feedback
    /// session held open; a G923 measured 17 degrees of travel where an
    /// RS50 without one measured 1500, so it is always considered fine.
    pub(crate) fn is_stabilised(&self) -> bool {
        match self {
            WheelStream::Dd(s) => s.is_stabilised(),
            WheelStream::G923(_) => true,
        }
    }

    pub(crate) fn push(&mut self, samples: &[f32]) -> Result<()> {
        match self {
            WheelStream::Dd(s) => s.push(samples),
            WheelStream::G923(s) => {
                s.push(samples).map_err(|e| Error::Io("G923 TrueForce stream write".into(), e))
            }
        }
    }

    /// Menu standby: the DD path has libtrueforce send the captured
    /// 0x04+0x03 teardown pair and go silent (engine flushed, armed,
    /// unfed - the state Windows leaves the wheel in). The G923 path has
    /// no TF engine to disarm: the grace period already delivered zero
    /// force, and simply not writing holds it there.
    pub(crate) fn standby(&mut self) {
        if let WheelStream::Dd(s) = self {
            s.standby();
        }
    }

    /// Leave standby; the DD engine stayed armed, so the next push
    /// resumes the stream without re-init.
    pub(crate) fn resume(&mut self) {
        if let WheelStream::Dd(s) = self {
            s.resume();
        }
    }
}

/// What [`SilenceGate::observe`] wants done about this block of output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateAction {
    /// Nothing changes; push or stay silent per [`SilenceGate::in_standby`].
    Stay,
    /// The grace period just elapsed: put the stream in standby.
    EnterStandby,
    /// Force returned while in standby: resume the stream, then push.
    Resume,
}

/// Content-based standby detector for game menus.
///
/// The effects engine deliberately snaps silenced effects to exact zero
/// (`effects.rs`, `SMOOTH_SNAP`), so "the mixer produced only zeros" is a
/// reliable one-comparison signal that no force is being commanded. After
/// [`ZERO_FORCE_STANDBY_MS`] of that the stream goes to standby even though
/// telemetry still flows; the first non-zero block resumes it. Extracted
/// from the loop so the transition arithmetic is testable against a
/// simulated clock, like [`plan_generation`].
#[derive(Debug, Default)]
pub(crate) struct SilenceGate {
    /// Milliseconds of consecutive all-zero output so far.
    zero_ms: u64,
    standby: bool,
}

impl SilenceGate {
    pub(crate) fn in_standby(&self) -> bool {
        self.standby
    }

    /// Park again after a [`GateAction::Resume`] the caller could not act
    /// on, so the next block of force asks once more.
    ///
    /// The daemon needs this when it cannot re-take the wheel's streaming
    /// lease on the way out of standby: the gate has already decided force
    /// returned, but the stream must stay silent until whoever else has the
    /// wheel is done with it. The zero counter is left at the grace period
    /// so this does not re-announce a standby it never left.
    pub(crate) fn hold_standby(&mut self) {
        self.standby = true;
        self.zero_ms = ZERO_FORCE_STANDBY_MS;
    }

    /// Account one rendered block: `silent` is "every sample was exactly
    /// zero", `audio_ms` how much audio time the block covers.
    pub(crate) fn observe(&mut self, silent: bool, audio_ms: u64) -> GateAction {
        if !silent {
            self.zero_ms = 0;
            if self.standby {
                self.standby = false;
                return GateAction::Resume;
            }
            return GateAction::Stay;
        }
        if self.standby {
            return GateAction::Stay;
        }
        self.zero_ms += audio_ms;
        if self.zero_ms >= ZERO_FORCE_STANDBY_MS {
            self.standby = true;
            return GateAction::EnterStandby;
        }
        GateAction::Stay
    }
}

/// How this daemon names itself in the streaming lease, for whoever it
/// refuses next.
const LEASE_HOLDER: &str = "logi-tf-sim";

/// An open wheel, everything the caller needs to keep it open, and the
/// identity it was opened against.
pub(crate) struct OpenWheel {
    pub(crate) stream: WheelStream,
    /// The HID device carrying this wheel's rev display, when it is known.
    /// `None` means "scan for one", which is only right with a single wheel
    /// attached (see [`crate::leds::RevLeds::discover`]).
    pub(crate) led_owner: Option<String>,
    /// Held for as long as the stream is open, and released with it. See
    /// [`crate::lease`] for why streaming without it is a 500 Hz buzz.
    pub(crate) lease: Lease,
    /// Which wheel the lease is for, so a holder that released it in
    /// standby can ask for the same one back.
    pub(crate) lease_key: String,
}

/// The HID device id (`0003:046D:C276.0003`) of the first attached
/// direct-drive wheel, or `None`.
///
/// libtrueforce hands back no sysfs path at all, so this is where the DD
/// path gets an identity: the driver's own attribute directory, which is
/// the same device that carries `wheel_rev_level` (both live in one
/// attribute group in the driver). It is used for two things, the lease key
/// and the rev display's owner, and both were previously answered by
/// "whichever one sysfs listed first", independently of each other.
///
/// `None` under the `LOGI_WHEEL_SYSFS_DIR` development override: a fixture
/// directory's name is not a HID device id, and passing it on would send
/// the rev-display lookup somewhere that cannot exist. The override's own
/// handling in [`crate::leds::RevLeds::discover`] covers that case.
fn dd_hid_id() -> Option<String> {
    if crate::leds::sysfs_dir_override().is_some() {
        return None;
    }
    logi_wheel_core::Device::discover_all()
        .into_iter()
        .find(|d| d.model() != logi_wheel_core::WheelModel::G923)
        .and_then(|d| d.hid_id())
}

/// Take the streaming lease for `key`, turning a refusal into the error the
/// caller reports.
fn take_lease(key: &str) -> Result<Lease> {
    crate::lease::try_acquire(key, LEASE_HOLDER).map_err(|busy| Error::Busy(busy.holder))
}

/// Open whichever wheel is present: a G923 (via [`g923::discover`], which
/// libtrueforce cannot see) takes priority since a G923 never answers
/// libtrueforce's own RS50-family discovery; otherwise fall back to the DD
/// wheels' libtrueforce-backed [`TfStream`].
pub(crate) fn open_wheel_stream(cfg: &Config) -> Result<OpenWheel> {
    open_wheel_stream_with_leds(cfg)
}

/// Open a G923 at `paths`, taking the lease first.
///
/// The lease is taken BEFORE the stream, not after: the point is to leave
/// the wheel alone when somebody else has it, and a stream opened and
/// dropped again has already touched the device.
fn open_g923(cfg: &Config, paths: &g923::G923Paths) -> Result<OpenWheel> {
    // No force to mirror means streaming would take the wheel's force
    // feedback away, so do not stream.
    //
    // Once a type-0x01 stream runs, this wheel's motor follows the
    // stream's `cur` field and stops reacting to the classic force
    // commands. That is why the stream carries the live force alongside
    // the texture, read from `ffb_output`. The Xbox edition has no such
    // attribute, because its force feedback is downloaded into the
    // wheel's own firmware slots (HID++ 0x8123) and summed there, so
    // nothing in the kernel knows the net force to publish.
    //
    // Treating that as "no merge, carry on" is what shipped, and it is
    // wrong in the one way that matters: the mirror then feeds a constant
    // zero, so starting the daemon silently zeroed the steering force
    // while the rev lights and engine texture kept working, which is a
    // hard fault to attribute (issue #72, found on hardware nobody here
    // has). Refusing is the honest outcome: on that wheel it is force
    // feedback or synthesized haptics, not both.
    //
    // `kernel_carries_force` is the way out that is not a config key: when
    // the driver's own engine has that wheel (the `g923_xbox_dd_engine`
    // module parameter), it splices the live force into the packets this
    // daemon writes, so there is a force in the stream and nothing to
    // mirror.
    if paths.ffb_output.is_none()
        && !paths.kernel_carries_force
        && !cfg.g923_stream_without_ffb_mirror
    {
        return Err(Error::Io(
            "this G923 publishes no live force to mirror (the Xbox edition), and streaming \
             would silence its force feedback; set g923.stream_without_ffb_mirror=1 to \
             stream anyway and lose force while it runs"
                .into(),
            std::io::Error::from(std::io::ErrorKind::Unsupported),
        ));
    }
    // Discovery already correlated the TrueForce interface with its
    // interface-0 sibling to find `ffb_output`; that sibling is the device
    // carrying the rev LEDs, so the identity comes free here.
    let led_owner = paths
        .ffb_output
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned());
    let lease_key = led_owner.clone().unwrap_or_else(|| crate::lease::UNKNOWN_WHEEL_KEY.to_string());
    let lease = take_lease(&lease_key)?;
    let sign = g923::Sign::resolve(cfg.g923_ffb_invert);
    let stream = g923::G923Stream::open(paths, sign)
        .map_err(|e| Error::Io("open G923 TrueForce stream".into(), e))?;
    Ok(OpenWheel { stream: WheelStream::G923(stream), led_owner, lease, lease_key })
}

/// Open the direct-drive wheel through libtrueforce, taking the lease first.
fn open_dd() -> Result<OpenWheel> {
    let led_owner = dd_hid_id();
    let lease_key = led_owner.clone().unwrap_or_else(|| crate::lease::UNKNOWN_WHEEL_KEY.to_string());
    let lease = take_lease(&lease_key)?;
    let stream = TfStream::open(0)?;
    Ok(OpenWheel { stream: WheelStream::Dd(stream), led_owner, lease, lease_key })
}

/// Open a wheel, and say which HID device its rev display belongs to.
///
/// The second half exists because the two used to be found independently:
/// the stream picked a wheel and the rev display picked whichever wheel
/// sysfs listed first. With one wheel attached those always agree. With two
/// they need not, and on the development rig they did not: the G923 got the
/// haptics and the RS50 got the lights.
pub(crate) fn open_wheel_stream_with_leds(cfg: &Config) -> Result<OpenWheel> {
    // Which wheel to drive. `auto` prefers a G923 whenever one is present,
    // which is right for the overwhelmingly common case of a single wheel
    // and wrong for a rig with both: the direct-drive wheel would be
    // unreachable, because nothing below is ever consulted.
    //
    // The `wheel` config key is the answer to that, and is what the apps
    // set. LOGI_TF_SIM_WHEEL still overrides it for a one-off run without
    // editing anyone's configuration, which is what it was always for.
    use logi_wheel_core::tfsim::WheelChoice;
    let choice = match std::env::var("LOGI_TF_SIM_WHEEL") {
        Ok(v) if !v.trim().is_empty() => WheelChoice::parse(&v).unwrap_or(cfg.wheel),
        _ => cfg.wheel,
    };
    if choice == WheelChoice::DirectDrive {
        return open_dd();
    }
    if choice == WheelChoice::G923 {
        let paths = g923::discover().ok_or_else(|| {
            Error::Io(
                "no G923 found, and the configuration asks for one (wheel = g923)".into(),
                std::io::Error::from(std::io::ErrorKind::NotFound),
            )
        })?;
        return open_g923(cfg, &paths);
    }
    if let Some(paths) = g923::discover() {
        return open_g923(cfg, &paths);
    }
    open_dd()
}

/// A live wheel stream plus the state that feeds it.
/// Marks an [`Active`] stream fed by captured TrueForce rather than by a
/// telemetry decoder. Not a real game id: nothing gates on it, because a
/// game sending its own TrueForce has already decided it wants haptics.
const CAPTURED_GAME: &str = "captured";

/// Take every captured-TrueForce datagram waiting on `sock`, newest last.
///
/// Concatenated rather than deduplicated: these are consecutive runs of a
/// waveform, so dropping any would put a gap in it. Malformed packets are
/// skipped silently, which is the same treatment the telemetry decoders give
/// a packet they cannot read.
fn drain_captured_tf(sock: &UdpSocket, buf: &mut [u8]) -> Vec<f32> {
    let mut out = Vec::new();
    while let Ok(n) = sock.recv(buf) {
        if let Some(mut s) = logi_wheel_core::tfstream::decode(&buf[..n]) {
            out.append(&mut s);
        }
        if n == 0 {
            break;
        }
    }
    out
}

/// How long one captured-TrueForce datagram keeps synthesis quiet.
///
/// The rule is "a game that sent its own TrueForce gets its own TrueForce,
/// not an engine note invented over the top of it". What that rule must NOT
/// depend on is who opened the stream first, which is what it used to key
/// on: `game == CAPTURED_GAME` is true only when the captured path opened
/// the stream, so a game whose telemetry arrived first got both, its own
/// samples AND synthesis, in the same iteration.
///
/// Long enough to bridge the gaps between a game's sample runs (the SDK
/// hands them over in bursts, not evenly), short enough that synthesis
/// comes back promptly when the game stops sending. Half the telemetry
/// watchdog, which is the same scale of judgement.
pub(crate) const CAPTURED_PRECEDENCE_MS: u64 = 250;

/// Whether captured TrueForce is recent enough to own the stream.
///
/// Pure and content-based: the argument is the age of the newest captured
/// sample, which is the only thing that should decide this.
pub(crate) fn captured_has_precedence(age: Option<Duration>) -> bool {
    age.is_some_and(|age| age < Duration::from_millis(CAPTURED_PRECEDENCE_MS))
}

/// Whether a session for `id` has any haptics to stream.
///
/// Zero is a real setting, not a very quiet one: master strength zero, or
/// this game's own zero, means every sample would be exactly zero, and a
/// stream of zeroes is worth less than no stream at all (it arms the
/// wheel's engine and holds the lease). The rev display is unaffected,
/// which is the combination issue #59 asked for.
fn wants_haptics(cfg: &Config, id: &str) -> bool {
    cfg.effective_intensity(id) > 0.0
}

struct Active {
    /// The wheel's TrueForce stream, or `None` for a lights-only session.
    ///
    /// Intensity zero means the haptics are switched off, and an open
    /// session at zero is not the same thing as no session: it arms the
    /// wheel's engine, holds the one-writer lease, and leaves a wheel that
    /// somebody wanted quiet doing something. The rev display is driven
    /// from the same telemetry and has no reason to stop, so zero opens
    /// the lights and nothing else (requested in issue #59).
    stream: Option<WheelStream>,
    /// The running game's own force-feedback strength, polled from the
    /// wheel, so its slider governs these haptics as well as its forces
    /// (issue #59).
    game_gain: crate::game_gain::GameGain,
    mixer: Mixer,
    game: &'static str,
    tel: Telemetry,
    last_telemetry: Instant,
    last_gen: Instant,
    samples: Vec<f32>,
    /// When captured TrueForce last arrived, so
    /// [`captured_has_precedence`] can decide by content rather than by
    /// which path happened to open the stream.
    last_captured: Option<Instant>,
    /// The wheel's rev display, when the config enables it and a rev
    /// display (either the DD wheels' `wheel_rev_level` attribute or the
    /// G923's LED classdevs) was found at stream start; `None` otherwise.
    /// Stopped (blanked) with the stream.
    leds: Option<RevLeds>,
    /// The base's screen as a dashboard, when configured and present.
    screen: Option<crate::screen::Screen>,
    /// Menu standby: zero-force output past the grace period parks the
    /// stream even while telemetry keeps arriving.
    gate: SilenceGate,
    /// The wheel's streaming lease, held while this stream is fed and
    /// released in standby so a test sweep can have the wheel between
    /// sessions. `None` means "not currently held", which is standby, or a
    /// standby we could not come back from because somebody else took it.
    lease: Option<Lease>,
    /// Which wheel to ask for when coming out of standby.
    lease_key: String,
    /// Whether the failure to re-take the lease has already been reported.
    /// The retry runs per iteration, and one line per 50 ms is a log nobody
    /// can read.
    warned_busy: bool,
}

fn bind(port: u16) -> Result<UdpSocket> {
    let sock = UdpSocket::bind(("0.0.0.0", port))
        .map_err(|e| Error::Io(format!("bind UDP port {port}"), e))?;
    sock.set_nonblocking(true)
        .map_err(|e| Error::Io(format!("set_nonblocking on port {port}"), e))?;
    Ok(sock)
}

/// Join the relay stream, whether or not somebody else is already reading
/// it.
///
/// The relay port is wanted by logi-rpm-bridge too, which reads the very
/// same LTFR datagrams to feed the kernel's texture merge and the rev
/// lights, and the kernel will not deliver one datagram to two sockets
/// (see [`relay::RelayListener`] for the measurements and the reasoning).
/// So the port is not shared, it is relayed: whoever has it forwards to the
/// fan-out ports, and whoever arrives second reads one of those. Neither
/// program has to be stopped for the other to work.
///
/// `None` only when the relay port and every fan-out port behind it are
/// taken, which means more readers than the fan-out was built for. That is
/// still not fatal here: this daemon has four other telemetry sockets and a
/// wheel to drive, so a UDP-telemetry game keeps its simulated TrueForce
/// and only the shared-memory titles go quiet.
fn open_relay(port: u16) -> Option<relay::RelayListener> {
    match relay::RelayListener::open(port) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("logi-tf-sim: cannot listen for the shared-memory relay: {e}");
            eprintln!(
                "logi-tf-sim: udp/{port} and its fan-out ports ({}) are all taken, so there is \
                 nowhere left to be fed from",
                relay::fanout_ports(port)
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            eprintln!(
                "logi-tf-sim: the games that publish only into Windows shared memory (the \
                 Assetto family, iRacing, RaceRoom, rFactor 2, Le Mans Ultimate) drive nothing \
                 here until one of those frees up"
            );
            eprintln!("logi-tf-sim: everything else keeps working");
            None
        }
    }
}

/// Block until any of the sockets is readable or the timeout expires.
fn poll_sockets(socks: &[&UdpSocket]) {
    let mut fds: Vec<libc::pollfd> = socks
        .iter()
        .map(|s| libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 })
        .collect();
    // SAFETY: fds points at a valid array of initialized pollfd. EINTR and
    // other failures just fall through to the (nonblocking) reads.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, POLL_TIMEOUT_MS) };
}

/// The stateful telemetry decoders. Only the formats that omit a redline
/// (`f1`, `beamng`) need state; the rest parse purely per packet. Held
/// across iterations so their running `max_rpm` survives, and reset when a
/// stream is torn down so a new session re-learns.
#[derive(Default)]
struct Decoders {
    f1: f1::Decoder,
    beamng: beamng::Decoder,
}

impl Decoders {
    /// Parse a datagram arriving on the Codemasters port (20777), which is
    /// shared by three formats: the classic float array, modern F1, and the
    /// logi-tf-sim WRC packet. Each is told apart by length and header, so
    /// trying them in turn never cross-matches.
    fn parse_codemasters_port(&mut self, pkt: &[u8]) -> Option<(&'static str, Telemetry)> {
        codemasters::parse(pkt)
            .or_else(|| self.f1.parse(pkt))
            .or_else(|| wrc::parse(pkt))
    }

    /// Forget every learned redline (called when a stream is torn down).
    fn reset(&mut self) {
        self.f1.reset();
        self.beamng.reset();
    }
}

/// Drain every pending datagram on `sock` through `parse`, keeping the
/// newest sample that parsed.
fn drain(
    sock: &UdpSocket,
    buf: &mut [u8],
    mut parse: impl FnMut(&[u8]) -> Option<(&'static str, Telemetry)>,
    latest: &mut Option<(&'static str, Telemetry)>,
) {
    loop {
        match sock.recv_from(buf) {
            Ok((n, _peer)) => {
                if let Some(sample) = parse(&buf[..n]) {
                    *latest = Some(sample);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Run the daemon until SIGINT/SIGTERM. Binds both telemetry ports up
/// front (that failure is fatal); a missing wheel is not fatal, it is
/// retried whenever telemetry is flowing.
pub fn run(cfg: &Config) -> Result<()> {
    let cm_sock = bind(cfg.codemasters_port)?;
    let pc_sock = bind(cfg.pcars_port)?;
    let bn_sock = bind(cfg.beamng_port)?;
    let mut relay_sock = open_relay(cfg.relay_port);
    // TrueForce a game produced itself, captured from its SDK calls by the
    // proxy DLL and forwarded here. Separate from the telemetry sockets
    // because it is finished haptics rather than an input to synthesis: the
    // samples go to the wheel as they are.
    let tf_sock = bind(logi_wheel_core::tfstream::DEFAULT_PORT)?;
    install_signal_handlers()?;

    eprintln!(
        "logi-tf-sim: listening (codemasters/F1/WRC on udp/{}, pcars2/ams2 on udp/{}, \
         beamng on udp/{}, shared-memory relay on {}, captured TrueForce on udp/{})",
        cfg.codemasters_port,
        cfg.pcars_port,
        cfg.beamng_port,
        // The line is the daemon's own account of what it is doing, so it
        // must not claim a socket the bind above lost, and it names the
        // fan-out port when that is where the datagrams are arriving.
        match &relay_sock {
            Some(l) => l.describe(),
            None => format!("udp/{} (NOT LISTENING, see above)", cfg.relay_port),
        },
        logi_wheel_core::tfstream::DEFAULT_PORT
    );
    if !cfg.enabled {
        eprintln!("logi-tf-sim: master switch is off in the config; listening but not synthesizing");
    }

    let mut active: Option<Active> = None;
    let mut decoders = Decoders::default();
    let mut next_open_attempt = Instant::now();
    let mut buf = [0u8; 2048];

    while !STOP.load(Ordering::SeqCst) {
        // Rebuilt per iteration rather than once: the relay listener can
        // swap its socket underneath us when it is promoted from a fan-out
        // port to the relay port, and a poll set built at startup would
        // then be watching a closed file descriptor. Five borrows is not a
        // cost worth caching against that.
        let mut polled = vec![&cm_sock, &pc_sock, &bn_sock, &tf_sock];
        polled.extend(relay_sock.as_ref().map(|l| l.socket()));
        poll_sockets(&polled);
        drop(polled);

        // Drained first and kept separate: captured samples are the game's
        // own TrueForce and must not be mixed with, or replaced by, anything
        // synthesized from telemetry the same game also emits.
        let captured = drain_captured_tf(&tf_sock, &mut buf);

        let mut latest: Option<(&'static str, Telemetry)> = None;
        drain(&cm_sock, &mut buf, |p| decoders.parse_codemasters_port(p), &mut latest);
        drain(&pc_sock, &mut buf, pcars::parse, &mut latest);
        drain(&bn_sock, &mut buf, |p| decoders.beamng.parse(p), &mut latest);
        if let Some(listener) = &relay_sock {
            listener.drain(&mut buf, |pkt| {
                if let Some(sample) = relay::parse(pkt) {
                    latest = Some(sample);
                }
            });
        }

        let now = Instant::now();

        // A follower takes the relay port back when whoever held it exits,
        // so a daemon left running between sessions ends up as the hub
        // again and the next program to start is the one that gets fed.
        if let Some(listener) = &mut relay_sock {
            if listener.poll_promotion(now) {
                eprintln!(
                    "logi-tf-sim: shared-memory relay: took over udp/{} (the reader that held \
                     it is gone)",
                    listener.port()
                );
            }
        }

        if let Some((id, tel)) = latest {
            if cfg.game_enabled(id) {
                match &mut active {
                    Some(a) => {
                        if a.game != id {
                            eprintln!("logi-tf-sim: telemetry switched ({} -> {id})", a.game);
                            a.game = id;
                        }
                        a.tel = tel;
                        a.last_telemetry = now;
                    }
                    // Haptics off for this game: drive the rev display and
                    // leave the wheel's TrueForce engine alone entirely. The
                    // silence gate would park an opened session within a
                    // second or two anyway, but "opened, then parked" still
                    // arms the engine and takes the lease, and neither is
                    // something a driver who set the strength to zero asked
                    // for. Nothing to do at all when the lights are off too.
                    None if now >= next_open_attempt
                        && !wants_haptics(cfg, id)
                        && cfg.leds
                        && crate::leds::other_owner().is_none() =>
                    {
                        if let Some(leds) = RevLeds::discover() {
                            eprintln!(
                                "logi-tf-sim: rev display only ({id}): strength is zero, so the \
                                 TrueForce stream stays closed"
                            );
                            active = Some(Active {
                                stream: None,
                                game_gain: crate::game_gain::GameGain::new(None, false),
                                mixer: Mixer::engine_only(
                                    cfg.cylinders,
                                    f32::from(cfg.pitch_pct) / 100.0,
                                    cfg.effect_gains,
                                ),
                                game: id,
                                tel,
                                last_telemetry: now,
                                last_gen: now,
                                samples: Vec::new(),
                                last_captured: None,
                                leds: Some(leds),
                                screen: if cfg.screen { crate::screen::Screen::discover() } else { None },
                                gate: SilenceGate::default(),
                                lease: None,
                                lease_key: String::new(),
                                warned_busy: false,
                            });
                        } else {
                            next_open_attempt = now + OPEN_RETRY;
                        }
                    }
                    None if now >= next_open_attempt => match open_wheel_stream_with_leds(cfg) {
                        Ok(OpenWheel { stream, led_owner, lease, lease_key }) => {
                            eprintln!(
                                "logi-tf-sim: stream start ({id}, rpm {:.0}/{:.0}, speed {:.0} m/s)",
                                tel.rpm, tel.max_rpm, tel.speed
                            );
                            // One rev-display writer per session. When the
                            // texture merge's bridge is up it owns the
                            // strip and drives it from the game's own
                            // telemetry triple, which is the better feed
                            // and the narrower claim (see
                            // `leds::other_owner`); ours would fight it.
                            let taken = if cfg.leds { crate::leds::other_owner() } else { None };
                            let leds = match (cfg.leds, &taken, led_owner.as_deref()) {
                                (false, _, _) | (_, Some(_), _) => None,
                                (true, None, Some(owner)) => RevLeds::discover_for(owner),
                                (true, None, None) => RevLeds::discover(),
                            };
                            if let Some(owner) = &taken {
                                eprintln!(
                                    "logi-tf-sim: leaving the rev display to {owner}, which is \
                                     already driving it"
                                );
                            }
                            if leds.is_some() {
                                eprintln!("logi-tf-sim: driving the wheel's rev display");
                            }
                            active = Some(Active {
                                stream: Some(stream),
                                game_gain: crate::game_gain::GameGain::new(
                                    led_owner.as_deref(),
                                    cfg.follow_game_gain,
                                ),
                                mixer: if cfg.effects {
                                    Mixer::new(
                                        cfg.cylinders,
                                        f32::from(cfg.pitch_pct) / 100.0,
                                        cfg.effect_gains,
                                    )
                                } else {
                                    Mixer::engine_only(
                                        cfg.cylinders,
                                        f32::from(cfg.pitch_pct) / 100.0,
                                        cfg.effect_gains,
                                    )
                                },
                                game: id,
                                tel,
                                last_telemetry: now,
                                last_gen: now,
                                samples: Vec::with_capacity(MAX_GEN_MS as usize * crate::synth::SAMPLES_PER_MS),
                                last_captured: None,
                                leds,
                                screen: if cfg.screen { crate::screen::Screen::discover() } else { None },
                                gate: SilenceGate::default(),
                                lease: Some(lease),
                                lease_key,
                                warned_busy: false,
                            });
                        }
                        Err(e) => {
                            eprintln!("logi-tf-sim: cannot open wheel ({e}); retrying in {}s", OPEN_RETRY.as_secs());
                            next_open_attempt = now + OPEN_RETRY;
                        }
                    },
                    None => {}
                }
            }
        }

        // Captured TrueForce takes precedence over anything synthesized.
        //
        // Assetto Corsa Competizione and Assetto Corsa EVO produce real
        // TrueForce; a G923 never receives it, because that generation's SDK
        // path wants a G HUB agent Linux does not have. When the proxy
        // forwards those samples here, they go to the wheel as they are.
        // Synthesizing an engine note over the top would be inventing
        // haptics for a game that already sent us its own.
        if !captured.is_empty() {
            if let Some(a) = &mut active {
                a.last_telemetry = now;
                a.last_gen = now;
                a.last_captured = Some(now);
                // Standby gave the wheel's lease up, and this path drives
                // the wheel directly, so it has to be taken back first.
                // Dropping a few milliseconds of the game's audio while
                // somebody else has the wheel is the right cost; pushing
                // into an endpoint another program is streaming to is the
                // failure the lease exists to prevent.
                if a.lease.is_none() {
                    match take_lease(&a.lease_key) {
                        Ok(lease) => {
                            a.lease = Some(lease);
                            a.warned_busy = false;
                            // The DD path's standby sent the teardown pair,
                            // so the stream is parked; samples pushed into a
                            // parked stream go nowhere.
                            if let Some(stream) = a.stream.as_mut() {
                                stream.resume();
                            }
                            a.gate = SilenceGate::default();
                        }
                        Err(e) => {
                            if !a.warned_busy {
                                a.warned_busy = true;
                                eprintln!("logi-tf-sim: dropping captured TrueForce: {e}");
                            }
                        }
                    }
                }
                if a.lease.is_some() {
                    if let Err(e) = a.stream.as_mut().map_or(Ok(()), |s| s.push(&captured)) {
                        eprintln!("logi-tf-sim: captured TrueForce push failed: {e}");
                    }
                }
            } else if now >= next_open_attempt {
                match open_wheel_stream(cfg) {
                    Ok(OpenWheel { stream, lease, lease_key, .. }) => {
                        eprintln!(
                            "logi-tf-sim: stream start (captured TrueForce from the game's own SDK)"
                        );
                        // No mixer and no rev display: this path carries the
                        // game's finished haptics, and the game drives its
                        // own rev lights through the SDK it is already
                        // talking to. Synthesizing either would be adding
                        // something nobody asked for.
                        active = Some(Active {
                            stream: Some(stream),
                            // The captured path carries the game's own
                            // finished haptics, which the game has already
                            // scaled by its own slider; scaling again here
                            // would apply it twice.
                            game_gain: crate::game_gain::GameGain::new(None, false),
                            mixer: Mixer::engine_only(
                                cfg.cylinders,
                                f32::from(cfg.pitch_pct) / 100.0,
                                cfg.effect_gains,
                            ),
                            game: CAPTURED_GAME,
                            tel: Telemetry::default(),
                            last_telemetry: now,
                            last_gen: now,
                            samples: Vec::with_capacity(MAX_GEN_MS as usize * crate::synth::SAMPLES_PER_MS),
                            last_captured: Some(now),
                            leds: None,
                            screen: None,
                            gate: SilenceGate::default(),
                            lease: Some(lease),
                            lease_key,
                            warned_busy: false,
                        });
                    }
                    Err(e) => {
                        eprintln!("logi-tf-sim: cannot open wheel ({e}); retrying in {}s", OPEN_RETRY.as_secs());
                        next_open_attempt = now + OPEN_RETRY;
                    }
                }
            }
        }

        // Watchdog + generation for the active stream.
        let mut stop_reason: Option<String> = None;
        if let Some(a) = &mut active {
            if captured_has_precedence(a.last_captured.map(|t| now.duration_since(t))) {
                // The game's own TrueForce is flowing and was pushed above;
                // synthesizing over the top of it is the one thing this
                // must not do. Decided by how recently captured samples
                // arrived, NOT by which path opened the stream: a game
                // whose telemetry arrived first used to get both.
            } else if now.duration_since(a.last_telemetry) >= Duration::from_millis(SILENCE_TIMEOUT_MS) {
                stop_reason = Some(format!("telemetry silent for {SILENCE_TIMEOUT_MS} ms"));
            } else {
                let elapsed_ms = now.duration_since(a.last_gen).as_millis() as u64;
                if let Some(plan) = plan_generation(elapsed_ms) {
                    a.last_gen = if plan.dropped_backlog {
                        now
                    } else {
                        a.last_gen + Duration::from_millis(plan.audio_ms)
                    };
                    // The configured strength AND the running game's own,
                    // so a game's force-feedback slider governs the engine
                    // note it never asked for as well as the forces it did
                    // (issue #59). A game at zero goes fully silent, which
                    // the gate below then parks, releasing the wheel.
                    let intensity = cfg.effective_intensity(a.game) * a.game_gain.scale(now);
                    // The mixer owns the engine layer along with the rest,
                    // including the over-redline cap the synth call used to
                    // apply here: an effect's reading of the sample is the
                    // effect's business.
                    a.mixer.render(&a.tel, intensity, plan.samples, &mut a.samples);
                    // Menus: telemetry keeps flowing while the mixer emits
                    // exact zeros. Past the grace period the stream parks
                    // (teardown pair + silence) instead of holding an open
                    // session at zero force, and pushes stop until force
                    // returns. Resume happens in the same iteration force
                    // reappears, so no samples are lost.
                    let silent = a.samples.iter().all(|&s| s == 0.0);
                    match a.gate.observe(silent, plan.audio_ms) {
                        GateAction::EnterStandby => {
                            if let Some(stream) = a.stream.as_mut() {
                                stream.standby();
                            }
                            // The wheel is not being driven while parked,
                            // so it is not ours to hold: a test sweep
                            // fired from the app between sessions should
                            // get it rather than be refused by a daemon
                            // that is emitting nothing.
                            a.lease = None;
                            eprintln!(
                                "logi-tf-sim: standby ({}): zero force for {ZERO_FORCE_STANDBY_MS} ms, telemetry still flowing",
                                a.game
                            );
                        }
                        GateAction::Resume => match take_lease(&a.lease_key) {
                            Ok(lease) => {
                                a.lease = Some(lease);
                                a.warned_busy = false;
                                if let Some(stream) = a.stream.as_mut() {
                                    stream.resume();
                                }
                                eprintln!("logi-tf-sim: resume ({}): force returned", a.game);
                            }
                            Err(e) => {
                                // Somebody took the wheel while we were
                                // parked. Stay parked and try again on the
                                // next block of force: sharing the endpoint
                                // is the failure this whole lease exists to
                                // prevent.
                                a.gate.hold_standby();
                                if !a.warned_busy {
                                    a.warned_busy = true;
                                    eprintln!("logi-tf-sim: staying in standby ({}): {e}", a.game);
                                }
                            }
                        },
                        GateAction::Stay => {}
                    }
                    if !a.gate.in_standby() {
                        if let Err(e) = a.stream.as_mut().map_or(Ok(()), |s| s.push(&a.samples)) {
                            stop_reason = Some(format!("stream push failed: {e}"));
                        }
                    }
                }
                // The rev display rides the same telemetry: RevLeds
                // paces itself (see `leds::MIN_WRITE_INTERVAL`) and only
                // writes changed levels, so this per-iteration call is
                // cheap.
                if let Some(leds) = &mut a.leds {
                    leds.update(a.tel.rpm, a.tel.max_rpm, a.tel.pit_limiter, now);
                }
                if let Some(screen) = &mut a.screen {
                    screen.update(&a.tel, &cfg.screen_template, now);
                }
            }
        }
        if let Some(reason) = stop_reason {
            if let Some(mut a) = active.take() {
                if let Some(leds) = &mut a.leds {
                    leds.stop();
                }
                if let Some(screen) = &mut a.screen {
                    screen.stop();
                }
                eprintln!("logi-tf-sim: stream stop ({}): {reason}", a.game);
            }
            // A new session re-learns the running redlines from scratch.
            decoders.reset();
        }
    }

    if let Some(mut a) = active.take() {
        if let Some(leds) = &mut a.leds {
            leds.stop();
        }
        if let Some(screen) = &mut a.screen {
            screen.stop();
        }
        eprintln!("logi-tf-sim: stream stop ({}): shutting down", a.game);
    }
    eprintln!("logi-tf-sim: exiting");
    Ok(())
}

#[cfg(test)]
mod generation_tests {
    use super::{plan_generation, MAX_GEN_MS};
    use crate::synth::SAMPLES_PER_MS;

    /// The property every one of the four bugs violated: the audio produced
    /// must be worth exactly the wall-clock time it consumed.
    ///
    /// This is what makes the loop self-correcting. Break it and the
    /// generation clock drifts against the wall clock, which starves the
    /// stream in one direction and bursts it in the other, and stretches or
    /// compresses every effect that measures itself in milliseconds.
    #[test]
    fn audio_generated_equals_wall_time_consumed() {
        for elapsed in [1u64, 2, 5, 17, 33, 50, 99, MAX_GEN_MS] {
            let plan = plan_generation(elapsed).expect("{elapsed} ms should generate");
            assert_eq!(plan.audio_ms, elapsed, "{elapsed} ms of wall clock");
            assert_eq!(
                plan.samples,
                elapsed as usize * SAMPLES_PER_MS,
                "{elapsed} ms should be {elapsed} ms of samples, at whatever the rate is",
            );
            assert!(!plan.dropped_backlog, "{elapsed} ms is within the cap");
        }
    }

    /// Simulate the loop against a fake clock and check it neither drifts nor
    /// stalls, at every iteration period the daemon really sees: a tight
    /// loop, 60 Hz and 20 Hz telemetry, and a stall longer than the cap.
    ///
    /// The bug this was written for advanced the clock by a sample count
    /// rather than a duration, so at 4 kHz it ran four times ahead of `now`,
    /// after which elapsed read zero and generation stopped until the wall
    /// clock caught up. Here that shows as generated audio far short of the
    /// wall time.
    #[test]
    fn a_simulated_loop_neither_drifts_nor_stalls() {
        for period_ms in [1u64, 17, 50] {
            let wall_total = 10_000u64;
            let mut clock = 0u64;      // wall clock, ms
            let mut last_gen = 0u64;   // generation clock, ms
            let mut audio_ms = 0u64;

            while clock < wall_total {
                clock += period_ms;
                let elapsed = clock.saturating_sub(last_gen);
                if let Some(plan) = plan_generation(elapsed) {
                    last_gen = if plan.dropped_backlog { clock } else { last_gen + plan.audio_ms };
                    audio_ms += plan.audio_ms;
                }
            }

            // Within one iteration's worth: nothing accumulates.
            let drift = wall_total.abs_diff(audio_ms);
            assert!(
                drift <= period_ms,
                "at a {period_ms} ms period, {wall_total} ms of wall clock produced \
                 {audio_ms} ms of audio (drift {drift})",
            );
        }
    }

    /// A stall longer than the cap drops the backlog instead of bursting it,
    /// and does not leave the clock behind for the next iteration to chase.
    #[test]
    fn a_long_stall_drops_the_backlog_and_resyncs() {
        let plan = plan_generation(5_000).expect("a 5 s stall still generates something");
        assert!(plan.dropped_backlog, "5 s is well past the cap");
        assert_eq!(plan.audio_ms, MAX_GEN_MS, "capped, not burst");
        assert_eq!(plan.samples, MAX_GEN_MS as usize * SAMPLES_PER_MS);
    }

    #[test]
    fn nothing_is_generated_for_less_than_a_millisecond() {
        assert_eq!(plan_generation(0), None);
    }
}

#[cfg(test)]
mod captured_precedence_tests {
    use super::{captured_has_precedence, CAPTURED_PRECEDENCE_MS};
    use std::time::Duration;

    /// A game sending its own TrueForce owns the stream for as long as it
    /// keeps sending, whoever opened it. The old rule keyed on the opener
    /// (`game == CAPTURED_GAME`), so a game whose telemetry arrived first
    /// got its own samples AND an engine note synthesized over them.
    #[test]
    fn recent_captured_samples_own_the_stream() {
        assert!(captured_has_precedence(Some(Duration::ZERO)), "just arrived");
        assert!(captured_has_precedence(Some(Duration::from_millis(CAPTURED_PRECEDENCE_MS - 1))));
    }

    /// And give it back when they stop, so a game that only sends TrueForce
    /// in some sessions still gets synthesis in the others.
    #[test]
    fn stale_captured_samples_hand_the_stream_back() {
        assert!(!captured_has_precedence(Some(Duration::from_millis(CAPTURED_PRECEDENCE_MS))));
        assert!(!captured_has_precedence(Some(Duration::from_secs(10))));
    }

    /// Nothing captured yet is not "captured owns it": a telemetry-only
    /// game must synthesize from its first packet.
    #[test]
    fn no_captured_samples_means_synthesis() {
        assert!(!captured_has_precedence(None));
    }
}

#[cfg(test)]
mod silence_gate_tests {
    use super::{GateAction, SilenceGate, ZERO_FORCE_STANDBY_MS};

    /// Menus at a 50 ms iteration period: zeros accumulate to exactly the
    /// grace period, then one EnterStandby, then Stay forever after.
    #[test]
    fn standby_after_the_grace_period_and_not_before() {
        let mut gate = SilenceGate::default();
        let block_ms = 50;
        let blocks_to_grace = ZERO_FORCE_STANDBY_MS / block_ms;

        for i in 1..blocks_to_grace {
            assert_eq!(gate.observe(true, block_ms), GateAction::Stay, "block {i}");
            assert!(!gate.in_standby(), "still inside the grace period at block {i}");
        }
        assert_eq!(gate.observe(true, block_ms), GateAction::EnterStandby);
        assert!(gate.in_standby());
        for _ in 0..100 {
            assert_eq!(gate.observe(true, block_ms), GateAction::Stay, "standby is stable");
            assert!(gate.in_standby());
        }
    }

    /// A single non-zero block anywhere inside the grace period resets it:
    /// intermittent force (kerb taps in a slow corner) never parks the
    /// stream.
    #[test]
    fn any_force_resets_the_grace_period() {
        let mut gate = SilenceGate::default();

        for _ in 0..ZERO_FORCE_STANDBY_MS - 1 {
            assert_eq!(gate.observe(true, 1), GateAction::Stay);
        }
        assert_eq!(gate.observe(false, 1), GateAction::Stay, "force arrives, no transition");
        for _ in 0..ZERO_FORCE_STANDBY_MS - 1 {
            assert_eq!(gate.observe(true, 1), GateAction::Stay, "counter restarted from zero");
        }
        assert_eq!(gate.observe(true, 1), GateAction::EnterStandby);
    }

    /// Force returning while parked resumes exactly once, and the same
    /// iteration's samples are pushable (in_standby is already false).
    #[test]
    fn force_returning_resumes_once() {
        let mut gate = SilenceGate::default();

        assert_eq!(gate.observe(true, ZERO_FORCE_STANDBY_MS), GateAction::EnterStandby);
        assert_eq!(gate.observe(false, 50), GateAction::Resume);
        assert!(!gate.in_standby(), "the resuming block itself must be pushed");
        assert_eq!(gate.observe(false, 50), GateAction::Stay, "no second resume");
    }

    /// A Resume the daemon could not act on (the wheel's lease went to
    /// somebody else while it was parked) parks it again, and the next
    /// block of force asks once more rather than never.
    #[test]
    fn a_refused_resume_parks_again_and_retries() {
        let mut gate = SilenceGate::default();
        assert_eq!(gate.observe(true, ZERO_FORCE_STANDBY_MS), GateAction::EnterStandby);
        assert_eq!(gate.observe(false, 50), GateAction::Resume);

        gate.hold_standby();
        assert!(gate.in_standby(), "still parked, so nothing is pushed");
        assert_eq!(gate.observe(false, 50), GateAction::Resume, "asks again on the next force");
        assert!(!gate.in_standby(), "and streams once the wheel is free");
    }

    /// The park-resume cycle repeats: menus, race, menus again.
    #[test]
    fn the_cycle_repeats() {
        let mut gate = SilenceGate::default();

        for _ in 0..3 {
            assert_eq!(gate.observe(true, ZERO_FORCE_STANDBY_MS), GateAction::EnterStandby);
            assert_eq!(gate.observe(false, 10), GateAction::Resume);
        }
    }
}

#[cfg(test)]
mod ffb_mirror_guard_tests {
    use super::*;

    /// The lease directory is a process-global environment variable, and
    /// cargo runs tests in parallel: two tests each setting it, opening a
    /// lease, and unsetting it raced, and one refused the other with its
    /// own pid in the message. Held for the whole of any test that touches
    /// the variable, so they take turns.
    static LEASE_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lease_env_guard() -> std::sync::MutexGuard<'static, ()> {
        LEASE_ENV.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A wheel with no live force to mirror is refused, because streaming
    /// to it takes its force feedback away for as long as the stream runs
    /// (issue #72). The refusal names the way out.
    #[test]
    fn a_wheel_without_a_force_mirror_is_refused() {
        let cfg = Config::default();
        assert!(!cfg.g923_stream_without_ffb_mirror, "refusing is the default");

        let paths = g923::G923Paths {
            hidraw: std::path::PathBuf::from("/dev/null"),
            ffb_output: None,
            kernel_carries_force: false,
        };
        let Err(err) = open_g923(&cfg, &paths) else { panic!("must refuse") };
        let text = err.to_string();
        assert!(text.contains("silence its force feedback"), "says what it protects: {text}");
        assert!(text.contains("g923.stream_without_ffb_mirror"), "names the override: {text}");
    }

    /// A wheel the driver's own engine drives needs no override and no
    /// mirror: the kernel puts the live force into the packets this
    /// daemon writes.
    #[test]
    fn the_drivers_own_engine_needs_no_override() {
        let _serial = lease_env_guard();
        let dir = std::env::temp_dir().join(format!("tfsim-dd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: single-threaded at this point in the test.
        unsafe { std::env::set_var(crate::lease::DIR_ENV, &dir) };

        let cfg = Config::default();
        let paths = g923::G923Paths {
            hidraw: std::path::PathBuf::from("/dev/null"),
            ffb_output: None,
            kernel_carries_force: true,
        };
        let opened = open_g923(&cfg, &paths);
        assert!(opened.is_ok(), "no refusal for a wheel we drive: {:?}", opened.err());
        drop(opened);

        unsafe { std::env::remove_var(crate::lease::DIR_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And the override really overrides: the same call goes through.
    ///
    /// `/dev/null` stands in for the wheel, since it accepts the writes a
    /// stream makes; the point here is only that the guard is what
    /// stopped the first call and nothing else does.
    #[test]
    fn the_override_gets_past_the_guard() {
        let _serial = lease_env_guard();
        // Somewhere private for the lease, so a developer's running daemon
        // is not disturbed by a test taking the real one.
        let dir = std::env::temp_dir().join(format!("tfsim-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: single-threaded at this point in the test.
        unsafe { std::env::set_var(crate::lease::DIR_ENV, &dir) };

        let cfg = Config { g923_stream_without_ffb_mirror: true, ..Config::default() };
        let paths = g923::G923Paths {
            hidraw: std::path::PathBuf::from("/dev/null"),
            ffb_output: None,
            kernel_carries_force: false,
        };
        let opened = open_g923(&cfg, &paths);
        assert!(opened.is_ok(), "the guard was the only thing in the way: {:?}", opened.err());
        drop(opened);

        unsafe { std::env::remove_var(crate::lease::DIR_ENV) };
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod lights_only_tests {
    use super::*;

    /// Zero at either level means no stream. Both levels matter: a driver
    /// who zeroes the master expects silence everywhere, and one who zeroes
    /// a single game expects it there.
    #[test]
    fn zero_strength_wants_no_stream() {
        let quiet = Config { intensity: 0, ..Config::default() };
        assert!(!wants_haptics(&quiet, "assetto"), "master zero is silence");

        let mut per_game = Config::default();
        per_game.games.insert(
            "assetto".into(),
            crate::config::GameConfig { enabled: true, intensity: 0 },
        );
        assert!(!wants_haptics(&per_game, "assetto"), "this game is silenced");
        assert!(wants_haptics(&per_game, "dirt-rally-2"), "and only this game");

        assert!(wants_haptics(&Config::default(), "assetto"), "the default plays");
    }
}
