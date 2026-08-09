// SPDX-License-Identifier: GPL-2.0-only
//! Every helper we tell people to run by bare name must be on their PATH.
//!
//! These are shell scripts in `tools/` that the packaging installs under a
//! `logi-` name. The documentation gives them as bare commands, so a user
//! types `logi-launch %command%` into Steam or `sudo logi-rebind-wheel` into
//! a terminal and expects it to resolve. If a packaging path stops shipping
//! one, that instruction silently becomes "command not found" for everyone
//! on that distribution, and the failure lands on the user's machine rather
//! than in CI.
//!
//! This has already happened once: `logi-rebind-wheel` was named in the
//! apps' own diagnostics while existing only in a git checkout, so the fix
//! offered to the people who most needed it could never run.

use std::path::{Path, PathBuf};

/// `tools/` script -> the name it is installed as.
const HELPERS: &[(&str, &str)] = &[
    ("tools/g923-xbox-modeswitch.sh", "logi-g923-modeswitch"),
    ("tools/rebind-wheel.sh", "logi-rebind-wheel"),
    ("tools/logi-launch.sh", "logi-launch"),
];

/// Every path that installs onto a user's system: the four distribution
/// recipes plus the from-source route.
const INSTALL_PATHS: &[&str] = &[
    "packaging/debian/rules",
    "packaging/akmods/logitech-trueforce-kmod.spec",
    "packaging/obs/logitech-trueforce-dkms.spec",
    "packaging/aur/logitech-trueforce-dkms/PKGBUILD",
    "tools/dkms-update.sh",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

#[test]
fn every_helper_is_installed_by_every_path() {
    let root = repo();
    // Skipped when built outside the repo (a vendored or packaged source
    // tree), where the packaging is not present to check.
    if !root.join(INSTALL_PATHS[0]).is_file() {
        return;
    }
    for path in INSTALL_PATHS {
        let text = std::fs::read_to_string(root.join(path)).unwrap_or_default();
        for (script, installed_as) in HELPERS {
            assert!(
                text.contains(installed_as),
                "{path} does not install {installed_as} (from {script}), \
                 so the documented `{installed_as}` command will not exist there"
            );
        }
    }
}

#[test]
fn every_helper_script_exists_and_is_executable() {
    let root = repo();
    if !root.join(INSTALL_PATHS[0]).is_file() {
        return;
    }
    for (script, installed_as) in HELPERS {
        let p = root.join(script);
        assert!(p.is_file(), "{script} is missing, but packaging installs it as {installed_as}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).expect("stat").permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "{script} is not executable; it is installed with mode 0755 and run by name"
            );
        }
    }
}

/// `setup.sh` builds the apps for a from-source install with `cargo build
/// -p <package>`. Those are PACKAGE names, not binary names, and the two
/// differ for the terminal app: package `logi-wheel-tui` produces binary
/// `logi-wheel`. Getting it wrong fails at install time on a user's
/// machine, having passed every test here, which is exactly what happened
/// the first time this was written.
#[test]
fn setup_builds_packages_that_exist() {
    let root = repo();
    let setup = root.join("tools/setup.sh");
    if !setup.is_file() {
        return;
    }
    let text = std::fs::read_to_string(&setup).expect("read setup.sh");

    let mut known = Vec::new();
    let crates = root.join("userspace/logi-wheel/crates");
    for entry in std::fs::read_dir(&crates).expect("read crates dir").flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(m) = std::fs::read_to_string(&manifest) else { continue };
        if let Some(name) = m
            .lines()
            .find_map(|l| l.strip_prefix("name = ").map(|v| v.trim_matches('"').to_string()))
        {
            known.push(name);
        }
    }
    assert!(!known.is_empty(), "found no crates to check against");

    // Only real build lines: a naive scan for "-p " also finds `mkdir -p`
    // and the prose in this file's own comments.
    let mut checked = 0;
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') || !line.contains("cargo build") {
            continue;
        }
        for name in line
            .split_whitespace()
            .skip_while(|w| *w != "-p")
            .collect::<Vec<_>>()
            .chunks(2)
            .filter(|c| c.len() == 2 && c[0] == "-p")
            .map(|c| c[1].trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_').to_string())
        {
            if name.is_empty() {
                continue;
            }
        assert!(
            known.contains(&name),
            "tools/setup.sh builds `-p {name}`, which is not a package in the workspace.              Known packages: {known:?}"
        );
            checked += 1;
        }
    }
    assert!(checked > 0, "setup.sh no longer builds any package; is the apps step still there?");
}
