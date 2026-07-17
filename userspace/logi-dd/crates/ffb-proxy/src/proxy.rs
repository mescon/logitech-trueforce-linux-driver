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

        let vendor = read_hex_u16(&device_dir.join("id/vendor")).unwrap_or(descriptor::VENDOR);
        let product = read_hex_u16(&device_dir.join("id/product")).unwrap_or(descriptor::PRODUCT);

        return Ok(WheelPaths { evdev: format!("/dev/input/{event_name}"), vendor, product, name });
    }

    Err(Error::WheelNotFound)
}

/// Ties the virtual device, the real-wheel input source, and the real-wheel
/// FF sink together and drives the poll loop between them.
pub struct Proxy {
    device: uhid::Device,
    source: source::Source,
    sink: sink::Sink,
}

impl Proxy {
    /// Bring up the virtual uhid device and open the real wheel's evdev node
    /// both as an input source and as an FF sink.
    pub fn new(paths: WheelPaths) -> Result<Proxy> {
        let device = uhid::Device::create()?;
        let source = source::Source::open(&paths.evdev)?;
        let sink = sink::Sink::open(&paths.evdev)?;
        Ok(Proxy { device, source, sink })
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
}
