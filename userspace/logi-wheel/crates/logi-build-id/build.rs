// SPDX-License-Identifier: GPL-2.0-only
//! Stamps the build with the same identity the kernel module carries.
//!
//! The module prints `git describe --tags --always --dirty` at load time
//! (`hid-logitech-dd v0.40.3-19-g044d945`), so a user can read which build
//! is running. The apps only carried the Cargo version, which stays at the
//! last release between tags: an app rebuilt from a later checkout and one
//! left over from the release printed the same thing, and a stale daemon
//! next to a new module went unnoticed for a whole day of testing (#91).
//! `LOGI_BUILD_ID` is that describe, or the Cargo version when the source
//! is not a git checkout (a release tarball, a distribution package), which
//! the module's `.git_hash` fallback names the same way, so the two can be
//! compared on any install.
use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn main() {
    let id = git(&["describe", "--tags", "--always", "--dirty"])
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"));
    println!("cargo:rustc-env=LOGI_BUILD_ID={id}");

    // Rebuild when HEAD moves, a ref changes or the index does (dirty), so
    // the stamp cannot lag the checkout. A worktree keeps HEAD in its own
    // git dir and the refs in the common one; naming both is harmless.
    for dir in [git(&["rev-parse", "--git-dir"]), git(&["rev-parse", "--git-common-dir"])].into_iter().flatten() {
        let dir = PathBuf::from(dir);
        for f in ["HEAD", "index", "packed-refs"] {
            println!("cargo:rerun-if-changed={}", dir.join(f).display());
        }
        if let Ok(head) = std::fs::read_to_string(dir.join("HEAD")) {
            if let Some(r) = head.trim().strip_prefix("ref: ") {
                println!("cargo:rerun-if-changed={}", dir.join(r).display());
            }
        }
        println!("cargo:rerun-if-changed={}", dir.join("refs/tags").display());
    }
}
