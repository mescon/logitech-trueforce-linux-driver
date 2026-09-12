// SPDX-License-Identifier: GPL-2.0-only
//! The launcher's session-marker file name must be the one the daemon
//! looks for. The sanitiser is a `tr` class, and a dash in the wrong place
//! there is a range, not a character: `_-\n` is a reversed range, tr fails,
//! the id comes out empty and the marker lands as `native.`, which the
//! daemon never reads. That is what #91 was for two days, with both sides
//! rebuilt and matching. So the function is run under bash here, on the
//! ids the registry actually uses and on the hostile one the daemon's own
//! test uses, and the result must equal the daemon's `marker_path_in`.
use std::path::Path;
use std::process::Command;

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().unwrap()
}

/// What logi-tf-sim's native_session::marker_path_in produces for `id`.
fn daemon_rule(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect()
}

fn launcher_rule(id: &str) -> String {
    let script = repo().join("tools/logi-launch.sh");
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "source <(sed -n '/^native_marker_id()/,/^}}/p' '{}'); native_marker_id \"$1\"",
            script.display()
        ))
        .arg("_")
        .arg(id)
        .output()
        .expect("run bash");
    assert!(out.status.success(), "tr failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_launcher_names_the_marker_the_way_the_daemon_reads_it() {
    for id in ["ac-evo", "acc", "assetto", "iracing", "raceroom", "rf2", "lmu", "a/b c", "../../etc/passwd"] {
        assert_eq!(launcher_rule(id), daemon_rule(id), "id {id:?}");
        assert!(!launcher_rule(id).is_empty());
    }
}
