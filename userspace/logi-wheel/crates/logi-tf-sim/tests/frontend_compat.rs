// SPDX-License-Identifier: GPL-2.0-only
//! Cross-compatibility pin between this crate's config store (the FORMAT
//! AUTHORITY for tf-sim.conf) and the front-ends' format-compatible
//! reader/writer in `logi_wheel_core::tfsim`. The core module exists because
//! the GUI front-end (GPL-3.0-or-later) cannot link this GPL-2.0-only
//! crate; these tests are what keeps the two implementations honest.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use logi_tf_sim::config::{Config, GameConfig};
use logi_tf_sim::effects::{EffectGains, EffectId};

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
    let mut effect_gains = EffectGains::default();
    for (i, id) in EffectId::ALL.into_iter().enumerate() {
        effect_gains.set(id, (i as u8) * 7 + 3);
    }
    Config {
        enabled: false,
        intensity: 42,
        wheel: logi_wheel_core::tfsim::WheelChoice::Auto,
        pitch_pct: 50,
        cylinders: 8,
        leds: false,
        effects: false,
        // Non-default on purpose: a round trip that lands on the default
        // cannot see a field the writer forgot.
        screen: true,
        screen_template: "J|{gear}|{speed}|{rpm}|x".to_string(),
        effect_gains,
        codemasters_port: 30500,
        pcars_port: 5607,
        beamng_port: 4445,
        relay_port: 20780,
        g923_stream_without_ffb_mirror: false,
        follow_game_gain: true,
        g923_ffb_invert: false,
        games,
    }
}

#[test]
fn frontend_reader_parses_this_crates_writer() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();

    let seen = logi_wheel_core::tfsim::Config::load_from(&path);
    assert!(!seen.enabled);
    assert_eq!(seen.intensity, 42);
    assert_eq!(seen.pitch_pct, 50);
    assert!(seen.screen, "the screen switch round-trips");
    assert_eq!(seen.screen_template, "J|{gear}|{speed}|{rpm}|x");
    assert!(!seen.leds);
    assert_eq!(
        seen.game("dirt-rally-2"),
        logi_wheel_core::tfsim::GameConfig { enabled: true, intensity: 80 }
    );
    assert_eq!(
        seen.game("ams2-pcars2"),
        logi_wheel_core::tfsim::GameConfig { enabled: false, intensity: 100 }
    );
}

#[test]
fn frontend_edits_survive_this_crates_reader_and_keep_the_ports() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();

    // A front-end session: master on, intensity up, one game toggled, one
    // game's intensity trimmed, pitch changed, the rev display re-enabled.
    logi_wheel_core::tfsim::set_enabled_in(&path, true).unwrap();
    logi_wheel_core::tfsim::set_intensity_in(&path, 75).unwrap();
    logi_wheel_core::tfsim::set_pitch_in(&path, 120).unwrap();
    logi_wheel_core::tfsim::set_leds_in(&path, true).unwrap();
    logi_wheel_core::tfsim::set_game_enabled_in(&path, "ams2-pcars2", true).unwrap();
    logi_wheel_core::tfsim::set_game_intensity_in(&path, "dirt-rally-2", 65).unwrap();

    let seen = Config::load_from(&path);
    assert!(seen.enabled);
    assert_eq!(seen.intensity, 75);
    assert_eq!(seen.pitch_pct, 120);
    assert!(seen.leds);
    assert_eq!(seen.codemasters_port, 30500, "port keys the front-end never models survive");
    // The effects layer is likewise not modelled by the front-end yet. Its
    // keys must survive a front-end session untouched, or a user who tuned
    // the mix by hand loses it the first time they move a slider.
    assert!(!seen.effects, "the effects master switch survived");
    let mut want = EffectGains::default();
    for (i, id) in EffectId::ALL.into_iter().enumerate() {
        want.set(id, (i as u8) * 7 + 3);
    }
    assert_eq!(seen.effect_gains, want, "per-layer gains survived");
    assert_eq!(seen.pcars_port, 5607);
    assert_eq!(seen.games["ams2-pcars2"], GameConfig { enabled: true, intensity: 100 });
    assert_eq!(seen.games["dirt-rally-2"], GameConfig { enabled: true, intensity: 65 });
}

#[test]
fn frontend_writer_creates_a_file_this_crates_reader_accepts() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    // No daemon ever saved: the front-end creates the file from scratch.
    logi_wheel_core::tfsim::set_enabled_in(&path, false).unwrap();
    logi_wheel_core::tfsim::set_game_enabled_in(&path, "dirt-rally-2", false).unwrap();

    let seen = Config::load_from(&path);
    assert!(!seen.enabled);
    assert!(!seen.game_enabled("dirt-rally-2"));
    assert_eq!(seen.codemasters_port, logi_tf_sim::codemasters::DEFAULT_PORT, "absent keys default");
}

/// The front-end mirrors the daemon's effect list because the crates cannot
/// link. Nothing but this test stops the two drifting: a layer added to one
/// and not the other is a slider that writes a key nothing reads, or a key
/// nothing can reach.
#[test]
fn the_frontends_effect_list_matches_the_daemons() {
    let daemon: Vec<&str> = EffectId::ALL.iter().map(|id| id.key()).collect();
    let mut frontend: Vec<&str> =
        logi_wheel_core::tfsim::EFFECTS.iter().map(|e| e.key).collect();
    frontend.sort_unstable();
    let mut daemon_sorted = daemon.clone();
    daemon_sorted.sort_unstable();
    assert_eq!(daemon_sorted, frontend, "the two effect lists have drifted");

    // Defaults too: a front-end showing 40 where the daemon uses 60 lies
    // about the mix until the user touches the slider.
    for id in EffectId::ALL {
        let mirrored = logi_wheel_core::tfsim::effect_by_key(id.key())
            .unwrap_or_else(|| panic!("front-end is missing {}", id.key()));
        assert_eq!(
            mirrored.default_gain,
            id.default_gain(),
            "default gain for {} differs",
            id.key()
        );
    }
}

/// A gain written by the front-end has to be the gain the daemon renders.
#[test]
fn a_gain_the_frontend_writes_is_the_gain_the_daemon_reads() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();

    logi_wheel_core::tfsim::set_effects_in(&path, true).unwrap();
    for (i, id) in EffectId::ALL.into_iter().enumerate() {
        logi_wheel_core::tfsim::set_effect_gain_in(&path, id.key(), (i as u8) * 9 + 5).unwrap();
    }

    let seen = Config::load_from(&path);
    assert!(seen.effects);
    for (i, id) in EffectId::ALL.into_iter().enumerate() {
        assert_eq!(seen.effect_gains.get(id), (i as u8) * 9 + 5, "{}", id.key());
    }

    // And the front-end reads back what it wrote.
    let mirrored = logi_wheel_core::tfsim::Config::load_from(&path);
    for (i, id) in EffectId::ALL.into_iter().enumerate() {
        assert_eq!(mirrored.effect_gains.get(id.key()), (i as u8) * 9 + 5, "{}", id.key());
    }
}

/// A typo must not leave a dead key in the user's file.
#[test]
fn an_unknown_layer_name_writes_nothing() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    full_config().save_to(&path).unwrap();
    logi_wheel_core::tfsim::set_effect_gain_in(&path, "turbo_whistle", 50).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("turbo_whistle"), "a typo reached the file");
}

/// The daemon keeps `logi-wheel-core` as a dev-dependency, so its G923
/// product ids are a deliberate copy rather than an import. This is the
/// guard that copy did not have: it omitted `c267` while the kernel bound
/// all three, so a PlayStation/PC-edition G923 was named correctly by the
/// settings pages and then not found at all by discovery here.
#[test]
fn g923_product_ids_match_the_shared_list() {
    let mut mine = logi_tf_sim::g923::PIDS.to_vec();
    let mut theirs = logi_wheel_core::device::G923_PIDS.to_vec();
    mine.sort_unstable();
    theirs.sort_unstable();
    assert_eq!(
        mine, theirs,
        "logi-tf-sim's G923 product ids disagree with logi-wheel-core's. A wheel \
         missing from one of them is identified by the settings pages and then \
         invisible to the daemon, or the reverse."
    );
}

/// Scalar defaults are part of the format contract too.
///
/// The effect gains below were already pinned; the scalars were not, and
/// `pitch_pct` drifted to three different values: 25 in the daemon (which
/// this module's own doc calls the format authority), 50 in the front-end
/// mirror, and 100 as the Slint initial. A fresh install therefore showed a
/// rev rate of 50% while the daemon ran at 25%.
#[test]
fn scalar_defaults_match_the_daemon() {
    let daemon = logi_tf_sim::config::Config::default();
    let mirror = logi_wheel_core::tfsim::Config::default();

    assert_eq!(mirror.pitch_pct, daemon.pitch_pct, "pitch_pct (rev rate)");
    assert_eq!(mirror.intensity, daemon.intensity, "intensity");
    assert_eq!(mirror.enabled, daemon.enabled, "master enable");
    assert_eq!(mirror.leds, daemon.leds, "rev-LED feeder");
}

/// The `wheel` key must mean the same thing on both sides.
///
/// The daemon reads it to decide which attached wheel to drive, and the
/// front-ends write it. Two independent implementations of the same file
/// format is the whole reason this test file exists, and a key that one
/// side writes and the other ignores would silently strand a wheel.
#[test]
fn wheel_key_round_trips_between_daemon_and_frontends() {
    use logi_wheel_core::tfsim::WheelChoice;

    for choice in WheelChoice::ALL {
        let tree = TempTree::new();
        let path = tree.path().join("tf-sim.conf");

        // The front-end writes it...
        logi_wheel_core::tfsim::set_wheel_in(&path, choice).expect("front-end write");
        // ...and the daemon must read back exactly that.
        let daemon = Config::load_from(&path);
        assert_eq!(
            daemon.wheel, choice,
            "the daemon read {:?} where the front-end wrote {choice:?}",
            daemon.wheel
        );
        // And the front-end's own reader must agree with both.
        let frontend = logi_wheel_core::tfsim::Config::load_from(&path);
        assert_eq!(frontend.wheel, choice);
    }
}

/// An unset key means automatic on both sides, not "whatever the struct
/// default happened to be".
#[test]
fn wheel_key_absent_means_auto_on_both_sides() {
    use logi_wheel_core::tfsim::WheelChoice;
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");
    fs::write(&path, "intensity = 40\n").expect("write");

    assert_eq!(Config::load_from(&path).wheel, WheelChoice::Auto);
    assert_eq!(logi_wheel_core::tfsim::Config::load_from(&path).wheel, WheelChoice::Auto);
}

/// The unrecognised-line count must agree between the daemon's loader
/// (`LoadReport`) and the front-ends' scan (`tfsim::unrecognised_lines`),
/// or the app warns about a file the daemon reads clean, or stays quiet
/// about one it does not.
#[test]
fn unrecognised_line_counts_agree_between_daemon_and_frontends() {
    let tree = TempTree::new();
    let path = tree.path().join("tf-sim.conf");

    // Everything the daemon's writer emits parses clean on both sides.
    full_config().save_to(&path).unwrap();
    let (_, report) = Config::load_from_with_report(&path);
    assert_eq!((report.skipped, report.first.clone()), (0, None));
    assert_eq!(logi_wheel_core::tfsim::unrecognised_lines(&path), (0, None));

    // A mixed file: opaque-to-the-apps keys (ports, cylinders, the G923
    // sign flag) beside genuine garbage of each kind the parser refuses.
    fs::write(
        &path,
        "# comment\n\
         port.codemasters=20777\n\
         cylinders=8\n\
         g923.ffb_invert=0\n\
         intensty=80\n\
         pitch=5\n\
         not a line\n\
         game.f1.bogus=1\n\
         effect_typo=50\n",
    )
    .unwrap();
    let (_, report) = Config::load_from_with_report(&path);
    let mirrored = logi_wheel_core::tfsim::unrecognised_lines(&path);
    assert_eq!((report.skipped, report.first), (mirrored.0, mirrored.1.clone()));
    assert_eq!(mirrored, (5, Some("intensty=80".to_string())));

    // And the two warning sentences are the same text, so the app and the
    // daemon's log name one problem one way.
    let (_, report) = Config::load_from_with_report(&path);
    assert_eq!(report.warning(), logi_wheel_core::tfsim::conf_warning(&path));
}
