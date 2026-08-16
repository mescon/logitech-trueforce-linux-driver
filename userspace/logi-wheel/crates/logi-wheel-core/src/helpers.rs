//! Locating the game-helper executables the Setup pages manage: the
//! `logi-launch` wrapper, the `logi-ffb` DirectInput FFB proxy, the
//! `logi-tf-sim` daemon, the `logi-rpm-bridge` RPM feed and the TrueForce
//! SDK shim installer.
//!
//! All are searched on `$PATH` first (the packaged install), then in the
//! places a plain repo checkout puts them: the Rust binaries are built into
//! the same target directory as the running front-end, and the scripts and
//! C tools live in the checkout's `tools/` directory some levels above. The
//! resolution is pure over its inputs (the `PATH` value and the current
//! executable's path), so it is unit-testable against fixture trees; the
//! `*_path()` wrappers feed it the real process environment.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The FFB proxy's binary name.
pub const FFB_BIN: &str = "logi-ffb";

/// The simulated-TrueForce daemon's binary name.
pub const TF_SIM_BIN: &str = "logi-tf-sim";

/// The launch wrapper's packaged binary name. Every launch line the Setup
/// pages hand out is `logi-launch %command%`, so whether this resolves is
/// the first thing those pages should say about the games area.
pub const LAUNCH_BIN: &str = "logi-launch";

/// The RPM bridge's binary name: the small daemon that feeds relayed game
/// RPM to the driver's texture merge and rev lights. Started and stopped
/// per game by `logi-launch`.
pub const RPM_BRIDGE_BIN: &str = "logi-rpm-bridge";

/// The launch wrapper's path inside a repo checkout, relative to the
/// checkout root (it is packaged as `logi-launch`, but the repo keeps the
/// `.sh` suffix).
const REPO_LAUNCH: &str = "tools/logi-launch.sh";

/// The RPM bridge's path inside a repo checkout, relative to the checkout
/// root (`tools/Makefile` builds it next to its source).
const REPO_RPM_BRIDGE: &str = "tools/logi-rpm-bridge";

/// The shim installer's candidate names, in preferred order: the packaged
/// name, then the name it was packaged under before v0.22.0 (an app from
/// this release can still be sitting next to an older install), then the
/// repo script's own name, since some setups put `tools/` on `PATH`.
pub const INSTALLER_BINS: [&str; 3] =
    ["logi-shim", "logitech-trueforce-install-shim", "install-tf-shim.sh"];

/// The installer's path inside a repo checkout, relative to the checkout
/// root.
const REPO_INSTALLER: &str = "tools/install-tf-shim.sh";

/// How many directory levels above the running executable to look for a
/// checkout root. A workspace build sits 4 levels down
/// (`<repo>/userspace/logi-wheel/target/release/logi-wheel-gui`); 8 leaves slack
/// for target-dir overrides without walking the whole filesystem.
const MAX_WALK_UP: usize = 8;

/// The first `dir/bin` regular file across the `PATH`-style `path_var`.
fn find_on_path(bin: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let paths = path_var?;
    std::env::split_paths(paths).map(|dir| dir.join(bin)).find(|p| p.is_file())
}

/// Resolve a helper that is built into the same target directory as the
/// front-ends: `$PATH` first (the packaged install), else next to the
/// running executable. `path_var` is the `PATH` value and `exe` the
/// current executable's path; both parameterized for tests.
fn resolve_sibling(bin: &str, path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    find_on_path(bin, path_var).or_else(|| {
        let sibling = exe?.parent()?.join(bin);
        sibling.is_file().then_some(sibling)
    })
}

/// Resolve `logi-ffb`: `$PATH` first, else next to the running executable
/// (`cargo build` drops `logi-ffb` and the front-ends into the same
/// `target/<profile>` directory).
pub fn resolve_ffb(path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    resolve_sibling(FFB_BIN, path_var, exe)
}

/// Resolve `logi-tf-sim`, the simulated-TrueForce daemon: same rule as
/// [`resolve_ffb`] (`$PATH`, else the sibling next to the running
/// executable, where a workspace build drops it).
pub fn resolve_tf_sim(path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    resolve_sibling(TF_SIM_BIN, path_var, exe)
}

/// Walk up from the running executable's directory (at most
/// [`MAX_WALK_UP`] levels) looking for `repo_rel`, a checkout-relative path
/// like `tools/install-tf-shim.sh`.
fn walk_up_for(repo_rel: &str, exe: Option<&Path>) -> Option<PathBuf> {
    let mut dir = exe?.parent()?;
    for _ in 0..MAX_WALK_UP {
        let candidate = dir.join(repo_rel);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
    None
}

/// Resolve the TrueForce SDK shim installer: each candidate name on
/// `$PATH` first (packaged name preferred), else walk up from the running
/// executable's directory looking for the checkout's
/// `tools/install-tf-shim.sh`. Same parameterization as [`resolve_ffb`].
pub fn resolve_installer(path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    INSTALLER_BINS
        .iter()
        .find_map(|bin| find_on_path(bin, path_var))
        .or_else(|| walk_up_for(REPO_INSTALLER, exe))
}

/// Resolve `logi-launch`, the launch wrapper: `$PATH` first (the packaged
/// name), else the checkout's `tools/logi-launch.sh`, found by the same
/// walk up the installer uses (the wrapper is a script that never lands in
/// the target directory, so the sibling rule cannot apply).
pub fn resolve_launch(path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    find_on_path(LAUNCH_BIN, path_var).or_else(|| walk_up_for(REPO_LAUNCH, exe))
}

/// Resolve `logi-rpm-bridge`: `$PATH` first, else the built binary in the
/// checkout's `tools/`, same walk up as the installer (it is a C tool
/// built by `tools/Makefile`, so it is never a Cargo target-dir sibling).
pub fn resolve_rpm_bridge(path_var: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    find_on_path(RPM_BRIDGE_BIN, path_var).or_else(|| walk_up_for(REPO_RPM_BRIDGE, exe))
}

/// [`resolve_ffb`] over the real process environment.
pub fn ffb_path() -> Option<PathBuf> {
    resolve_ffb(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

/// [`resolve_installer`] over the real process environment.
pub fn installer_path() -> Option<PathBuf> {
    resolve_installer(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

/// [`resolve_tf_sim`] over the real process environment.
pub fn tf_sim_path() -> Option<PathBuf> {
    resolve_tf_sim(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

/// [`resolve_launch`] over the real process environment.
pub fn launch_path() -> Option<PathBuf> {
    resolve_launch(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

/// [`resolve_rpm_bridge`] over the real process environment.
pub fn rpm_bridge_path() -> Option<PathBuf> {
    resolve_rpm_bridge(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique fixture directory under the system temp dir, removed on
    /// drop. Std-only stand-in for a tempdir crate (same pattern as
    /// `steam.rs`'s tests).
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "logi-wheel-helpers-test-{}-{}",
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

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "bin").unwrap();
    }

    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs.iter().map(|d| d.to_path_buf())).unwrap()
    }

    /// A repo-checkout layout: the installer under `<repo>/tools/`, the
    /// binaries (including a built logi-ffb) 4 levels down in
    /// `<repo>/userspace/logi-wheel/target/release/`. Returns (repo root,
    /// fake exe path).
    fn checkout(tree: &TempTree) -> (PathBuf, PathBuf) {
        let repo = tree.path().join("repo");
        touch(&repo.join(REPO_INSTALLER));
        let release = repo.join("userspace/logi-wheel/target/release");
        let exe = release.join("logi-wheel-gui");
        touch(&exe);
        (repo, exe)
    }

    #[test]
    fn ffb_prefers_the_path_hit() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(FFB_BIN));
        let (_repo, exe) = checkout(&tree);
        touch(&exe.parent().unwrap().join(FFB_BIN));
        let found = resolve_ffb(Some(&path_var(&[&bindir])), Some(&exe)).unwrap();
        assert_eq!(found, bindir.join(FFB_BIN), "PATH wins over the sibling");
    }

    #[test]
    fn ffb_falls_back_to_the_exe_sibling() {
        let tree = TempTree::new();
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let (_repo, exe) = checkout(&tree);
        let sibling = exe.parent().unwrap().join(FFB_BIN);
        touch(&sibling);
        let found = resolve_ffb(Some(&path_var(&[&empty])), Some(&exe)).unwrap();
        assert_eq!(found, sibling);
    }

    #[test]
    fn ffb_not_found_anywhere_is_none() {
        let tree = TempTree::new();
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let (_repo, exe) = checkout(&tree);
        assert_eq!(resolve_ffb(Some(&path_var(&[&empty])), Some(&exe)), None);
        assert_eq!(resolve_ffb(None, None), None, "no PATH and no exe never panics");
    }

    #[test]
    fn tf_sim_resolves_like_ffb() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(TF_SIM_BIN));
        let (_repo, exe) = checkout(&tree);
        let sibling = exe.parent().unwrap().join(TF_SIM_BIN);
        touch(&sibling);
        let found = resolve_tf_sim(Some(&path_var(&[&bindir])), Some(&exe)).unwrap();
        assert_eq!(found, bindir.join(TF_SIM_BIN), "PATH wins over the sibling");
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let found = resolve_tf_sim(Some(&path_var(&[&empty])), Some(&exe)).unwrap();
        assert_eq!(found, sibling, "sibling fallback");
        assert_eq!(resolve_tf_sim(None, None), None, "nothing found never panics");
    }

    #[test]
    fn launch_prefers_the_path_hit_and_falls_back_to_the_repo_script() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(LAUNCH_BIN));
        let (repo, exe) = checkout(&tree);
        touch(&repo.join(REPO_LAUNCH));
        let found = resolve_launch(Some(&path_var(&[&bindir])), Some(&exe)).unwrap();
        assert_eq!(found, bindir.join(LAUNCH_BIN), "PATH wins over the checkout script");
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let found = resolve_launch(Some(&path_var(&[&empty])), Some(&exe)).unwrap();
        assert_eq!(found, repo.join(REPO_LAUNCH), "the checkout's tools/ script");
        assert_eq!(resolve_launch(None, None), None, "nothing found never panics");
    }

    #[test]
    fn rpm_bridge_resolves_like_launch() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(RPM_BRIDGE_BIN));
        let (repo, exe) = checkout(&tree);
        touch(&repo.join(REPO_RPM_BRIDGE));
        let found = resolve_rpm_bridge(Some(&path_var(&[&bindir])), Some(&exe)).unwrap();
        assert_eq!(found, bindir.join(RPM_BRIDGE_BIN), "PATH wins over the checkout build");
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let found = resolve_rpm_bridge(Some(&path_var(&[&empty])), Some(&exe)).unwrap();
        assert_eq!(found, repo.join(REPO_RPM_BRIDGE), "the checkout's tools/ build");
        assert_eq!(resolve_rpm_bridge(None, None), None, "nothing found never panics");
    }

    #[test]
    fn installer_prefers_the_packaged_name_on_path() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(INSTALLER_BINS[0]));
        touch(&bindir.join(INSTALLER_BINS[1]));
        let found = resolve_installer(Some(&path_var(&[&bindir])), None).unwrap();
        assert_eq!(found, bindir.join(INSTALLER_BINS[0]));
    }

    #[test]
    fn installer_takes_the_script_name_on_path_too() {
        let tree = TempTree::new();
        let bindir = tree.path().join("bin");
        touch(&bindir.join(INSTALLER_BINS[1]));
        let found = resolve_installer(Some(&path_var(&[&bindir])), None).unwrap();
        assert_eq!(found, bindir.join(INSTALLER_BINS[1]));
    }

    #[test]
    fn installer_walks_up_to_the_checkouts_tools_script() {
        let tree = TempTree::new();
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let (repo, exe) = checkout(&tree);
        let found = resolve_installer(Some(&path_var(&[&empty])), Some(&exe)).unwrap();
        assert_eq!(found, repo.join(REPO_INSTALLER), "4 levels up from target/release");
    }

    #[test]
    fn installer_walk_up_is_bounded() {
        // A tools/ script more than MAX_WALK_UP levels above the exe is
        // never picked up (the walk must not scan the whole filesystem).
        let tree = TempTree::new();
        let root = tree.path().join("deep");
        touch(&root.join(REPO_INSTALLER));
        let mut exe_dir = root.clone();
        for i in 0..(MAX_WALK_UP + 1) {
            exe_dir = exe_dir.join(format!("level{i}"));
        }
        let exe = exe_dir.join("logi-wheel-gui");
        touch(&exe);
        assert_eq!(resolve_installer(None, Some(&exe)), None);
    }

    #[test]
    fn installer_not_found_anywhere_is_none() {
        let tree = TempTree::new();
        let empty = tree.path().join("empty-bin");
        fs::create_dir_all(&empty).unwrap();
        let exe = tree.path().join("standalone/logi-wheel-gui");
        touch(&exe);
        assert_eq!(resolve_installer(Some(&path_var(&[&empty])), Some(&exe)), None);
        assert_eq!(resolve_installer(None, None), None);
    }
}
