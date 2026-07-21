// SPDX-License-Identifier: GPL-2.0-only
//! The daemon loop: listen on both telemetry ports, synthesize while
//! telemetry flows, stop within [`SILENCE_TIMEOUT_MS`] of it stopping.
//!
//! One `poll(2)` over both UDP sockets with a short timeout drives
//! everything: packet parsing, sample generation paced by wall clock
//! (1 sample per elapsed millisecond, capped so scheduling stalls never
//! burst-force the wheel), the silence watchdog, and the SIGINT/SIGTERM
//! stop flag. The wheel stream is opened lazily on the first enabled
//! telemetry and torn down (with a clear) on silence, error, or exit,
//! so no force is ever left queued.

use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::leds::RevLeds;
use crate::synth::EngineSynth;
use crate::telemetry::Telemetry;
use crate::tf::TfStream;
use crate::{beamng, codemasters, f1, pcars, wrc};

/// Stop the stream after this much telemetry silence (spec safety rail).
pub const SILENCE_TIMEOUT_MS: u64 = 500;
/// Poll timeout; bounds both watchdog latency and shutdown latency.
const POLL_TIMEOUT_MS: i32 = 50;
/// Cap on samples generated per iteration (1 ms each): a scheduling stall
/// longer than this drops the backlog instead of bursting it.
const MAX_GEN_SAMPLES: u64 = 100;
/// How long to wait before re-probing for a wheel after a failed open.
const OPEN_RETRY: Duration = Duration::from_secs(5);

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

/// A live wheel stream plus the state that feeds it.
struct Active {
    stream: TfStream,
    synth: EngineSynth,
    game: &'static str,
    tel: Telemetry,
    last_telemetry: Instant,
    last_gen: Instant,
    samples: Vec<f32>,
    /// The wheel's rev display, when the config enables it and a wheel
    /// exposing `wheel_rev_level` was found at stream start; `None`
    /// otherwise. Stopped (blank + idle-pattern restore) with the stream.
    leds: Option<RevLeds>,
}

fn bind(port: u16) -> Result<UdpSocket> {
    let sock = UdpSocket::bind(("0.0.0.0", port))
        .map_err(|e| Error::Io(format!("bind UDP port {port}"), e))?;
    sock.set_nonblocking(true)
        .map_err(|e| Error::Io(format!("set_nonblocking on port {port}"), e))?;
    Ok(sock)
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
    install_signal_handlers()?;

    eprintln!(
        "logi-tf-sim: listening (codemasters/F1/WRC on udp/{}, pcars2/ams2 on udp/{}, beamng on udp/{})",
        cfg.codemasters_port, cfg.pcars_port, cfg.beamng_port
    );
    if !cfg.enabled {
        eprintln!("logi-tf-sim: master switch is off in the config; listening but not synthesizing");
    }

    let mut active: Option<Active> = None;
    let mut decoders = Decoders::default();
    let mut next_open_attempt = Instant::now();
    let mut buf = [0u8; 2048];

    while !STOP.load(Ordering::SeqCst) {
        poll_sockets(&[&cm_sock, &pc_sock, &bn_sock]);

        let mut latest: Option<(&'static str, Telemetry)> = None;
        drain(&cm_sock, &mut buf, |p| decoders.parse_codemasters_port(p), &mut latest);
        drain(&pc_sock, &mut buf, pcars::parse, &mut latest);
        drain(&bn_sock, &mut buf, |p| decoders.beamng.parse(p), &mut latest);

        let now = Instant::now();

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
                    None if now >= next_open_attempt => match TfStream::open(0) {
                        Ok(stream) => {
                            eprintln!(
                                "logi-tf-sim: stream start ({id}, rpm {:.0}/{:.0}, speed {:.0} m/s)",
                                tel.rpm, tel.max_rpm, tel.speed
                            );
                            let leds = if cfg.leds { RevLeds::discover() } else { None };
                            if leds.is_some() {
                                eprintln!("logi-tf-sim: driving the wheel's rev display");
                            }
                            active = Some(Active {
                                stream,
                                synth: EngineSynth::new(),
                                game: id,
                                tel,
                                last_telemetry: now,
                                last_gen: now,
                                samples: Vec::with_capacity(MAX_GEN_SAMPLES as usize),
                                leds,
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

        // Watchdog + generation for the active stream.
        let mut stop_reason: Option<String> = None;
        if let Some(a) = &mut active {
            if now.duration_since(a.last_telemetry) >= Duration::from_millis(SILENCE_TIMEOUT_MS) {
                stop_reason = Some(format!("telemetry silent for {SILENCE_TIMEOUT_MS} ms"));
            } else {
                let elapsed_ms = now.duration_since(a.last_gen).as_millis() as u64;
                let count = elapsed_ms.min(MAX_GEN_SAMPLES);
                if count > 0 {
                    // Advance by what we generated; drop any capped backlog.
                    a.last_gen = if elapsed_ms > MAX_GEN_SAMPLES {
                        now
                    } else {
                        a.last_gen + Duration::from_millis(count)
                    };
                    let intensity = cfg.effective_intensity(a.game);
                    let pitch = f32::from(cfg.pitch_pct) / 100.0;
                    let rpm = a.tel.rpm.min(a.tel.max_rpm * 1.05);
                    a.samples.clear();
                    a.synth.generate(rpm, a.tel.throttle, intensity, pitch, count as usize, &mut a.samples);
                    if let Err(e) = a.stream.push(&a.samples) {
                        stop_reason = Some(format!("stream push failed: {e}"));
                    }
                }
                // The rev display rides the same telemetry: RevLeds
                // paces itself (>=160 ms between writes) and only writes
                // changed levels, so this per-iteration call is cheap.
                if let Some(leds) = &mut a.leds {
                    leds.update(a.tel.rpm, a.tel.max_rpm, now);
                }
            }
        }
        if let Some(reason) = stop_reason {
            if let Some(mut a) = active.take() {
                if let Some(leds) = &mut a.leds {
                    leds.stop();
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
        eprintln!("logi-tf-sim: stream stop ({}): shutting down", a.game);
    }
    eprintln!("logi-tf-sim: exiting");
    Ok(())
}
