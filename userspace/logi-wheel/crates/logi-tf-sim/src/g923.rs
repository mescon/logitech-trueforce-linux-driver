// SPDX-License-Identifier: GPL-2.0-only
//! TrueForce for the Logitech G923 (PS edition, PID 0xC266; Xbox edition,
//! PID 0xC26E, once it lands on the same interface-2 transport).
//!
//! libtrueforce's own discovery only recognizes the RS50-family PIDs (see
//! `userspace/libtrueforce/src/discovery.c`'s `is_supported_wheel`), so
//! this module talks to the G923 directly: it finds the wheel's
//! interface-2 hidraw node (the vendor-page-0xFFFD TrueForce transport)
//! and, separately, the interface-0 HID device's `ffb_output` sysfs
//! attribute (the classic FFB engine's live net force, added by the
//! kernel driver), and streams the same 64-byte type-0x01 packets the DD
//! wheels get.
//!
//! # Why the FFB mirror is not optional
//!
//! Hardware fact (verified live on this wheel 2026-07-26): once a
//! type-0x01 sample stream is running on interface 2, the wheel's motor
//! follows that stream's `cur` field and *stops* reacting to interface
//! 0's classic FFB commands. So the moment this module starts streaming,
//! it becomes the wheel's only path to feeling anything at all: each
//! outgoing packet's samples are the game's synthesized engine texture
//! (if any) merged with the classic engine's own live force
//! (`ffb_output`, read fresh each packet), not just the texture on its
//! own. Skip that merge and the driver's steering-force FFB simply goes
//! silent for as long as the stream runs.
//!
//! # State machine
//!
//! ```text
//! Streaming --(a tick's samples are all exactly 0.0 AND ffb_output == 0,
//!              for IDLE_TIMEOUT)--> Idle
//! Idle --(a nonzero sample or a nonzero ffb_output)--> Streaming
//! ```
//!
//! "Silence" here is literal: every sample in the tick's chunk reads
//! exactly `0.0`, not merely quiet. [`crate::synth::EngineSynth`] keeps a
//! nonzero idle floor whenever the engine is nominally running, so in
//! practice this only happens when the synthesizer itself is producing
//! exact zero (engine stopped, i.e. `rpm == 0`, or master/per-game
//! intensity == 0) at the same time `ffb_output` reads zero.
//!
//! A dedicated writer thread (spawned by [`G923Stream::open`], after the
//! initial init sequence) owns the hidraw handle and paces outgoing
//! packets on a steady [`TICK_INTERVAL`], the same cadence libtrueforce's
//! own DD-wheel stream thread runs at. [`G923Stream::push`] only queues
//! samples for it over a bounded channel, load-shedding (dropping the
//! newest sample and rate-limit-logging it) instead of blocking the
//! caller if the writer ever falls behind; see [`CMD_CHANNEL_CAPACITY`].
//! Each tick the writer takes up to
//! [`NEW_PER_PACKET`] of them (padding any shortfall by repeating the
//! last sample, or zero if none arrived at all, so a tick never skips a
//! packet while waiting on the producer), merges in a freshly-read
//! `ffb_output`, and either builds+writes one sample packet or, per the
//! state machine above, withholds it or re-inits. Crossing into `Idle`
//! sends the type-0x04 stop template exactly once (handing the wheel back
//! to its native interface-0 FFB) and then withholds packets entirely:
//! any further type-0x01 traffic would immediately re-arm the follow
//! behavior above. Leaving `Idle` re-runs the full two-pass init sequence
//! (the same one [`G923Stream::open`] sends) before resuming samples,
//! since the wheel's stream-side state cannot be trusted to have survived
//! the gap. [`G923Stream::stop`] sends that same stop template at most
//! once, even if called more than once or followed by [`Drop`] (which
//! calls it too, and which also runs during panic unwinding): TF4ALL
//! issue #13 documents a stream left following a stale `cur` walking the
//! wheel to its force limit.
//!
//! # Sign flag
//!
//! `ffb_output`'s sign relative to the wheel's felt push direction was
//! calibrated on a c266 on hardware (2026-07-26): the classic engine
//! needs the same negation TF4ALL's own HID++-family path uses, even
//! though it is a different engine entirely, so the config default is
//! inverted. See [`Sign::resolve`]. A unit that turns out to push the
//! wrong way can flip this back with a config/env change, no code
//! change required.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Logitech's USB vendor id.
pub const VID: u16 = 0x046D;
/// G923, PlayStation/PC edition.
pub const PID_PS: u16 = 0xC266;
/// G923, Xbox/PC edition (same transport once it lands on interface 2;
/// recognized here so the Xbox-edition PC-mode switch has nothing left
/// to do here).
pub const PID_XBOX: u16 = 0xC26E;

/// G923, PlayStation/PC edition. The kernel binds this alongside the other
/// two; it was missing here entirely.
pub const PID_PS_PC: u16 = 0xC267;

/// USB interface number of the TrueForce (vendor page 0xFFFD) transport.
/// The kernel zero-pads the sysfs value ("02"), so compare numerically.
/// Where the TrueForce interface sits on every three-interface wheel seen
/// so far. Recorded for orientation only: discovery matches the vendor page
/// (see [`report_descriptor_matches`]), never this number, so a wheel that
/// numbers its interfaces differently still works.
#[allow(dead_code)]
const TF_IFACE_TYPICAL: u8 = 2;
/// First three bytes of a report descriptor opening with
/// `Usage Page (0xFFFD)` (tag 0x06, 2-byte little-endian data FD FF).
const VENDOR_PAGE_PREFIX: [u8; 3] = [0x06, 0xFD, 0xFF];

const HIDRAW_ROOT: &str = "/sys/class/hidraw";
const HID_BUS_ROOT: &str = "/sys/bus/hid/devices";
/// The kernel driver's read-only classic-FFB mirror attribute (interface 0).
const FFB_OUTPUT_ATTR: &str = "ffb_output";

/// Marker for "the driver's own engine is producing this wheel's force":
/// one attribute from the direct-drive sysfs group, which only exists
/// where that engine is running.
const DD_ENGINE_ATTR: &str = "wheel_strength";

/// Samples per packet's rolling window (13 slots, oldest first).
pub const WINDOW: usize = 13;
/// New samples appended per emitted packet.
pub const NEW_PER_PACKET: usize = 4;

/// Most unsent samples the writer will hold, in samples (each is 1 ms of
/// audio, so this is also the worst-case added latency in ms).
///
/// The producer generates 1000 samples/sec and the writer consumes
/// `NEW_PER_PACKET` per tick, which is also 1000/sec *if a tick is exactly
/// `TICK_INTERVAL`*. It never is: a tick is the sleep plus the sysfs read,
/// the packet build and the hidraw write. So the writer runs fractionally
/// slow, and without a bound the surplus accumulates forever. Measured on a
/// G923 on 2026-08-06 as throttle response that lagged further behind the
/// longer a session ran, while a steady idle felt correct, because a
/// constant signal hides latency and a changing one does not.
///
/// When it overflows the OLDEST samples go. For a live haptic stream,
/// freshness beats completeness: nobody can feel a dropped millisecond, and
/// everybody can feel a second of delay.
///
/// It must still sit ABOVE the producer's worst-case single push, or it
/// sheds during perfectly normal play. Written as a flat 32 it did exactly
/// that: the daemon hands over up to [`daemon::MAX_GEN_MS`] (100 ms) of
/// audio in one call when telemetry arrives slowly or a scheduling stall
/// bunches an iteration up, so a third of every such batch was discarded on
/// arrival, and the count only surfaced at teardown. So it is derived from
/// that cap rather than picked, with headroom for the writer's own lag: the
/// bound is a latency ceiling for a stalled transport, not a working queue
/// depth. 128 ms is also what the direct-drive path allows
/// (`LOGITF_TF_MAX_PENDING_MS`), so both wheel families have the same
/// worst-case haptic delay.
pub const MAX_PENDING_MS: usize = crate::daemon::MAX_GEN_MS as usize + MAX_PENDING_HEADROOM_MS;
/// Slack above one producer burst, so a burst arriving on top of the tail
/// of the previous one is still not shed.
const MAX_PENDING_HEADROOM_MS: usize = 28;

/// Backlog bound, in samples, derived from [`MAX_PENDING_MS`].
///
/// Written as `NEW_PER_PACKET * 8` while the stream ran at 1 kHz, where it
/// happened to equal 32 ms of audio. It is a LATENCY bound, so milliseconds
/// are what it means; as a fixed sample count it silently shrank to 8 ms
/// when the stream went to 4 kHz, and every 20 ms chunk the producer pushed
/// overflowed it instantly. That looked exactly like the wheel being unable
/// to keep up, when the transport was in fact delivering all 4000
/// samples/sec it was asked for.
pub const MAX_PENDING: usize = MAX_PENDING_MS * crate::synth::SAMPLES_PER_MS;
/// Wire-format zero force (offset-binary center).
pub const CENTER: u16 = 0x8000;
/// Wire packet length.
pub const PACKET_LEN: usize = 64;

/// How long a tick's samples must all read exactly zero, together with a
/// zero `ffb_output`, before [`G923Stream`] sends the stop template and
/// stops streaming (see module docs).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Spacing between init packets, matching libtrueforce's `session.c`
/// (the capture this was extracted from showed ~2-4 ms; below that risks
/// overrunning the device's interrupt-OUT processing on slower firmware).
/// Kept mid-range rather than at the bottom of that window for margin.
const INIT_INTERPACKET: Duration = Duration::from_micros(3000);
/// The writer thread's steady per-packet cadence: [`NEW_PER_PACKET`] new
/// samples every tick at the synthesizer's 1 kHz output rate works out to
/// 250 packets/sec, matching libtrueforce's own DD-wheel stream thread
/// (`stream.c`, 250 Hz).
/// The writer thread's steady per-packet cadence: [`NEW_PER_PACKET`] new
/// samples every tick at the synthesiser's 4 kHz output rate works out to
/// 1000 packets/sec, matching libtrueforce's DD-wheel stream thread
/// (`stream.c`).
///
/// Was 4 ms (250 packets/sec, a 1 kHz stream) until the transports were
/// measured: this wheel sustained 1000 ticks/sec delivering all 3999
/// samples/sec asked of it, with nothing dropped. Verified on a c266,
/// 2026-08-08.
const TICK_INTERVAL: Duration = Duration::from_millis(1);
/// Capacity of the channel [`G923Stream::push`] feeds the writer thread
/// through. Bounded so a wedged hidraw write cannot grow the backlog
/// without limit; sized well above one daemon poll's worth of `push`
/// calls (the daemon calls `push` roughly once per ~50 ms iteration, and
/// the writer drains the channel every tick).
///
/// Load-shedding, deliberately: [`G923Stream::push`] feeds this channel
/// with `try_send`, never the blocking `send`. If the writer thread ever
/// falls behind (e.g. a hung hidraw write) and the channel fills, `push`
/// drops the newest sample and rate-limit-logs it rather than blocking
/// the daemon thread. The daemon serves every other wheel too, so it
/// must never wedge on one stalled G923; stale force data delivered late
/// is worse than a dropped sample.
const CMD_CHANNEL_CAPACITY: usize = 16;
/// Minimum spacing between "writer channel full, dropping" warnings, so
/// a sustained stall logs occasionally instead of once per dropped push.
const DROP_WARN_INTERVAL: Duration = Duration::from_secs(5);

include!(concat!(env!("OUT_DIR"), "/g923_init_data.rs"));

/// Every G923 product id this transport recognizes.
///
/// The daemon keeps `logi-wheel-core` as a dev-dependency only, on purpose,
/// so this cannot import the shared list and holds its own copy. That copy
/// omitted `c267`: a PlayStation/PC-edition G923 was identified correctly by
/// the settings pages and then skipped entirely by discovery here, leaving
/// the daemon to fall through to a stream that wheel never answers.
///
/// `tests/frontend_compat.rs` fails if this disagrees with
/// `logi_wheel_core::device::G923_PIDS`, which is the guard the previous
/// copy did not have.
pub const PIDS: &[u16] = &[PID_PS, PID_PS_PC, PID_XBOX];

/// True if `pid` is a recognized G923 edition.
fn is_g923_pid(pid: u16) -> bool {
    PIDS.contains(&pid)
}

// ---------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------

/// Paths [`discover`] found for one G923: the TrueForce hidraw node to
/// stream to, and (best-effort) the classic engine's `ffb_output`
/// attribute to mirror. A missing `ffb_output` is not a discovery
/// failure: [`FfbMirror`] treats it as a permanently-zero mirror, which
/// degrades to "no force merge" rather than refusing to stream at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct G923Paths {
    pub hidraw: PathBuf,
    pub ffb_output: Option<PathBuf>,
    /// True when this wheel's force feedback runs on the driver's own
    /// direct-drive engine, which splices the live force into whatever
    /// the stream owner writes. Nothing to mirror here, and nothing to
    /// lose by streaming: the kernel carries the force itself.
    ///
    /// Recognised by the direct-drive sysfs group being present on a
    /// sibling interface, so it follows whatever the driver actually did
    /// rather than a product id. On the G923 Xbox edition that group
    /// appears only under the `g923_xbox_dd_engine` module parameter.
    pub kernel_carries_force: bool,
}

/// Scan the real sysfs (`/sys/class/hidraw`, `/sys/bus/hid/devices`) for a
/// G923's TrueForce interface. See [`discover_at`] for the algorithm.
pub fn discover() -> Option<G923Paths> {
    discover_at(Path::new(HIDRAW_ROOT), Path::new(HID_BUS_ROOT))
}

/// Same as [`discover`], against caller-supplied sysfs roots (unit tests
/// point these at a fabricated tree; see the `tests` module below for its
/// shape).
///
/// Algorithm, mirroring libtrueforce's `discovery.c`/`sysfs.c`:
/// 1. Walk `hidraw_root` for a `hidrawN` entry whose `device` symlink
///    resolves to a HID device sitting on USB interface 2 of a G923
///    (`bInterfaceNumber`/`idVendor`/`idProduct` read from the resolved
///    path's parent USB-interface and USB-device directories).
/// 2. Confirm it via `device/report_descriptor`'s vendor-page prefix, the
///    same signal `ffb-proxy` uses to pick its own hidraw node.
/// 3. Correlate the resolved USB device root against every entry under
///    `hid_bus_root` to find the sibling interface-0 HID device, for
///    `ffb_output` if it has one and for the direct-drive engine's own
///    attributes if that engine is what drives this wheel.
pub fn discover_at(hidraw_root: &Path, hid_bus_root: &Path) -> Option<G923Paths> {
    let entries = std::fs::read_dir(hidraw_root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("hidraw") {
            continue;
        }
        let device_link = entry.path().join("device");
        let Ok(hid_device_dir) = std::fs::canonicalize(&device_link) else { continue };
        if !is_g923_device(&hid_device_dir) {
            continue;
        }
        if !report_descriptor_matches(&device_link) {
            continue;
        }
        return Some(G923Paths {
            hidraw: PathBuf::from("/dev").join(&*name),
            ffb_output: find_sibling_attr(&hid_device_dir, hid_bus_root, FFB_OUTPUT_ATTR),
            kernel_carries_force: find_sibling_attr(&hid_device_dir, hid_bus_root, DD_ENGINE_ATTR)
                .is_some(),
        });
    }
    None
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn parse_hex_u16(s: &str) -> Option<u16> {
    u16::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
}

/// True if the (already-canonicalized) HID device directory belongs to a
/// G923, by the `idVendor`/`idProduct` of the USB device two levels up.
///
/// Deliberately says nothing about WHICH interface. The interface number is
/// not what identifies the TrueForce transport; the vendor usage page in its
/// report descriptor is, and [`report_descriptor_matches`] checks that. This
/// used to also require `bInterfaceNumber == 2`, which is true of every
/// wheel with three interfaces but describes where the transport happened to
/// sit rather than what it is. A G923 Xbox has two interfaces, with the
/// joystick and HID++ sharing interface 0, so anything it carries on
/// interface 1 was unreachable however plainly its descriptor announced
/// itself (issue #27).
fn is_g923_device(hid_device_dir: &Path) -> bool {
    let Some(iface_dir) = hid_device_dir.parent() else { return false };
    let Some(usb_dir) = iface_dir.parent() else { return false };
    let vid = read_trim(&usb_dir.join("idVendor")).and_then(|s| parse_hex_u16(&s));
    let pid = read_trim(&usb_dir.join("idProduct")).and_then(|s| parse_hex_u16(&s));
    vid == Some(VID) && pid.is_some_and(is_g923_pid)
}

/// True if `device_link`'s `report_descriptor` opens with the vendor-page
/// prefix ([`VENDOR_PAGE_PREFIX`], usage page `0xFFFD`).
///
/// This is the discriminator, not a secondary check: it is what tells the
/// TrueForce transport apart from the wheel's other interfaces. On an RS50
/// the joystick opens `05 01`, HID++ opens `06 43 ff` (page `0xFF43`), and
/// only the TrueForce interface opens `06 fd ff`, so the page selects
/// exactly one interface per wheel regardless of how they are numbered.
fn report_descriptor_matches(device_link: &Path) -> bool {
    let Ok(bytes) = std::fs::read(device_link.join("report_descriptor")) else { return false };
    bytes.starts_with(&VENDOR_PAGE_PREFIX)
}

/// Find the sibling HID device (same physical USB device as
/// `hid_device_dir`, i.e. same wheel) exposing [`FFB_OUTPUT_ATTR`], by
/// scanning `hid_bus_root`.
fn find_sibling_attr(hid_device_dir: &Path, hid_bus_root: &Path, attr: &str) -> Option<PathBuf> {
    // Two levels up from the HID device dir: past the USB interface dir,
    // to the USB device dir shared by every interface of this one wheel.
    let usb_root = hid_device_dir.parent()?.parent()?;
    let entries = std::fs::read_dir(hid_bus_root).ok()?;
    for entry in entries.flatten() {
        let Ok(candidate) = std::fs::canonicalize(entry.path()) else { continue };
        if !candidate.starts_with(usb_root) {
            continue;
        }
        let path = entry.path().join(attr);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------
// Wire-format conversions
// ---------------------------------------------------------------------

/// The classic engine's mirror sign relative to the wire's offset-binary
/// convention. Hardware-calibrated on a c266 (2026-07-26): the wheel
/// interprets cur with the OPPOSITE sign from the native classic force,
/// matching TF4ALL's finding on the HID++ path, so the config defaults
/// to inverted. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Normal,
    Inverted,
}

impl Sign {
    /// Resolve the sign flag: `LOGI_TF_SIM_G923_FFB_SIGN` (`"invert"`,
    /// `"inverted"`, or `"1"` selects [`Sign::Inverted`]; anything else,
    /// including unset, falls through) overrides `cfg_invert` (the
    /// persisted `g923.ffb_invert` config key) when the variable is set at
    /// all, so a one-off hardware check does not require editing the
    /// config file. The config's own default is inverted (hardware-
    /// calibrated); the env var, when set, wins over whatever the config
    /// says.
    pub fn resolve(cfg_invert: bool) -> Sign {
        match std::env::var("LOGI_TF_SIM_G923_FFB_SIGN") {
            Ok(v) if matches!(v.as_str(), "invert" | "inverted" | "1") => Sign::Inverted,
            Ok(_) => Sign::Normal,
            Err(_) => {
                if cfg_invert {
                    Sign::Inverted
                } else {
                    Sign::Normal
                }
            }
        }
    }

    fn apply(self, v: i16) -> i16 {
        match self {
            Sign::Normal => v,
            // i16::MIN has no positive i16 representation; saturate to
            // the max magnitude instead of overflowing.
            Sign::Inverted => v.checked_neg().unwrap_or(i16::MAX),
        }
    }
}

/// Clamp a driver-reported `ffb_output` (documented range -32768..32767,
/// but read from a sysfs `%d` with no enforced bound) into `i16`.
pub fn clamp_ffb(raw: i32) -> i16 {
    raw.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// `sample` (-1.0..1.0) to a signed 16-bit wire amplitude, matching
/// libtrueforce's `logitf_float_to_wire` scaling (clamp, then `* 32767`).
fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Signed 16-bit amplitude to the wire's offset-binary `u16`
/// (`0x8000` = zero force), matching libtrueforce's `logitf_s16_to_wire`.
pub fn i16_to_wire(sample: i16) -> u16 {
    (i32::from(sample) + 0x8000) as u16
}

/// `sample` (-1.0..1.0) to the wire's offset-binary `u16` directly, with no
/// force merge (used by the packet-builder tests below; the merged path
/// used at runtime is [`mix_to_wire`]).
pub fn float_to_wire(sample: f32) -> u16 {
    i16_to_wire(float_to_i16(sample))
}

/// The per-packet force merge: `sample` (the synthesized engine-texture
/// amplitude tf-sim would otherwise send alone, -1.0..1.0) plus the
/// classic engine's live `ffb_output` (raw driver units, sign-and-clamp
/// adjusted), saturating to the wire's signed 16-bit range before the
/// offset-binary conversion. See the module docs for why this merge, not
/// a plain mirror or a plain pass-through, is what keeps the wheel's real
/// steering force alive while the TrueForce stream runs.
pub fn mix_to_wire(sample: f32, ffb_raw: i32, sign: Sign) -> u16 {
    let sample_i = i32::from(float_to_i16(sample));
    let ffb_i = i32::from(sign.apply(clamp_ffb(ffb_raw)));
    let mixed = (sample_i + ffb_i).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    i16_to_wire(mixed)
}

// ---------------------------------------------------------------------
// Packet builder
// ---------------------------------------------------------------------

/// Shift `window` left by `new.len()` (dropping the oldest entries) and
/// append `new` at the tail, in place. `new` must be no longer than
/// [`WINDOW`]; the DD-wheel format only ever calls this with exactly
/// [`NEW_PER_PACKET`] entries.
fn slide_window(window: &mut [u16; WINDOW], new: &[u16]) {
    let n = new.len().min(WINDOW);
    window.copy_within(n.., 0);
    window[WINDOW - n..].copy_from_slice(&new[..n]);
}

/// Build one type-0x01 sample packet from `window` (already updated by
/// [`slide_window`]), byte-for-byte matching libtrueforce's `stream.c`
/// `build_packet` (kept here, not reused, because the G923 is outside
/// libtrueforce's own supported-wheel table; see the module docs):
///
/// ```text
///  0       0x01           HID report id
///  4       0x01           packet type: sample
///  5       seq            u8, wraps
///  6..9    cur, duplicated (= window[WINDOW-1])
///  10      0x04           new-samples-this-packet (NEW_PER_PACKET)
///  11      0x0d           constant per captures
///  12..63  13 x 4B        window slots, oldest first, each duplicated
/// ```
fn build_sample_packet(seq: u8, window: &[u16; WINDOW]) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x01;
    pkt[4] = 0x01;
    pkt[5] = seq;
    let cur = window[WINDOW - 1];
    pkt[6] = (cur & 0xff) as u8;
    pkt[7] = (cur >> 8) as u8;
    pkt[8] = (cur & 0xff) as u8;
    pkt[9] = (cur >> 8) as u8;
    pkt[10] = NEW_PER_PACKET as u8;
    pkt[11] = 0x0d;
    for (i, &v) in window.iter().enumerate() {
        let off = 12 + i * 4;
        pkt[off] = (v & 0xff) as u8;
        pkt[off + 1] = (v >> 8) as u8;
        pkt[off + 2] = (v & 0xff) as u8;
        pkt[off + 3] = (v >> 8) as u8;
    }
    pkt
}

/// The type-0x04 stop template from the embedded init sequence (its
/// second-to-last packet), with `seq` written into offset 5. Sending this
/// hands the wheel back to native interface-0 FFB.
fn stop_packet(seq: u8) -> [u8; PACKET_LEN] {
    let mut pkt = TF_INIT_PACKETS[TF_INIT_PACKET_COUNT - 2];
    pkt[5] = seq;
    pkt
}

// ---------------------------------------------------------------------
// ffb_output mirror
// ---------------------------------------------------------------------

/// Reads the classic engine's live net force from an already-open
/// `ffb_output` attribute file, or reports a constant zero when no
/// attribute was found at discovery time.
struct FfbMirror {
    file: Option<File>,
}

impl FfbMirror {
    fn open(path: Option<&Path>) -> FfbMirror {
        FfbMirror { file: path.and_then(|p| File::open(p).ok()) }
    }

    /// `pread`s the attribute (offset 0, no seek needed) and parses it as
    /// a decimal `i32`. Any failure (missing file, transient read error,
    /// unparsable content) reads as zero: a wheel this module cannot read
    /// force from is treated the same as one reporting no force, which is
    /// the safe direction to fail in (never invents force that is not
    /// there).
    fn read_raw(&self) -> i32 {
        let Some(file) = &self.file else { return 0 };
        let mut buf = [0u8; 16];
        let Ok(n) = file.read_at(&mut buf, 0) else { return 0 };
        std::str::from_utf8(&buf[..n]).ok().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------
// Idle policy (pure, clock-injected state machine)
// ---------------------------------------------------------------------

/// What [`G923Stream::push`] should do this tick, per [`IdlePolicy::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleAction {
    /// Keep streaming samples normally.
    Stream,
    /// Just crossed the idle timeout: send the stop template once, then
    /// withhold sample packets.
    EnterIdle,
    /// Already idle; keep withholding.
    StayIdle,
    /// Activity resumed: re-init before this tick's sample packet.
    Resume,
}

/// The clock-injected state machine behind the module's `Streaming`/`Idle`
/// states (see module docs). Pure and independent of any file descriptor,
/// so it is unit-tested without a real wheel.
#[derive(Debug)]
struct IdlePolicy {
    /// When the current run of silent ticks started; `None` while active.
    silent_since: Option<Instant>,
    idle: bool,
}

impl IdlePolicy {
    fn new() -> IdlePolicy {
        IdlePolicy { silent_since: None, idle: false }
    }

    fn tick(&mut self, now: Instant, silent: bool, timeout: Duration) -> IdleAction {
        if silent {
            if self.idle {
                return IdleAction::StayIdle;
            }
            let since = *self.silent_since.get_or_insert(now);
            if now.duration_since(since) >= timeout {
                self.idle = true;
                return IdleAction::EnterIdle;
            }
            return IdleAction::Stream;
        }
        self.silent_since = None;
        if self.idle {
            self.idle = false;
            return IdleAction::Resume;
        }
        IdleAction::Stream
    }
}

// ---------------------------------------------------------------------
// Writer-thread pacing
// ---------------------------------------------------------------------

/// Supplies the writer thread's per-tick timing signal, decoupled from
/// the writer's own logic so tests can drive ticks deterministically
/// (see `ManualPacer` in the tests module below) instead of racing a real
/// sleep or a real multi-second idle timeout. Each tick also carries the
/// `Instant` to treat as "now", so tests can jump the clock forward (past
/// [`IDLE_TIMEOUT`], for instance) without actually waiting.
trait Pacer: Send {
    /// Block until the next tick is due, returning the `Instant` to treat
    /// as "now" for it. `None` once no further ticks will ever arrive (a
    /// test's manual sender was dropped without an explicit stop), telling
    /// the writer thread to send the safety-net stop and exit.
    fn wait(&mut self) -> Option<Instant>;

    /// Called once per full tick cycle, after the writer thread has acted
    /// on it (packet written, packet withheld, or stop sent). Production's
    /// [`SteadyPacer`] no-ops; the test pacer uses it to hand the test
    /// thread a synchronization point so assertions never race the write.
    fn ack(&mut self) {}
}

/// Real-time pacing: a plain sleep loop at [`TICK_INTERVAL`], matching the
/// DD-wheel path's dedicated 250 Hz stream thread.
/// Paces the writer at a fixed RATE rather than a fixed sleep.
///
/// `thread::sleep(TICK_INTERVAL)` between iterations yields a period of
/// `TICK_INTERVAL + however long the work took`, so the writer always runs
/// slower than the nominal rate and the backlog grows. Sleeping until the
/// next deadline instead keeps the long-run rate correct, and a deadline
/// already in the past (a scheduling stall) is skipped forward rather than
/// chased, which would burst.
struct SteadyPacer {
    next: Option<Instant>,
}

impl SteadyPacer {
    fn new() -> Self {
        SteadyPacer { next: None }
    }
}

impl Pacer for SteadyPacer {
    fn wait(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let deadline = self.next.unwrap_or(now + TICK_INTERVAL);
        if let Some(delay) = deadline.checked_duration_since(now) {
            thread::sleep(delay);
        }
        let now = Instant::now();
        // Re-base rather than accumulate when we have fallen behind by more
        // than a whole tick, so a stall costs one late packet, not a burst.
        self.next = Some(if now.duration_since(deadline) > TICK_INTERVAL {
            now + TICK_INTERVAL
        } else {
            deadline + TICK_INTERVAL
        });
        Some(now)
    }
}

/// A command sent from [`G923Stream`] (the caller-facing handle) to the
/// writer thread that owns the hidraw file descriptor.
enum Cmd {
    /// New samples to append to the writer's pending queue.
    Push(Vec<f32>),
    /// Send the stop template once and exit the thread.
    Stop,
}

// ---------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------

/// Abstraction over the writer thread's device handle. The only
/// production implementation is [`File`]; tests inject a sink that fails
/// on a chosen call to get a deterministic write error without needing a
/// real hidraw node or an OS-level trick (e.g. a pipe with its read end
/// closed) to force one.
trait HidrawSink: Send {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
}

impl HidrawSink for File {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        Write::write_all(self, buf)
    }
}

/// The writer thread's private state: the interface-2 hidraw node, the
/// interface-0 `ffb_output` mirror, and everything [`Writer::tick`] needs
/// to build the next packet. Lives entirely on the background thread
/// spawned by [`G923Stream::open`]; the caller-facing [`G923Stream`] only
/// talks to it over a channel.
struct Writer {
    hidraw: Box<dyn HidrawSink>,
    ffb: FfbMirror,
    sign: Sign,
    seq: u8,
    window: [u16; WINDOW],
    /// Samples queued by [`Cmd::Push`] between whole
    /// [`NEW_PER_PACKET`]-sized chunks.
    pending: Vec<f32>,
    /// Samples discarded to keep the backlog bounded; a running total, so a
    /// steadily climbing figure means the writer cannot keep pace.
    dropped_stale: usize,
    idle: IdlePolicy,
}

impl Writer {
    fn new(hidraw: impl HidrawSink + 'static, ffb: FfbMirror, sign: Sign) -> Writer {
        Writer {
            hidraw: Box::new(hidraw),
            ffb,
            sign,
            seq: 0,
            window: [CENTER; WINDOW],
            pending: Vec::new(),
            dropped_stale: 0,
            idle: IdlePolicy::new(),
        }
    }

    /// Send the embedded 68-packet sequence twice, sequence byte
    /// restarting at 1 each pass, matching libtrueforce's `session.c`.
    fn send_init(&mut self) -> io::Result<()> {
        for _pass in 0..2 {
            for (i, packet) in TF_INIT_PACKETS.iter().enumerate() {
                let mut pkt = *packet;
                pkt[5] = ((i + 1) & 0xff) as u8;
                self.hidraw.write_all(&pkt)?;
                thread::sleep(INIT_INTERPACKET);
            }
        }
        self.seq = ((TF_INIT_PACKET_COUNT + 1) & 0xff) as u8;
        self.window = [CENTER; WINDOW];
        Ok(())
    }

    fn next_seq(&mut self) -> u8 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }

    fn send_stop(&mut self) -> io::Result<()> {
        let seq = self.next_seq();
        self.hidraw.write_all(&stop_packet(seq))
    }

    /// Take up to [`NEW_PER_PACKET`] samples FIFO off `pending`, padding
    /// any shortfall by repeating the last sample taken (or zero, if none
    /// were available at all). Steady packet cadence means a tick can
    /// come due before the producer has delivered a full chunk; padding
    /// keeps every tick emitting a packet on schedule instead of skipping
    /// it or blocking, at the cost of repeating the most recent sample
    /// into the remaining slot(s).
    /// Append `samples`, discarding the oldest beyond [`MAX_PENDING`] so a
    /// producer/consumer rate mismatch cannot turn into unbounded latency.
    fn push_pending(&mut self, samples: Vec<f32>) {
        self.pending.extend(samples);
        if self.pending.len() > MAX_PENDING {
            let excess = self.pending.len() - MAX_PENDING;
            self.pending.drain(..excess);
            self.dropped_stale += excess;
        }
    }

    fn take_chunk(&mut self) -> [f32; NEW_PER_PACKET] {
        let mut chunk = [0.0f32; NEW_PER_PACKET];
        let n = self.pending.len().min(NEW_PER_PACKET);
        chunk[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        let fill = if n > 0 { chunk[n - 1] } else { 0.0 };
        for slot in &mut chunk[n..] {
            *slot = fill;
        }
        chunk
    }

    /// One packet tick: read the live `ffb_output`, take this tick's
    /// sample chunk, run the idle policy, and either withhold, re-init, or
    /// build+write one sample packet. See the module docs' state machine.
    fn tick(&mut self, now: Instant) -> io::Result<()> {
        let ffb_raw = self.ffb.read_raw();
        let chunk = self.take_chunk();
        let silent = ffb_raw == 0 && chunk.iter().all(|&s| s == 0.0);

        match self.idle.tick(now, silent, IDLE_TIMEOUT) {
            IdleAction::Stream => {}
            IdleAction::EnterIdle => return self.send_stop(),
            IdleAction::StayIdle => return Ok(()),
            IdleAction::Resume => self.send_init()?,
        }

        let mut new_wire = [CENTER; NEW_PER_PACKET];
        for (slot, &sample) in new_wire.iter_mut().zip(chunk.iter()) {
            *slot = mix_to_wire(sample, ffb_raw, self.sign);
        }
        slide_window(&mut self.window, &new_wire);
        let seq = self.next_seq();
        self.hidraw.write_all(&build_sample_packet(seq, &self.window))
    }
}

/// The writer thread's body: wait for each tick, drain whatever commands
/// arrived since the last one, and either stop or perform the tick.
/// Exits (after a best-effort stop write) on an explicit [`Cmd::Stop`], a
/// fatal write error, or `pacer` running out of ticks.
fn run_writer(mut writer: Writer, cmd_rx: mpsc::Receiver<Cmd>, mut pacer: impl Pacer) {
    while let Some(now) = pacer.wait() {
        let mut stop_requested = false;
        loop {
            match cmd_rx.try_recv() {
                Ok(Cmd::Push(samples)) => writer.push_pending(samples),
                Ok(Cmd::Stop) => stop_requested = true,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    stop_requested = true;
                    break;
                }
            }
        }
        if stop_requested {
            let _ = writer.send_stop();
            // Say whether the writer kept pace. A running total that is not
            // zero means the producer outran this transport and the oldest
            // samples were thrown away, which is silent otherwise: dropping
            // stale samples is normal behaviour here, so a rate the wheel
            // cannot sustain looks exactly like a healthy stream.
            if writer.dropped_stale > 0 {
                eprintln!(
                    "logi-tf-sim: G923 writer dropped {} stale samples \
                     (the producer outran this transport)",
                    writer.dropped_stale
                );
            }
            pacer.ack();
            return;
        }
        // A write failure (e.g. the wheel was unplugged) is fatal: send a
        // best-effort stop first (ignoring its error too - if the device
        // is truly gone this write just fails harmlessly, but if the
        // failure was transient the wheel is not left following a stale
        // `cur`, the TF4ALL issue #13 failure this exists to prevent),
        // then stop trying and exit, which drops `cmd_rx` so the next
        // `push()` call fails and the daemon notices and tears the
        // stream down. `ack` is called only once the best-effort stop
        // write has actually happened (mirroring the `stop_requested`
        // branch above), so a test driving this via `ManualPacer` never
        // races the write.
        let fatal = writer.tick(now).is_err();
        if fatal {
            let _ = writer.send_stop();
            pacer.ack();
            return;
        }
        pacer.ack();
    }
    // The pacer ran out of ticks without an explicit Stop (production:
    // never happens, `SteadyPacer` never returns `None`). Still send the
    // safety-net stop before exiting.
    let _ = writer.send_stop();
}

/// An open G923 TrueForce session. [`G923Stream::open`] sends the
/// two-pass init sequence on the calling thread, then hands the hidraw
/// file and the `ffb_output` mirror off to a dedicated writer thread
/// ([`run_writer`]) that paces outgoing packets on [`TICK_INTERVAL`];
/// [`push`](Self::push) only queues samples for it. [`stop`](Self::stop)
/// (and [`Drop`], which calls it too) sends the stop template exactly
/// once so the wheel is never left following a stale `cur`.
pub struct G923Stream {
    cmd_tx: mpsc::SyncSender<Cmd>,
    handle: Option<thread::JoinHandle<()>>,
    stopped: bool,
    /// Total samples load-shed by [`push`](Self::push) because the
    /// writer's channel was full. See [`CMD_CHANNEL_CAPACITY`].
    dropped: u64,
    /// When [`push`](Self::push) last logged a drop, for
    /// [`DROP_WARN_INTERVAL`] rate-limiting.
    last_drop_warn: Option<Instant>,
}

impl G923Stream {
    /// Open `paths.hidraw`, send the two-pass init sequence (blocks for
    /// roughly `2 * 68 * INIT_INTERPACKET`, a bit under half a second),
    /// spawn the writer thread, and return a stream ready for
    /// [`push`](Self::push).
    pub fn open(paths: &G923Paths, sign: Sign) -> io::Result<G923Stream> {
        Self::open_with_pacer(paths, sign, SteadyPacer::new())
    }

    fn open_with_pacer(paths: &G923Paths, sign: Sign, pacer: impl Pacer + 'static) -> io::Result<G923Stream> {
        let hidraw = OpenOptions::new().read(true).write(true).open(&paths.hidraw)?;
        Self::open_with_sink(hidraw, FfbMirror::open(paths.ffb_output.as_deref()), sign, pacer)
    }

    /// Common tail of [`open_with_pacer`](Self::open_with_pacer): build
    /// the [`Writer`] from an already-open sink, send the init sequence,
    /// and spawn the writer thread. Split out so tests can supply a
    /// [`HidrawSink`] other than a real hidraw [`File`] (see
    /// `FlakyFile` in the tests module) without duplicating any of this.
    fn open_with_sink(
        hidraw: impl HidrawSink + 'static,
        ffb: FfbMirror,
        sign: Sign,
        pacer: impl Pacer + 'static,
    ) -> io::Result<G923Stream> {
        let mut writer = Writer::new(hidraw, ffb, sign);
        writer.send_init()?;
        let (cmd_tx, cmd_rx) = mpsc::sync_channel(CMD_CHANNEL_CAPACITY);
        let handle = thread::Builder::new().name("g923-tf-writer".into()).spawn(move || run_writer(writer, cmd_rx, pacer))?;
        Ok(G923Stream { cmd_tx, handle: Some(handle), stopped: false, dropped: 0, last_drop_warn: None })
    }

    /// Queue `samples` (each -1.0..1.0, tf-sim's usual synthesized-audio
    /// rate) for the writer thread; it takes them off in
    /// [`NEW_PER_PACKET`]-sample chunks at its own steady pace (see the
    /// module docs). Fails if the writer thread has already stopped
    /// (explicit [`stop`](Self::stop) or a fatal write error).
    ///
    /// Never blocks: if the writer's channel is full (it has fallen
    /// behind, e.g. a hung hidraw write), this sample is dropped and
    /// counted instead of blocking the caller (see
    /// [`CMD_CHANNEL_CAPACITY`]'s docs for why). The caller - the
    /// daemon's poll loop, which also services every other wheel - is
    /// never told about the drop via the return value, since it is not
    /// an error from the caller's point of view, only logged
    /// (rate-limited by [`DROP_WARN_INTERVAL`]).
    pub fn push(&mut self, samples: &[f32]) -> io::Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        match self.cmd_tx.try_send(Cmd::Push(samples.to_vec())) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "G923 TrueForce writer thread has stopped"))
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.dropped += 1;
                let now = Instant::now();
                let should_warn = match self.last_drop_warn {
                    Some(last) => now.duration_since(last) >= DROP_WARN_INTERVAL,
                    None => true,
                };
                if should_warn {
                    self.last_drop_warn = Some(now);
                    eprintln!(
                        "logi-tf-sim: G923 writer channel full, dropped {} sample push(es) so far (hidraw stalled?)",
                        self.dropped
                    );
                }
                Ok(())
            }
        }
    }

    /// Best-effort stop: tells the writer thread to send the stop template
    /// and waits for it to actually do so before returning. Idempotent:
    /// a second call (including the one from [`Drop`]) is a no-op, so the
    /// stop template is written at most once per stream regardless of how
    /// many times `stop` runs.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = self.cmd_tx.send(Cmd::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for G923Stream {
    fn drop(&mut self) {
        // Runs during panic unwinding too: the last-resort guard against
        // TF4ALL issue #13 (a stream left following a stale `cur` walks
        // the wheel to its force limit). `stop`'s idempotence means this
        // is a no-op if the caller already stopped the stream explicitly.
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-sim-g923-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- discovery -------------------------------------------------------

    /// Build a fake sysfs tree with the same relative shape as the real
    /// one: a `devices/usb1/1-1` USB device directory holding an
    /// interface-0 and an interface-2 HID device dir, a `hidraw` root with
    /// a symlinked `device`, and a `hid_bus` root with symlinked entries
    /// for both interfaces (mirroring `/sys/bus/hid/devices`, whose
    /// entries are themselves symlinks into the same device tree).
    struct FakeSysfs {
        root: PathBuf,
        hidraw_root: PathBuf,
        hid_bus_root: PathBuf,
    }

    impl FakeSysfs {
        fn build(vid: &str, pid: &str, report_descriptor: &[u8]) -> FakeSysfs {
            let root = tempdir();
            let usb_dev = root.join("devices/usb1/1-1");
            std::fs::create_dir_all(&usb_dev).unwrap();
            std::fs::write(usb_dev.join("idVendor"), format!("{vid}\n")).unwrap();
            std::fs::write(usb_dev.join("idProduct"), format!("{pid}\n")).unwrap();

            let if0 = usb_dev.join("1-1:1.0");
            let hid0 = if0.join("0003:046D:C266.0003");
            std::fs::create_dir_all(&hid0).unwrap();
            std::fs::write(if0.join("bInterfaceNumber"), "00\n").unwrap();
            std::fs::write(hid0.join(FFB_OUTPUT_ATTR), "0\n").unwrap();

            let if2 = usb_dev.join("1-1:1.2");
            let hid2 = if2.join("0003:046D:C266.0005");
            std::fs::create_dir_all(&hid2).unwrap();
            std::fs::write(if2.join("bInterfaceNumber"), "02\n").unwrap();
            std::fs::write(hid2.join("report_descriptor"), report_descriptor).unwrap();

            let hidraw_root = root.join("class/hidraw");
            let hidraw3 = hidraw_root.join("hidraw3");
            std::fs::create_dir_all(&hidraw3).unwrap();
            symlink(&hid2, hidraw3.join("device")).unwrap();

            let hid_bus_root = root.join("bus/hid/devices");
            std::fs::create_dir_all(&hid_bus_root).unwrap();
            symlink(&hid0, hid_bus_root.join("0003:046D:C266.0003")).unwrap();
            symlink(&hid2, hid_bus_root.join("0003:046D:C266.0005")).unwrap();

            FakeSysfs { root, hidraw_root, hid_bus_root }
        }
    }

    /// A two-interface wheel, the G923 Xbox shape: interface 0 carries the
    /// joystick AND HID++, and the TrueForce transport is on interface 1.
    /// The old interface-number gate made this undiscoverable.
    fn build_two_interface(pid: &str, tf_descriptor: &[u8]) -> FakeSysfs {
        let root = tempdir();
        let usb_dev = root.join("devices/usb1/1-1");
        std::fs::create_dir_all(&usb_dev).unwrap();
        std::fs::write(usb_dev.join("idVendor"), "046d\n").unwrap();
        std::fs::write(usb_dev.join("idProduct"), format!("{pid}\n")).unwrap();

        let if0 = usb_dev.join("1-1:1.0");
        let hid0 = if0.join("0003:046D:C26E.0007");
        std::fs::create_dir_all(&hid0).unwrap();
        std::fs::write(if0.join("bInterfaceNumber"), "00\n").unwrap();
        std::fs::write(hid0.join(FFB_OUTPUT_ATTR), "0\n").unwrap();

        let if1 = usb_dev.join("1-1:1.1");
        let hid1 = if1.join("0003:046D:C26E.0008");
        std::fs::create_dir_all(&hid1).unwrap();
        std::fs::write(if1.join("bInterfaceNumber"), "01\n").unwrap();
        std::fs::write(hid1.join("report_descriptor"), tf_descriptor).unwrap();

        let hidraw_root = root.join("class/hidraw");
        let hidraw7 = hidraw_root.join("hidraw7");
        std::fs::create_dir_all(&hidraw7).unwrap();
        symlink(&hid1, hidraw7.join("device")).unwrap();

        let hid_bus_root = root.join("bus/hid/devices");
        std::fs::create_dir_all(&hid_bus_root).unwrap();
        symlink(&hid0, hid_bus_root.join("0003:046D:C26E.0007")).unwrap();
        symlink(&hid1, hid_bus_root.join("0003:046D:C26E.0008")).unwrap();

        FakeSysfs { root, hidraw_root, hid_bus_root }
    }

    #[test]
    fn finds_the_transport_on_a_two_interface_wheel() {
        // The G923 Xbox layout from issue #27: no interface 2 exists at
        // all, so requiring one made TrueForce unreachable on that wheel.
        let fs = build_two_interface("c26e", &vendor_page_descriptor());
        let found = discover_at(&fs.hidraw_root, &fs.hid_bus_root)
            .expect("the vendor page identifies the transport wherever it sits");
        assert!(found.hidraw.ends_with("hidraw7"));
        assert!(found.ffb_output.is_some(), "the interface-0 sibling is still correlated");
    }

    #[test]
    fn an_interface_without_the_vendor_page_is_not_the_transport() {
        // Same two-interface wheel, but interface 1 announces the HID++
        // page (0xFF43) instead. Nothing should match: the page is what
        // discriminates, so a wrong page must not be rescued by position.
        let fs = build_two_interface("c26e", &[0x06, 0x43, 0xFF, 0x09, 0x01]);
        assert!(discover_at(&fs.hidraw_root, &fs.hid_bus_root).is_none());
    }

    fn vendor_page_descriptor() -> Vec<u8> {
        let mut d = VENDOR_PAGE_PREFIX.to_vec();
        d.extend_from_slice(&[0x09, 0x01, 0xA1, 0x01]); // arbitrary trailing bytes
        d
    }

    #[test]
    fn discovers_the_g923_tf_interface_and_its_ffb_mirror() {
        let fs = FakeSysfs::build("046d", "c266", &vendor_page_descriptor());
        let paths = discover_at(&fs.hidraw_root, &fs.hid_bus_root).expect("discovered");
        assert_eq!(paths.hidraw, PathBuf::from("/dev/hidraw3"));
        let ffb = paths.ffb_output.expect("ffb_output found");
        assert!(ffb.ends_with("0003:046D:C266.0003/ffb_output"));
        let _ = fs.root; // keep the tempdir alive until here
    }

    #[test]
    fn discovers_the_xbox_pid_too() {
        let fs = FakeSysfs::build("046d", "c26e", &vendor_page_descriptor());
        assert!(discover_at(&fs.hidraw_root, &fs.hid_bus_root).is_some());
    }

    #[test]
    fn ignores_a_non_g923_vendor_or_product() {
        let fs = FakeSysfs::build("046d", "c276", &vendor_page_descriptor());
        assert!(discover_at(&fs.hidraw_root, &fs.hid_bus_root).is_none(), "RS50 PID must not match");

        let fs = FakeSysfs::build("1234", "c266", &vendor_page_descriptor());
        assert!(discover_at(&fs.hidraw_root, &fs.hid_bus_root).is_none(), "foreign vendor must not match");
    }

    #[test]
    fn ignores_a_report_descriptor_without_the_vendor_page_prefix() {
        let fs = FakeSysfs::build("046d", "c266", &[0x05, 0x01, 0x09, 0x04]); // generic desktop page
        assert!(discover_at(&fs.hidraw_root, &fs.hid_bus_root).is_none());
    }

    #[test]
    fn missing_ffb_output_still_discovers_the_hidraw() {
        // Interface-0 present but without the attribute (older driver, or
        // a wheel plugged into a kernel predating the ffb_output attribute).
        let fs = FakeSysfs::build("046d", "c266", &vendor_page_descriptor());
        std::fs::remove_file(fs.root.join("devices/usb1/1-1/1-1:1.0/0003:046D:C266.0003").join(FFB_OUTPUT_ATTR))
            .unwrap();
        let paths = discover_at(&fs.hidraw_root, &fs.hid_bus_root).expect("hidraw still found");
        assert_eq!(paths.ffb_output, None);
    }

    #[test]
    fn the_direct_drive_engine_is_recognised_by_its_own_attributes() {
        let fs = FakeSysfs::build("046d", "c26e", &vendor_page_descriptor());
        let iface0 = fs.root.join("devices/usb1/1-1/1-1:1.0/0003:046D:C266.0003");
        std::fs::remove_file(iface0.join(FFB_OUTPUT_ATTR)).unwrap();

        let paths = discover_at(&fs.hidraw_root, &fs.hid_bus_root).expect("discovered");
        assert!(!paths.kernel_carries_force, "no direct-drive attributes, no claim");

        // What the g923_xbox_dd_engine module parameter leaves behind.
        std::fs::write(iface0.join(DD_ENGINE_ATTR), "100\n").unwrap();
        let paths = discover_at(&fs.hidraw_root, &fs.hid_bus_root).expect("discovered");
        assert!(paths.kernel_carries_force, "the driver's engine has this wheel");
        assert_eq!(paths.ffb_output, None, "and it publishes no mirror either way");
    }

    #[test]
    fn missing_hidraw_root_is_not_a_wheel() {
        assert!(discover_at(Path::new("/nonexistent-hidraw-root"), Path::new("/nonexistent-bus-root")).is_none());
    }

    // -- wire conversions -------------------------------------------------

    #[test]
    fn i16_to_wire_is_offset_binary() {
        assert_eq!(i16_to_wire(0), 0x8000);
        assert_eq!(i16_to_wire(i16::MIN), 0x0000);
        assert_eq!(i16_to_wire(i16::MAX), 0xFFFF);
        assert_eq!(i16_to_wire(-1), 0x7FFF);
        assert_eq!(i16_to_wire(1), 0x8001);
    }

    #[test]
    fn float_to_wire_clamps_and_scales() {
        assert_eq!(float_to_wire(0.0), 0x8000);
        assert_eq!(float_to_wire(1.0), i16_to_wire(32767));
        assert_eq!(float_to_wire(-1.0), i16_to_wire(-32767));
        assert_eq!(float_to_wire(5.0), float_to_wire(1.0), "over-range clamps");
        assert_eq!(float_to_wire(-5.0), float_to_wire(-1.0), "under-range clamps");
    }

    #[test]
    fn clamp_ffb_bounds_to_i16() {
        assert_eq!(clamp_ffb(0), 0);
        assert_eq!(clamp_ffb(32767), i16::MAX);
        assert_eq!(clamp_ffb(-32768), i16::MIN);
        assert_eq!(clamp_ffb(1_000_000), i16::MAX, "over-range clamps rather than wraps");
        assert_eq!(clamp_ffb(-1_000_000), i16::MIN);
    }

    #[test]
    fn sign_normal_passes_through_and_inverted_negates() {
        assert_eq!(Sign::Normal.apply(1000), 1000);
        assert_eq!(Sign::Inverted.apply(1000), -1000);
        assert_eq!(Sign::Inverted.apply(-1000), 1000);
        assert_eq!(Sign::Inverted.apply(i16::MIN), i16::MAX, "MIN negation saturates instead of overflowing");
    }

    #[test]
    fn mix_to_wire_combines_audio_and_force_with_saturation() {
        assert_eq!(mix_to_wire(0.0, 0, Sign::Normal), CENTER, "silence both ways stays centered");
        assert_eq!(mix_to_wire(0.0, 16384, Sign::Normal), i16_to_wire(16384), "pure force mirror");
        assert_eq!(mix_to_wire(0.5, 0, Sign::Normal), float_to_wire(0.5), "pure audio, no force");
        assert_eq!(
            mix_to_wire(0.0, 16384, Sign::Inverted),
            i16_to_wire(-16384),
            "inverted sign flips the mirrored force"
        );
        // Both near-max, same direction: must saturate rather than wrap.
        assert_eq!(mix_to_wire(1.0, 32767, Sign::Normal), i16_to_wire(i16::MAX));
        assert_eq!(mix_to_wire(-1.0, -32768, Sign::Normal), i16_to_wire(i16::MIN));
    }

    #[test]
    fn sign_resolve_prefers_env_then_config_then_default() {
        // The only test in this module that touches the environment, so
        // it cannot race any other test over LOGI_TF_SIM_G923_FFB_SIGN.
        std::env::remove_var("LOGI_TF_SIM_G923_FFB_SIGN");
        assert_eq!(Sign::resolve(false), Sign::Normal, "default is non-inverted");
        assert_eq!(Sign::resolve(true), Sign::Inverted, "config value used when env unset");

        std::env::set_var("LOGI_TF_SIM_G923_FFB_SIGN", "invert");
        assert_eq!(Sign::resolve(false), Sign::Inverted, "env overrides config");
        std::env::set_var("LOGI_TF_SIM_G923_FFB_SIGN", "0");
        assert_eq!(Sign::resolve(true), Sign::Normal, "any other env value forces Normal");
        std::env::remove_var("LOGI_TF_SIM_G923_FFB_SIGN");
    }

    // -- packet builder ---------------------------------------------------

    #[test]
    fn build_sample_packet_matches_the_exact_wire_layout() {
        let mut window = [CENTER; WINDOW];
        // A distinctive, easily-traced value per slot: 0x8000 + 100*i.
        for (i, slot) in window.iter_mut().enumerate() {
            *slot = CENTER + (i as u16) * 100;
        }
        let pkt = build_sample_packet(0x2a, &window);

        let mut expected = [0u8; PACKET_LEN];
        expected[0] = 0x01;
        expected[4] = 0x01;
        expected[5] = 0x2a;
        let cur = window[WINDOW - 1];
        expected[6] = (cur & 0xff) as u8;
        expected[7] = (cur >> 8) as u8;
        expected[8] = (cur & 0xff) as u8;
        expected[9] = (cur >> 8) as u8;
        expected[10] = NEW_PER_PACKET as u8;
        expected[11] = 0x0d;
        for (i, &v) in window.iter().enumerate() {
            let off = 12 + i * 4;
            expected[off] = (v & 0xff) as u8;
            expected[off + 1] = (v >> 8) as u8;
            expected[off + 2] = (v & 0xff) as u8;
            expected[off + 3] = (v >> 8) as u8;
        }
        assert_eq!(pkt, expected);

        // A fully spelled-out spot check on the header bytes, independent
        // of the generic per-slot loop above: cur = window[12] =
        // 0x8000 + 12*100 = 0x84B0, little-endian, duplicated.
        assert_eq!(pkt[0], 0x01);
        assert_eq!(pkt[4], 0x01);
        assert_eq!(pkt[5], 0x2a);
        assert_eq!(&pkt[6..10], &[0xb0, 0x84, 0xb0, 0x84]);
        assert_eq!(pkt[10], 0x04);
        assert_eq!(pkt[11], 0x0d);
    }

    #[test]
    fn build_sample_packet_cur_matches_the_newest_window_slot() {
        let window: [u16; WINDOW] = [
            0x8000, 0x8001, 0x8002, 0x8003, 0x8004, 0x8005, 0x8006, 0x8007, 0x8008, 0x8009, 0x800a, 0x800b, 0x800c,
        ];
        let pkt = build_sample_packet(1, &window);
        assert_eq!(&pkt[6..10], &[0x0c, 0x80, 0x0c, 0x80]);
        assert_eq!(&pkt[60..64], &[0x0c, 0x80, 0x0c, 0x80], "last window slot occupies the last 4 bytes too");
    }

    #[test]
    fn slide_window_shifts_and_appends() {
        let mut window = [CENTER; WINDOW];
        slide_window(&mut window, &[1, 2, 3, 4]);
        let mut expected = [CENTER; WINDOW];
        expected[WINDOW - 4..].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(window, expected);

        slide_window(&mut window, &[5, 6, 7, 8]);
        assert_eq!(&window[WINDOW - 4..], &[5, 6, 7, 8]);
        assert_eq!(&window[WINDOW - 8..WINDOW - 4], &[1, 2, 3, 4]);
    }

    #[test]
    fn stop_packet_is_the_embedded_type_0x04_template() {
        let pkt = stop_packet(7);
        assert_eq!(pkt[4], 0x04, "must be the stop template, not the start one");
        assert_eq!(pkt[5], 7, "sequence byte overwritten");
    }

    // -- init data ----------------------------------------------------

    #[test]
    fn embedded_init_data_has_the_documented_shape() {
        assert_eq!(TF_INIT_PACKET_COUNT, 68);
        assert_eq!(TF_INIT_PACKET_LEN, 64);
        assert_eq!(TF_INIT_PACKETS.len(), 68);
        assert_eq!(TF_INIT_PACKETS[0][0], 0x01, "every packet opens with the HID report id");
        assert_eq!(TF_INIT_PACKETS[TF_INIT_PACKET_COUNT - 2][4], 0x04, "second-to-last is the stop template");
        assert_eq!(TF_INIT_PACKETS[TF_INIT_PACKET_COUNT - 1][4], 0x03, "last is the start template");
    }

    // -- ffb mirror -----------------------------------------------------

    #[test]
    fn ffb_mirror_reads_and_reparses_without_seeking() {
        let dir = tempdir();
        let path = dir.join("ffb_output");
        std::fs::write(&path, "-1234\n").unwrap();
        let mirror = FfbMirror::open(Some(&path));
        assert_eq!(mirror.read_raw(), -1234);
        // A real attribute changes value between reads without the reader
        // re-opening or seeking; read_at(0) must see the update.
        std::fs::write(&path, "500\n").unwrap();
        assert_eq!(mirror.read_raw(), 500);
    }

    #[test]
    fn ffb_mirror_defaults_to_zero_when_absent_or_unparsable() {
        assert_eq!(FfbMirror::open(None).read_raw(), 0);

        let dir = tempdir();
        let path = dir.join("ffb_output");
        std::fs::write(&path, "not a number\n").unwrap();
        assert_eq!(FfbMirror::open(Some(&path)).read_raw(), 0);
    }

    // -- idle policy ------------------------------------------------------

    #[test]
    fn idle_policy_stays_streaming_under_the_timeout() {
        let mut policy = IdlePolicy::new();
        let t0 = Instant::now();
        let timeout = Duration::from_secs(5);
        assert_eq!(policy.tick(t0, true, timeout), IdleAction::Stream);
        assert_eq!(policy.tick(t0 + Duration::from_secs(4), true, timeout), IdleAction::Stream);
    }

    #[test]
    fn idle_policy_enters_idle_exactly_once_at_the_timeout() {
        let mut policy = IdlePolicy::new();
        let t0 = Instant::now();
        let timeout = Duration::from_secs(5);
        policy.tick(t0, true, timeout);
        assert_eq!(policy.tick(t0 + timeout, true, timeout), IdleAction::EnterIdle);
        assert_eq!(policy.tick(t0 + timeout + Duration::from_secs(1), true, timeout), IdleAction::StayIdle);
    }

    #[test]
    fn idle_policy_resumes_on_activity_after_idle() {
        let mut policy = IdlePolicy::new();
        let t0 = Instant::now();
        let timeout = Duration::from_secs(5);
        policy.tick(t0, true, timeout);
        policy.tick(t0 + timeout, true, timeout);
        assert_eq!(policy.tick(t0 + timeout + Duration::from_secs(1), false, timeout), IdleAction::Resume);
        // Back to normal streaming immediately after the resume tick.
        assert_eq!(policy.tick(t0 + timeout + Duration::from_secs(2), false, timeout), IdleAction::Stream);
    }

    #[test]
    fn idle_policy_activity_before_the_timeout_resets_the_clock() {
        let mut policy = IdlePolicy::new();
        let t0 = Instant::now();
        let timeout = Duration::from_secs(5);
        policy.tick(t0, true, timeout);
        assert_eq!(policy.tick(t0 + Duration::from_secs(3), false, timeout), IdleAction::Stream, "not idle yet, no resume needed");
        // Silence again: the 3s of prior silence must not carry over.
        assert_eq!(policy.tick(t0 + Duration::from_secs(3) + Duration::from_secs(4), true, timeout), IdleAction::Stream);
    }

    // -- end-to-end G923Stream (writer thread, against a temp file) ------
    //
    // These drive the real `G923Stream` public API (`open`/`push`/`stop`/
    // `Drop`) with its hidraw handle pointed at a plain temp file standing
    // in for the device node. The writer thread's pacing is swapped for
    // `ManualPacer`, a rendezvous-channel pacer the test drives one tick
    // at a time (each tick carrying whatever `Instant` the test chooses,
    // so idle-timeout crossings never require an actual multi-second
    // sleep): tests are event-driven, not sleep-based.

    /// Test-only [`Pacer`]: `tick_rx` supplies each tick's `Instant` on
    /// demand (nothing happens until the test sends one), and `ack_tx`
    /// hands the test a synchronization point once the writer thread has
    /// finished acting on it, so assertions never race the write.
    struct ManualPacer {
        tick_rx: mpsc::Receiver<Instant>,
        ack_tx: mpsc::SyncSender<()>,
    }

    impl Pacer for ManualPacer {
        fn wait(&mut self) -> Option<Instant> {
            self.tick_rx.recv().ok()
        }
        fn ack(&mut self) {
            let _ = self.ack_tx.send(());
        }
    }

    /// Open a [`G923Stream`] against `paths` paced by [`ManualPacer`]
    /// instead of the real [`SteadyPacer`]. Returns the stream, a sender
    /// for driving one tick at a time, and the matching ack receiver.
    fn open_test(paths: &G923Paths, sign: Sign) -> io::Result<(G923Stream, mpsc::SyncSender<Instant>, mpsc::Receiver<()>)> {
        let (tick_tx, tick_rx) = mpsc::sync_channel(0);
        let (ack_tx, ack_rx) = mpsc::sync_channel(0);
        let stream = G923Stream::open_with_pacer(paths, sign, ManualPacer { tick_rx, ack_tx })?;
        Ok((stream, tick_tx, ack_rx))
    }

    /// A `G923Paths` pointing at a fresh, empty temp file standing in for
    /// the hidraw node, with no `ffb_output` (mirror reads as a constant
    /// zero, keeping the merge math trivial for these tests).
    fn temp_hidraw_paths() -> (PathBuf, G923Paths) {
        let dir = tempdir();
        let hidraw = dir.join("hidraw");
        std::fs::write(&hidraw, []).unwrap();
        let paths = G923Paths {
            hidraw: hidraw.clone(),
            ffb_output: None,
            kernel_carries_force: false,
        };
        (hidraw, paths)
    }

    const INIT_LEN: usize = 2 * TF_INIT_PACKET_COUNT * PACKET_LEN;

    #[test]
    fn open_writes_the_full_two_pass_init_sequence() {
        let (hidraw_path, paths) = temp_hidraw_paths();
        let (stream, tick_tx, _ack_rx) = open_test(&paths, Sign::Normal).expect("open");

        // send_init() runs synchronously on the calling thread before
        // open() returns, so the full sequence is already on disk with no
        // ticks needed.
        let bytes = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(bytes.len(), INIT_LEN, "136 packets, 64 bytes each");

        let packet = |i: usize| &bytes[i * PACKET_LEN..(i + 1) * PACKET_LEN];
        for pass in 0..2 {
            let base = pass * TF_INIT_PACKET_COUNT;
            assert_eq!(packet(base)[5], 1, "pass {pass}: seq restarts at 1");
            assert_eq!(packet(base + TF_INIT_PACKET_COUNT - 2)[4], 0x04, "pass {pass}: second-to-last is the stop template");
            assert_eq!(packet(base + TF_INIT_PACKET_COUNT - 1)[4], 0x03, "pass {pass}: last is the start template");
        }

        drop(tick_tx);
        drop(stream);
    }

    #[test]
    fn streaming_ticks_emit_correctly_paced_and_sequenced_sample_packets() {
        let (hidraw_path, paths) = temp_hidraw_paths();
        let (mut stream, tick_tx, ack_rx) = open_test(&paths, Sign::Normal).expect("open");

        // Two ticks' worth of distinct, nonzero samples, delivered in one
        // push (mirroring the daemon's batched poll-loop calls).
        stream.push(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]).unwrap();

        let t0 = Instant::now();
        tick_tx.send(t0).unwrap();
        ack_rx.recv().unwrap();
        tick_tx.send(t0 + TICK_INTERVAL).unwrap();
        ack_rx.recv().unwrap();

        let bytes = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(bytes.len(), INIT_LEN + 2 * PACKET_LEN, "exactly two sample packets, back to back");
        let pkt1 = &bytes[INIT_LEN..INIT_LEN + PACKET_LEN];
        let pkt2 = &bytes[INIT_LEN + PACKET_LEN..INIT_LEN + 2 * PACKET_LEN];

        // No ffb_output, so the merge is a plain pass-through; window
        // starts centered (reset by send_init) and slides by 4 each tick.
        let seq0 = (TF_INIT_PACKET_COUNT as u16 + 1) as u8;
        let mut window1 = [CENTER; WINDOW];
        slide_window(&mut window1, &[0.1f32, 0.2, 0.3, 0.4].map(float_to_wire));
        assert_eq!(pkt1, &build_sample_packet(seq0, &window1)[..], "first tick's window and sequence byte");

        let mut window2 = window1;
        slide_window(&mut window2, &[0.5f32, 0.6, 0.7, 0.8].map(float_to_wire));
        assert_eq!(pkt2, &build_sample_packet(seq0.wrapping_add(1), &window2)[..], "second tick slides the window again");

        drop(tick_tx);
    }

    #[test]
    fn idle_after_timeout_sends_exactly_one_stop_and_resume_reinits() {
        let (hidraw_path, paths) = temp_hidraw_paths();
        let (mut stream, tick_tx, ack_rx) = open_test(&paths, Sign::Normal).expect("open");
        let t0 = Instant::now();

        // Genuine silence (all-zero samples, no ffb): starts the idle
        // clock, but one tick under the timeout still streams normally.
        stream.push(&[0.0; NEW_PER_PACKET]).unwrap();
        tick_tx.send(t0).unwrap();
        ack_rx.recv().unwrap();
        let after_first = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(after_first.len(), INIT_LEN + PACKET_LEN, "still streaming, not idle yet");

        // Jump straight to the timeout - no real waiting.
        tick_tx.send(t0 + IDLE_TIMEOUT).unwrap();
        ack_rx.recv().unwrap();
        let after_idle = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(after_idle.len(), INIT_LEN + 2 * PACKET_LEN, "idle transition wrote exactly one extra packet");
        let stop_pkt = &after_idle[after_idle.len() - PACKET_LEN..];
        assert_eq!(stop_pkt[4], 0x04, "the idle-entry packet is the stop template");

        // Further silent ticks while idle withhold packets entirely.
        tick_tx.send(t0 + IDLE_TIMEOUT + Duration::from_secs(1)).unwrap();
        ack_rx.recv().unwrap();
        assert_eq!(std::fs::read(&hidraw_path).unwrap().len(), after_idle.len(), "stayed idle: no new packet");

        // Activity resumes: the tick re-sends the full init sequence,
        // then emits one sample packet in the same tick.
        stream.push(&[0.5; NEW_PER_PACKET]).unwrap();
        tick_tx.send(t0 + IDLE_TIMEOUT + Duration::from_secs(2)).unwrap();
        ack_rx.recv().unwrap();
        let after_resume = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(
            after_resume.len(),
            after_idle.len() + INIT_LEN + PACKET_LEN,
            "resume re-sent the full init sequence plus one sample packet"
        );

        drop(tick_tx);
    }

    #[test]
    fn explicit_stop_then_drop_does_not_double_send_the_stop_packet() {
        let (hidraw_path, paths) = temp_hidraw_paths();
        let (mut stream, tick_tx, ack_rx) = open_test(&paths, Sign::Normal).expect("open");

        // `stop()` sends Cmd::Stop and then blocks joining the writer
        // thread, which is parked waiting for its next tick. Drive that
        // (and, in case one races ahead of the Cmd::Stop send and lands
        // as an ordinary streaming tick first, any further) tick from a
        // helper thread so this thread can call the real, public
        // `stop()` and actually block inside it, instead of poking the
        // private `cmd_tx`/`stopped` fields to fake the same effect. The
        // helper stops driving once the writer thread has exited and
        // dropped its end of the tick channel (`send` then fails).
        let ticker = thread::spawn(move || loop {
            if tick_tx.send(Instant::now()).is_err() {
                return;
            }
            if ack_rx.recv().is_err() {
                return;
            }
        });

        stream.stop();
        ticker.join().expect("ticker thread panicked");

        // A possible leading tick (see above) writes one ordinary
        // (non-stop) sample packet before the stop is observed, so
        // rather than assert a fixed total length, check the property
        // that actually matters: exactly one stop packet was ever
        // written, and it is the last thing in the file.
        let after_stop = std::fs::read(&hidraw_path).unwrap();
        let tail = &after_stop[INIT_LEN..];
        assert_eq!(tail.len() % PACKET_LEN, 0, "only whole packets after init");
        let stop_packets = tail.chunks(PACKET_LEN).filter(|p| p[4] == 0x04).count();
        assert_eq!(stop_packets, 1, "exactly one stop packet must ever be written");
        let last_packet = &after_stop[after_stop.len() - PACKET_LEN..];
        assert_eq!(last_packet[4], 0x04, "the last packet written is the stop template");

        // Now exercise the real public API further: a further explicit
        // `stop()` and the subsequent `Drop` must both be no-ops - the
        // double-send this test guards against.
        stream.stop();
        drop(stream);
        let after_drop = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(after_drop.len(), after_stop.len(), "stop()/Drop after the writer already stopped must not double-send");
    }

    // -- load-shedding ------------------------------------------------------

    /// A sink that accepts everything and keeps nothing: these tests are
    /// about the backlog, not about what reaches the wire.
    struct NullSink;
    impl HidrawSink for NullSink {
        fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
            Ok(())
        }
    }


    /// Normal operation must never shed. The daemon renders in one burst
    /// whatever wall-clock time an iteration covered, up to MAX_GEN_MS, and
    /// hands it over in a single push; a bound below that discards part of
    /// every such burst the instant it arrives, while the transport is
    /// perfectly healthy and with nothing said until teardown. That is what a
    /// flat 32 ms bound did against a 100 ms cap.
    #[test]
    fn a_full_producer_burst_survives_arrival() {
        let mut writer = Writer::new(NullSink, FfbMirror::open(None), Sign::Normal);
        let burst = crate::daemon::MAX_GEN_MS as usize * crate::synth::SAMPLES_PER_MS;

        writer.push_pending(vec![0.5; burst]);

        assert_eq!(writer.dropped_stale, 0, "a single full-sized burst must arrive intact");
        assert_eq!(writer.pending.len(), burst);
        assert!(
            MAX_PENDING_MS >= crate::daemon::MAX_GEN_MS as usize,
            "the latency bound ({MAX_PENDING_MS} ms) must not sit below the producer's burst cap",
        );
    }

    /// The writer consumes NEW_PER_PACKET samples per tick, which equals the
    /// producer's rate only if a tick is exactly TICK_INTERVAL. It never is,
    /// because a tick is the sleep plus the sysfs read plus the write. So the
    /// backlog must be bounded, or a small rate mismatch becomes unbounded
    /// latency: measured on hardware as throttle response that fell further
    /// behind the longer a session ran.
    #[test]
    fn a_producer_faster_than_the_writer_cannot_grow_unbounded_latency() {
        let mut writer = Writer::new(NullSink, FfbMirror::open(None), Sign::Normal);

        // Enough pushes of a full tick's worth to overrun the bound, with
        // nothing consumed: a producer comfortably outrunning the writer.
        // Counted off MAX_PENDING rather than a fixed number of pushes, so
        // raising the bound cannot quietly stop this test from overflowing
        // anything (a flat 100 stopped doing so the moment the bound went
        // from 32 ms to 128).
        for _ in 0..(MAX_PENDING / NEW_PER_PACKET + 10) {
            writer.push_pending(vec![0.5; NEW_PER_PACKET]);
        }
        assert!(
            writer.pending.len() <= MAX_PENDING,
            "backlog grew to {} samples ({} ms of latency)",
            writer.pending.len(),
            writer.pending.len()
        );
        assert!(writer.dropped_stale > 0, "overflow must be counted, not silent");
    }

    /// When the backlog overflows the OLDEST samples go, so what reaches the
    /// wheel is the freshest audio. Dropping the newest would keep latency
    /// bounded too, and would be exactly wrong.
    #[test]
    fn overflow_discards_the_oldest_samples_not_the_newest() {
        let mut writer = Writer::new(NullSink, FfbMirror::open(None), Sign::Normal);

        writer.push_pending(vec![-1.0; MAX_PENDING]); // stale
        writer.push_pending(vec![1.0; NEW_PER_PACKET]); // fresh

        let chunk = writer.take_chunk();
        assert!(
            chunk.iter().all(|&s| s == -1.0) || chunk.contains(&1.0),
            "chunk should be drawn from what survived"
        );
        assert_eq!(writer.pending.len() + chunk.len(), MAX_PENDING);
        // The freshest samples must still be in there somewhere.
        let survived_fresh =
            writer.pending.contains(&1.0) || chunk.contains(&1.0);
        assert!(survived_fresh, "the newest samples were discarded instead of the oldest");
    }

    #[test]
    fn push_does_not_block_when_the_channel_is_full_and_counts_the_drop() {
        let (_hidraw_path, paths) = temp_hidraw_paths();
        // The pacer never ticks in this test, so the writer thread stays
        // parked in `Pacer::wait` and never drains `cmd_rx`: fill the
        // channel to capacity first (these must all succeed, since it is
        // not full yet).
        let (mut stream, tick_tx, _ack_rx) = open_test(&paths, Sign::Normal).expect("open");
        for _ in 0..CMD_CHANNEL_CAPACITY {
            stream.push(&[0.1]).expect("push into a non-full channel must succeed");
        }

        // One more push, with the channel now full and nobody draining
        // it: if `push` ever regresses to the blocking `send`, this call
        // would hang forever, so drive it from a second thread and give
        // it a generous timeout instead of wedging the whole test suite
        // on a regression.
        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = stream.push(&[0.2]);
            let dropped = stream.dropped;
            let _ = done_tx.send((result.is_ok(), dropped));
            stream
        });
        let (ok, dropped) = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("push() blocked instead of load-shedding when the channel was full");
        assert!(ok, "a full channel must be load-shed silently, not surfaced as an error to the caller");
        assert_eq!(dropped, 1, "exactly one push was dropped");

        // Clean up: let the writer thread (and the still-running helper
        // thread, parked in `Drop`'s `stop()`) exit.
        drop(tick_tx);
        handle.join().expect("helper thread panicked");
    }

    // -- fatal write error still sends a stop --------------------------------

    /// Test-only [`HidrawSink`] that fails exactly its `fail_at`-th call
    /// (0-indexed) and forwards every other call through to a real
    /// backing file, so a test can assert exactly which packets did and
    /// did not make it to "the device" around one injected write
    /// failure, instead of a real hidraw file or an OS-level trick like
    /// a pipe with its read end closed (which cannot target one specific
    /// write deterministically).
    struct FlakyFile {
        file: File,
        call: usize,
        fail_at: usize,
    }

    impl HidrawSink for FlakyFile {
        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            let call = self.call;
            self.call += 1;
            if call == self.fail_at {
                return Err(io::Error::other("synthetic write failure"));
            }
            Write::write_all(&mut self.file, buf)
        }
    }

    #[test]
    fn a_fatal_write_error_still_sends_a_best_effort_stop_before_exiting() {
        let dir = tempdir();
        let hidraw_path = dir.join("hidraw");
        std::fs::write(&hidraw_path, []).unwrap();
        let backing = OpenOptions::new().read(true).write(true).open(&hidraw_path).unwrap();

        // Fail exactly the first sample-packet write (call index
        // TF_INIT_PACKET_COUNT * 2, right after the two init passes)
        // to simulate one transient hidraw hiccup - not the device
        // being gone outright, so the following best-effort stop write
        // (the next call) is expected to succeed and land in the file.
        let fail_at = 2 * TF_INIT_PACKET_COUNT;
        let sink = FlakyFile { file: backing, call: 0, fail_at };

        let (tick_tx, tick_rx) = mpsc::sync_channel(0);
        let (ack_tx, ack_rx) = mpsc::sync_channel(0);
        let mut stream =
            G923Stream::open_with_sink(sink, FfbMirror::open(None), Sign::Normal, ManualPacer { tick_rx, ack_tx })
                .expect("open");

        // Drive one tick: `Writer::tick`'s final sample-packet write is
        // the injected failure, making the writer thread treat it as
        // fatal, attempt the best-effort stop (succeeds, since only the
        // one call was made to fail), and exit. `ack` confirms that tick's
        // actions (including the best-effort stop write) already
        // happened, but not that the writer thread's stack has finished
        // unwinding, so call the real `stop()` next to deterministically
        // wait for the thread (and its drop of `cmd_rx`) to fully exit
        // before relying on the channel being disconnected: `stop()` is
        // documented as safe to call on an already-stopped writer, and
        // internally does exactly the join needed here.
        tick_tx.send(Instant::now()).unwrap();
        ack_rx.recv().unwrap();
        stream.stop();

        // The writer thread has fully exited, so `push()` now observes
        // the disconnected channel.
        let err = stream.push(&[0.5]).expect_err("writer thread should have exited after the fatal error");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);

        let bytes = std::fs::read(&hidraw_path).unwrap();
        assert_eq!(bytes.len(), INIT_LEN + PACKET_LEN, "init sequence plus exactly the best-effort stop packet");
        let last_packet = &bytes[bytes.len() - PACKET_LEN..];
        assert_eq!(last_packet[4], 0x04, "the last packet written before exit is the stop template");

        drop(tick_tx);
    }
}
