// SPDX-License-Identifier: GPL-2.0-only
//! The tf-sim configuration store.
//!
//! `$XDG_CONFIG_HOME/logi-dd/tf-sim.conf` (falling back to
//! `~/.config/logi-dd/tf-sim.conf`), hand-rolled key=value in the same
//! discipline as the logi-dd profile store: trivial format, std only,
//! comments and blank lines allowed, unknown or unparsable lines ignored
//! individually so a hand-edited file never fails wholesale.
//!
//! Keys:
//! - `enabled` (0/1): master switch
//! - `intensity` (0-100): master intensity
//! - `port.codemasters`, `port.pcars`: UDP listen ports
//! - `game.<id>.enabled` (0/1), `game.<id>.intensity` (0-100)

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{codemasters, pcars};

/// First line of every saved file.
pub const FILE_HEADER: &str = "# logi-tf-sim configuration";
/// File name under the logi-dd config directory.
pub const FILE_NAME: &str = "tf-sim.conf";

/// Default master intensity (percent).
pub const DEFAULT_INTENSITY: u8 = 60;
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
    /// Codemasters/EA family listen port.
    pub codemasters_port: u16,
    /// PCARS2/AMS2 listen port.
    pub pcars_port: u16,
    /// Per-game overrides, keyed by game id.
    pub games: BTreeMap<String, GameConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            intensity: DEFAULT_INTENSITY,
            codemasters_port: codemasters::DEFAULT_PORT,
            pcars_port: pcars::DEFAULT_PORT,
            games: BTreeMap::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/logi-dd/tf-sim.conf`, falling back to
/// `~/.config/logi-dd/tf-sim.conf` when the variable is unset or empty.
pub fn default_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("logi-dd").join(FILE_NAME);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("logi-dd").join(FILE_NAME)
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
                _ => {
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
        out.push_str(&format!("port.codemasters={}\n", self.codemasters_port));
        out.push_str(&format!("port.pcars={}\n", self.pcars_port));
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
        assert_eq!(cfg.intensity, DEFAULT_INTENSITY);
        assert_eq!(cfg.codemasters_port, 20777);
        assert_eq!(cfg.pcars_port, 5606);
    }

    #[test]
    fn save_load_round_trips() {
        let path = tempdir().join(FILE_NAME);
        let mut cfg = Config { enabled: false, intensity: 42, codemasters_port: 30500, pcars_port: 5607, games: BTreeMap::new() };
        cfg.games.insert("dirt-rally-2".into(), GameConfig { enabled: true, intensity: 80 });
        cfg.games.insert("ams2-pcars2".into(), GameConfig { enabled: false, intensity: 100 });
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), cfg);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(FILE_HEADER));
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
                 game.dirt-rally-2.enabled=0\nintensity2=99\nenabled=maybe\n"
            ),
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, 55, "good line before the bad one sticks");
        assert!(cfg.enabled, "unparsable bool keeps the default");
        assert!(!cfg.game_enabled("dirt-rally-2"));
        assert_eq!(cfg.games.len(), 1);
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
        assert_eq!(default_path(), dir.join("logi-dd").join(FILE_NAME));
        let cfg = Config { intensity: 33, ..Config::default() };
        cfg.save().unwrap();
        assert_eq!(Config::load(), cfg);
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
