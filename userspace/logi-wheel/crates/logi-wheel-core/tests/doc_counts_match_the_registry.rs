// SPDX-License-Identifier: GPL-2.0-only
//! Numbers the documentation quotes must match the registry it quotes them
//! from.
//!
//! "It knows 28 titles by their Steam appid" appeared in three files and was
//! wrong in all of them: the registry had 29. Nobody miscounted, the number
//! was right when written and games were added afterwards. A count in prose
//! has no way to notice that, which is what this test is for.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// Files that quote a count, and are worth checking.
const DOCS: &[&str] = &["README.md", "docs/LAUNCH_OPTIONS.md"];

#[test]
fn quoted_appid_counts_match_the_registry() {
    let root = repo();
    // Skipped outside the repo (a vendored or packaged source tree).
    if !root.join(DOCS[0]).is_file() {
        return;
    }
    let want = logi_wheel_core::games::STEAM_APPIDS.len();

    for doc in DOCS {
        let text = std::fs::read_to_string(root.join(doc)).unwrap_or_default();
        for line in text.lines() {
            // "knows 29 titles by ... appid" in any of its phrasings.
            let Some(at) = line.find(" titles by") else { continue };
            let n: usize = line[..at]
                .rsplit(|c: char| !c.is_ascii_digit())
                .find(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            assert_eq!(
                n, want,
                "{doc} says {n} titles by appid; the registry has {want}.\n  line: {line}"
            );
        }
    }
}

/// The same for the whole-registry count, which `--launch-plan --list`
/// prints and the docs describe.
#[test]
fn quoted_registry_counts_match_the_registry() {
    let root = repo();
    if !root.join(DOCS[0]).is_file() {
        return;
    }
    let want = logi_wheel_core::games::sorted_by_name().len();

    for doc in DOCS {
        let text = std::fs::read_to_string(root.join(doc)).unwrap_or_default();
        for line in text.lines() {
            let Some(at) = line.find(" titles the registry knows") else { continue };
            let n: usize = line[..at]
                .rsplit(|c: char| !c.is_ascii_digit())
                .find(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            assert_eq!(
                n, want,
                "{doc} says the registry knows {n} titles; it knows {want}.\n  line: {line}"
            );
        }
    }
}

/// A numbered list has to have as many steps as it announces.
///
/// The getting-started section said "Three steps" and had four, because a
/// step was added and the sentence above it was not. It is the first thing a
/// new owner reads.
#[test]
fn the_getting_started_step_count_matches_the_steps() {
    let root = repo();
    let readme = root.join("README.md");
    if !readme.is_file() {
        return;
    }
    let text = std::fs::read_to_string(&readme).expect("read README.md");
    let Some(start) = text.find("## Getting started") else {
        panic!("README has no Getting started section; did it move?");
    };
    let section = &text[start..];
    let end = section[1..].find("\n## ").map(|i| i + 1).unwrap_or(section.len());
    let section = &section[..end];

    let announced = ["One step", "Two steps", "Three steps", "Four steps", "Five steps"]
        .iter()
        .position(|w| section.contains(w))
        .map(|i| i + 1)
        .expect("Getting started does not announce how many steps it has");

    // Steps are bold and numbered: "**1. Install it.**"
    let actual = (1..=9)
        .take_while(|n| section.contains(&format!("**{n}. ")))
        .count();

    assert_eq!(
        announced, actual,
        "Getting started announces {announced} steps and has {actual}"
    );
}
