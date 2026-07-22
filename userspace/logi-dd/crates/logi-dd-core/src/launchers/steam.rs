//! Steam backend for the unified game-discovery model: every installed
//! game across the Steam libraries, Proton or native, as a
//! [`DiscoveredGame`]. Reuses [`crate::steam`]'s VDF scraping and library
//! discovery; unlike [`crate::steam::installed_games`] (Proton-only, for
//! the shim installer's own listing), this also reports native games so
//! the aggregator can show a complete library.

use super::{DiscoveredGame, GameKind, Source};
use crate::steam::{is_runtime_tooling, parse_manifest, SHIM_MARKER};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Every installed Steam game across `roots` (see
/// [`crate::steam::library_roots`]), sorted by name. A game whose
/// `steamapps/compatdata/<appid>/pfx` prefix exists is reported as
/// [`GameKind::Wine`] with `shim_installed` reflecting the shim's marker
/// DLL in that prefix; a game with only an `appmanifest_*.acf` (no prefix)
/// is reported as [`GameKind::Native`]. Runtime tooling entries (Proton
/// builds, Steam Linux Runtime, redistributables) are skipped.
pub fn steam_games(roots: &[PathBuf]) -> Vec<DiscoveredGame> {
    let mut games: Vec<DiscoveredGame> = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let steamapps = root.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !fname.starts_with("appmanifest_") || !fname.ends_with(".acf") {
                continue;
            }
            let Some((appid, name)) = parse_manifest(&path) else { continue };
            if is_runtime_tooling(&name) || !seen.insert(appid) {
                continue;
            }
            let prefix = steamapps.join("compatdata").join(appid.to_string()).join("pfx");
            let kind = if prefix.is_dir() { GameKind::Wine { prefix } } else { GameKind::Native };
            let shim_installed = match &kind {
                GameKind::Wine { prefix } => prefix.join(SHIM_MARKER).is_file(),
                GameKind::Native => false,
            };
            games.push(DiscoveredGame { name, source: Source::Steam, kind, shim_installed });
        }
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (mirrors `steam.rs`).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-launchers-steam-test-{}-{}",
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

    fn manifest(appid: u32, name: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"4\"\n}}\n"
        )
    }

    /// A library with one Proton game (100, "Assetto Corsa Competizione",
    /// with a compatdata prefix) and one native game (500, "Euro Truck
    /// Simulator 2", no prefix at all).
    fn fixture() -> (TempTree, PathBuf) {
        let tree = TempTree::new();
        let root = tree.path().join("Steam");

        write(&root.join("steamapps").join("appmanifest_100.acf"), &manifest(100, "Assetto Corsa Competizione"));
        write(&root.join("steamapps").join("appmanifest_500.acf"), &manifest(500, "Euro Truck Simulator 2"));

        let acc_pfx = root.join("steamapps").join("compatdata").join("100").join("pfx");
        fs::create_dir_all(&acc_pfx).unwrap();

        (tree, root)
    }

    #[test]
    fn steam_games_reports_wine_and_native_kinds() {
        let (_tree, root) = fixture();
        let games = steam_games(std::slice::from_ref(&root));
        let names: Vec<&str> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Assetto Corsa Competizione", "Euro Truck Simulator 2"], "sorted by name");

        let acc = &games[0];
        assert_eq!(acc.source, Source::Steam);
        assert!(!acc.shim_installed);
        let expected_pfx = root.join("steamapps").join("compatdata").join("100").join("pfx");
        assert_eq!(acc.kind, GameKind::Wine { prefix: expected_pfx.clone() });
        assert_eq!(acc.prefix(), Some(expected_pfx.as_path()));

        let ets2 = &games[1];
        assert_eq!(ets2.source, Source::Steam);
        assert_eq!(ets2.kind, GameKind::Native);
        assert!(!ets2.shim_installed);
        assert_eq!(ets2.prefix(), None);
    }
}
