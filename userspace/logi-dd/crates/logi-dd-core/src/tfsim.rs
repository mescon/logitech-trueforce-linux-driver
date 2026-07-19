//! Front-end access to the simulated-TrueForce daemon (`logi-tf-sim`): its
//! configuration file and its process state.
//!
//! The FORMAT AUTHORITY for `tf-sim.conf` is the tf-sim crate's `config`
//! module; this module is a format-compatible reader/writer, not a second
//! source of truth. It lives here rather than linking the tf-sim crate
//! because tf-sim is GPL-2.0-only while the GUI front-end is
//! GPL-3.0-or-later (the two cannot be combined), and both front-ends
//! already depend on this crate. Cross-compatibility is pinned by a
//! fixture test in the tf-sim crate (`tests/frontend_compat.rs`, via a
//! dev-dependency on this crate) that parses files produced by tf-sim's
//! own writer.
//!
//! Two deliberate differences from tf-sim's own store:
//! - the front-ends only model the keys they edit (`enabled`, `intensity`,
//!   `pitch`, `leds`, `game.<id>.*`); everything else (the `port.*` keys,
//!   comments, hand-added lines) is opaque and
//! - writes go through [`write_key_in`], which rewrites ONE key in place
//!   and preserves every other line verbatim, so a front-end edit can never
//!   drop a key it does not know about.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// First line written into a fresh file (same header tf-sim writes).
pub const FILE_HEADER: &str = "# logi-tf-sim configuration";
/// File name under the logi-dd config directory.
pub const FILE_NAME: &str = "tf-sim.conf";

/// Default master intensity (percent); mirrors tf-sim's default.
pub const DEFAULT_INTENSITY: u8 = 60;
/// Default per-game intensity (percent), relative to the master.
pub const DEFAULT_GAME_INTENSITY: u8 = 100;
/// Default pitch scale (percent of the crank rate).
pub const DEFAULT_PITCH: u8 = 100;

/// The daemon's process name, as `/proc/<pid>/stat` reports it (11 chars,
/// safely under the kernel's 15-char comm truncation).
pub const DAEMON_COMM: &str = "logi-tf-sim";

/// Per-game overrides, mirroring tf-sim's `GameConfig`.
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

/// The keys of tf-sim's configuration the front-ends edit. The `port.*`
/// keys are intentionally absent: the front-ends never touch them, and
/// [`write_key_in`] preserves them (and anything else) on every write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Master switch; when false the daemon idles.
    pub enabled: bool,
    /// Master intensity, 0-100.
    pub intensity: u8,
    /// Felt rev-rate scale in percent (10-200; 100 = the crank rate).
    pub pitch_pct: u8,
    /// Whether the daemon also drives the wheel's rev display
    /// (`wheel_rev_level`) from telemetry RPM while streaming.
    pub leds: bool,
    /// Per-game overrides, keyed by tf-sim game id.
    pub games: BTreeMap<String, GameConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            intensity: DEFAULT_INTENSITY,
            pitch_pct: DEFAULT_PITCH,
            leds: true,
            games: BTreeMap::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/logi-dd/tf-sim.conf`, falling back to
/// `~/.config/logi-dd/tf-sim.conf` when the variable is unset or empty.
/// Same resolution as tf-sim's own `default_path`.
pub fn default_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("logi-dd").join(FILE_NAME);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("logi-dd").join(FILE_NAME)
}

/// tf-sim's boolean spellings.
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

    /// Load from `path`. Same forgiveness rules as tf-sim's reader: a
    /// missing or unreadable file yields the defaults, and within a
    /// readable file each unknown or unparsable line is ignored
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
                "pitch" => {
                    if let Ok(v) = raw.parse::<u8>() {
                        if (10..=200u16).contains(&u16::from(v)) {
                            cfg.pitch_pct = v;
                        }
                    }
                }
                "leds" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.leds = v;
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

    /// The effective per-game state for `id` (the stored override, or the
    /// defaults for an unlisted game, exactly like the daemon treats it).
    pub fn game(&self, id: &str) -> GameConfig {
        self.games.get(id).copied().unwrap_or_default()
    }
}

/// Rewrite exactly one `key=value` line in the file at `path`, preserving
/// every other line (unknown keys, the `port.*` settings, comments, blank
/// lines) verbatim. The first line carrying `key` is replaced in place and
/// any duplicates of it are dropped; a key not present yet is appended. A
/// missing file is created (with tf-sim's header) so a front-end edit
/// works before the daemon ever saved.
pub fn write_key_in(path: &Path, key: &str, value: &str) -> Result<(), Error> {
    let text = fs::read_to_string(path).unwrap_or_else(|_| format!("{FILE_HEADER}\n"));
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 2);
    let mut replaced = false;
    for line in text.lines() {
        let is_key = line
            .split_once('=')
            .is_some_and(|(k, _)| !line.trim_start().starts_with('#') && k.trim() == key);
        if is_key {
            if !replaced {
                out.push_str(&format!("{key}={value}\n"));
                replaced = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&format!("{key}={value}\n"));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::Io(format!("create {}: {e}", dir.display())))?;
    }
    fs::write(path, out).map_err(|e| Error::Io(format!("write {}: {e}", path.display())))
}

/// Write the master switch.
pub fn set_enabled_in(path: &Path, enabled: bool) -> Result<(), Error> {
    write_key_in(path, "enabled", if enabled { "1" } else { "0" })
}

/// Write the master intensity (clamped to 0-100).
pub fn set_intensity_in(path: &Path, intensity: u8) -> Result<(), Error> {
    write_key_in(path, "intensity", &intensity.min(100).to_string())
}

/// Write the pitch scale (clamped to 10-200).
pub fn set_pitch_in(path: &Path, pitch_pct: u8) -> Result<(), Error> {
    write_key_in(path, "pitch", &pitch_pct.clamp(10, 200).to_string())
}

/// Write the rev-display switch.
pub fn set_leds_in(path: &Path, leds: bool) -> Result<(), Error> {
    write_key_in(path, "leds", if leds { "1" } else { "0" })
}

/// Write one game's enable switch.
pub fn set_game_enabled_in(path: &Path, id: &str, enabled: bool) -> Result<(), Error> {
    write_key_in(path, &format!("game.{id}.enabled"), if enabled { "1" } else { "0" })
}

/// Write one game's intensity (clamped to 0-100).
pub fn set_game_intensity_in(path: &Path, id: &str, intensity: u8) -> Result<(), Error> {
    write_key_in(path, &format!("game.{id}.intensity"), &intensity.min(100).to_string())
}

/// The tf-sim game id for a games-list title, or `None` when the daemon
/// has no per-game id for it. Deliberately conservative: only titles whose
/// ids actually exist in the daemon's telemetry detection map here
/// (matching is case-insensitive but otherwise exact, so "DiRT Rally 2.0"
/// from Steam and "Dirt Rally 2.0" from the compatibility tables both
/// match while remasters or sequels never do).
pub fn game_id_for_title(title: &str) -> Option<&'static str> {
    match title.trim().to_lowercase().as_str() {
        "dirt rally 2.0" => Some("dirt-rally-2"),
        "automobilista 2" | "project cars 2" => Some("ams2-pcars2"),
        _ => None,
    }
}

/// The comm field out of one `/proc/<pid>/stat` line: the text between the
/// first `(` and the LAST `)` (the kernel does not escape parentheses in
/// comm, so only the last close-paren is safe).
pub fn stat_comm(stat: &str) -> Option<&str> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close <= open {
        return None;
    }
    Some(&stat[open + 1..close])
}

/// The state character after the comm field of a `/proc/<pid>/stat` line
/// (`R`, `S`, `Z`, ...).
fn stat_state(stat: &str) -> Option<char> {
    stat[stat.rfind(')')? + 1..].trim_start().chars().next()
}

/// Every pid under `proc_root` whose stat comm equals `comm`, ascending.
/// Zombies are excluded: a front-end that spawned the daemon detached may
/// hold its exit status un-reaped for a while, and a zombie's comm still
/// matches even though nothing is running. Parameterized over the proc
/// root so the scan is testable against a fixture tree; unreadable
/// entries are skipped.
pub fn pids_by_comm_in(proc_root: &Path, comm: &str) -> Vec<i32> {
    let Ok(entries) = fs::read_dir(proc_root) else { return Vec::new() };
    let mut pids: Vec<i32> = entries
        .flatten()
        .filter_map(|entry| {
            let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
            if matches!(stat_state(&stat), Some('Z') | Some('X')) {
                return None;
            }
            (stat_comm(&stat)? == comm).then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The running `logi-tf-sim` daemon's pid (the lowest, if several), or
/// `None` while it is not running. Scans `/proc` directly, so no `pgrep`
/// dependency.
pub fn daemon_pid() -> Option<i32> {
    pids_by_comm_in(Path::new("/proc"), DAEMON_COMM).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (same pattern as the
    /// `steam`/`helpers` tests).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-tfsim-test-{}-{}",
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

    /// A file in tf-sim's own save layout (see its `Config::save_to`),
    /// pinned here as a literal so a drift in either writer or reader
    /// fails a test. The authoritative cross-check against tf-sim's real
    /// writer lives in that crate's `tests/frontend_compat.rs`.
    const TFSIM_WRITER_FIXTURE: &str = "# logi-tf-sim configuration\n\
         enabled=0\n\
         intensity=42\n\
         pitch=50\n\
         leds=0\n\
         port.codemasters=30500\n\
         port.pcars=5607\n\
         game.ams2-pcars2.enabled=0\n\
         game.ams2-pcars2.intensity=100\n\
         game.dirt-rally-2.enabled=1\n\
         game.dirt-rally-2.intensity=80\n";

    #[test]
    fn missing_file_is_the_default_config() {
        let cfg = Config::load_from(Path::new("/nonexistent-tf-sim.conf"));
        assert_eq!(cfg, Config::default());
        assert!(cfg.enabled);
        assert!(cfg.leds, "the rev display defaults on");
        assert_eq!(cfg.intensity, DEFAULT_INTENSITY);
        assert_eq!(cfg.pitch_pct, DEFAULT_PITCH);
        assert_eq!(cfg.game("dirt-rally-2"), GameConfig::default());
    }

    #[test]
    fn reads_the_tfsim_writer_layout() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(&path, TFSIM_WRITER_FIXTURE).unwrap();
        let cfg = Config::load_from(&path);
        assert!(!cfg.enabled);
        assert_eq!(cfg.intensity, 42);
        assert_eq!(cfg.pitch_pct, 50);
        assert!(!cfg.leds);
        assert_eq!(cfg.game("dirt-rally-2"), GameConfig { enabled: true, intensity: 80 });
        assert_eq!(cfg.game("ams2-pcars2"), GameConfig { enabled: false, intensity: 100 });
        assert_eq!(cfg.game("unlisted"), GameConfig::default());
    }

    #[test]
    fn malformed_and_out_of_range_lines_are_ignored() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(
            &path,
            format!(
                "{FILE_HEADER}\nintensity=55\nintensity=notanumber\nbogus=7\n\
                 game..enabled=1\ngame.f1.intensity=101\npitch=5\nenabled=maybe\n"
            ),
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, 55);
        assert!(cfg.enabled, "unparsable bool keeps the default");
        assert_eq!(cfg.pitch_pct, DEFAULT_PITCH, "pitch below 10 is ignored");
        assert!(cfg.games.is_empty(), "empty id and out-of-range intensity are ignored");
    }

    #[test]
    fn write_key_preserves_unknown_keys_and_comments() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(&path, TFSIM_WRITER_FIXTURE).unwrap();
        set_intensity_in(&path, 70).unwrap();
        set_game_enabled_in(&path, "dirt-rally-2", false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(FILE_HEADER), "header preserved");
        assert!(text.contains("port.codemasters=30500\n"), "unknown key preserved");
        assert!(text.contains("port.pcars=5607\n"), "unknown key preserved");
        assert!(text.contains("intensity=70\n"));
        assert!(text.contains("game.dirt-rally-2.enabled=0\n"));
        // The edit replaced in place, it did not append a duplicate.
        assert_eq!(text.matches("\nintensity=").count(), 1);
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, 70);
        assert!(!cfg.game("dirt-rally-2").enabled);
        assert_eq!(cfg.game("dirt-rally-2").intensity, 80, "sibling key untouched");
    }

    #[test]
    fn write_key_does_not_match_prefixed_or_commented_keys() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(&path, "# intensity=1 in a comment\nintensity2=99\nintensity=10\n").unwrap();
        set_intensity_in(&path, 33).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# intensity=1 in a comment\n"), "comment untouched");
        assert!(text.contains("intensity2=99\n"), "longer key untouched");
        assert!(text.contains("intensity=33\n"));
    }

    #[test]
    fn write_key_creates_a_missing_file_with_the_header() {
        let tree = TempTree::new();
        let path = tree.path().join("nested").join(FILE_NAME);
        set_enabled_in(&path, false).unwrap();
        set_pitch_in(&path, 150).unwrap();
        set_leds_in(&path, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(FILE_HEADER));
        let cfg = Config::load_from(&path);
        assert!(!cfg.enabled);
        assert_eq!(cfg.pitch_pct, 150);
        assert!(!cfg.leds);
    }

    #[test]
    fn write_key_collapses_duplicates_in_hand_edited_files() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(&path, "enabled=1\nintensity=10\nintensity=20\n").unwrap();
        set_intensity_in(&path, 30).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "enabled=1\nintensity=30\n");
    }

    #[test]
    fn setters_clamp_their_ranges() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        set_intensity_in(&path, 200).unwrap();
        set_pitch_in(&path, 5).unwrap();
        set_game_intensity_in(&path, "ams2-pcars2", 130).unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.intensity, 100);
        assert_eq!(cfg.pitch_pct, 10);
        assert_eq!(cfg.game("ams2-pcars2").intensity, 100);
    }

    #[test]
    fn game_id_mapping_is_conservative() {
        assert_eq!(game_id_for_title("Dirt Rally 2.0"), Some("dirt-rally-2"));
        assert_eq!(game_id_for_title("DiRT Rally 2.0"), Some("dirt-rally-2"), "Steam's casing");
        assert_eq!(game_id_for_title("Automobilista 2"), Some("ams2-pcars2"));
        assert_eq!(game_id_for_title("Project CARS 2"), Some("ams2-pcars2"));
        assert_eq!(game_id_for_title("DiRT Rally"), None, "predecessor never matches");
        assert_eq!(game_id_for_title("EA SPORTS WRC"), None);
        assert_eq!(game_id_for_title("Le Mans Ultimate"), None);
        assert_eq!(game_id_for_title(""), None);
    }

    #[test]
    fn stat_comm_handles_parens_and_spaces() {
        assert_eq!(stat_comm("1234 (logi-tf-sim) S 1 1234"), Some("logi-tf-sim"));
        assert_eq!(stat_comm("77 ((sd-pam)) S 1 77"), Some("(sd-pam)"));
        assert_eq!(stat_comm("9 (tmux: server) S 1 9"), Some("tmux: server"));
        assert_eq!(stat_comm("no parens here"), None);
        assert_eq!(stat_comm(""), None);
    }

    #[test]
    fn pid_scan_finds_only_the_daemon_comm() {
        let tree = TempTree::new();
        let proc_root = tree.path();
        let write_stat = |pid: &str, comm: &str| {
            let dir = proc_root.join(pid);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("stat"), format!("{pid} ({comm}) S 1 {pid}")).unwrap();
        };
        write_stat("100", "bash");
        write_stat("250", DAEMON_COMM);
        write_stat("90", DAEMON_COMM);
        // A zombie's comm still matches, but nothing is running: it must
        // not count (a front-end that spawned the daemon detached may
        // hold the exit status un-reaped for a while).
        let zombie = proc_root.join("400");
        fs::create_dir_all(&zombie).unwrap();
        fs::write(zombie.join("stat"), format!("400 ({DAEMON_COMM}) Z 1 400")).unwrap();
        // Non-pid entries (like /proc/self) and pid dirs without a
        // readable stat are skipped, not errors.
        fs::create_dir_all(proc_root.join("self")).unwrap();
        fs::create_dir_all(proc_root.join("300")).unwrap();
        assert_eq!(pids_by_comm_in(proc_root, DAEMON_COMM), vec![90, 250]);
        assert_eq!(pids_by_comm_in(proc_root, "nothing-runs-this"), Vec::<i32>::new());
        assert_eq!(pids_by_comm_in(Path::new("/nonexistent-proc"), DAEMON_COMM), Vec::<i32>::new());
    }
}
