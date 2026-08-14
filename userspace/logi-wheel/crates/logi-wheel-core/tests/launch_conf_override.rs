// SPDX-License-Identifier: GPL-2.0-only
//! A `games.conf` line must override only the keys it states.
//!
//! `tools/logi-launch.sh` merges a user's games.conf line into the plan the
//! registry computed. The first version of that merge replaced the whole
//! plan, so an old `3058630 hidraw=1` line, written before the kernel
//! texture merge existed, silently dropped `texture=merge` for every
//! release after it: the wheel lost the headline feature and nothing said
//! why. These tests run the real script against a stub `logi-wheel` and
//! read back the effective plan it logs.
//!
//! The scripted session is deliberately inert: the Steam prefix it points
//! at exists but holds no TrueForce files, so the script takes the
//! "files missing" path before it would export `PROTON_ENABLE_HIDRAW`,
//! write any `wheel_tf_merge` sysfs attribute, or start a bridge. Nothing
//! here may ever reach a real wheel from a test run.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// The plan the stub `logi-wheel` answers with: AC EVO on a direct-drive
/// wheel, scoped hidraw, texture merge on. The same lines
/// `LaunchPlan::lines()` would print.
const STUB_PLAN: &str = "wheel=direct-drive\n\
                         game=Assetto Corsa EVO\n\
                         hidraw=0x046D/0xC276\n\
                         texture=merge\n\
                         tfsim=0\n\
                         relay=none\n";

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run logi-launch.sh with `conf_line` as the games.conf entry for AC EVO's
/// appid and return the "plan:" line it logged, i.e. the effective plan
/// after the merge.
fn effective_plan(conf_line: &str) -> Option<String> {
    let script = repo().join("tools/logi-launch.sh");
    // Skipped outside the repo (a vendored or packaged source tree).
    if !script.is_file() {
        return None;
    }

    let tmp = std::env::temp_dir().join(format!(
        "logi-launch-conf-{}-{}",
        std::process::id(),
        conf_line.len()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let stub = tmp.join("bin");
    let cfg = tmp.join("config/logi-wheel");
    // A prefix directory that exists but holds no TrueForce files, which
    // keeps every hardware-touching branch of the script switched off.
    let compat = tmp.join("compat");
    fs::create_dir_all(&stub).unwrap();
    fs::create_dir_all(&cfg).unwrap();
    fs::create_dir_all(compat.join("pfx")).unwrap();

    let mut stub_wheel = String::from("#!/bin/sh\n");
    for line in STUB_PLAN.lines() {
        stub_wheel.push_str(&format!("echo \"{line}\"\n"));
    }
    write_exec(&stub.join("logi-wheel"), &stub_wheel);
    // Belt and braces: even if a future edit reaches these, they must be
    // the stubs, never a real daemon or bridge from the developer's PATH.
    for helper in ["logi-tf-sim", "logi-rpm-bridge", "logi-ffb"] {
        write_exec(&stub.join(helper), "#!/bin/sh\nexit 0\n");
    }

    fs::write(cfg.join("games.conf"), format!("3058630 {conf_line}\n")).unwrap();

    let log = tmp.join("launch.log");
    let path = format!(
        "{}:{}",
        stub.display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    );
    let status = Command::new("bash")
        .arg(&script)
        .arg("true")
        .env("HOME", &tmp)
        .env("XDG_CONFIG_HOME", tmp.join("config"))
        .env("SteamAppId", "3058630")
        .env("STEAM_COMPAT_DATA_PATH", &compat)
        .env("LOGI_LAUNCH_LOG", &log)
        .env("LOGI_LAUNCH_TF_SIM", "0")
        .env("PATH", path)
        .env_remove("LOGI_LAUNCH_EXE")
        .env_remove("LOGI_LAUNCH_HELPERS")
        .status()
        .expect("run logi-launch.sh");
    assert!(status.success(), "logi-launch.sh exited nonzero");

    let text = fs::read_to_string(&log).expect("read the launch log");
    let plan = text
        .lines()
        .find(|l| l.contains("plan:"))
        .unwrap_or_else(|| panic!("no plan line in the log:\n{text}"))
        .to_string();
    let _ = fs::remove_dir_all(&tmp);
    Some(plan)
}

#[test]
fn an_unstated_key_keeps_the_built_in_plans_value() {
    // The line that motivated the change: hidraw stated, texture not.
    let Some(plan) = effective_plan("hidraw=1") else { return };
    assert!(plan.contains("hidraw=1"), "the stated key must win: {plan}");
    assert!(
        plan.contains("texture=merge"),
        "an unstated key must inherit the built-in plan, not vanish: {plan}"
    );
    assert!(plan.contains("relay=none"), "{plan}");
}

#[test]
fn a_stated_key_beats_the_built_in_plan() {
    let Some(plan) = effective_plan("texture=none tfsim=1") else { return };
    assert!(
        plan.contains("texture=none"),
        "stating a key must still force it off: {plan}"
    );
    assert!(plan.contains("tfsim=1"), "{plan}");
    assert!(
        plan.contains("hidraw=0x046D/0xC276"),
        "the scoped hidraw value must survive untouched: {plan}"
    );
}
