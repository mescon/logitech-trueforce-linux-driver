// SPDX-License-Identifier: GPL-2.0-only
//! Checks that `tools/setup.sh` installs the telemetry helpers where the app
//! looks for them.
//!
//! Two components now place the same two files: `setup.sh`, so a fresh
//! install needs nothing hunted down, and `logi_wheel_core::telemetry_helpers`,
//! so the Setup pages can do it per game. Neither can call the other, so both
//! carry the file names, the shared directory and the in-game destinations.
//!
//! That is the shape of bug this project keeps finding: one fact stated in
//! two places, drifting. If the shell writes the plugin somewhere the app
//! does not look, the app reports it missing on a machine where it is
//! installed, and the person is told to fix something that is not broken.
//! Copying is fine; copying without a guard is what fails.

use logi_wheel_core::telemetry_helpers as th;
use std::path::{Path, PathBuf};

fn setup_sh() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../tools/setup.sh");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[test]
fn the_shell_and_the_app_agree_on_the_helper_file_names() {
    let sh = setup_sh();
    assert!(
        sh.contains(&format!("RELAY_BIN=\"{}\"", th::RELAY_BIN)),
        "setup.sh must name the relay {:?}",
        th::RELAY_BIN
    );
    assert!(
        sh.contains(&format!("SCS_PLUGIN=\"{}\"", th::SCS_PLUGIN)),
        "setup.sh must name the plugin {:?}",
        th::SCS_PLUGIN
    );
}

#[test]
fn the_shell_and_the_app_agree_on_where_packages_stage_the_helpers() {
    let sh = setup_sh();
    assert!(
        sh.contains(th::SHARED_DIR),
        "setup.sh must look in {} for a packaged copy",
        th::SHARED_DIR
    );
}

/// The plugin's destination is inside the game, and the game loads it by
/// path. Disagreeing here means the shell installs it somewhere the game
/// never looks and the app reports it missing, both silently.
#[test]
fn the_shell_and_the_app_agree_on_the_plugin_directory() {
    let sh = setup_sh();
    assert!(
        sh.contains(&format!("SCS_PLUGIN_DIR=\"{}\"", th::SCS_PLUGIN_DIR)),
        "setup.sh must install the plugin into {:?}",
        th::SCS_PLUGIN_DIR
    );

    let target = th::scs_target(Path::new("/games/Euro Truck Simulator 2"));
    assert!(
        target.ends_with("bin/linux_x64/plugins/liblogi_tf_scs.so"),
        "unexpected plugin destination: {}",
        target.display()
    );
}

/// The relay goes to the prefix's drive root. `setup.sh` writes that path
/// literally, so this pins the two together.
#[test]
fn the_shell_and_the_app_agree_on_where_the_relay_goes_in_a_prefix() {
    let sh = setup_sh();
    assert!(
        sh.contains("$pfx/drive_c/$RELAY_BIN"),
        "setup.sh must place the relay at the prefix's drive root"
    );

    let target = th::relay_target(Path::new("/steam/compatdata/805550/pfx"));
    assert_eq!(target, Path::new("/steam/compatdata/805550/pfx/drive_c/logi-tf-relay.exe"));
}

/// Both truck sims must be covered by the shell loop, or one of them
/// silently never gets the plugin. This is the same failure the appid guard
/// exists for, on a different list.
#[test]
fn the_shell_covers_every_truck_sim_the_app_knows_about() {
    let sh = setup_sh();
    for (appid, name) in th::SCS_APPIDS {
        assert!(
            sh.contains(&appid.to_string()),
            "setup.sh never mentions appid {appid} ({name}), so it would skip it"
        );
    }
}
