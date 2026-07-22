//! Unified game-discovery data model. Each launcher backend (Steam, Lutris,
//! Heroic) scans its own install for games and reports them as
//! [`DiscoveredGame`]s; an aggregator merges the backends' results for the
//! Setup pages, which offer a shim install for any Wine game.

pub mod heroic;
pub mod lutris;
pub mod steam;

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Which launcher (or manual entry) reported a [`DiscoveredGame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Steam,
    Lutris,
    Heroic,
    Manual,
}

impl Source {
    /// The label the front-ends show next to a discovered game.
    pub fn label(self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Lutris => "Lutris",
            Source::Heroic => "Heroic",
            Source::Manual => "Manual",
        }
    }
}

/// How a game runs: a native Linux build, or a Windows build under Wine in
/// the given prefix (the shim installer's target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameKind {
    Native,
    Wine { prefix: PathBuf },
}

/// One game found by a launcher backend (or entered manually), with enough
/// information for the Setup pages to offer a shim install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredGame {
    pub name: String,
    pub source: Source,
    pub kind: GameKind,
    /// Whether the TrueForce SDK shim's marker DLL is present in the game's
    /// wine prefix. Always `false` for [`GameKind::Native`]: the shim is
    /// Wine-only, there is no prefix to install it into.
    pub shim_installed: bool,
}

impl DiscoveredGame {
    /// The wine prefix backing this game, or `None` for a native game.
    pub fn prefix(&self) -> Option<&Path> {
        match &self.kind {
            GameKind::Wine { prefix } => Some(prefix),
            GameKind::Native => None,
        }
    }
}

/// Discover installed games across all launchers. `home` is $HOME: Steam's
/// libraries come from [`crate::steam::library_roots`], and Lutris/Heroic
/// read from `$XDG_CONFIG_HOME` (falling back to `home/.config`). Results
/// are deduped and sorted for a stable "Your games" list; see
/// [`dedupe_and_sort`].
pub fn discover(home: &Path) -> Vec<DiscoveredGame> {
    discover_in(home, &config_dir(home))
}

/// [`discover`]'s aggregation, with the launcher config directory passed in
/// explicitly instead of resolved from the environment, so a fixture home
/// is not at the mercy of the real process's `$XDG_CONFIG_HOME`.
fn discover_in(home: &Path, config: &Path) -> Vec<DiscoveredGame> {
    let mut found = steam::steam_games(&crate::steam::library_roots(home));
    found.extend(lutris::lutris_games(config));
    found.extend(heroic::heroic_games(config));
    dedupe_and_sort(found)
}

/// Collapse duplicate discoveries and order the rest for display.
///
/// Entries are grouped by [`crate::games::normalize_title`] so the same game
/// reported by two launchers (or twice by one) lands in the same group.
/// Within a group: if any entry is [`GameKind::Wine`], every native entry is
/// dropped (the shim is only actionable on Wine, so a native duplicate adds
/// nothing); remaining entries are then deduped by their prefix (`None` for
/// native, so all native entries in a group collapse to one). The result is
/// sorted by lowercased name, then by source label, for a stable order.
fn dedupe_and_sort(games: Vec<DiscoveredGame>) -> Vec<DiscoveredGame> {
    let mut by_title: HashMap<String, Vec<DiscoveredGame>> = HashMap::new();
    for game in games {
        by_title.entry(crate::games::normalize_title(&game.name)).or_default().push(game);
    }

    let mut out = Vec::new();
    for mut entries in by_title.into_values() {
        if entries.iter().any(|g| matches!(g.kind, GameKind::Wine { .. })) {
            entries.retain(|g| matches!(g.kind, GameKind::Wine { .. }));
        }
        let mut seen_prefixes: HashSet<Option<PathBuf>> = HashSet::new();
        entries.retain(|g| seen_prefixes.insert(g.prefix().map(Path::to_path_buf)));
        out.extend(entries);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.source.label().cmp(b.source.label())));
    out
}

/// Whether the compat registry recognises this title (see
/// [`crate::games::match_title`]), for the Setup pages' "known game" badge.
pub fn is_recognized(name: &str) -> bool {
    crate::games::match_title(name).is_some()
}

/// `$XDG_CONFIG_HOME` if set and non-empty, else `home/.config`.
fn config_dir(home: &Path) -> PathBuf {
    config_dir_in(home, std::env::var_os("XDG_CONFIG_HOME").as_deref())
}

/// [`config_dir`]'s logic with the environment variable passed in
/// explicitly, so it is unit-tested without reading (or racing) the real
/// process environment. Mirrors [`crate::steam::resolve_sdk_dir_in`].
fn config_dir_in(home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf {
    xdg_config_home.filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (mirrors `steam.rs`).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-launchers-mod-test-{}-{}",
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

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn steam_manifest(appid: u32, name: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"4\"\n}}\n"
        )
    }

    #[test]
    fn is_recognized_matches_the_compat_registry() {
        assert!(is_recognized("Assetto Corsa Competizione"));
        assert!(!is_recognized("TEKKEN 8"));
    }

    #[test]
    fn config_dir_in_prefers_a_nonempty_xdg_override() {
        let home = Path::new("/home/x");
        assert_eq!(config_dir_in(home, Some(OsStr::new("/custom/config"))), PathBuf::from("/custom/config"));
    }

    #[test]
    fn config_dir_in_falls_back_to_dot_config_when_unset_or_empty() {
        let home = Path::new("/home/x");
        assert_eq!(config_dir_in(home, None), home.join(".config"));
        assert_eq!(config_dir_in(home, Some(OsStr::new(""))), home.join(".config"));
    }

    #[test]
    fn discover_aggregates_steam_and_lutris_sorted() {
        let tree = TempTree::new();
        let home = tree.path().join("home");
        let steam_root = home.join(".local").join("share").join("Steam");
        write(&steam_root.join("steamapps").join("appmanifest_500.acf"), &steam_manifest(500, "Euro Truck Simulator 2"));

        let config = home.join(".config");
        write(&config.join("lutris").join("games").join("dirt-100.yml"), "name: DiRT Rally 2.0\ngame:\n  exe: /games/dirt\n");

        let games = discover_in(&home, &config);
        let names: Vec<&str> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["DiRT Rally 2.0", "Euro Truck Simulator 2"], "sorted by lowercased name");

        let dirt = &games[0];
        assert_eq!(dirt.source, Source::Lutris);
        assert_eq!(dirt.kind, GameKind::Native);

        let ets2 = &games[1];
        assert_eq!(ets2.source, Source::Steam);
        assert_eq!(ets2.kind, GameKind::Native);
    }

    #[test]
    fn discover_prefers_a_wine_entry_over_a_native_duplicate() {
        let tree = TempTree::new();
        let home = tree.path().join("home");
        let steam_root = home.join(".local").join("share").join("Steam");
        write(&steam_root.join("steamapps").join("appmanifest_600.acf"), &steam_manifest(600, "Wreckfest"));

        let config = home.join(".config");
        let pfx = tree.path().join("pfx-wreckfest");
        write(
            &config.join("lutris").join("games").join("wreckfest-1600000000.yml"),
            &format!("name: Wreckfest\ngame:\n  prefix: {}\n", pfx.display()),
        );

        let games = discover_in(&home, &config);
        assert_eq!(games.len(), 1, "the native duplicate is dropped: {games:?}");
        assert_eq!(games[0].kind, GameKind::Wine { prefix: pfx });
        assert_eq!(games[0].source, Source::Lutris);
    }
}
