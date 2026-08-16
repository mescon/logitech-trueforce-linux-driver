//! The launch-behaviour settings the front-ends persist for `logi-launch`:
//! `$XDG_CONFIG_HOME/logi-wheel/launch.conf` (falling back to
//! `~/.config/logi-wheel/launch.conf`), hand-rolled key=value in the same
//! discipline as tf-sim.conf.
//!
//! Why its own file rather than one of the two neighbours:
//! - `games.conf` is the user's own hand-written per-appid override file.
//!   `logi-launch` merges its lines into the plan itself and nothing in the
//!   apps ever writes it, and an app that started rewriting a hand-edited
//!   file would be a new way to lose someone's lines.
//! - `tf-sim.conf` belongs to the simulated-TrueForce daemon, and the
//!   rev-light mode configures `logi-rpm-bridge` during NATIVE TrueForce
//!   sessions, where the daemon is deliberately not running.
//!
//! So the app-owned launch behaviour gets the tf-sim.conf idiom in a file
//! of its own: trivial key=value, comments and blank lines allowed,
//! one-key writes that preserve every line the writer does not know.
//!
//! Keys:
//! - `revleds` (bar/shift): how `logi-rpm-bridge` maps the rev strip while
//!   the kernel texture merge drives it (see [`crate::games::RevLeds`]).
//!
//! `logi-wheel --launch-plan` reads this when building a plan and emits the
//! mode as the plan's `revleds=` key, which `logi-launch` turns into the
//! bridge's `LOGI_REV_MODE` environment.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::games::RevLeds;

/// First line written into a fresh file.
pub const FILE_HEADER: &str = "# logi-wheel launch configuration";
/// File name under the logi-wheel config directory.
pub const FILE_NAME: &str = "launch.conf";

/// `$XDG_CONFIG_HOME/logi-wheel/launch.conf`, falling back to
/// `~/.config/logi-wheel/launch.conf` when the variable is unset or empty.
/// No pre-rename migration here, unlike tf-sim.conf: this file postdates
/// the rename, so there is no `logi-dd` copy to inherit.
pub fn default_path() -> PathBuf {
    crate::profiles::config_root().join("logi-wheel").join(FILE_NAME)
}

/// The persisted rev-light mode from the file at `path`. A missing or
/// unreadable file, an absent key, or an unparsable value all yield the
/// bridge's own default ([`RevLeds::Bar`]).
pub fn rev_leds_from(path: &Path) -> RevLeds {
    let Ok(text) = fs::read_to_string(path) else { return RevLeds::default() };
    let mut mode = RevLeds::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else { continue };
        if key.trim() == "revleds" {
            if let Some(v) = RevLeds::parse(raw) {
                mode = v;
            }
        }
    }
    mode
}

/// [`rev_leds_from`] over [`default_path`].
pub fn rev_leds() -> RevLeds {
    rev_leds_from(&default_path())
}

/// Persist the rev-light mode into the file at `path`.
pub fn set_rev_leds_in(path: &Path, mode: RevLeds) -> Result<(), Error> {
    write_key_in(path, "revleds", mode.as_str())
}

/// [`set_rev_leds_in`] over [`default_path`].
pub fn set_rev_leds(mode: RevLeds) -> Result<(), Error> {
    set_rev_leds_in(&default_path(), mode)
}

/// Rewrite exactly one `key=value` line in the file at `path`, preserving
/// every other line (unknown keys, comments, blank lines) verbatim; a
/// missing file is created with this file's header. Mirrored, not shared,
/// from [`crate::tfsim::write_key_in`]: that one stamps tf-sim's header
/// into a fresh file, and a launch.conf born with the wrong banner would
/// misname its own format authority.
fn write_key_in(path: &Path, key: &str, value: &str) -> Result<(), Error> {
    let text = fs::read_to_string(path).unwrap_or_else(|_| format!("{FILE_HEADER}\n"));
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 2);
    let mut replaced = false;
    for line in text.lines() {
        let is_key = line
            .split_once('=')
            .is_some_and(|(k, _)| !line.trim_start().starts_with('#') && k.trim() == key);
        if is_key {
            if !replaced {
                out.push_str(&format!("{key}={value}\n"));
                replaced = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&format!("{key}={value}\n"));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::Io(format!("create {}: {e}", dir.display())))?;
    }
    fs::write(path, out).map_err(|e| Error::Io(format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (same pattern as the
    /// `tfsim` tests).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-wheel-launch-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TempTree(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_is_the_bridge_default() {
        assert_eq!(rev_leds_from(Path::new("/nonexistent-launch.conf")), RevLeds::Bar);
    }

    #[test]
    fn rev_leds_round_trips_and_creates_the_file_with_the_header() {
        let tree = TempTree::new();
        let path = tree.path().join("nested").join(FILE_NAME);
        set_rev_leds_in(&path, RevLeds::Shift).unwrap();
        assert_eq!(rev_leds_from(&path), RevLeds::Shift);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(FILE_HEADER), "its own header, not tf-sim's: {text}");
        assert!(text.contains("revleds=shift\n"));

        set_rev_leds_in(&path, RevLeds::Bar).unwrap();
        assert_eq!(rev_leds_from(&path), RevLeds::Bar);
        let text = fs::read_to_string(&path).unwrap();
        // The edit replaced in place, it did not append a duplicate.
        assert_eq!(text.matches("revleds=").count(), 1, "{text}");
    }

    #[test]
    fn hand_added_lines_survive_a_write_and_garbage_keeps_the_default() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(&path, "# my notes\nfuture_key=7\nrevleds=dashboard\n").unwrap();
        assert_eq!(rev_leds_from(&path), RevLeds::Bar, "an unparsable value keeps the default");

        set_rev_leds_in(&path, RevLeds::Shift).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my notes\n"), "comment preserved: {text}");
        assert!(text.contains("future_key=7\n"), "unknown key preserved: {text}");
        assert!(text.contains("revleds=shift\n"), "{text}");
        assert_eq!(rev_leds_from(&path), RevLeds::Shift);
    }
}
