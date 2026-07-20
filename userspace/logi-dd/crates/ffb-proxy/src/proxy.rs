//! Proxy orchestration: wheel discovery and the poll loop that ties the
//! virtual uhid device, the real-wheel input source, and the real-wheel FF
//! sink together.
//!
//! [`discover_wheel`] finds the real wheel's evdev node by scanning sysfs.
//! [`Proxy::new`] brings up the virtual device plus the source/sink pair
//! against it. [`Proxy::run`] is the poll loop: input flows up (real wheel ->
//! virtual device), force feedback flows down (virtual device PID output
//! reports -> real wheel evdev FF).

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use std::os::fd::BorrowedFd;

use crate::{descriptor, pidff, sink, source, uhid, Error, Result};

const SYSFS_INPUT: &str = "/sys/class/input";

/// How long each `poll()` call blocks before re-checking the stop flag.
const POLL_TIMEOUT_MS: u16 = 200;

/// Identity and evdev path of the discovered real wheel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelPaths {
    pub evdev: String,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
}

/// True if `name` looks like a Logitech direct-drive wheel and not one of
/// its sibling input nodes (the same physical device exposes separate
/// evdev nodes for consumer-control keys, and some setups have unrelated
/// keyboard/mouse nodes with overlapping substrings).
pub fn is_wheel(name: &str) -> bool {
    let upper = name.to_uppercase();
    let looks_like_wheel = upper.contains("RS50") || upper.contains("G PRO");
    let excluded =
        upper.contains("CONSUMER CONTROL") || upper.contains("KEYBOARD") || upper.contains("MOUSE");
    looks_like_wheel && !excluded
}

/// Reads a hex string (with or without a `0x` prefix) from `path` as a `u16`.
fn read_hex_u16(path: &Path) -> Option<u16> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(trimmed, 16).ok()
}

/// Numeric suffix of an `eventN` entry name, used only to scan `/dev/input`
/// nodes in a stable, predictable order.
fn event_index(file_name: &str) -> u32 {
    file_name.trim_start_matches("event").parse().unwrap_or(u32::MAX)
}

/// Scan `/sys/class/input/event*` for a Logitech direct-drive wheel that
/// advertises a force-feedback capability, and return its identity.
///
/// Returns the first match, in ascending `eventN` order. Vendor/product are
/// read from `device/id/{vendor,product}` and default to `0x046d`/`0xc276`
/// (the RS50) if those sysfs files are missing or unparsable.
pub fn discover_wheel() -> Result<WheelPaths> {
    // LOGI_FFB_DEVICE pins the source to one eventN node explicitly: for
    // multi-wheel rigs, and for tests that must not race the sort order
    // against a physically attached wheel. Accepts "eventN" or a full
    // /dev/input/eventN path.
    if let Ok(forced) = std::env::var("LOGI_FFB_DEVICE") {
        let event_name = forced.rsplit('/').next().unwrap_or(&forced).to_string();
        let device_dir = std::path::Path::new(SYSFS_INPUT).join(&event_name).join("device");
        if device_dir.exists() {
            return wheel_paths_for(&event_name, &device_dir);
        }
        return Err(Error::WheelNotFound);
    }
    let mut entries: Vec<_> = fs::read_dir(SYSFS_INPUT)
        .map_err(|e| Error::Io(format!("read {SYSFS_INPUT}"), e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
        .collect();
    entries.sort_by_key(|e| event_index(&e.file_name().to_string_lossy()));

    for entry in entries {
        let event_name = entry.file_name().to_string_lossy().into_owned();
        let device_dir = entry.path().join("device");

        let name = match fs::read_to_string(device_dir.join("name")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        if !is_wheel(&name) {
            continue;
        }

        let ff = fs::read_to_string(device_dir.join("capabilities/ff")).unwrap_or_default();
        let ff = ff.trim();
        if ff.is_empty() || ff == "0" {
            continue;
        }

        return wheel_paths_for(&event_name, &device_dir);
    }

    Err(Error::WheelNotFound)
}

/// Build the [`WheelPaths`] for one eventN sysfs entry (shared by normal
/// discovery and the `LOGI_FFB_DEVICE` override).
fn wheel_paths_for(event_name: &str, device_dir: &std::path::Path) -> Result<WheelPaths> {
    let name = fs::read_to_string(device_dir.join("name"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let vendor = read_hex_u16(&device_dir.join("id/vendor")).unwrap_or(descriptor::VENDOR);
    let product = read_hex_u16(&device_dir.join("id/product")).unwrap_or(descriptor::PRODUCT);
    Ok(WheelPaths { evdev: format!("/dev/input/{event_name}"), vendor, product, name })
}

/// Assigns the next PID effect block index and advances `next_block` past
/// it. Pure and side-effect-free besides the counter, so it is unit
/// testable without a live uhid device. Block indices are 1-based (the PID
/// protocol's Effect Block Index usages all declare a logical minimum of 1,
/// see `descriptor::PID_COLLECTION`'s report id 0x54/0x56 collections) and
/// increment by one per call: 1, 2, 3, ...
///
/// Wraps rather than panics past 255: exhausting a `u8` of effect blocks
/// without ever destroying one is not expected in practice (DirectInput
/// games reuse a handful of blocks), and a wrapped index simply reuses an
/// earlier block's slot rather than crashing the proxy.
fn assign_block(next_block: &mut u8) -> u8 {
    let block = *next_block;
    *next_block = next_block.wrapping_add(1);
    block
}

/// Ties the virtual device, the real-wheel input source, and the real-wheel
/// FF sink together and drives the poll loop between them.
pub struct Proxy {
    device: uhid::Device,
    source: source::Source,
    sink: sink::Sink,
    /// Next PID effect block index to assign on a Create New Effect
    /// (`0x54`) request. Starts at 1 (0 is not a valid Effect Block Index
    /// per the descriptor's logical range).
    next_block: u8,
    /// The most recently assigned block index, reported back on a PID Block
    /// Load (`0x56`) Get_Report. 0 until the first Create.
    last_created_block: u8,
}

impl Proxy {
    /// Bring up the virtual uhid device and open the real wheel's evdev node
    /// both as an input source and as an FF sink.
    pub fn new(paths: WheelPaths) -> Result<Proxy> {
        let device = uhid::Device::create()?;
        let source = source::Source::open(&paths.evdev)?;
        let sink = sink::Sink::open(&paths.evdev)?;
        Ok(Proxy { device, source, sink, next_block: 1, last_created_block: 0 })
    }

    /// Run the poll loop until `stop` is set (checked at least every
    /// `POLL_TIMEOUT_MS` milliseconds) or the real wheel goes away.
    ///
    /// On every iteration: a readable source fd is drained into an
    /// `InputReport` and, once a complete frame arrives, forwarded to the
    /// virtual device; a readable uhid fd is read as an `Event`, and any
    /// `Output` report is decoded and applied to the real wheel's FF sink.
    /// `sink.shutdown()` always runs before this returns, whether the loop
    /// ended because `stop` was set or because the wheel disappeared.
    pub fn run(&mut self, stop: &AtomicBool) -> Result<()> {
        let mut report = descriptor::InputReport::default();

        let outcome = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }

            // SAFETY: `raw_fd()` returns a plain fd copy, not a reference
            // tied to `self`; the fds themselves stay open and owned by
            // `self.source`/`self.device` for the whole loop body below, so
            // borrowing them here does not outlive the underlying open fd.
            let source_fd = unsafe { BorrowedFd::borrow_raw(self.source.raw_fd()) };
            let device_fd = unsafe { BorrowedFd::borrow_raw(self.device.raw_fd()) };

            let mut fds =
                [PollFd::new(source_fd, PollFlags::POLLIN), PollFd::new(device_fd, PollFlags::POLLIN)];

            match poll(&mut fds, PollTimeout::from(POLL_TIMEOUT_MS)) {
                Ok(_) => {}
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e) => break Err(Error::Io("poll".into(), std::io::Error::from(e))),
            }

            let source_revents = fds[0].revents().unwrap_or_else(PollFlags::empty);
            let device_revents = fds[1].revents().unwrap_or_else(PollFlags::empty);

            // The wheel being unplugged surfaces on the source fd as
            // POLLHUP/POLLERR, not as a read() error: a closed evdev node's
            // read() returns EOF, which `read_into` treats as "no complete
            // frame yet" (Ok(false)), not an error. Without this check the
            // loop would spin at the poll timeout forever on a dead fd.
            // Treat a hangup as the wheel being gone and exit cleanly.
            if source_revents.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                break Ok(());
            }

            if source_revents.contains(PollFlags::POLLIN) {
                match self.source.read_into(&mut report) {
                    Ok(true) => {
                        if let Err(e) = self.device.send_input(&report.to_bytes()) {
                            break Err(e);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => break Err(e),
                }
            }

            if device_revents.contains(PollFlags::POLLIN) {
                match self.device.read_event() {
                    Ok(uhid::Event::Output(bytes)) => {
                        if let Some(op) = pidff::decode(&bytes) {
                            if let Err(e) = self.sink.apply(op) {
                                eprintln!("logi-ffb: failed to apply FF effect: {e}");
                            }
                        }
                    }

                    // Create New Effect (0x54) is a Feature report: the host
                    // sends a one-byte Effect Type via Set_Report(Feature),
                    // and we assign the block index (the device's job, not
                    // the host's), record it via the same Create handling
                    // the Output path uses, and ack. Set_Report `data` carries
                    // the report id in byte 0 (hidraw passes the whole buffer
                    // for a numbered report), so the Effect Type is data[1].
                    // An unrecognized type byte or short body still gets an ack
                    // (err 0) so the host is never left blocked on this request;
                    // we simply skip creating a block for it.
                    Ok(uhid::Event::SetReport { rnum: 0x54, data, id, .. }) => {
                        if let Some(kind) = data.get(1).and_then(|&b| pidff::effect_kind_from_type_byte(b)) {
                            let block = assign_block(&mut self.next_block);
                            self.last_created_block = block;
                            if let Err(e) = self.sink.apply(pidff::EffectOp::Create { block, kind }) {
                                eprintln!("logi-ffb: failed to create FF effect: {e}");
                            }
                        }
                        if let Err(e) = self.device.send_set_report_reply(id, 0) {
                            break Err(e);
                        }
                    }

                    // Any other Feature Set_Report (e.g. Device Control,
                    // 0x50): best-effort, feed it straight to the Output
                    // decoder. Set_Report `data` already leads with the report
                    // id byte the decoder keys on, so it is passed as-is (not
                    // re-prefixed). Apply if it decodes to something, then ack
                    // regardless.
                    Ok(uhid::Event::SetReport { data, id, .. }) => {
                        if let Some(op) = pidff::decode(&data) {
                            if let Err(e) = self.sink.apply(op) {
                                eprintln!("logi-ffb: failed to apply FF feature report: {e}");
                            }
                        }
                        if let Err(e) = self.device.send_set_report_reply(id, 0) {
                            break Err(e);
                        }
                    }

                    // PID Block Load (0x56): report the block just assigned
                    // by the most recent 0x54, load success, and a nonzero
                    // RAM pool so the host does not treat us as full.
                    Ok(uhid::Event::GetReport { rnum: 0x56, id, .. }) => {
                        let reply = pidff::pid_block_load_reply(self.last_created_block);
                        if let Err(e) = self.device.send_get_report_reply(id, 0, &reply) {
                            break Err(e);
                        }
                    }

                    // PID Pool (0x57): capacity/capability reply, built from
                    // the descriptor's field layout (see
                    // pidff::pid_pool_reply's doc comment for how each byte
                    // was derived and what is still unconfirmed).
                    Ok(uhid::Event::GetReport { rnum: 0x57, id, .. }) => {
                        let reply = pidff::pid_pool_reply();
                        if let Err(e) = self.device.send_get_report_reply(id, 0, &reply) {
                            break Err(e);
                        }
                    }

                    // Any other Feature Get_Report: a minimal success reply
                    // carrying just the report id (byte 0 of a numbered report)
                    // so the host is never left blocking on a request we do not
                    // specifically implement, and never sees the reply body
                    // shifted into the report-id position.
                    Ok(uhid::Event::GetReport { rnum, id, .. }) => {
                        if let Err(e) = self.device.send_get_report_reply(id, 0, &[rnum]) {
                            break Err(e);
                        }
                    }

                    Ok(_) => {}
                    Err(e) => break Err(e),
                }
            }
        };

        self.sink.shutdown();
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_block_returns_sequential_indices_and_advances_counter() {
        let mut next = 1u8;
        assert_eq!(assign_block(&mut next), 1);
        assert_eq!(assign_block(&mut next), 2);
        assert_eq!(assign_block(&mut next), 3);
        assert_eq!(next, 4);
    }

    #[test]
    fn recognizes_dd_wheel_names() {
        assert!(is_wheel("Logitech RS50 Base for PlayStation/PC"));
        assert!(is_wheel("Logitech G PRO Racing Wheel"));
        assert!(!is_wheel("Logi Litra Glow Consumer Control"));
    }

    #[test]
    fn rejects_keyboard_and_mouse_names_even_with_a_marker_substring() {
        assert!(!is_wheel("RS50 Wireless Keyboard"));
        assert!(!is_wheel("G PRO Wireless Mouse"));
    }

    #[test]
    fn rejects_the_virtual_wheel_name() {
        // If the virtual device's name ever matches the wheel heuristic, a
        // restarted proxy could discover and bind its own stale virtual
        // device as its source.
        assert!(!is_wheel(descriptor::VIRTUAL_NAME));
    }
}
