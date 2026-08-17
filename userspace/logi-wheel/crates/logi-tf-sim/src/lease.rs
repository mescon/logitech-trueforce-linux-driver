// SPDX-License-Identifier: GPL-2.0-only
//! Who owns a wheel's TrueForce stream, as an advisory lease.
//!
//! The wheel's stream endpoint carries one packet per millisecond, so two
//! programs streaming to it at once do not share it, they take turns. On the
//! wire that reads as the motor being square-modulated at 500 Hz, which is
//! the buzz users reported and which was root-caused there. The kernel side
//! is already arbitrated: the driver yields to a userspace writer and
//! carries its own force inside that writer's packets. What was missing is
//! arbitration between two USERSPACE writers, because the kernel yields to
//! both of them equally.
//!
//! This is that piece: one lock file per wheel, taken before a stream is
//! opened and released when the holder drops it. `flock(2)` rather than a
//! pid file, because the lock lives on the open file description and the
//! kernel releases it when the process dies: a crashed or SIGKILLed holder
//! leaves nothing to reap, which is exactly the property a lease wants. The
//! holder writes its own name into the file, so a refused caller can name
//! WHO has the wheel instead of only reporting that something does.
//!
//! Keyed on the wheel, never on the process: the file name derives from the
//! wheel's HID device id, so two wheels are two leases and a second daemon
//! aimed at the other wheel is not blocked by the first.
//!
//! Deliberately advisory and deliberately optional. A program that knows
//! nothing about this file still streams (nothing here can stop it), and a
//! system with nowhere to put the file gets a lease that grants everything
//! rather than a wheel that refuses to work. Arbitrating between the
//! streamers this project ships is the whole claim.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Overrides the directory the lock files live in. For tests, and for a
/// setup that wants them somewhere specific; both the daemon and the sweep
/// read it, so a value that only reaches one of them would silently split
/// the arbitration in two.
pub const DIR_ENV: &str = "LOGI_WHEEL_RUNTIME_DIR";

/// The key used when the wheel could not be identified (no driver
/// attributes to read an id from, say). Everything then shares one lease,
/// which on a two-wheel rig serialises two streams that could have run at
/// once. That is the safe direction to be wrong in: the cost is one test
/// sweep waiting, and the alternative is two writers on one endpoint.
pub const UNKNOWN_WHEEL_KEY: &str = "unknown-wheel";

/// What a refused caller is told: the holder, spelled as it wrote itself
/// into the lock file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub holder: String,
}

/// Shown when the lock is held but the holder's name could not be read
/// (it crashed between locking and writing, or the file is unreadable).
const ANONYMOUS_HOLDER: &str = "another program";

/// A held lease. Dropping it releases the lock.
///
/// A lease with no file behind it is an *unsupported* lease: nowhere to put
/// the lock file, or a filesystem that cannot lock. It grants, because
/// refusing to stream over a missing runtime directory would break a
/// working setup to protect it from a conflict that may not exist.
#[derive(Debug)]
pub struct Lease {
    file: Option<File>,
    path: Option<PathBuf>,
}

impl Lease {
    /// A lease that grants without enforcing anything. See the type doc.
    fn unsupported() -> Lease {
        Lease { file: None, path: None }
    }

    /// Whether this lease is really held against other callers, as opposed
    /// to granted because there was nothing to lock.
    pub fn is_enforced(&self) -> bool {
        self.file.is_some()
    }

    /// The lock file backing it, for diagnostics.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Emptied rather than removed: unlinking races anyone already
        // holding the file open, and an empty file reads as
        // [`ANONYMOUS_HOLDER`] rather than as a name that has moved on.
        // The lock itself is released by the close that follows.
        if let Some(file) = &self.file {
            let _ = file.set_len(0);
        }
    }
}

/// Where the lock files live: [`DIR_ENV`] if set, else a subdirectory of
/// `XDG_RUNTIME_DIR`, else a per-uid directory under the temporary
/// directory.
///
/// The last case matters more than it looks: `XDG_RUNTIME_DIR` is set for a
/// login session and frequently is not for a systemd unit or an `ssh host
/// command`, which is exactly how a daemon gets started. Without the
/// fallback those runs would take an unsupported lease and arbitrate with
/// nobody.
pub fn dir() -> PathBuf {
    if let Ok(dir) = std::env::var(DIR_ENV) {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.trim().is_empty() {
            return Path::new(&dir).join("logi-wheel");
        }
    }
    // SAFETY: getuid cannot fail and takes no arguments.
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("logi-wheel-{uid}"))
}

/// The lock file name for `key`, with anything that is not a plain path
/// atom folded to `-` so a wheel id (`0003:046D:C276.0003`) is a legal
/// single-component file name.
fn file_name_for(key: &str) -> String {
    let mut name = String::with_capacity(key.len() + 16);
    name.push_str("tf-stream-");
    for c in key.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            name.push(c);
        } else {
            name.push('-');
        }
    }
    name.push_str(".lock");
    name
}

/// Take the streaming lease for the wheel identified by `key`.
///
/// `who` names the caller for the benefit of whoever is refused next; the
/// pid is appended here, so pass a program name ("logi-tf-sim") rather than
/// a sentence.
pub fn try_acquire(key: &str, who: &str) -> Result<Lease, Busy> {
    try_acquire_in(&dir(), key, who)
}

/// [`try_acquire`] against a caller-supplied directory, so the arbitration
/// can be tested without a runtime directory (and without two processes).
///
/// `flock` locks the open file description, not the process, so two opens
/// of one path conflict even inside a single process: the tests below are
/// the real thing, not an approximation of it.
pub fn try_acquire_in(dir: &Path, key: &str, who: &str) -> Result<Lease, Busy> {
    if std::fs::create_dir_all(dir).is_err() {
        return Ok(Lease::unsupported());
    }
    let path = dir.join(file_name_for(key));
    // 0600: the lease is between one person's programs. A file another
    // user pre-created is one we cannot open, which lands in the
    // unsupported branch below rather than handing them our arbitration.
    // truncate(false) is load-bearing, not a default worth leaving implicit:
    // opening happens before the lock is known to be ours, and truncating
    // here would wipe the current holder's name out from under it, leaving
    // the caller about to be refused with nobody to name.
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
    {
        Ok(file) => file,
        Err(_) => return Ok(Lease::unsupported()),
    };
    // SAFETY: a valid fd owned by `file`, which outlives the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            // EWOULDBLOCK is EAGAIN on Linux; this is the one case that
            // means "somebody else has the wheel".
            Some(libc::EWOULDBLOCK) => Err(Busy { holder: read_holder(&path) }),
            // Anything else (ENOLCK on a filesystem with no locking, a
            // kernel without flock) is our problem, not the caller's:
            // grant, and let the stream come up.
            _ => Ok(Lease::unsupported()),
        };
    }
    let mut file = file;
    // Best effort: the lock is already ours, and a name we failed to write
    // only costs the next caller a specific message.
    let _ = file.set_len(0);
    let _ = writeln!(file, "{who} (pid {})", std::process::id());
    let _ = file.flush();
    Ok(Lease { file: Some(file), path: Some(path) })
}

/// The name the current holder wrote, or [`ANONYMOUS_HOLDER`].
fn read_holder(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_string(&mut text);
    }
    match text.lines().next().map(str::trim).filter(|line| !line.is_empty()) {
        Some(line) => line.to_string(),
        None => ANONYMOUS_HOLDER.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("logi-tf-lease-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The point of the whole module: the second streamer is refused, and
    /// is told who has the wheel rather than only that someone does.
    #[test]
    fn a_second_streamer_is_refused_and_told_who_holds_it() {
        let dir = scratch("second");
        let first = try_acquire_in(&dir, "0003:046D:C276.0003", "logi-tf-sim").expect("first");
        assert!(first.is_enforced(), "a writable directory must give a real lock");

        let refused = try_acquire_in(&dir, "0003:046D:C276.0003", "logi-tf-sim --sweep")
            .expect_err("the second must be refused");
        assert!(refused.holder.contains("logi-tf-sim"), "names the holder: {}", refused.holder);
        assert!(
            refused.holder.contains(&std::process::id().to_string()),
            "names the holder's pid: {}",
            refused.holder
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Releasing hands the wheel over. This is the daemon's standby path:
    /// it drops the lease at the top of a menu and a test sweep may then
    /// run.
    #[test]
    fn releasing_lets_the_next_streamer_in() {
        let dir = scratch("release");
        let first = try_acquire_in(&dir, "wheel", "first").expect("first");
        assert!(try_acquire_in(&dir, "wheel", "second").is_err(), "held");
        drop(first);

        let second = try_acquire_in(&dir, "wheel", "second").expect("after release");
        assert!(second.is_enforced());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A holder that never wrote its name (crashed between the lock and the
    /// write) still refuses, with the generic wording rather than an empty
    /// one that would read as a bug.
    #[test]
    fn an_unnamed_holder_still_refuses() {
        let dir = scratch("anon");
        let held = try_acquire_in(&dir, "wheel", "first").expect("first");
        // Exactly what a crash between flock and write leaves behind.
        std::fs::write(held.path().expect("a real lock has a path"), "").expect("truncate");

        let refused = try_acquire_in(&dir, "wheel", "second").expect_err("still held");
        assert_eq!(refused.holder, ANONYMOUS_HOLDER);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two wheels are two leases: driving one must not block the other.
    #[test]
    fn different_wheels_do_not_collide() {
        let dir = scratch("two-wheels");
        let _g923 = try_acquire_in(&dir, "0003:046D:C266.0004", "g923").expect("g923");
        let dd = try_acquire_in(&dir, "0003:046D:C276.0003", "dd").expect("dd");
        assert!(dd.is_enforced());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nowhere to put the file: the lease grants, because a missing runtime
    /// directory must not be the reason a wheel stops working.
    #[test]
    fn a_lease_with_nowhere_to_live_still_grants() {
        // /proc rejects mkdir, so this is a directory that cannot exist.
        let dir = Path::new("/proc/logi-wheel-cannot-exist/leases");
        let lease = try_acquire_in(dir, "wheel", "logi-tf-sim").expect("granted anyway");
        assert!(!lease.is_enforced(), "granted, but honest that it enforces nothing");
        assert!(lease.path().is_none());
    }

    /// A wheel id is a legal file name after folding, and two ids that
    /// differ only in the folded characters stay different files.
    #[test]
    fn the_key_becomes_one_path_atom() {
        let name = file_name_for("0003:046D:C276.0003");
        assert!(!name.contains('/'), "no path separators: {name}");
        assert!(!name.contains(':'), "colons are folded: {name}");
        assert_eq!(name, "tf-stream-0003-046D-C276.0003.lock");
        assert_ne!(file_name_for("a/b"), file_name_for("a.b"));
    }
}
