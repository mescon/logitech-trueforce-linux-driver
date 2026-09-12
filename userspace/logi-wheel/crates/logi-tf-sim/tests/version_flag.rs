// SPDX-License-Identifier: GPL-2.0-only
//! The daemon's `--version` carries the build identity, which is what the
//! installer and the doctor read back to know a rebuild took.
use std::process::Command;

#[test]
fn version_flag_prints_the_build_id() {
    let out = Command::new(env!("CARGO_BIN_EXE_logi-tf-sim")).arg("--version").output().expect("run logi-tf-sim");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.trim(), logi_wheel_core::version::banner("logi-tf-sim"));
    assert!(text.contains(&format!("({})", logi_wheel_core::version::BUILD_ID)));
}
