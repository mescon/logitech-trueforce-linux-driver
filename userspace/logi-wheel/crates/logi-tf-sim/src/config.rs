// SPDX-License-Identifier: GPL-2.0-only
//! The tf-sim configuration store.
//!
//! `$XDG_CONFIG_HOME/logi-wheel/tf-sim.conf` (falling back to
//! `~/.config/logi-wheel/tf-sim.conf`), hand-rolled key=value in the same
//! discipline as the logi-wheel profile store: trivial format, std only,
//! comments and blank lines allowed, unknown or unparsable lines ignored
//! individually so a hand-edited file never fails wholesale.
//!
//! Keys:
//! - `enabled` (0/1): master switch
//! - `intensity` (0-100): master intensity
//! - `cylinders` (1-16): sets the engine note's firing rate with the RPM
//! - `wheel` (auto/dd/g923): which attached wheel to drive. `auto` prefers
//!   a G923 when one is present, which is right with a single wheel and
//!   leaves a direct-drive wheel unreachable on a rig with both
//! - `leds` (0/1): drive the wheel's rev display from telemetry RPM
//! - `effects` (0/1): the haptic layers beyond the engine note (limiters,
//!   shifts, ABS, traction, surface, impacts). Off leaves only the engine,
//!   which is what this daemon emitted before they existed.
//! - `effect_<name>` (0-100): gain for one layer, where `<name>` is one of
//!   `engine`, `rev_limiter`, `pit_limiter`, `gear_shift`, `abs`,
//!   `traction_loss`, `road_bumps`, `airborne`, `collision`, `drs`. 0
//!   silences that layer alone. `effect_airborne` is a depth rather than a
//!   level: it sets how far the road is quieted with the wheels off the
//!   ground. See [`crate::effects`].
//! - `port.codemasters` (also serves modern F1 and EA Sports WRC),
//!   `port.pcars`, `port.beamng`, `port.relay`: UDP listen ports
//! - `game.<id>.enabled` (0/1), `game.<id>.intensity` (0-100)
//! - `g923.ffb_invert` (0/1): sign flag for the G923 FFB mirror (see
//!   [`crate::g923::Sign`]); hardware-calibrated on a c266, defaults to 1
//!   (inverted). Set to 0 only if a given unit turns out to push the
//!   wrong way. The `LOGI_TF_SIM_G923_FFB_SIGN` environment variable
//!   overrides this for a one-off check without editing the file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{beamng, codemasters, pcars, relay};

/// First line of every saved file.
pub const FILE_HEADER: &str = "# logi-tf-sim configuration";
/// File name under the logi-wheel config directory.
pub const FILE_NAME: &str = "tf-sim.conf";

/// Default master intensity (percent).
/// Master intensity, percent.
///
/// 30, lowered from 60 on hardware evidence 2026-08-08. An RS50 owner
/// called 60 "way too powerful" and then reported 30 as fine across three
/// rev rates in the same session, which is both halves of the answer from
/// the same hands. Measured on the steering axis over a sweep, 60 moved the
/// wheel about 604 degrees and 30 about 214.
///
/// It is also the honest lever for the low-frequency haptic layers. The pit
/// limiter sits at 10 Hz, ABS at 15, the rev limiter at 25, and excursion
/// for a given torque goes roughly as 1/f^2, so those layers move a
/// direct-drive wheel rather than buzzing it. Master intensity scales all of
/// them at once, which beats guessing at a per-layer frequency curve nobody
/// has measured (see the module note in `effects.rs` for why that curve was
/// rejected).
///
/// A saved configuration is unaffected; this is the value for one that sets
/// none.
pub const DEFAULT_INTENSITY: u8 = 30;
/// Default per-game intensity (percent), relative to the master.
pub const DEFAULT_GAME_INTENSITY: u8 = 100;

/// Per-game overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameConfig {
    pub enabled: bool,
    /// 0-100, applied on top of the master intensity.
    pub intensity: u8,
}

impl Default for GameConfig {
    fn default() -> Self {
        GameConfig { enabled: true, intensity: DEFAULT_GAME_INTENSITY }
    }
}

/// The whole tf-sim configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Master switch; when false the daemon idles.
    pub enabled: bool,
    /// Master intensity, 0-100.
    pub intensity: u8,
    /// Which attached wheel to drive. Shared with the front-ends rather
    /// than redefined here, so the value the apps write is exactly the
    /// value this parses.
    pub wheel: logi_wheel_core::tfsim::WheelChoice,
    /// Felt rev-rate scale in percent (10-200). 100 puts the fundamental at
    /// the engine's true firing rate for [`Config::cylinders`]; lower is
    /// slower and heavier. See `synth::firing_frequency`.
    pub pitch_pct: u8,
    /// Cylinders, for the firing rate the engine note is built on. A
    /// four-stroke fires every cylinder once per two revolutions, so this
    /// sets the pitch as much as RPM does: a V8 fires twice as often as a
    /// four at the same RPM.
    ///
    /// Per-game override lives in [`GameConfig`]; a car-level source would
    /// be better still, since this is really a property of the car.
    pub cylinders: u8,
    /// Whether the daemon also drives the wheel's rev display
    /// (`wheel_rev_level`) from telemetry RPM while streaming.
    pub leds: bool,
    /// Whether the haptic layers beyond the engine note are mixed in.
    ///
    /// Off is the pre-effects behaviour: engine only. It exists because the
    /// effects layer changes what a wheel feels like mid-corner, and anyone
    /// who does not want that should be able to say so in one line rather
    /// than by zeroing ten gains.
    pub effects: bool,
    /// Per-layer gain; see [`crate::effects::EffectGains`].
    pub effect_gains: crate::effects::EffectGains,
    /// Codemasters/EA family listen port (classic float array, modern F1,
    /// and EA Sports WRC all arrive here, told apart by length and header).
    pub codemasters_port: u16,
    /// PCARS2/AMS2 listen port.
    pub pcars_port: u16,
    /// BeamNG OutGauge listen port.
    pub beamng_port: u16,
    /// Shared-memory telemetry relay listen port (see [`crate::relay`]).
    pub relay_port: u16,
    /// The G923 FFB-mirror sign flag, persisted; see
    /// [`crate::g923::Sign::resolve`] for how this combines with the
    /// environment override.
    pub g923_ffb_invert: bool,
    /// Per-game overrides, keyed by game id.
    pub games: BTreeMap<String, GameConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            intensity: DEFAULT_INTENSITY,
            // 35, chosen on hardware rather than by arithmetic.
            //
            // This read 25 for a long time, which reproduced exactly what
            // the daemon emitted before the firing rate was modelled
            // (rpm/60 * 4/2 * 0.25 == the old rpm/60 * 0.5). That was the
            // right way to correct the maths without changing anyone's
            // feel, and the note here said moving it wanted a hardware
            // feel-test rather than theory. That test has now happened
            // (RS50, 2026-08-07) and it says 25 is the wrong end of the
            // range:
            //
            // - By feel: 25 was reported jerky, 40 noticeably smoother.
            // - By measurement: sampling the steering axis through a sweep,
            //   25 moved the wheel 854 degrees and as far as 377 off
            //   centre, 40 moved it 552, and 60 through 100 settled around
            //   410. Lower pitch means a lower note, and a direct-drive
            //   wheel can physically follow a lower note further per half
            //   cycle, so the texture starts becoming steering input.
            //
            // 35 rather than 40 or higher because the wheel is only half
            // the audience: the note also has to sound like an engine, and
            // pitch above the firing rate makes a four sound like something
            // it is not. 35 takes most of the smoothness while staying
            // nearer the physically honest end.
            //
            // Existing users who have saved a config are unaffected: this
            // is the value for a config that does not set one.
            wheel: logi_wheel_core::tfsim::WheelChoice::Auto,
        pitch_pct: 35,
            cylinders: crate::synth::DEFAULT_CYLINDERS,
            effects: true,
            effect_gains: crate::effects::EffectGains::default(),
            leds: true,
            codemasters_port: codemasters::DEFAULT_PORT,
            pcars_port: pcars::DEFAULT_PORT,
            beamng_port: beamng::DEFAULT_PORT,
            relay_port: relay::DEFAULT_PORT,
            g923_ffb_invert: true,
            games: BTreeMap::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/logi-wheel/tf-sim.conf`, falling back to
/// `~/.config/logi-wheel/tf-sim.conf` when the variable is unset or empty.
///
/// Pre-rename installs saved this file under `logi-dd/tf-sim.conf` instead.
/// The first time the new file is needed and does not exist yet,
/// [`default_path`] copies the old file (if it exists) to the new location,
/// then uses the new path from then on; the old file is left untouched as a
/// safety net, and the copy never overwrites a file already at the new
/// path. A fresh install with neither file yet gets the new path directly,
/// with nothing to migrate. Once the new file exists, it always wins and no
/// further migration is attempted, even if the old one is still around. If
/// the copy itself cannot be completed (for example the new location is not
/// writable), this falls back to the old path for the current run instead
/// of losing anything. Mirrored, not shared, in the front-ends'
/// `logi-wheel-core::tfsim::default_path` (that crate cannot be linked here;
/// see the module doc).
pub fn default_path() -> PathBuf {
    resolve_path_in(&config_root())
}

/// `<root>/logi-wheel/tf-sim.conf` if it exists, else a one-time copy of
/// `<root>/logi-dd/tf-sim.conf` (when that exists) followed by the new
/// path, else the new path outright (fresh install, nothing to migrate).
/// Split out from [`default_path`] as a pure function of `root` (no
/// environment access) so the migration behavior is testable without
/// touching `XDG_CONFIG_HOME`.
fn resolve_path_in(root: &Path) -> PathBuf {
    let new_path = root.join("logi-wheel").join(FILE_NAME);
    if new_path.is_file() {
        return new_path;
    }
    let old_path = root.join("logi-dd").join(FILE_NAME);
    if old_path.is_file() {
        match migrate_file(&old_path, &new_path) {
            Ok(()) => return new_path,
            Err(e) => {
                eprintln!(
                    "logi-tf-sim: could not migrate {} to {}: {e}; using the old file this run",
                    old_path.display(),
                    new_path.display()
                );
                return old_path;
            }
        }
    }
    new_path
}

/// Copy `old_path` to `new_path`, creating the parent directory first. The
/// destination is created with create-if-absent semantics: an
/// `AlreadyExists` from the per-file create (another process winning the
/// same race, or a file already there for some other reason) is not an
/// error and leaves that file untouched. The original at `old_path` is
/// never touched or removed. Any other I/O failure is returned so the
/// caller can fall back to reading the old path for this run rather than
/// losing data.
fn migrate_file(old_path: &Path, new_path: &Path) -> io::Result<()> {
    if let Some(dir) = new_path.parent() {
        fs::create_dir_all(dir)?;
    }
    let data = fs::read(old_path)?;
    match fs::OpenOptions::new().write(true).create_new(true).open(new_path) {
        Ok(mut f) => f.write_all(&data),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// `$XDG_CONFIG_HOME`, falling back to `~/.config` when unset or empty.
fn config_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config")
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

fn parse_percent(raw: &str) -> Option<u8> {
    raw.parse::<u8>().ok().filter(|v| *v <= 100)
}

impl Config {
    /// Load from [`default_path`]; a missing file is the default config.
    pub fn load() -> Config {
        Config::load_from(&default_path())
    }

    /// Load from `path`. A missing or unreadable file yields the defaults;
    /// within a readable file, each unknown or unparsable line is ignored
    /// individually.
    pub fn load_from(path: &Path) -> Config {
        let mut cfg = Config::default();
        let Ok(text) = fs::read_to_string(path) else { return cfg };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, raw)) = line.split_once('=') else { continue };
            let (key, raw) = (key.trim(), raw.trim());
            match key {
                "enabled" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.enabled = v;
                    }
                }
                "intensity" => {
                    if let Some(v) = parse_percent(raw) {
                        cfg.intensity = v;
                    }
                }
                "wheel" => {
                    if let Some(v) = logi_wheel_core::tfsim::WheelChoice::parse(raw) {
                        cfg.wheel = v;
                    }
                }
                "pitch" => {
                    if let Ok(v) = raw.parse::<u8>() {
                        if (10..=200u16).contains(&u16::from(v)) {
                            cfg.pitch_pct = v;
                        }
                    }
                }
                "cylinders" => {
                    // 1..16 covers a Ducati twin through a W16. Out of range
                    // keeps the default rather than producing an engine note
                    // nothing on earth makes.
                    if let Ok(v) = raw.parse::<u8>() {
                        if (1..=16).contains(&v) {
                            cfg.cylinders = v;
                        }
                    }
                }
                "leds" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.leds = v;
                    }
                }
                "port.codemasters" => {
                    if let Ok(v) = raw.parse::<u16>() {
                        cfg.codemasters_port = v;
                    }
                }
                "port.pcars" => {
                    if let Ok(v) = raw.parse::<u16>() {
                        cfg.pcars_port = v;
                    }
                }
                "port.beamng" => {
                    if let Ok(v) = raw.parse::<u16>() {
                        cfg.beamng_port = v;
                    }
                }
                "port.relay" => {
                    if let Ok(v) = raw.parse::<u16>() {
                        cfg.relay_port = v;
                    }
                }
                "effects" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.effects = v;
                    }
                }
                "g923.ffb_invert" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.g923_ffb_invert = v;
                    }
                }
                _ => {
                    // One arm serves all ten layers: `effect_<name>`.
                    if let Some(name) = key.strip_prefix("effect_") {
                        if let (Some(id), Some(v)) =
                            (crate::effects::EffectId::from_key(name), parse_percent(raw))
                        {
                            cfg.effect_gains.set(id, v);
                        }
                        continue;
                    }
                    let Some(rest) = key.strip_prefix("game.") else { continue };
                    let Some((id, field)) = rest.rsplit_once('.') else { continue };
                    if id.is_empty() {
                        continue;
                    }
                    match field {
                        "enabled" => {
                            if let Some(v) = parse_bool(raw) {
                                cfg.games.entry(id.to_string()).or_default().enabled = v;
                            }
                        }
                        "intensity" => {
                            if let Some(v) = parse_percent(raw) {
                                cfg.games.entry(id.to_string()).or_default().intensity = v;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        cfg
    }

    /// Save to [`default_path`], creating the directory as needed.
    pub fn save(&self) -> Result<()> {
        self.save_to(&default_path())
    }

    /// Save to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let mut out = String::from(FILE_HEADER);
        out.push('\n');
        out.push_str(&format!("enabled={}\n", u8::from(self.enabled)));
        out.push_str(&format!("intensity={}\n", self.intensity));
        out.push_str(&format!("pitch={}\n", self.pitch_pct));
        out.push_str(&format!("cylinders={}\n", self.cylinders));
        out.push_str(&format!("leds={}\n", u8::from(self.leds)));
        out.push_str(&format!("port.codemasters={}\n", self.codemasters_port));
        out.push_str(&format!("port.pcars={}\n", self.pcars_port));
        out.push_str(&format!("port.beamng={}\n", self.beamng_port));
        out.push_str(&format!("port.relay={}\n", self.relay_port));
        out.push_str(&format!("effects={}\n", u8::from(self.effects)));
        for id in crate::effects::EffectId::ALL {
            out.push_str(&format!("effect_{}={}\n", id.key(), self.effect_gains.get(id)));
        }
        out.push_str(&format!("g923.ffb_invert={}\n", u8::from(self.g923_ffb_invert)));
        for (id, game) in &self.games {
            out.push_str(&format!("game.{id}.enabled={}\n", u8::from(game.enabled)));
            out.push_str(&format!("game.{id}.intensity={}\n", game.intensity));
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::Io(format!("create {}", dir.display()), e))?;
        }
        fs::write(path, out).map_err(|e| Error::Io(format!("write {}", path.display()), e))
    }

    /// Whether synthesis may run for `id`: the master switch AND the
    /// per-game switch (games default to enabled when not listed).
    pub fn game_enabled(&self, id: &str) -> bool {
        self.enabled && self.games.get(id).map_or(true, |g| g.enabled)
    }

    /// Effective intensity for `id` as 0.0..1.0: master x per-game.
    pub fn effective_intensity(&self, id: &str) -> f32 {
        let game = self.games.get(id).map_or(DEFAULT_GAME_INTENSITY, |g| g.intensity);
        (f32::from(self.intensity) / 100.0 * f32::from(game) / 100.0).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, unique temp directory per test (std only, no tempfile dep).
    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-sim-config-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_the_default_config() {
        let cfg = Config::load_from(Path::new("/nonexistent-tf-sim.conf"));
        assert_eq!(cfg, Config::default());
        assert!(cfg.enabled);
        assert!(cfg.leds, "the rev display defaults on");
        assert_eq!(cfg.intensity, DEFAULT_INTENSITY);
        assert_eq!(cfg.codemasters_port, 20777);
        assert_eq!(cfg.pcars_port, 5606);
        assert_eq!(cfg.beamng_port, 4444);
        assert_eq!(cfg.relay_port, 20780);
        assert!(cfg.g923_ffb_invert, "the FFB mirror sign defaults inverted (hardware-calibrated on a c266)");
    }

    #[test]
    fn save_load_round_trips() {
        let path = tempdir().join(FILE_NAME);
        let mut gains = crate::effects::EffectGains::default();
        for (i, id) in crate::effects::EffectId::ALL.into_iter().enumerate() {
            // A distinct value per layer, so a writer that transposed two
            // of them would not round-trip.
            gains.set(id, (i as u8) * 7 + 3);
        }
        let mut cfg = Config {
            enabled: false,
            intensity: 42,
            wheel: logi_wheel_core::tfsim::WheelChoice::Auto,
        pitch_pct: 50,
            // Deliberately not the default: this field was written to the
            // parser but not to the writer, and a round-trip test that
            // happens to pick the default value cannot see that.
            cylinders: 8,
            leds: false,
            effects: false,
            effect_gains: gains,
            codemasters_port: 30500,
            pcars_port: 5607,
            beamng_port: 4445,
            relay_port: 20781,
            g923_ffb_invert: true,
            games: BTreeMap::new(),
        };
        cfg.games.insert("dirt-rally-2".into(), GameConfig { enabled: true, intensity: 80 });
        cfg.games.insert("ams2-pcars2".into(), GameConfig { enabled: false, intensity: 100 });
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), cfg);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(FILE_HEADER));
        assert!(text.contains("leds=0\n"));
        assert!(text.contains("port.relay=20781\n"));
        assert!(text.contains("g923.ffb_invert=1\n"));
        assert!(text.contains("game.dirt-rally-2.intensity=80\n"));
    }

    #[test]
    fn save_creates_parent_directories() {
        let path = tempdir().join("nested").join("deeper").join(FILE_NAME);
        Config::default().save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), Config::default());
    }

    #[test]
    fn unknown_and_malformed_lines_are_ignored() {
        let path = tempdir().join(FILE_NAME);
        fs::write(
            &path,
            format!(
                "{FILE_HEADER}\nintensity=55\nbogus_key=7\nintensity=notanumber\n\
                 not a line\ngame..enabled=1\ngame.dirt-rally-2.bogus=3\n\
                 game.dirt-rally-2.enabled=0\nintensity2=99\nenabled=maybe\nleds=maybe\n"
            ),
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, 55, "good line before the bad one sticks");
        assert!(cfg.enabled, "unparsable bool keeps the default");
        assert!(cfg.leds, "unparsable leds bool keeps the default");
        assert!(!cfg.game_enabled("dirt-rally-2"));
        assert_eq!(cfg.games.len(), 1);
    }

    #[test]
    fn g923_ffb_invert_parses_and_defaults() {
        let path = tempdir().join(FILE_NAME);
        fs::write(&path, format!("{FILE_HEADER}\ng923.ffb_invert=1\n")).unwrap();
        assert!(Config::load_from(&path).g923_ffb_invert);

        fs::write(&path, format!("{FILE_HEADER}\ng923.ffb_invert=0\n")).unwrap();
        assert!(!Config::load_from(&path).g923_ffb_invert, "explicit 0 overrides the inverted default");

        fs::write(&path, format!("{FILE_HEADER}\ng923.ffb_invert=maybe\n")).unwrap();
        assert!(Config::load_from(&path).g923_ffb_invert, "unparsable bool keeps the (inverted) default");
    }

    #[test]
    fn out_of_range_percentages_are_ignored() {
        let path = tempdir().join(FILE_NAME);
        fs::write(&path, format!("{FILE_HEADER}\nintensity=150\ngame.f1.intensity=101\n")).unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, DEFAULT_INTENSITY);
        assert!(cfg.games.is_empty());
    }

    #[test]
    fn gating_and_effective_intensity() {
        let mut cfg = Config { intensity: 50, ..Config::default() };
        cfg.games.insert("f1".into(), GameConfig { enabled: false, intensity: 100 });
        cfg.games.insert("dirt-rally-2".into(), GameConfig { enabled: true, intensity: 50 });

        assert!(cfg.game_enabled("dirt-rally-2"));
        assert!(!cfg.game_enabled("f1"), "per-game off wins");
        assert!(cfg.game_enabled("codemasters"), "unlisted games default on");

        assert!((cfg.effective_intensity("dirt-rally-2") - 0.25).abs() < 1e-6);
        assert!((cfg.effective_intensity("codemasters") - 0.5).abs() < 1e-6);

        cfg.enabled = false;
        assert!(!cfg.game_enabled("dirt-rally-2"), "master off wins");
    }

    #[test]
    fn default_path_honors_xdg_config_home() {
        // The only test in this crate that touches the environment, so it
        // cannot race the others.
        let dir = tempdir();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert_eq!(default_path(), dir.join("logi-wheel").join(FILE_NAME));
        let cfg = Config { intensity: 33, ..Config::default() };
        cfg.save().unwrap();
        assert_eq!(Config::load(), cfg);
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// Fresh install: neither directory exists yet. [`resolve_path_in`]
    /// returns the new path outright and never even creates a `logi-dd`
    /// directory to check.
    #[test]
    fn resolve_path_in_fresh_install_is_the_new_path_untouched() {
        let root = tempdir();
        let resolved = resolve_path_in(&root);
        assert_eq!(resolved, root.join("logi-wheel").join(FILE_NAME));
        assert!(!resolved.exists(), "nothing to migrate, nothing created");
        assert!(!root.join("logi-dd").exists(), "the old directory was never touched, let alone created");
    }

    /// Old-only: a pre-rename install with no new file yet. The old file is
    /// copied to the new location, the resolved path is the new one, and
    /// the original is left in place as a safety net.
    #[test]
    fn resolve_path_in_migrates_the_old_file_once() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd")).unwrap();
        fs::write(root.join("logi-dd").join(FILE_NAME), "intensity=22\n").unwrap();

        let resolved = resolve_path_in(&root);
        assert_eq!(resolved, root.join("logi-wheel").join(FILE_NAME), "the new path wins after migrating");
        assert_eq!(Config::load_from(&resolved).intensity, 22);
        assert_eq!(
            fs::read_to_string(root.join("logi-dd").join(FILE_NAME)).unwrap(),
            "intensity=22\n",
            "the original is left in place untouched"
        );

        // A second resolution finds the new file directly and does not
        // need to migrate again.
        assert_eq!(resolve_path_in(&root), root.join("logi-wheel").join(FILE_NAME));
    }

    /// Both exist: the new file wins outright, and the old one is left
    /// completely alone (no copy is even attempted).
    #[test]
    fn resolve_path_in_prefers_the_new_file_when_both_exist() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd")).unwrap();
        fs::write(root.join("logi-dd").join(FILE_NAME), "intensity=33\n").unwrap();
        fs::create_dir_all(root.join("logi-wheel")).unwrap();
        fs::write(root.join("logi-wheel").join(FILE_NAME), "intensity=44\n").unwrap();

        let resolved = resolve_path_in(&root);
        assert_eq!(resolved, root.join("logi-wheel").join(FILE_NAME));
        assert_eq!(Config::load_from(&resolved).intensity, 44, "the new file wins");
        assert_eq!(
            fs::read_to_string(root.join("logi-dd").join(FILE_NAME)).unwrap(),
            "intensity=33\n",
            "the old file is untouched, not overwritten by the new one's content"
        );
    }

    /// Migration failure (the new location cannot be created, simulated
    /// cheaply by putting a plain file where the `logi-wheel` directory
    /// needs to go): resolution still falls back to the old path rather
    /// than panicking or losing the original.
    #[test]
    fn resolve_path_in_falls_back_to_the_old_file_when_migration_fails() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd")).unwrap();
        fs::write(root.join("logi-dd").join(FILE_NAME), "intensity=55\n").unwrap();
        // Block `<root>/logi-wheel` from ever becoming a directory.
        fs::write(root.join("logi-wheel"), "not a directory").unwrap();

        let resolved = resolve_path_in(&root);
        assert_eq!(resolved, root.join("logi-dd").join(FILE_NAME), "falls back to the old file");
        assert_eq!(Config::load_from(&resolved).intensity, 55);
        assert_eq!(
            fs::read_to_string(root.join("logi-dd").join(FILE_NAME)).unwrap(),
            "intensity=55\n",
            "the original survives the failed migration"
        );
    }
}
