// SPDX-License-Identifier: GPL-2.0-only
//! Every environment variable `logi-launch` reads must be documented.
//!
//! `logi-launch` is configured entirely through the environment, because
//! Steam's launch options are a single command line and that is the only
//! place a user can put anything. A variable the script honours but nobody
//! wrote down is a feature that exists only for whoever read the source.
//!
//! This is not hypothetical: `LOGI_LAUNCH_TF_SIM` shipped undocumented and
//! was found only when a later change happened to diff the two lists.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// Every `LOGI_LAUNCH_*` name appearing in a file, deduplicated.
///
/// A deliberately crude scan: it matches the names wherever they appear,
/// including in comments. Over-matching is the safe direction here, since
/// the consequence is documenting one variable too many rather than missing
/// one that a user will look for and not find.
fn launch_vars(text: &str) -> Vec<String> {
    const PREFIX: &str = "LOGI_LAUNCH_";
    let bytes = text.as_bytes();
    let mut found: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(hit) = text[at..].find(PREFIX) {
        let start = at + hit;
        let mut end = start + PREFIX.len();
        while end < bytes.len() && (bytes[end].is_ascii_uppercase() || bytes[end] == b'_') {
            end += 1;
        }
        // A bare `LOGI_LAUNCH_` with nothing after it is prose, not a name.
        let name = &text[start..end];
        if name.len() > PREFIX.len() && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
        at = end.max(start + 1);
    }
    found.sort();
    found
}

#[test]
fn every_launch_variable_is_documented() {
    let root = repo();
    let script = root.join("tools/logi-launch.sh");
    let doc = root.join("docs/LAUNCH_OPTIONS.md");
    // Skipped outside the repo (a vendored or packaged source tree), where
    // neither file is present to compare.
    if !script.is_file() || !doc.is_file() {
        return;
    }

    let script_text = std::fs::read_to_string(&script).expect("read logi-launch.sh");
    let doc_text = std::fs::read_to_string(&doc).expect("read LAUNCH_OPTIONS.md");

    let used = launch_vars(&script_text);
    assert!(!used.is_empty(), "found no LOGI_LAUNCH_* variables; did the script move?");

    let undocumented: Vec<&String> = used.iter().filter(|v| !doc_text.contains(*v)).collect();
    assert!(
        undocumented.is_empty(),
        "logi-launch.sh reads {undocumented:?}, which docs/LAUNCH_OPTIONS.md never mentions. \
         A user cannot discover a variable that is only in the source."
    );
}

/// The reverse: a documented variable the script stopped reading is worse
/// than an undocumented one, because it tells the user to set something
/// that now does nothing at all.
#[test]
fn no_documented_variable_is_dead() {
    let root = repo();
    let script = root.join("tools/logi-launch.sh");
    let doc = root.join("docs/LAUNCH_OPTIONS.md");
    if !script.is_file() || !doc.is_file() {
        return;
    }

    let script_text = std::fs::read_to_string(&script).expect("read logi-launch.sh");
    let doc_text = std::fs::read_to_string(&doc).expect("read LAUNCH_OPTIONS.md");

    let documented = launch_vars(&doc_text);
    let dead: Vec<&String> = documented.iter().filter(|v| !script_text.contains(*v)).collect();
    assert!(
        dead.is_empty(),
        "docs/LAUNCH_OPTIONS.md documents {dead:?}, which logi-launch.sh no longer reads"
    );
}

/// `LOGI_LAUNCH_EXE` replaces the relay and `LOGI_LAUNCH_HELPERS` adds to
/// it. Those are opposite behaviours reached through near-identical names,
/// so the difference has to be stated where someone choosing between them
/// is looking. Getting it backwards costs exactly what prompted the
/// feature: SimHub fed on the other machine, rev lights dark on this one.
#[test]
fn replace_versus_add_is_spelled_out() {
    let root = repo();
    let doc = root.join("docs/LAUNCH_OPTIONS.md");
    if !doc.is_file() {
        return;
    }
    let text = std::fs::read_to_string(&doc).expect("read LAUNCH_OPTIONS.md");

    for (var, word) in [("LOGI_LAUNCH_EXE", "instead of"), ("LOGI_LAUNCH_HELPERS", "as well as")] {
        let line = text
            .lines()
            .find(|l| l.contains(var) && l.contains(word))
            .map(str::to_string);
        assert!(
            line.is_some(),
            "no line in LAUNCH_OPTIONS.md describes {var} as running something \"{word}\" the relay"
        );
    }
}
