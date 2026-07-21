// SPDX-License-Identifier: GPL-2.0-only
//! Cross-compatibility pin between this crate's config store (the FORMAT
//! AUTHORITY for tf-sim.conf) and the front-ends' format-compatible
//! reader/writer in `logi_dd_core::tfsim`. The core module exists because
//! the GUI front-end (GPL-3.0-or-later) cannot link this GPL-2.0-only
//! crate; these tests are what keeps the two implementations honest.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tf_sim::config::{Config, GameConfig};

/// A unique fixture directory under the system temp dir, removed on drop.
struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-sim-frontend-compat-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempTree(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A config exercising every key this crate's writer emits.
fn full_config() -> Config {
    let mut games = BTreeMap::new();
    games.insert("dirt-rally-2".to_string(), GameConfig { enabled: true, intensity: 80 });
    games.insert("ams2-pcars2".to_string(), GameConfig { enabled: false, intensity: 100 });
    Config {
        enabled: false,
        intensity: 42,
        pitch_pct: 50,
        leds: false,
        codemasters_port: 30500,
        pcars_port: 5607,
        beamng_port: 4445,
        games,
    }
}

#[test]
fn frontend_reader_parses_this_crates_writer() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();

    let seen = logi_dd_core::tfsim::Config::load_from(&path);
    assert!(!seen.enabled);
    assert_eq!(seen.intensity, 42);
    assert_eq!(seen.pitch_pct, 50);
    assert!(!seen.leds);
    assert_eq!(
        seen.game("dirt-rally-2"),
        logi_dd_core::tfsim::GameConfig { enabled: true, intensity: 80 }
    );
    assert_eq!(
        seen.game("ams2-pcars2"),
        logi_dd_core::tfsim::GameConfig { enabled: false, intensity: 100 }
    );
}

#[test]
fn frontend_edits_survive_this_crates_reader_and_keep_the_ports() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();

    // A front-end session: master on, intensity up, one game toggled, one
    // game's intensity trimmed, pitch changed, the rev display re-enabled.
    logi_dd_core::tfsim::set_enabled_in(&path, true).unwrap();
    logi_dd_core::tfsim::set_intensity_in(&path, 75).unwrap();
    logi_dd_core::tfsim::set_pitch_in(&path, 120).unwrap();
    logi_dd_core::tfsim::set_leds_in(&path, true).unwrap();
    logi_dd_core::tfsim::set_game_enabled_in(&path, "ams2-pcars2", true).unwrap();
    logi_dd_core::tfsim::set_game_intensity_in(&path, "dirt-rally-2", 65).unwrap();

    let seen = Config::load_from(&path);
    assert!(seen.enabled);
    assert_eq!(seen.intensity, 75);
    assert_eq!(seen.pitch_pct, 120);
    assert!(seen.leds);
    assert_eq!(seen.codemasters_port, 30500, "port keys the front-end never models survive");
    assert_eq!(seen.pcars_port, 5607);
    assert_eq!(seen.games["ams2-pcars2"], GameConfig { enabled: true, intensity: 100 });
    assert_eq!(seen.games["dirt-rally-2"], GameConfig { enabled: true, intensity: 65 });
}

#[test]
fn frontend_writer_creates_a_file_this_crates_reader_accepts() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    // No daemon ever saved: the front-end creates the file from scratch.
    logi_dd_core::tfsim::set_enabled_in(&path, false).unwrap();
    logi_dd_core::tfsim::set_game_enabled_in(&path, "dirt-rally-2", false).unwrap();

    let seen = Config::load_from(&path);
    assert!(!seen.enabled);
    assert!(!seen.game_enabled("dirt-rally-2"));
    assert_eq!(seen.codemasters_port, tf_sim::codemasters::DEFAULT_PORT, "absent keys default");
}
