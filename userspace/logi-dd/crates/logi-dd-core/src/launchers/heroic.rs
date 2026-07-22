//! Heroic backend for the unified game-discovery model: every game Heroic
//! has an installed-games manifest for, as a [`DiscoveredGame`]. Heroic's
//! manifests and per-game configs are JSON; logi-dd-core is std-only, so
//! this line-scrapes the handful of keys it needs rather than pulling in
//! a JSON crate. Heroic games always run under Wine.

use super::{DiscoveredGame, GameKind, Source};
use crate::steam::SHIM_MARKER;
use std::fs;
use std::path::{Path, PathBuf};

/// The value of the first `"key": "value"` string field on a line, trimmed
/// of surrounding whitespace and quotes. `None` if the line has no such
/// field for `key`.
fn scrape_str_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = line.find(&needle)?;
    let after_colon = line[after_key + needle.len()..].find(':')? + after_key + needle.len() + 1;
    let rest = line[after_colon..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// One entry from an installed-games manifest: the launcher-internal id
/// paired with the display title, if both were found on (or near) the
/// same line.
struct Entry {
    app_name: String,
    title: String,
}

/// Scrape every `app_name`/`appName` + `title` pair out of an
/// installed-games manifest, tolerant of either key spelling, of both the
/// array-of-objects and object-of-objects shapes Heroic has used, and of
/// either field coming first within a game's block: each pair is emitted
/// as soon as both fields have been seen since the last pair.
fn scrape_entries(content: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pending_id: Option<String> = None;
    let mut pending_title: Option<String> = None;
    for line in content.lines() {
        if let Some(id) = scrape_str_field(line, "app_name").or_else(|| scrape_str_field(line, "appName")) {
            pending_id = Some(id);
        }
        if let Some(title) = scrape_str_field(line, "title") {
            pending_title = Some(title);
        }
        if pending_id.is_some() && pending_title.is_some() {
            entries.push(Entry { app_name: pending_id.take().unwrap(), title: pending_title.take().unwrap() });
        }
    }
    entries
}

/// Every game Heroic has an installed-games manifest for under
/// `config_home/heroic`, sorted by lowercased name. Checks both the
/// current store-cache layout (`store_cache/legendary_library.json`,
/// `store_cache/gog_library.json`) and the older per-store layout
/// (`legendary/installed.json`, `gog_store/installed.json`); whichever
/// files exist are scraped. Each installed game's wine prefix comes from
/// `GamesConfig/<app_name>.json`'s `winePrefix` field; a game whose
/// config file or `winePrefix` is missing is skipped, since a shim can
/// only be offered into a known prefix.
pub fn heroic_games(config_home: &Path) -> Vec<DiscoveredGame> {
    let heroic_dir = config_home.join("heroic");
    let manifests = [
        heroic_dir.join("store_cache").join("legendary_library.json"),
        heroic_dir.join("store_cache").join("gog_library.json"),
        heroic_dir.join("legendary").join("installed.json"),
        heroic_dir.join("gog_store").join("installed.json"),
    ];

    let mut games: Vec<DiscoveredGame> = Vec::new();
    for manifest in &manifests {
        let Ok(content) = fs::read_to_string(manifest) else { continue };
        for entry in scrape_entries(&content) {
            let config_path = heroic_dir.join("GamesConfig").join(format!("{}.json", entry.app_name));
            let Ok(config_content) = fs::read_to_string(&config_path) else { continue };
            let Some(prefix) = scrape_field(&config_content, "winePrefix") else { continue };
            let prefix = PathBuf::from(prefix);
            let shim_installed = prefix.join(SHIM_MARKER).is_file();
            games.push(DiscoveredGame {
                name: entry.title,
                source: Source::Heroic,
                kind: GameKind::Wine { prefix },
                shim_installed,
            });
        }
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

/// The value of the first line with a `key` string field (see
/// [`scrape_str_field`]).
fn scrape_field(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| scrape_str_field(line, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (mirrors `steam.rs`).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-launchers-heroic-test-{}-{}",
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
    fn heroic_games_reads_prefix_and_skips_missing_config() {
        let tree = TempTree::new();
        let config_home = tree.path().join("config");
        let heroic_dir = config_home.join("heroic");
        let prefix = tree.path().join("heroic-dirt4");

        write(
            &heroic_dir.join("gog_store").join("installed.json"),
            r#"[
  {
    "app_name": "1207659144",
    "title": "DiRT 4",
    "install_path": "/games/dirt4"
  },
  {
    "app_name": "9999999999",
    "title": "No Prefix Yet",
    "install_path": "/games/noprefix"
  }
]"#,
        );

        write(
            &heroic_dir.join("GamesConfig").join("1207659144.json"),
            &format!("{{\n  \"winePrefix\": \"{}\",\n  \"language\": \"en\"\n}}\n", prefix.display()),
        );
        // No GamesConfig file at all for "9999999999": it must be skipped.

        let games = heroic_games(&config_home);
        assert_eq!(games.len(), 1, "the game with no GamesConfig is skipped");

        let dirt = &games[0];
        assert_eq!(dirt.name, "DiRT 4");
        assert_eq!(dirt.source, Source::Heroic);
        assert_eq!(dirt.kind, GameKind::Wine { prefix: prefix.clone() });
        assert!(!dirt.shim_installed);
        assert_eq!(dirt.prefix(), Some(prefix.as_path()));
    }
}
