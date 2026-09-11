// SPDX-License-Identifier: GPL-2.0-only
//! The per-session marker that says a game's own TrueForce is reaching
//! the wheel through Logitech's SDK, so this daemon must not synthesise
//! haptics for it and runs for the rev lights and the screen only.
//!
//! The launcher writes it while raw HID is granted to an SDK title and
//! removes it when the game exits; this daemon reads it at every session
//! open. A file rather than an environment variable because the daemon is
//! usually already running when a game starts, and a file is visible to
//! it without a restart. Lives in the same runtime directory as the
//! stream lease (`crate::lease::dir`), so the two share one fallback
//! story when `XDG_RUNTIME_DIR` is unset.

use std::path::{Path, PathBuf};

/// `dir/native.<id>`, with `id` reduced to plain path characters so a
/// live id can never escape the directory.
pub fn marker_path_in(dir: &Path, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    dir.join(format!("native.{safe}"))
}

/// Whether the launcher has marked `id`'s current session as one whose
/// own TrueForce reaches the wheel.
pub fn is_active(id: &str) -> bool {
    marker_path_in(&crate::lease::dir(), id).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_a_file_named_after_the_live_id() {
        let dir = std::env::temp_dir().join(format!("logi-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = marker_path_in(&dir, "ac-evo");
        assert_eq!(p, dir.join("native.ac-evo"));
        assert!(!p.exists());
        std::fs::write(&p, b"").unwrap();
        assert!(p.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hostile_id_cannot_leave_the_directory() {
        let dir = Path::new("/run/x");
        assert_eq!(marker_path_in(dir, "../../etc/passwd"), dir.join("native...-..-etc-passwd"));
        assert_eq!(marker_path_in(dir, "a/b c"), dir.join("native.a-b-c"));
    }
}
