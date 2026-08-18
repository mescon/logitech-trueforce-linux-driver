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
    ("tools/xbox-modeswitch.sh", "logi-wheel-modeswitch"),
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

/// The same channels, but each as ALL the files that make it up, because
/// two of them are split: Debian lists most of its payload in `.install`
/// files rather than in `rules`, and the from-source route is `setup.sh`
/// plus the DKMS script it calls.
const CHANNELS: &[(&str, &[&str])] = &[
    (
        "Debian",
        &[
            "packaging/debian/rules",
            "packaging/debian/logitech-trueforce-dkms.install",
            "packaging/debian/logi-wheel.install",
            "packaging/debian/logi-wheel-gui.install",
        ],
    ),
    ("Arch (AUR)", &["packaging/aur/logitech-trueforce-dkms/PKGBUILD"]),
    ("openSUSE (OBS)", &["packaging/obs/logitech-trueforce-dkms.spec"]),
    ("Fedora (akmods)", &["packaging/akmods/logitech-trueforce-kmod.spec"]),
    ("Nix", &["flake.nix"]),
    ("from source", &["tools/setup.sh", "tools/dkms-update.sh"]),
];

/// Everything a working install needs, beyond the driver itself: what it
/// is, and a string that must appear in a channel's recipe for it to be
/// installed there.
///
/// Not a style check. Each of these is loaded, run or read at runtime by
/// something else in the project, so a channel missing one ships an
/// install where a specific feature cannot work, and the failure surfaces
/// on a user's machine rather than here. That has happened repeatedly:
/// `logi-rebind-wheel` was offered by the apps' diagnostics while existing
/// only in a git checkout (#60 is the same class), and before this test
/// grew, `tf-init.bin` was in exactly one of the six.
///
/// Logitech's own SDK DLLs are deliberately absent: they are the user's to
/// install from G HUB, and this project never redistributes them.
const PAYLOAD: &[(&str, &str)] = &[
    ("the terminal app", "logi-wheel"),
    ("the window", "logi-wheel-gui"),
    ("the simulated-TrueForce daemon", "logi-tf-sim"),
    ("the DirectInput FFB proxy", "logi-ffb"),
    ("the rev-light and texture RPM feed", "logi-rpm-bridge"),
    ("the Steam launch wrapper", "logi-launch"),
    ("the rebind helper the apps offer as a fix", "logi-rebind-wheel"),
    ("the Xbox-mode switch", "logi-wheel-modeswitch"),
    ("the shim installer the apps run", "logi-shim"),
    ("the shared-memory telemetry relay", "logi-tf-relay.exe"),
    ("the range-answering proxy", "tf-range-proxy.dll"),
    ("the dinput8 escape proxy", "dinput8-escape.dll"),
    ("the recorded TrueForce init burst", "tf-init.bin"),
    ("the truck sims' telemetry plugin", "logi_tf_scs"),
    ("the desktop menu entry", "logi-wheel-gui.desktop"),
    ("the sysfs permissions rule", "70-logitech-trueforce.rules"),
    ("the uhid rule logi-ffb needs", "71-logi-ffb-uhid.rules"),
    ("the G923 rebind rule", "72-logitech-g923-rebind.rules"),
    ("the Xbox mode-switch rule", "73-logitech-xbox-modeswitch.rules"),
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

/// Every channel installs every piece, or names which one it is missing.
///
/// The matrix is the point: a gap is invisible when each recipe is read on
/// its own, and only shows up as "this works on Arch but not on Fedora" in
/// an issue months later.
#[test]
fn every_channel_installs_the_whole_payload() {
    let root = repo();
    if !root.join(INSTALL_PATHS[0]).is_file() {
        return;
    }
    let mut missing = Vec::new();
    for (channel, files) in CHANNELS {
        let mut text = String::new();
        for f in *files {
            text.push_str(&std::fs::read_to_string(root.join(f)).unwrap_or_default());
        }
        for (what, needle) in PAYLOAD {
            // The modprobe config is a file on every channel but NixOS,
            // which cannot take one and writes the same two lines through
            // boot.extraModprobeConfig instead.
            if !text.contains(needle) {
                missing.push(format!("{channel}: {what} ({needle})"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these installs would be incomplete:\n  {}",
        missing.join("\n  ")
    );
}

/// The load-order hint and the narrow blacklist reach every channel, even
/// the one that cannot install a file for them.
#[test]
fn the_modprobe_settings_reach_every_channel() {
    let root = repo();
    if !root.join(INSTALL_PATHS[0]).is_file() {
        return;
    }
    for (channel, files) in CHANNELS {
        let mut text = String::new();
        for f in *files {
            text.push_str(&std::fs::read_to_string(root.join(f)).unwrap_or_default());
        }
        let by_file = text.contains("hid-logitech-dd.conf");
        // NixOS is declarative: the same softdep and blacklist go in
        // through boot.extraModprobeConfig rather than as a file.
        let inline = text.contains("softdep hid-logitech-dd") && text.contains("hid-logitech-new");
        assert!(
            by_file || inline,
            "{channel} sets neither the softdep ordering nor the new-lg4ff blacklist, \
             so this driver may lose the bind race there and never be noticed"
        );
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
