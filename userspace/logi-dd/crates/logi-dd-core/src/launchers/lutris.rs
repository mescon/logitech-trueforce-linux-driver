//! Lutris backend for the unified game-discovery model: every game Lutris
//! has a config file for, as a [`DiscoveredGame`]. Lutris config files are
//! YAML; logi-dd-core is std-only, so this line-scrapes the handful of keys
//! it needs rather than pulling in a YAML crate.

use super::{DiscoveredGame, GameKind, Source};
use crate::steam::SHIM_MARKER;
use std::fs;
use std::path::{Path, PathBuf};

/// The value of the first line matching `key:` (leading whitespace and a
/// single space after the colon both allowed), trimmed. `None` if no such
/// line exists.
fn scrape(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in content.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(&prefix) {
            let value = rest.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Derive a display name from a Lutris config file's stem when the file
/// itself has no `name:` line: strip a trailing `-<digits>` id (Lutris
/// suffixes slugs with a numeric id), then turn `-` and `_` into spaces.
/// E.g. `dirt-rally-2-0-1700000000` -> `dirt rally 2 0`.
fn name_from_stem(stem: &str) -> String {
    let trimmed = match stem.rfind('-') {
        Some(i) if stem[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < stem.len() => &stem[..i],
        _ => stem,
    };
    trimmed.replace(['-', '_'], " ")
}

/// Every game Lutris has a config file for under
/// `config_home/lutris/games/*.yml`, sorted by lowercased name. Each file's
/// `name:` line (if present) is the display name, else it is derived from
/// the file stem (see [`name_from_stem`]). A `prefix:` line makes the game
/// [`GameKind::Wine`], with `shim_installed` reflecting the shim's marker
/// DLL in that prefix; no `prefix:` line makes it [`GameKind::Native`].
pub fn lutris_games(config_home: &Path) -> Vec<DiscoveredGame> {
    let games_dir = config_home.join("lutris").join("games");
    let mut games: Vec<DiscoveredGame> = Vec::new();
    let Ok(entries) = fs::read_dir(&games_dir) else { return games };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yml") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else { continue };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let name = scrape(&content, "name").unwrap_or_else(|| name_from_stem(stem));
        let kind = match scrape(&content, "prefix") {
            Some(prefix) => GameKind::Wine { prefix: PathBuf::from(prefix) },
            None => GameKind::Native,
        };
        let shim_installed = match &kind {
            GameKind::Wine { prefix } => prefix.join(SHIM_MARKER).is_file(),
            GameKind::Native => false,
        };
        games.push(DiscoveredGame { name, source: Source::Lutris, kind, shim_installed });
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (mirrors `steam.rs`).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-launchers-lutris-test-{}-{}",
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

    #[test]
    fn lutris_games_reports_wine_and_native_kinds() {
        let tree = TempTree::new();
        let config_home = tree.path().join("config");
        let games_dir = config_home.join("lutris").join("games");
        let pfx_dirt = tree.path().join("pfx-dirt");
        fs::create_dir_all(&pfx_dirt).unwrap();

        // Stem-only file: no `name:` line, id suffix stripped, dashes
        // become spaces.
        write(
            &games_dir.join("dirt-rally-2-0-1700000000.yml"),
            &format!("game:\n  prefix: {}\nwine:\n  version: staging\n", pfx_dirt.display()),
        );

        // Named file: an explicit `name:` line wins over the stem.
        write(
            &games_dir.join("wreckfest-1600000000.yml"),
            "name: Wreckfest\ngame:\n  exe: /games/wreckfest/wreckfest\n",
        );

        let games = lutris_games(&config_home);
        let names: Vec<&str> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["dirt rally 2 0", "Wreckfest"], "sorted by lowercased name");

        let dirt = &games[0];
        assert_eq!(dirt.source, Source::Lutris);
        assert_eq!(dirt.kind, GameKind::Wine { prefix: pfx_dirt.clone() });
        assert!(!dirt.shim_installed);
        assert_eq!(dirt.prefix(), Some(pfx_dirt.as_path()));

        let wreckfest = &games[1];
        assert_eq!(wreckfest.source, Source::Lutris);
        assert_eq!(wreckfest.kind, GameKind::Native);
        assert!(!wreckfest.shim_installed);
        assert_eq!(wreckfest.prefix(), None);
    }

    #[test]
    fn lutris_slug_derived_name_matches_the_registry_row() {
        let tree = TempTree::new();
        let config_home = tree.path().join("config");
        let games_dir = config_home.join("lutris").join("games");
        let pfx = tree.path().join("pfx-dirt");
        fs::create_dir_all(&pfx).unwrap();

        write(&games_dir.join("dirt-rally-2-0-1700000000.yml"), &format!("game:\n  prefix: {}\n", pfx.display()));

        let games = lutris_games(&config_home);
        let dirt = games.iter().find(|g| g.kind != GameKind::Native).unwrap();
        assert_eq!(games::match_title(&dirt.name).map(|g| g.name), Some("DiRT Rally 2.0"));
    }
}
