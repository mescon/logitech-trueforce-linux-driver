//! Steam library discovery for the front-ends' Setup pages: which Proton
//! games are installed, and whether the TrueForce SDK shim is present in
//! each game's wine prefix. Pure `std::fs` line-scraping of Steam's VDF
//! files (no VDF parser dependency), so everything here is unit-testable
//! against a plain fixture tree.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The marker file the shim installer drops into a prefix, relative to the
/// prefix root. Mirrors `tools/install-tf-shim.sh`'s `TF_PFX_DIR` layout.
const SHIM_MARKER: &str = "drive_c/Program Files/Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll";

/// The marker file a populated SDK directory must contain, relative to the
/// SDK directory root. Same layout Logitech ships on Windows and the same
/// marker `install-tf-shim.sh` checks (`SDK_MARKER` there).
const SDK_MARKER: &str = "Logi/Trueforce/1_3_11/trueforce_sdk_x64.dll";

/// One installed Proton game: its Steam appid and display name, the wine
/// prefix the shim installer targets (the `.../compatdata/<appid>/pfx`
/// directory), and whether the shim's marker DLL is present in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    pub prefix: PathBuf,
    pub shim_installed: bool,
}

/// Extract the value of a one-line VDF `"key" "value"` pair, or `None` when
/// `line` is not that key's line. Steam's manifests put each pair on its
/// own line, so a line scrape is enough; no escape handling (Linux paths
/// and game names do not need it in practice).
fn vdf_string<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix('"')?.strip_prefix(key)?.strip_prefix('"')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    rest.find('"').map(|end| &rest[..end])
}

/// Append `dir` to `roots` if it exists and was not seen before. Dedupe is
/// by canonical path, so the usual `~/.steam/steam` symlink to
/// `~/.local/share/Steam` (and a `libraryfolders.vdf` entry repeating the
/// main library) collapse to one root.
fn push_root(dir: PathBuf, roots: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    let key = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    if seen.insert(key) {
        roots.push(dir);
    }
}

/// Every Steam library root under `home`: the standard install locations
/// (`~/.steam/steam`, `~/.local/share/Steam`), plus every `"path"` entry in
/// those candidates' `steamapps/libraryfolders.vdf` (libraries the user
/// added on other drives). Deduped; only directories that exist are kept.
pub fn library_roots(home: &Path) -> Vec<PathBuf> {
    let candidates = [home.join(".steam").join("steam"), home.join(".local").join("share").join("Steam")];
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for candidate in &candidates {
        push_root(candidate.clone(), &mut roots, &mut seen);
        let vdf = candidate.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = fs::read_to_string(&vdf) {
            for line in text.lines() {
                if let Some(path) = vdf_string(line, "path") {
                    push_root(PathBuf::from(path), &mut roots, &mut seen);
                }
            }
        }
    }
    roots
}

/// Whether an app manifest's name is Steam tooling rather than a game
/// (Proton builds, the Steam Linux Runtime containers, redistributables):
/// none of them wants the shim, so the Setup pages hide them.
fn is_runtime_tooling(name: &str) -> bool {
    ["Proton", "Steam Linux Runtime", "Steamworks Common"].iter().any(|p| name.starts_with(p))
}

/// Every installed Proton game across `roots` (see [`library_roots`]),
/// sorted by name. A game qualifies when its `appmanifest_*.acf` parses and
/// its `steamapps/compatdata/<appid>/pfx` prefix exists (i.e. it runs, or
/// ran, under Proton); native Linux games and runtime tooling entries are
/// skipped. `shim_installed` reflects the shim's marker DLL in the prefix.
pub fn installed_games(roots: &[PathBuf]) -> Vec<SteamGame> {
    let mut games: Vec<SteamGame> = Vec::new();
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
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let mut appid = None;
            let mut name = None;
            for line in text.lines() {
                if appid.is_none() {
                    if let Some(v) = vdf_string(line, "appid") {
                        appid = v.parse::<u32>().ok();
                    }
                }
                if name.is_none() {
                    if let Some(v) = vdf_string(line, "name") {
                        name = Some(v.to_string());
                    }
                }
                if appid.is_some() && name.is_some() {
                    break;
                }
            }
            let (Some(appid), Some(name)) = (appid, name) else { continue };
            if is_runtime_tooling(&name) || !seen.insert(appid) {
                continue;
            }
            let prefix = steamapps.join("compatdata").join(appid.to_string()).join("pfx");
            if !prefix.is_dir() {
                continue;
            }
            let shim_installed = prefix.join(SHIM_MARKER).is_file();
            games.push(SteamGame { appid, name, prefix, shim_installed });
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.appid.cmp(&b.appid)));
    games
}

/// Whether `dir` holds a populated SDK tree (the marker DLL exists under
/// `<dir>/Logi/Trueforce/1_3_11/`). The Setup pages use this for the SDK
/// folder field's live validity indicator.
pub fn sdk_dir_valid(dir: &Path) -> bool {
    dir.join(SDK_MARKER).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-dd-steam-test-{}-{}",
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

    /// A home with the main library under `.local/share/Steam` whose
    /// `libraryfolders.vdf` lists itself (as Steam really does) plus a
    /// second library elsewhere. Games:
    ///   - 100 "Assetto Corsa Competizione": Proton prefix, shim installed
    ///   - 200 "Native Game": no compatdata prefix (native, excluded)
    ///   - 300 "Proton 9.0": prefix exists but runtime tooling (excluded)
    ///   - 400 "Le Mans Ultimate" (second library): Proton prefix, no shim
    fn fixture() -> (TempTree, PathBuf, PathBuf) {
        let tree = TempTree::new();
        let home = tree.path().join("home");
        let main = home.join(".local").join("share").join("Steam");
        let second = tree.path().join("library2");

        write(
            &main.join("steamapps").join("libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                main.display(),
                second.display()
            ),
        );

        write(&main.join("steamapps").join("appmanifest_100.acf"), &manifest(100, "Assetto Corsa Competizione"));
        write(&main.join("steamapps").join("appmanifest_200.acf"), &manifest(200, "Native Game"));
        write(&main.join("steamapps").join("appmanifest_300.acf"), &manifest(300, "Proton 9.0"));
        write(&second.join("steamapps").join("appmanifest_400.acf"), &manifest(400, "Le Mans Ultimate"));

        let acc_pfx = main.join("steamapps").join("compatdata").join("100").join("pfx");
        write(&acc_pfx.join(SHIM_MARKER), "dll");
        fs::create_dir_all(main.join("steamapps").join("compatdata").join("300").join("pfx")).unwrap();
        fs::create_dir_all(second.join("steamapps").join("compatdata").join("400").join("pfx")).unwrap();

        (tree, home, second)
    }

    #[test]
    fn library_roots_finds_both_and_dedupes_the_vdf_self_entry() {
        let (_tree, home, second) = fixture();
        let roots = library_roots(&home);
        assert_eq!(roots.len(), 2, "roots: {roots:?}");
        assert_eq!(roots[0], home.join(".local").join("share").join("Steam"));
        assert_eq!(roots[1], second);
    }

    #[test]
    fn library_roots_is_empty_without_a_steam_install() {
        let tree = TempTree::new();
        assert!(library_roots(tree.path()).is_empty());
    }

    #[test]
    fn installed_games_keeps_proton_games_and_flags_the_shim() {
        let (_tree, home, _second) = fixture();
        let games = installed_games(&library_roots(&home));
        let names: Vec<&str> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Assetto Corsa Competizione", "Le Mans Ultimate"], "sorted by name");

        let acc = &games[0];
        assert_eq!(acc.appid, 100);
        assert!(acc.shim_installed);
        assert!(acc.prefix.ends_with("steamapps/compatdata/100/pfx"));

        let lmu = &games[1];
        assert_eq!(lmu.appid, 400);
        assert!(!lmu.shim_installed);
    }

    #[test]
    fn installed_games_excludes_native_and_runtime_entries() {
        let (_tree, home, _second) = fixture();
        let games = installed_games(&library_roots(&home));
        assert!(!games.iter().any(|g| g.appid == 200), "native game (no prefix) excluded");
        assert!(!games.iter().any(|g| g.appid == 300), "Proton tooling excluded");
    }

    #[test]
    fn sdk_dir_valid_both_ways() {
        let tree = TempTree::new();
        let sdk = tree.path().join("sdk");
        assert!(!sdk_dir_valid(&sdk), "missing tree is invalid");
        write(&sdk.join(SDK_MARKER), "dll");
        assert!(sdk_dir_valid(&sdk), "marker DLL makes it valid");
    }

    #[test]
    fn vdf_string_matches_only_the_exact_key() {
        assert_eq!(vdf_string("\t\"appid\"\t\t\"244210\"", "appid"), Some("244210"));
        assert_eq!(vdf_string("\t\"name\"\t\t\"Le Mans Ultimate\"", "name"), Some("Le Mans Ultimate"));
        assert_eq!(vdf_string("\t\"appid\"\t\t\"244210\"", "name"), None);
        assert_eq!(vdf_string("\t\"installdir\"\t\"x\"", "dir"), None, "no substring matches");
        assert_eq!(vdf_string("not a pair", "appid"), None);
    }
}
