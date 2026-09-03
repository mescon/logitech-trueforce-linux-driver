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
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::Error;

/// First line written into a fresh file (same header tf-sim writes).
pub const FILE_HEADER: &str = "# logi-tf-sim configuration";
/// File name under the logi-wheel config directory.
pub const FILE_NAME: &str = "tf-sim.conf";

/// Default master intensity (percent); mirrors tf-sim's default.
/// Master intensity, percent. 30, matching the daemon; see its
/// `DEFAULT_INTENSITY` for why. Pinned against it by tf-sim's
/// `frontend_compat` test, because this pair has drifted before.
pub const DEFAULT_INTENSITY: u8 = 30;
/// Default per-game intensity (percent), relative to the master.
pub const DEFAULT_GAME_INTENSITY: u8 = 100;
/// Default screen template: layout G, gear and speed. Mirrors the daemon.
pub const DEFAULT_SCREEN_TEMPLATE: &str = "G|{gear}|{speed}";
/// Default pitch scale (percent of the crank rate).
///
/// 35, matching the daemon, which is the only thing that makes this correct:
/// this read 50 against a daemon on 25 for a while, so a fresh install's Rev
/// rate slider said 50% while the daemon ran at 25%, and nudging the slider
/// and putting it back silently doubled it. Pinned against the daemon by
/// tf-sim's `frontend_compat` test; see the daemon's `Config::default` for
/// why the value is 35.
pub const DEFAULT_PITCH: u8 = 35;

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


/// One haptic layer of the daemon's effects engine.
///
/// Mirrored, not shared, with `logi_tf_sim::effects::EffectId` for the same
/// reason the rest of this module mirrors the daemon's config: the crates
/// cannot link (see the module doc). The `frontend_compat` integration test
/// in the daemon's crate is what keeps the two lists from drifting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// The config key suffix, as in `effect_rev_limiter`.
    pub key: &'static str,
    /// What to call it in a user interface.
    pub label: &'static str,
    /// One line on what it feels like.
    pub blurb: &'static str,
    /// Percent gain when nothing has been configured.
    pub default_gain: u8,
    /// Whether any game the daemon reads supplies this layer's input today.
    ///
    /// A layer with nothing feeding it is silent, not broken: the effect is
    /// implemented and the missing piece is a decoder field.
    pub fed: bool,
    /// The caveat to show beside this layer, or "" when there is none.
    ///
    /// [`fed`](Effect::fed) alone is too coarse to be honest. Most games send
    /// only engine speed, redline, throttle and road speed; the gear, the
    /// pedals and the ABS and traction lamps come from OutGauge, which among
    /// the games here means BeamNG. So a layer can be genuinely implemented
    /// and genuinely fed, and still do nothing at all in the game the person
    /// reading the slider actually plays. Saying which is the difference
    /// between a control and a puzzle.
    pub note: &'static str,
}

/// Every layer, in the order a user interface should present them.
///
/// Deliberately not the daemon's mix order: this runs from the layer that is
/// always there to the ones that only fire on an event, which is the order
/// somebody tuning the mix reads it in.
pub const EFFECTS: &[Effect] = &[
    Effect {
        key: "engine",
        label: "Engine",
        blurb: "The engine note itself, rising and falling with the revs.",
        default_gain: 100,
        fed: true,
        note: "",
    },
    Effect {
        key: "rev_limiter",
        label: "Rev limiter",
        blurb: "The hard chop of an engine sitting against its limiter.",
        default_gain: 70,
        fed: true,
        note: "",
    },
    Effect {
        key: "pit_limiter",
        label: "Pit limiter",
        blurb: "A slower pulse while the pit-lane speed limiter is engaged.",
        default_gain: 50,
        fed: true,
        note: "Only BeamNG sends this today; silent in the other games.",
    },
    Effect {
        key: "gear_shift",
        label: "Gear shifts",
        blurb: "A thump through the drivetrain as the gear changes.",
        default_gain: 60,
        fed: true,
        note: "Only BeamNG sends this today; silent in the other games.",
    },
    Effect {
        key: "abs",
        label: "ABS",
        blurb: "The pulsing of the ABS pump under heavy braking.",
        default_gain: 60,
        fed: true,
        note: "Only BeamNG sends this today; silent in the other games.",
    },
    Effect {
        key: "traction_loss",
        label: "Traction loss",
        blurb: "A buzz as the driven wheels start to let go.",
        default_gain: 50,
        fed: true,
        note: "Only BeamNG sends this today; silent in the other games.",
    },
    Effect {
        key: "road_bumps",
        label: "Road surface",
        blurb: "Texture from the road, rising with speed.",
        default_gain: 40,
        fed: false,
        note: "No game we read sends this yet, so it stays silent.",
    },
    Effect {
        key: "airborne",
        label: "Airborne",
        blurb: "How far the road quiets with the wheels off the ground.",
        default_gain: 85,
        fed: false,
        note: "No game we read sends this yet, so it stays silent.",
    },
    Effect {
        key: "collision",
        label: "Impacts",
        blurb: "A hit when the car strikes something.",
        default_gain: 80,
        fed: false,
        note: "No game we read sends this yet, so it stays silent.",
    },
    Effect {
        key: "drs",
        label: "DRS",
        blurb: "A tick as a drag-reduction wing opens or closes.",
        default_gain: 40,
        fed: false,
        note: "No game we read sends this yet, so it stays silent.",
    },
];

/// Look up a layer by its config key.
pub fn effect_by_key(key: &str) -> Option<&'static Effect> {
    EFFECTS.iter().find(|e| e.key == key)
}

/// Per-layer gains, in percent, parallel to [`EFFECTS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectGains {
    gains: [u8; EFFECTS.len()],
}

impl Default for EffectGains {
    fn default() -> Self {
        let mut gains = [0u8; EFFECTS.len()];
        for (slot, effect) in gains.iter_mut().zip(EFFECTS) {
            *slot = effect.default_gain;
        }
        EffectGains { gains }
    }
}

impl EffectGains {
    /// Gain for `key`, or that layer's default if the key is unknown.
    pub fn get(&self, key: &str) -> u8 {
        match EFFECTS.iter().position(|e| e.key == key) {
            Some(i) => self.gains[i],
            None => 0,
        }
    }

    /// Set `key`'s gain, clamped to 100. Unknown keys are ignored.
    pub fn set(&mut self, key: &str, pct: u8) {
        if let Some(i) = EFFECTS.iter().position(|e| e.key == key) {
            self.gains[i] = pct.min(100);
        }
    }
}

/// Which attached wheel simulated TrueForce should drive.
///
/// Exists because "the wheel" stops being a single thing the moment two are
/// plugged in. The daemon prefers a G923 whenever it finds one, which is
/// right for one wheel and leaves a direct-drive wheel unreachable on a rig
/// with both. Before this the only way out was an environment variable,
/// which nobody discovers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WheelChoice {
    /// Whatever the daemon finds, preferring a G923. The right answer with
    /// one wheel attached, and the reason this is the default.
    #[default]
    Auto,
    /// A direct-drive wheel: RS50 or G PRO.
    DirectDrive,
    /// A G923.
    G923,
}

impl WheelChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            WheelChoice::Auto => "auto",
            WheelChoice::DirectDrive => "dd",
            WheelChoice::G923 => "g923",
        }
    }

    /// Parse a config value. Accepts the wheel names people actually type
    /// as well as the stored spellings, because "rs50" in a config file
    /// silently meaning "auto" is worse than any parsing strictness.
    pub fn parse(raw: &str) -> Option<WheelChoice> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(WheelChoice::Auto),
            "dd" | "rs50" | "gpro" | "g pro" | "g-pro" | "direct-drive" | "directdrive" => {
                Some(WheelChoice::DirectDrive)
            }
            "g923" | "923" => Some(WheelChoice::G923),
            _ => None,
        }
    }

    /// The label the apps show.
    pub fn label(self) -> &'static str {
        match self {
            WheelChoice::Auto => "Automatic",
            WheelChoice::DirectDrive => "Direct drive (RS50 / G PRO)",
            WheelChoice::G923 => "G923",
        }
    }

    /// Every choice, in the order a picker should list them.
    pub const ALL: [WheelChoice; 3] =
        [WheelChoice::Auto, WheelChoice::DirectDrive, WheelChoice::G923];
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
    /// Which attached wheel to drive; see [`WheelChoice`].
    pub wheel: WheelChoice,
    /// Whether the daemon also drives the wheel's rev display
    /// (`wheel_rev_level`) from telemetry RPM while streaming.
    pub leds: bool,
    /// Drive the base's screen from telemetry while a session runs.
    pub screen: bool,
    /// The `wheel_oled` frame template with telemetry placeholders.
    pub screen_template: String,
    /// Whether the haptic layers beyond the engine note are mixed in.
    pub effects: bool,
    /// Per-layer gain; see [`EFFECTS`].
    pub effect_gains: EffectGains,
    /// Per-game overrides, keyed by tf-sim game id.
    pub games: BTreeMap<String, GameConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            intensity: DEFAULT_INTENSITY,
            wheel: WheelChoice::default(),
            pitch_pct: DEFAULT_PITCH,
            leds: true,
            screen: false,
            screen_template: DEFAULT_SCREEN_TEMPLATE.to_string(),
            effects: true,
            effect_gains: EffectGains::default(),
            games: BTreeMap::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/logi-wheel/tf-sim.conf`, falling back to
/// `~/.config/logi-wheel/tf-sim.conf` when the variable is unset or empty.
/// Same resolution as tf-sim's own `default_path`.
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
/// of losing anything. Mirrored, not shared, in tf-sim's own `default_path`
/// (see the module doc for why the crates cannot link).
pub fn default_path() -> PathBuf {
    resolve_path_in(&crate::profiles::config_root())
}

/// `<root>/logi-wheel/tf-sim.conf` if it exists, else a one-time copy of
/// `<root>/logi-dd/tf-sim.conf` (when that exists) followed by the new
/// path, else the new path outright (fresh install, nothing to migrate).
/// Split out from [`default_path`] as a pure function of `root` (no
/// environment access) so the migration behavior is testable without
/// touching `XDG_CONFIG_HOME` - [`crate::profiles`] has its own equivalent
/// split for the same reason.
fn resolve_path_in(root: &Path) -> PathBuf {
    let new_path = root.join("logi-wheel").join(FILE_NAME);
    if new_path.is_file() {
        return new_path;
    }
    let old_path = root.join("logi-dd").join(FILE_NAME);
    if old_path.is_file() {
        if migrate_file(&old_path, &new_path).is_ok() {
            return new_path;
        }
        return old_path;
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
                "wheel" => {
                    if let Some(v) = WheelChoice::parse(raw) {
                        cfg.wheel = v;
                    }
                }
                "leds" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.leds = v;
                    }
                }
                "screen" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.screen = v;
                    }
                }
                "screen.template" => {
                    let t = raw.trim();
                    if !t.is_empty() && t.len() <= 96 {
                        cfg.screen_template = t.to_string();
                    }
                }
                "effects" => {
                    if let Some(v) = parse_bool(raw) {
                        cfg.effects = v;
                    }
                }
                _ => {
                    if let Some(name) = key.strip_prefix("effect_") {
                        if let Some(v) = parse_percent(raw) {
                            cfg.effect_gains.set(name, v);
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

    /// The effective per-game state for `id` (the stored override, or the
    /// defaults for an unlisted game, exactly like the daemon treats it).
    pub fn game(&self, id: &str) -> GameConfig {
        self.games.get(id).copied().unwrap_or_default()
    }
}

/// Whether the DAEMON's parser would consume this `key=value` pair.
///
/// Deliberately wider than [`Config::load_from`] above: the front-ends
/// only model the keys they edit and treat the rest (`port.*`,
/// `cylinders`, `g923.ffb_invert`) as opaque, but "opaque to the apps" is
/// not "unrecognised", and a warning that flagged every port line would
/// cry wolf. So this mirrors the daemon's full grammar, the same way the
/// rest of this module mirrors its config (the crates cannot link; see
/// the module doc), and the daemon's `frontend_compat` test pins the two
/// against each other.
fn daemon_recognises(key: &str, raw: &str) -> bool {
    match key {
        "enabled"
        | "screen"
        | "leds"
        | "effects"
        | "follow_game_gain"
        | "g923.ffb_invert"
        | "g923.stream_without_ffb_mirror" => parse_bool(raw).is_some(),
        "intensity" => parse_percent(raw).is_some(),
        "wheel" => WheelChoice::parse(raw).is_some(),
        "screen.template" => !raw.trim().is_empty() && raw.trim().len() <= 96,
        "pitch" => raw.parse::<u8>().is_ok_and(|v| (10..=200u16).contains(&u16::from(v))),
        "cylinders" => raw.parse::<u8>().is_ok_and(|v| (1..=16).contains(&v)),
        "port.codemasters" | "port.pcars" | "port.beamng" | "port.relay" => {
            raw.parse::<u16>().is_ok()
        }
        _ => {
            if let Some(name) = key.strip_prefix("effect_") {
                return effect_by_key(name).is_some() && parse_percent(raw).is_some();
            }
            let Some(rest) = key.strip_prefix("game.") else { return false };
            let Some((id, field)) = rest.rsplit_once('.') else { return false };
            if id.is_empty() {
                return false;
            }
            match field {
                "enabled" => parse_bool(raw).is_some(),
                "intensity" => parse_percent(raw).is_some(),
                _ => false,
            }
        }
    }
}

/// The non-comment lines in the file at `path` that the daemon's parser
/// would skip (unknown keys and unparsable or out-of-range values alike):
/// the count and the first such line's text. A missing or unreadable file
/// is `(0, None)`, the daemon's own answer for it.
pub fn unrecognised_lines(path: &Path) -> (usize, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else { return (0, None) };
    let mut skipped = 0;
    let mut first = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let used = line
            .split_once('=')
            .is_some_and(|(key, raw)| daemon_recognises(key.trim(), raw.trim()));
        if !used {
            skipped += 1;
            if first.is_none() {
                first = Some(line.to_string());
            }
        }
    }
    (skipped, first)
}

/// The warning the Setup pages show beside the Simulated TrueForce
/// section, or `None` while every line of the file parses. The same text
/// the daemon logs at its own startup, so the app and the log name one
/// problem one way.
pub fn conf_warning(path: &Path) -> Option<String> {
    let (skipped, first) = unrecognised_lines(path);
    let first = first?;
    Some(format!(
        "{skipped} unrecognised line{} in {FILE_NAME}, first: {first}",
        if skipped == 1 { "" } else { "s" }
    ))
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
/// Which wheel simulated TrueForce should drive.
pub fn set_wheel_in(path: &Path, wheel: WheelChoice) -> Result<(), Error> {
    write_key_in(path, "wheel", wheel.as_str())
}

pub fn set_leds_in(path: &Path, leds: bool) -> Result<(), Error> {
    write_key_in(path, "leds", if leds { "1" } else { "0" })
}

/// Write the screen switch.
pub fn set_screen_in(path: &Path, on: bool) -> Result<(), Error> {
    write_key_in(path, "screen", if on { "1" } else { "0" })
}

/// Write the screen template (trimmed; empty or over 96 bytes is refused).
pub fn set_screen_template_in(path: &Path, template: &str) -> Result<(), Error> {
    let t = template.trim();
    if t.is_empty() || t.len() > 96 {
        return Err(Error::Invalid);
    }
    write_key_in(path, "screen.template", t)
}

/// Write one game's enable switch.
/// Turn the whole effects layer on or off, leaving each layer's own gain
/// as it was so that switching back restores the tuned mix.
pub fn set_effects_in(path: &Path, effects: bool) -> Result<(), Error> {
    write_key_in(path, "effects", if effects { "1" } else { "0" })
}

/// Set one layer's gain. An unknown key is rejected rather than written, so
/// a typo cannot leave a dead `effect_*` line in the user's file.
pub fn set_effect_gain_in(path: &Path, key: &str, pct: u8) -> Result<(), Error> {
    if effect_by_key(key).is_none() {
        return Ok(());
    }
    write_key_in(path, &format!("effect_{key}"), &pct.min(100).to_string())
}

pub fn set_game_enabled_in(path: &Path, id: &str, enabled: bool) -> Result<(), Error> {
    write_key_in(path, &format!("game.{id}.enabled"), if enabled { "1" } else { "0" })
}

/// Write one game's intensity (clamped to 0-100).
pub fn set_game_intensity_in(path: &Path, id: &str, intensity: u8) -> Result<(), Error> {
    write_key_in(path, &format!("game.{id}.intensity"), &intensity.min(100).to_string())
}

/// The per-game ids the daemon's telemetry parsers actually emit, as
/// documented in tf-sim's own `--help` (`dirt-rally-2` and the `codemasters`
/// family from the Codemasters classic parser, `ams2-pcars2` from the
/// Project CARS 2 / Automobilista 2 parser, `f1` from the modern F1 parser,
/// `beamng` from the OutGauge parser, `ea-wrc` from the WRC parser). A
/// front-end's live per-game cell must key off one of these: `game.<id>.*`
/// for anything else is a key the daemon would never read. Mirrored here
/// because the front-ends cannot link the tf-sim crate (a GPL-2.0-only /
/// GPL-3.0-or-later boundary), same reason [`game_id_for_title`] hardcodes
/// the ids.
pub const DAEMON_GAME_IDS: &[&str] = &[
    "dirt-rally-2",
    "codemasters",
    "ams2-pcars2",
    "f1",
    "beamng",
    "ea-wrc",
    // Shared-memory and plugin sources, each carrying its own id on the
    // relay wire so a truck sim and a GT car do not share one intensity.
    // `relay` remains the fallback for a sender this build does not know.
    "ets2",
    "ats",
    "iracing",
    "raceroom",
    "assetto",
    "acc",
    "ac-evo",
    "lmu",
    "rf2",
    "relay",
];

/// The tf-sim game id for a games-list title, or `None` when the daemon
/// has no per-game id for it. Deliberately conservative: only titles whose
/// ids actually exist in the daemon's telemetry detection map here
/// (matching is case-insensitive but otherwise exact, so "DiRT Rally 2.0"
/// from Steam and "Dirt Rally 2.0" from the compatibility tables both
/// match while remasters or sequels never do). DiRT 4 rides the
/// `codemasters` family id: its packets are not the DR2 signature, so the
/// daemon's classic parser reports the family id for it. The four modern F1
/// titles (F1 22-25) all speak the same versioned format, so each maps to
/// the one `f1` id.
pub fn game_id_for_title(title: &str) -> Option<&'static str> {
    match title.trim().to_lowercase().as_str() {
        "dirt rally 2.0" => Some("dirt-rally-2"),
        "dirt 4" => Some("codemasters"),
        "automobilista 2" | "project cars 2" => Some("ams2-pcars2"),
        "beamng.drive" => Some("beamng"),
        "f1 22" | "f1 23" | "f1 24" | "f1 25" => Some("f1"),
        "ea sports wrc" | "ea sports™ wrc" => Some("ea-wrc"),
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
                "logi-wheel-tfsim-test-{}-{}",
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

    /// Fresh install: neither directory exists yet. [`resolve_path_in`]
    /// returns the new path outright and never even creates a `logi-dd`
    /// directory to check.
    #[test]
    fn resolve_path_in_fresh_install_is_the_new_path_untouched() {
        let tree = TempTree::new();
        let resolved = resolve_path_in(tree.path());
        assert_eq!(resolved, tree.path().join("logi-wheel").join(FILE_NAME));
        assert!(!resolved.exists(), "nothing to migrate, nothing created");
        assert!(!tree.path().join("logi-dd").exists(), "the old directory was never touched");
    }

    /// Old-only: a pre-rename install with no new file yet. The old file is
    /// copied to the new location, the resolved path is the new one, and
    /// the original is left in place as a safety net.
    #[test]
    fn resolve_path_in_migrates_the_old_file_once() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.path().join("logi-dd")).unwrap();
        fs::write(tree.path().join("logi-dd").join(FILE_NAME), "intensity=22\n").unwrap();

        let resolved = resolve_path_in(tree.path());
        assert_eq!(resolved, tree.path().join("logi-wheel").join(FILE_NAME), "the new path wins after migrating");
        assert_eq!(Config::load_from(&resolved).intensity, 22);
        assert_eq!(
            fs::read_to_string(tree.path().join("logi-dd").join(FILE_NAME)).unwrap(),
            "intensity=22\n",
            "the original is left in place untouched"
        );

        // A second resolution finds the new file directly and does not
        // need to migrate again.
        assert_eq!(resolve_path_in(tree.path()), tree.path().join("logi-wheel").join(FILE_NAME));
    }

    /// Both exist: the new file wins outright, and the old one is left
    /// completely alone (no copy is even attempted).
    #[test]
    fn resolve_path_in_prefers_the_new_file_when_both_exist() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.path().join("logi-dd")).unwrap();
        fs::write(tree.path().join("logi-dd").join(FILE_NAME), "intensity=33\n").unwrap();
        fs::create_dir_all(tree.path().join("logi-wheel")).unwrap();
        fs::write(tree.path().join("logi-wheel").join(FILE_NAME), "intensity=44\n").unwrap();

        let resolved = resolve_path_in(tree.path());
        assert_eq!(resolved, tree.path().join("logi-wheel").join(FILE_NAME));
        assert_eq!(Config::load_from(&resolved).intensity, 44, "the new file wins");
        assert_eq!(
            fs::read_to_string(tree.path().join("logi-dd").join(FILE_NAME)).unwrap(),
            "intensity=33\n",
            "the old file is untouched"
        );
    }

    /// Migration failure (the new location cannot be created, simulated
    /// cheaply by putting a plain file where the `logi-wheel` directory
    /// needs to go): resolution still falls back to the old path rather
    /// than panicking or losing the original.
    #[test]
    fn resolve_path_in_falls_back_to_the_old_file_when_migration_fails() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.path().join("logi-dd")).unwrap();
        fs::write(tree.path().join("logi-dd").join(FILE_NAME), "intensity=55\n").unwrap();
        // Block `<root>/logi-wheel` from ever becoming a directory.
        fs::write(tree.path().join("logi-wheel"), "not a directory").unwrap();

        let resolved = resolve_path_in(tree.path());
        assert_eq!(resolved, tree.path().join("logi-dd").join(FILE_NAME), "falls back to the old file");
        assert_eq!(Config::load_from(&resolved).intensity, 55);
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

    /// The warning scan must speak the DAEMON's grammar, not the
    /// front-ends' subset: the keys the apps treat as opaque (`port.*`,
    /// `cylinders`, `g923.ffb_invert`) are all recognised, while a genuine
    /// typo is counted and quoted. Pinned against the daemon's own count
    /// by its `frontend_compat` test.
    #[test]
    fn unrecognised_scan_accepts_the_daemons_full_grammar() {
        let tree = TempTree::new();
        let path = tree.path().join(FILE_NAME);
        fs::write(
            &path,
            format!(
                "{FILE_HEADER}\n\
                 enabled=1\nintensity=30\npitch=35\ncylinders=8\nwheel=dd\n\
                 leds=1\neffects=0\neffect_engine=90\n\
                 port.codemasters=20777\nport.pcars=5606\nport.beamng=4444\nport.relay=20780\n\
                 g923.ffb_invert=1\ngame.f1.enabled=0\ngame.f1.intensity=70\n"
            ),
        )
        .unwrap();
        assert_eq!(unrecognised_lines(&path), (0, None), "every daemon key is recognised");
        assert_eq!(conf_warning(&path), None);

        fs::write(
            &path,
            format!("{FILE_HEADER}\nintensty=80\npitch=5\ngame.f1.bogus=1\nleds=1\n"),
        )
        .unwrap();
        assert_eq!(
            unrecognised_lines(&path),
            (3, Some("intensty=80".to_string())),
            "typos, out-of-range values and unknown per-game fields all count"
        );
        assert_eq!(
            conf_warning(&path).as_deref(),
            Some("3 unrecognised lines in tf-sim.conf, first: intensty=80")
        );

        assert_eq!(unrecognised_lines(Path::new("/nonexistent-tf-sim.conf")), (0, None));
        assert_eq!(conf_warning(Path::new("/nonexistent-tf-sim.conf")), None);
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
        assert_eq!(game_id_for_title("DiRT 4"), Some("codemasters"), "family id via classic parser");
        assert_eq!(game_id_for_title("BeamNG.drive"), Some("beamng"));
        assert_eq!(game_id_for_title("F1 24"), Some("f1"));
        assert_eq!(game_id_for_title("F1 22"), Some("f1"), "every modern F1 shares one id");
        assert_eq!(game_id_for_title("EA SPORTS WRC"), Some("ea-wrc"), "Steam's casing");
        assert_eq!(game_id_for_title("DiRT Rally"), None, "predecessor never matches");
        assert_eq!(game_id_for_title("F1 2021"), None, "the legacy-format titles never match");
        assert_eq!(game_id_for_title("Le Mans Ultimate"), None);
        assert_eq!(game_id_for_title(""), None);
        // Every id the mapping can produce must be one the daemon reads.
        for title in [
            "Dirt Rally 2.0", "DiRT 4", "Automobilista 2", "Project CARS 2", "BeamNG.drive",
            "F1 24", "EA SPORTS WRC",
        ] {
            let id = game_id_for_title(title).unwrap();
            assert!(DAEMON_GAME_IDS.contains(&id), "{title} maps to unknown id {id}");
        }
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

#[cfg(test)]
mod wheel_choice_tests {
    use super::*;

    #[test]
    fn accepts_what_people_actually_type() {
        // The stored spellings.
        assert_eq!(WheelChoice::parse("auto"), Some(WheelChoice::Auto));
        assert_eq!(WheelChoice::parse("dd"), Some(WheelChoice::DirectDrive));
        assert_eq!(WheelChoice::parse("g923"), Some(WheelChoice::G923));
        // The wheel names someone would reasonably write instead, because a
        // config that silently means "auto" is worse than being lenient.
        assert_eq!(WheelChoice::parse("RS50"), Some(WheelChoice::DirectDrive));
        assert_eq!(WheelChoice::parse("G PRO"), Some(WheelChoice::DirectDrive));
        assert_eq!(WheelChoice::parse(" G923 "), Some(WheelChoice::G923));
        // Empty means unset, which is auto.
        assert_eq!(WheelChoice::parse(""), Some(WheelChoice::Auto));
        // Nonsense is rejected rather than silently becoming a default, so
        // the parser leaves the previous value alone.
        assert_eq!(WheelChoice::parse("g924"), None);
    }

    #[test]
    fn round_trips_through_the_stored_form() {
        for c in WheelChoice::ALL {
            assert_eq!(WheelChoice::parse(c.as_str()), Some(c));
            assert!(!c.label().is_empty());
        }
    }
}
