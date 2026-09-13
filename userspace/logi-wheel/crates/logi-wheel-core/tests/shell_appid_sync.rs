// SPDX-License-Identifier: GPL-2.0-only
//! Checks that `tools/setup.sh`'s Steam appid lists agree with the registry.
//!
//! The shell tooling cannot import the registry, so it keeps its own copy of
//! which appid belongs to which group. That copy has drifted twice, both
//! times harmfully: once as a single list that told DirectInput sims to set
//! `PROTON_ENABLE_HIDRAW=1`, which is the setting that stops force feedback
//! reaching those games, and once, after the split, as a DirectInput list
//! carrying two of the four titles so the check written to catch the first
//! bug stayed silent for half its audience.
//!
//! Copying is fine; copying without a guard is what fails. This is the guard.

use logi_wheel_core::games;
use std::path::PathBuf;

fn setup_sh() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../tools/setup.sh");
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The appids assigned to `var` in setup.sh, e.g. `SDK_SIM_APPIDS="805550 …"`.
fn shell_appids(script: &str, var: &str) -> Vec<u32> {
    let prefix = format!("{var}=\"");
    let line = script
        .lines()
        .find(|l| l.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("{var} not found in setup.sh"));
    let body = line
        .split_once('"')
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(v, _)| v)
        .unwrap_or_else(|| panic!("{var} is not a quoted string"));
    let mut ids: Vec<u32> = body
        .split_whitespace()
        .map(|t| t.parse().unwrap_or_else(|e| panic!("{var} token {t:?}: {e}")))
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn setup_sh_appid_groups_match_the_registry() {
    let script = setup_sh();
    let (sdk, dinput) = games::launch_option_appid_groups();

    assert_eq!(
        shell_appids(&script, "SDK_SIM_APPIDS"),
        sdk,
        "SDK_SIM_APPIDS in tools/setup.sh disagrees with the registry's \
         Ffb::TrueForceShim titles. Update the shell list, or the registry \
         entry, so they say the same thing."
    );
    assert_eq!(
        shell_appids(&script, "DINPUT_SIM_APPIDS"),
        dinput,
        "DINPUT_SIM_APPIDS in tools/setup.sh disagrees with the registry's \
         Ffb::DirectInput titles. Getting this wrong means an owner of the \
         missing title is never warned that PROTON_ENABLE_HIDRAW=1 is what \
         stopped their force feedback."
    );
}

/// Every recorded appid must name a real registry entry, or the mapping is
/// describing a game the project does not otherwise know about.
#[test]
fn every_recorded_appid_names_a_registry_entry() {
    for (name, id) in games::STEAM_APPIDS {
        assert!(
            games::GAMES.iter().any(|g| g.name == *name),
            "STEAM_APPIDS has {name:?} ({id}) with no matching registry entry"
        );
    }
}

/// Appids must be unique: two titles sharing one would silently collapse the
/// groups and hide a game from its own check.
#[test]
fn recorded_appids_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for (name, id) in games::STEAM_APPIDS {
        assert!(seen.insert(id), "appid {id} is recorded twice (at {name:?})");
    }
}

/// A title that does not run on Linux must not reach the shell lists: doctor
/// only ever looks at installed Proton prefixes, so an entry there would be
/// dead weight that reads like coverage.
#[test]
fn unsupported_titles_are_excluded_from_the_groups() {
    let (sdk, dinput) = games::launch_option_appid_groups();
    for g in games::GAMES.iter().filter(|g| g.linux == games::Linux::Unsupported) {
        if let Some(id) = games::appid_for(g.name) {
            assert!(!sdk.contains(&id) && !dinput.contains(&id), "{}", g.name);
        }
    }
}
