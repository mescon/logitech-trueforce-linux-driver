// SPDX-License-Identifier: GPL-2.0-only
//! Hold an evdev force-feedback session open for as long as a direct-drive
//! TrueForce stream is running.
//!
//! Without one the RS50 becomes unstable: streaming TrueForce samples to it
//! drives the wheel into its stops and oscillates there for seconds. Measured
//! on the steering axis, a 3 s 50 Hz sine at amplitude 0.3 produced roughly
//! 1500 degrees of total travel with seven direction reversals; holding a
//! single zero-level `FF_CONSTANT` effect open across the same stream brings
//! that down to about 150 degrees with none. See issue #57.
//!
//! Why this was not obvious: nothing we send is wrong. The instability is
//! caused by what we *fail* to hold open, so every comparison of the
//! transmitted bytes came back clean. Games never hit it because a game
//! always has an FFB session of its own open, which is also why real
//! TrueForce works on the same wheel in the same session.
//!
//! The effect commands zero force by design. It exists to keep the wheel's
//! force-feedback loop alive, not to move anything, so it must not alter how
//! the wheel feels. Autocenter stabilises the wheel too, but it adds real
//! centring torque and would change the feel of every game.
//!
//! Best-effort throughout: a wheel with no evdev node, no `FF_CONSTANT`
//! support, or no write permission streams exactly as it did before rather
//! than failing to start.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use logi_wheel_core::fftest::{self, FfEffect, FfUnion};

/// The direct-drive wheels this stabiliser is for, taken from
/// [`logi_wheel_core::device::DD_PIDS`] rather than restated.
///
/// It was restated once, as two ids, and left out the G PRO PlayStation
/// edition (`c268`). On that wheel the keepalive silently never opened,
/// which since the self-test learned to refuse without one meant the app's
/// "Test simulated TrueForce" button stopped working on a fully supported
/// wheel, and the daemon streamed to it without the stabiliser issue #57
/// exists for.
///
/// The G923 reaches the wheel by a different path and needs no keepalive
/// (measured: 17 degrees of travel against the RS50's 1500).
const LOGITECH_VENDOR: &str = "046d";

fn is_dd_product(hex: &str) -> bool {
    u16::from_str_radix(hex, 16)
        .map(|pid| logi_wheel_core::device::DD_PIDS.contains(&pid))
        .unwrap_or(false)
}

/// `_IOW('E', nr, T)` as `linux/ioctl.h` encodes it on x86_64.
const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30) | ((size as libc::c_ulong) << 16) | (('E' as libc::c_ulong) << 8) | nr as libc::c_ulong
}

const EVIOCSFF: libc::c_ulong = iow(0x80, std::mem::size_of::<FfEffect>());
const EVIOCRMFF: libc::c_ulong = iow(0x81, std::mem::size_of::<libc::c_int>());

/// The evdev node belonging to a direct-drive wheel, chosen by product id.
///
/// Not by device name, and not "the first wheel found". On a rig with both
/// a G923 and an RS50 attached, name-order discovery returns the G923,
/// which opens a perfectly healthy force-feedback session on a wheel
/// nobody is streaming to while the RS50 carries on thrashing. That is not
/// hypothetical: it is how the first version of this fix failed, and it
/// looked exactly like the fix simply not working.
fn find_dd_event_node(sysfs_input: &Path) -> Option<String> {
    let mut entries: Vec<_> = fs::read_dir(sysfs_input)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
        .collect();
    // Numerically, not lexicographically: sorting the names as strings
    // puts event10 before event3. `evtest::scan_wheel_input` already sorts
    // this way, and two scans disagreeing about "the first wheel" is
    // exactly the class of bug that made the first version of this fix
    // open a session on the wrong wheel.
    entries.sort_by_key(|e| {
        e.file_name()
            .to_string_lossy()
            .trim_start_matches("event")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });

    for entry in entries {
        let dir = entry.path().join("device/id");
        let read = |f: &str| {
            fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_lowercase())
        };
        if read("vendor").as_deref() != Some(LOGITECH_VENDOR) {
            continue;
        }
        // `continue`, not `?`: an entry whose product cannot be read (a node
        // still appearing during hotplug, or one this user cannot read) must
        // skip that entry rather than abandon the search and leave a
        // direct-drive wheel with no stabiliser.
        let Some(product) = read("product") else {
            continue;
        };
        if is_dd_product(&product) {
            return Some(format!("/dev/input/{}", entry.file_name().to_string_lossy()));
        }
    }
    None
}

/// A zero-level constant force, played until stopped.
fn zero_constant() -> FfEffect {
    let mut u = FfUnion([0u8; fftest::FF_UNION_SIZE]);
    // `struct ff_constant_effect { __s16 level; struct ff_envelope; }`.
    // Level 0 with an all-zero envelope: the whole union stays zeroed, so
    // this is spelled out rather than written, and named so the next
    // reader does not go looking for a missing field.
    u.0[0..2].copy_from_slice(&0i16.to_le_bytes());
    FfEffect {
        type_: fftest::FF_CONSTANT,
        id: -1,                 // kernel assigns
        direction: 0,
        trigger_button: 0,
        trigger_interval: 0,
        replay_length: 0,       // 0 = until explicitly stopped
        replay_delay: 0,
        u,
    }
}

/// An open FFB session, stopped and erased on drop.
#[derive(Debug)]
pub struct FfbKeepalive {
    file: File,
    id: i16,
}

impl FfbKeepalive {
    /// Open the direct-drive wheel's evdev node and hold a zero-level
    /// effect on it.
    ///
    /// Returns `None` whenever anything is unavailable: this is a
    /// stabiliser, not a requirement, and a stream that cannot have one is
    /// still better than no stream.
    pub fn open() -> Option<Self> {
        Self::open_at(&find_dd_event_node(Path::new("/sys/class/input"))?)
    }

    /// The path-taking half, so tests can point at a node that is not a
    /// wheel and check the failure is quiet.
    pub fn open_at(path: &str) -> Option<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
        let mut effect = zero_constant();
        // SAFETY: `file` is a valid open evdev fd, and `effect` is a
        // repr(C) mirror of the kernel struct (its layout is unit-tested
        // in logi_wheel_core::fftest) that outlives the call. The kernel
        // writes the assigned id back through the same pointer.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), EVIOCSFF, &mut effect as *mut FfEffect) };
        if rc < 0 {
            return None;
        }
        let mut this = Self { file, id: effect.id };
        this.play(1).ok()?;
        Some(this)
    }

    fn play(&mut self, value: i32) -> std::io::Result<()> {
        self.file.write_all(&fftest::encode_ff_event(self.id as u16, value))
    }
}

impl Drop for FfbKeepalive {
    fn drop(&mut self) {
        let _ = self.play(0);
        // SAFETY: same fd, still open; EVIOCRMFF takes the id by value.
        // Best effort: the fd closes immediately after, which drops any
        // effect the kernel still holds.
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), EVIOCRMFF, self.id as libc::c_ulong);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effect_is_a_constant_force_of_exactly_zero() {
        let e = zero_constant();
        assert_eq!(e.type_, fftest::FF_CONSTANT);
        assert_eq!(e.id, -1, "kernel assigns the id");
        assert_eq!(e.replay_length, 0, "plays until stopped, not for a fixed time");
        assert_eq!(
            i16::from_le_bytes([e.u.0[0], e.u.0[1]]),
            0,
            "must command no force: this exists to keep the FFB loop alive, \
             not to move the wheel",
        );
        assert!(e.u.0.iter().all(|&b| b == 0), "envelope stays zeroed too");
    }

    /// A directory nothing else will collide with. The fixed names used
    /// before were shared across users and concurrent runs, and each test
    /// began by deleting its own path, so two builds on one host raced.
    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ffbka-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn fake_input(root: &Path, event: &str, vendor: &str, product: &str) {
        let dir = root.join(event).join("device/id");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(dir.join("product"), format!("{product}\n")).unwrap();
    }

    #[test]
    fn the_dd_wheel_is_chosen_even_when_a_g923_sorts_first() {
        // The exact rig this fix first failed on: a G923 on a lower event
        // number than the RS50. Picking by name order opens the session on
        // the G923 and the RS50 keeps thrashing.
        let tmp = unique_tmp("two_wheels");
        let _ = fs::remove_dir_all(&tmp);
        fake_input(&tmp, "event3", "046d", "c266");   // G923
        fake_input(&tmp, "event4", "046d", "c276");   // RS50
        assert_eq!(find_dd_event_node(&tmp).as_deref(), Some("/dev/input/event4"));
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Every wheel core calls direct drive must be found, not a subset.
    ///
    /// The first version of this file restated the list as two ids and left
    /// out the G PRO PlayStation edition, so that wheel lost its stabiliser
    /// and, once the self-test learned to refuse without one, its test
    /// button too.
    #[test]
    fn every_direct_drive_wheel_is_recognised() {
        for pid in logi_wheel_core::device::DD_PIDS {
            let tmp = unique_tmp(&format!("pid{pid:04x}"));
            fake_input(&tmp, "event7", "046d", &format!("{pid:04x}"));
            assert_eq!(
                find_dd_event_node(&tmp).as_deref(),
                Some("/dev/input/event7"),
                "DD_PIDS lists {pid:04x} and the keepalive must find it",
            );
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    #[test]
    fn a_rig_with_no_direct_drive_wheel_selects_nothing() {
        let tmp = unique_tmp("g923_only");
        let _ = fs::remove_dir_all(&tmp);
        fake_input(&tmp, "event3", "046d", "c266");
        fake_input(&tmp, "event9", "045e", "c276");   // right pid, wrong vendor
        assert_eq!(find_dd_event_node(&tmp), None);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_missing_node_is_quiet_rather_than_fatal() {
        assert!(FfbKeepalive::open_at("/nonexistent/event99").is_none());
    }
}
