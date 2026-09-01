//! Computer-side profile store for desktop mode.
//!
//! In desktop mode the wheel is host-driven (onboard slot 0), so "profiles"
//! live on the computer, not in the wheel: one plain-text file per profile
//! under `$XDG_CONFIG_HOME/logi-wheel/profiles` (falling back to
//! `~/.config/logi-wheel/profiles`). A profile is a snapshot of every writable,
//! currently-available setting, taken with [`save`] and replayed with
//! [`apply`]. The format is deliberately trivial (a header line, then one
//! `attr=<raw sysfs value>` line per setting, encoded by each setting's own
//! [`Kind`]) so a profile survives hand-editing and version drift: unknown
//! or unparsable lines fail individually on apply, never the whole file.
//!
//! Wheel-agnostic by construction: [`save_in`]/[`apply_in`] walk
//! `Device::settings()`, the registry the connected model's own rows come
//! from, not a fixed constant. A wheel with no onboard profile store or
//! desktop/onboard split at all (a G923, always reported as desktop mode by
//! [`crate::device::Device::current_mode`]) still gets a real snapshot of
//! whatever settings it does have (range, gain, autocenter,
//! combine_pedals), and applying one back to it works the same way as on a
//! direct-drive wheel.
//!
//! Excluded from a snapshot: read-only attrs (nothing to replay), actions
//! (a snapshot must never trigger a calibration), slot-text attrs (the
//! onboard slot names belong to the wheel, not a host profile), attrs the
//! wheel does not expose, attrs whose read fails, and onboard-only attrs
//! (these profiles are desktop-mode state; an onboard-only value could
//! never be written back from desktop mode and would fail every apply).
//!
//! `std` only, like the rest of the core crate.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::device::Device;
use crate::error::Error;
use crate::kind::Kind;
use crate::setting::{Access, ModeReq, Role, SettingSpec};
use crate::sysfs::SysfsIo;

/// The first line of every profile file.
pub const FILE_HEADER: &str = "# logi-wheel profile";

/// The store directory: `$XDG_CONFIG_HOME/logi-wheel/profiles`, falling back
/// to `~/.config/logi-wheel/profiles` when the variable is unset or empty.
///
/// Pre-rename installs saved profiles under `logi-dd/profiles` instead. The
/// first time the new directory is needed and does not exist yet,
/// [`default_dir`] copies every profile out of the old directory (if it has
/// any) into the new one, then uses the new directory from then on; the old
/// directory is left untouched as a safety net, and a file already present
/// at the destination is never overwritten. A fresh install with neither
/// directory yet gets the new path directly, with nothing to migrate. Once
/// the new directory exists, it always wins and no further migration is
/// attempted, even if the old directory is still around. If the copy itself
/// cannot be completed (for example the new location is not writable), this
/// falls back to the old directory for the current run instead of losing
/// anything.
pub fn default_dir() -> PathBuf {
    resolve_subdir_in(&config_root(), "profiles")
}

/// `<root>/logi-wheel/<subdir>` if it exists, else a one-time copy of
/// `<root>/logi-dd/<subdir>` (when that exists) followed by the new path,
/// else the new path outright (fresh install, nothing to migrate). Split
/// out from [`default_dir`] as a pure function of `root` (no environment
/// access) so the migration behavior is testable without touching
/// `XDG_CONFIG_HOME` - [`crate::tfsim`] has its own equivalent split for the
/// same reason.
fn resolve_subdir_in(root: &Path, subdir: &str) -> PathBuf {
    let new_dir = root.join("logi-wheel").join(subdir);
    if new_dir.is_dir() {
        return new_dir;
    }
    let old_dir = root.join("logi-dd").join(subdir);
    if old_dir.is_dir() {
        if migrate_dir(&old_dir, &new_dir).is_ok() {
            return new_dir;
        }
        return old_dir;
    }
    new_dir
}

/// Copy every regular file directly under `old_dir` into `new_dir`,
/// creating `new_dir` first. A destination file that already exists is
/// left alone (create-if-absent per file, tolerating `AlreadyExists`), so
/// running this concurrently from two processes - or finding a partial
/// migration from an earlier failed attempt - never overwrites anything.
/// The originals in `old_dir` are never touched or removed. Any other I/O
/// failure aborts and is returned so the caller can fall back to the old
/// directory for this run rather than losing data.
fn migrate_dir(old_dir: &Path, new_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(new_dir)?;
    for entry in fs::read_dir(old_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let data = fs::read(entry.path())?;
        let dest = new_dir.join(entry.file_name());
        match fs::OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(mut f) => f.write_all(&data)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `$XDG_CONFIG_HOME`, falling back to `~/.config` when unset or empty.
pub(crate) fn config_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config")
}

/// Validate a profile name: 1-32 characters after trimming, no path
/// separators (the name becomes the file name), no NUL, and not a dot
/// directory. Returns the trimmed name.
pub fn validate_name(name: &str) -> Result<String, Error> {
    let name = name.trim();
    let len = name.chars().count();
    if !(1..=32).contains(&len)
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name == "."
        || name == ".."
    {
        return Err(Error::Invalid);
    }
    Ok(name.to_string())
}

/// `<dir>/<name>.profile`, validating `name` first. `pub(crate)` so
/// [`crate::onboard`]'s "copy a computer profile into this slot" action can
/// read the same file `apply_in` would, without duplicating the naming
/// scheme.
pub(crate) fn profile_path(dir: &Path, name: &str) -> Result<PathBuf, Error> {
    Ok(dir.join(format!("{}.profile", validate_name(name)?)))
}

/// Whether `spec` belongs in a snapshot; see the module doc for the list.
fn snapshotted(spec: &SettingSpec) -> bool {
    spec.access == Access::ReadWrite
        && !matches!(spec.kind, Kind::SlotText { .. })
        && !matches!(spec.mode_req, ModeReq::OnboardOnly)
        && spec.role() != Role::StoreSelector
}

/// The saved profiles in `dir`, sorted by name. A missing directory is an
/// empty store, not an error.
pub fn list_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "profile"))
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

/// Snapshot the device's current settings into `<dir>/<name>.profile`,
/// creating the directory as needed. Unreadable or unavailable attrs are
/// skipped rather than failing the save.
///
/// Walks `dev.settings()` (the registry this specific model's rows come
/// from: the DD `wheel_*` set, or a classic wheel's own small set), not a
/// bare registry constant, so a wheel with no onboard profile store at all
/// (a G923) still gets a real snapshot of whatever it does have (range,
/// gain, autocenter, combine_pedals) instead of a header-only file.
pub fn save_in<S: SysfsIo>(dir: &Path, name: &str, dev: &Device<S>) -> Result<(), Error> {
    let path = profile_path(dir, name)?;
    let mut out = String::from(FILE_HEADER);
    out.push('\n');
    for spec in dev.settings().iter().filter(|s| snapshotted(s)) {
        if !dev.available(spec.attr) {
            continue;
        }
        let Ok(value) = dev.read(spec.attr) else { continue };
        let Ok(raw) = spec.kind.format(&value) else { continue };
        out.push_str(spec.attr);
        out.push('=');
        out.push_str(&raw);
        out.push('\n');
    }
    fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
    fs::write(path, out).map_err(|e| Error::Io(e.to_string()))
}

/// Replay `<dir>/<name>.profile` onto the device: parse each `attr=value`
/// line through the registry's own [`Kind`] and write it. Every line is
/// attempted; per-attr failures (unknown attr, parse error, rejected
/// write) are collected as `(attr, message)` pairs and returned, so one
/// bad line never aborts the rest. `Err` is reserved for the profile
/// itself being unreadable.
///
/// Order follows each attribute's [`Role`]: settings and slot content in
/// file order, the display selector after all of them, and store
/// selectors not at all. Both halves of that came from issue #73, where a
/// selector written after the values it governs undid them; the roles are
/// the registry's record of which attributes do that, so this function
/// does not need its own list.
pub fn apply_in<S: SysfsIo>(
    dir: &Path,
    name: &str,
    dev: &Device<S>,
) -> Result<Vec<(String, String)>, Error> {
    let path = profile_path(dir, name)?;
    let text = fs::read_to_string(path).map_err(|e| Error::Io(e.to_string()))?;
    let mut errors = Vec::new();

    let lines = || {
        text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
    };
    let role = |line: &str| -> Option<Role> {
        line.split_once('=').map(|(attr, _)| crate::registry::role_of(attr.trim()))
    };
    for line in lines().filter(|l| role(l) != Some(Role::DisplaySelector)) {
        apply_line(line, dev, &mut errors);
    }
    for line in lines().filter(|l| role(l) == Some(Role::DisplaySelector)) {
        apply_line(line, dev, &mut errors);
    }
    Ok(errors)
}

/// One `attr=value` line, written or recorded as a failure.
fn apply_line<S: SysfsIo>(line: &str, dev: &Device<S>, errors: &mut Vec<(String, String)>) {
    let Some((attr, raw)) = line.split_once('=') else {
        errors.push((line.to_string(), "not an attr=value line".to_string()));
        return;
    };
    // A store selector reloads the wheel's stored values over everything
    // else; never replayed, and not reported, since older files carry it.
    if crate::registry::role_of(attr.trim()) == Role::StoreSelector {
        return;
    }
    let Some(spec) = Device::<S>::spec(attr) else {
        errors.push((attr.to_string(), "unknown setting".to_string()));
        return;
    };
    if let Err(e) = spec.kind.parse(raw).and_then(|v| dev.write(attr, &v)) {
        errors.push((attr.to_string(), e.to_string()));
    }
}

/// Delete `<dir>/<name>.profile`.
pub fn delete_in(dir: &Path, name: &str) -> Result<(), Error> {
    let path = profile_path(dir, name)?;
    fs::remove_file(path).map_err(|e| Error::Io(e.to_string()))
}

/// [`list_in`] against [`default_dir`].
pub fn list() -> Vec<String> {
    list_in(&default_dir())
}

/// [`save_in`] against [`default_dir`].
pub fn save<S: SysfsIo>(name: &str, dev: &Device<S>) -> Result<(), Error> {
    save_in(&default_dir(), name, dev)
}

/// [`apply_in`] against [`default_dir`].
pub fn apply<S: SysfsIo>(name: &str, dev: &Device<S>) -> Result<Vec<(String, String)>, Error> {
    apply_in(&default_dir(), name, dev)
}

/// [`delete_in`] against [`default_dir`].
pub fn delete(name: &str) -> Result<(), Error> {
    delete_in(&default_dir(), name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::FakeSysfs;
    use crate::Value;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, unique temp directory per test (std only, no tempfile dep).
    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-profiles-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A desktop-mode fake wheel with a value for a spread of kinds.
    fn wheel() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "62"); // Percent
        fs.set("wheel_range", "900"); // IntRange
        fs.set("wheel_texture_route", "tf"); // Enum (worded read)
        fs.set("wheel_range_restore", "1"); // Toggle
        fs.set("wheel_throttle_deadzone", "3 5"); // Pair
        fs.set("wheel_response_curve", "0/64 points loaded (0 = built-in curve)"); // Curve
        fs.set("wheel_profile", "0");
        Device::with_io(fs)
    }

    /// A second wheel with every saved attr present but different values.
    fn other_wheel() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "100");
        fs.set("wheel_range", "540");
        fs.set("wheel_texture_route", "kf");
        fs.set("wheel_range_restore", "0");
        fs.set("wheel_throttle_deadzone", "0 0");
        fs.set("wheel_response_curve", "reset");
        fs.set("wheel_profile", "0");
        Device::with_io(fs)
    }

    /// The strip's selection is replayed after the slot content that would
    /// otherwise steal it. On hardware, writing `wheel_led_slot`,
    /// `wheel_led_colors` or `wheel_led_direction` moves the display onto
    /// the custom slot, so a profile saved on a built-in sweep came back on
    /// CUSTOM 1 (issue #73). A fake wheel cannot reproduce that side
    /// effect, so this pins the order instead, which is the thing the fix
    /// controls.
    #[test]
    fn the_light_strips_selection_is_written_after_the_slot_content() {
        let dir = tempdir();
        // `Rc<FakeSysfs>` so the test keeps its handle to the write log
        // after handing a clone to `Device`.
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_led_effect", "3");
        fs.set("wheel_led_slot", "0");
        fs.set("wheel_led_direction", "L to R");
        fs.set("wheel_led_colors", "ff0000 ff0000 ff0000 ff0000 ff0000 ff0000 ff0000 ff0000 ff0000 ff0000");
        let dev = Device::with_io(fs.clone());
        save_in(&dir, "sweep", &dev).unwrap();

        let dev = Device::with_io(fs.clone());
        apply_in(&dir, "sweep", &dev).unwrap();

        let order: Vec<String> = fs.writes().into_iter().map(|(a, _)| a).collect();
        let effect = order.iter().position(|a| a == "wheel_led_effect").expect("selection written");
        for content in ["wheel_led_slot", "wheel_led_direction", "wheel_led_colors"] {
            let at = order.iter().position(|a| a == content).unwrap_or_else(|| panic!("{content} written"));
            assert!(at < effect, "{content} must be written before wheel_led_effect, got {order:?}");
        }
    }

    /// The mode and slot selectors are neither saved nor replayed. Replaying
    /// `wheel_profile` makes the wheel reload the slot's stored settings
    /// over everything the same apply just wrote (issue #73).
    #[test]
    fn the_selectors_are_not_saved() {
        let dir = tempdir();
        let dev = wheel();
        save_in(&dir, "s", &dev).unwrap();
        let text = fs::read_to_string(dir.join("s.profile")).unwrap();
        assert!(!text.contains("wheel_mode="), "mode is a selector, not a setting: {text}");
        assert!(!text.contains("wheel_profile="), "slot is a selector, not a setting: {text}");
        assert!(text.contains("wheel_strength=62"), "the settings themselves are still there");
    }

    /// A file written before the selectors were dropped still applies
    /// cleanly: its settings land, its selector lines are skipped, and
    /// nothing is reported, because nothing about that file is wrong.
    #[test]
    fn an_older_file_with_selectors_applies_without_replaying_them() {
        let dir = tempdir();
        fs::write(
            dir.join("old.profile"),
            "# logi-wheel profile\nwheel_strength=33\nwheel_trueforce=0\nwheel_mode=1\nwheel_profile=2\n",
        )
        .unwrap();
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_strength", "100");
        fs.set("wheel_trueforce", "30");
        let dev = Device::with_io(fs.clone());

        let errors = apply_in(&dir, "old", &dev).unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(dev.read("wheel_trueforce").unwrap(), Value::Percent(0));
        let written: Vec<String> = fs.writes().into_iter().map(|(a, _)| a).collect();
        assert!(!written.iter().any(|a| a == "wheel_profile"), "slot must not be replayed: {written:?}");
        assert!(!written.iter().any(|a| a == "wheel_mode"), "mode must not be replayed: {written:?}");
    }

    #[test]
    fn save_apply_round_trips_the_snapshot() {
        let dir = tempdir();
        let a = wheel();
        save_in(&dir, "race", &a).unwrap();
        assert_eq!(list_in(&dir), vec!["race".to_string()]);

        let b = other_wheel();
        let errors = apply_in(&dir, "race", &b).unwrap();
        assert_eq!(errors, Vec::new(), "clean apply: {errors:?}");
        for attr in [
            "wheel_strength",
            "wheel_range",
            "wheel_texture_route",
            "wheel_range_restore",
            "wheel_throttle_deadzone",
            "wheel_response_curve",
        ] {
            assert_eq!(b.read(attr).unwrap(), a.read(attr).unwrap(), "{attr}");
        }
        // The worded-enum attr really landed as the driver's word.
        assert_eq!(b.read("wheel_texture_route").unwrap(), Value::Enum(1));
    }

    #[test]
    fn saving_an_existing_name_again_overwrites_it_in_place() {
        // The GUI's per-profile Save button (issue #61) calls `save_in`
        // with a name that already has a file: after tweaking settings and
        // saving again under the same name, the profile must reflect the
        // new snapshot, not the old one, and the store must still hold
        // exactly one entry for that name (no dupes, nothing appended).
        let dir = tempdir();
        save_in(&dir, "race", &wheel()).unwrap();
        assert_eq!(list_in(&dir), vec!["race".to_string()]);

        save_in(&dir, "race", &other_wheel()).unwrap();
        assert_eq!(list_in(&dir), vec!["race".to_string()], "still one entry, not appended");

        let b = wheel();
        let errors = apply_in(&dir, "race", &b).unwrap();
        assert_eq!(errors, Vec::new(), "clean apply: {errors:?}");
        for attr in ["wheel_strength", "wheel_range", "wheel_range_restore"] {
            assert_eq!(
                b.read(attr).unwrap(),
                other_wheel().read(attr).unwrap(),
                "{attr}: the second save should have won"
            );
        }
    }

    #[test]
    fn saved_file_has_the_header_and_raw_values() {
        let dir = tempdir();
        save_in(&dir, "race", &wheel()).unwrap();
        let text = fs::read_to_string(dir.join("race.profile")).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(FILE_HEADER));
        assert!(text.contains("wheel_strength=62\n"));
        assert!(text.contains("wheel_throttle_deadzone=3 5\n"));
        assert!(text.contains("wheel_response_curve=reset\n"), "built-in curve saves as reset");
        // Registry-driven exclusions.
        assert!(!text.contains("wheel_serial"), "read-only attrs are not saved");
        assert!(!text.contains("wheel_calibrate_here"), "actions are not saved");
        assert!(!text.contains("wheel_profile_names"), "slot text is not saved");
        assert!(!text.contains("wheel_brake_force"), "onboard-only attrs are not saved");
        assert!(!text.contains("wheel_trueforce"), "unavailable attrs are skipped");
    }

    #[test]
    fn save_skips_unreadable_values() {
        let dir = tempdir();
        // Present but unparsable: the read fails and the attr is skipped.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "not-a-number");
        fs.set("wheel_range", "900");
        let dev = Device::with_io(fs);
        save_in(&dir, "broken", &dev).unwrap();
        let text = fs::read_to_string(dir.join("broken.profile")).unwrap();
        assert!(!text.contains("wheel_strength"), "unreadable value skipped");
        assert!(text.contains("wheel_range=900\n"));
    }

    #[test]
    fn apply_collects_per_attr_errors_without_aborting() {
        let dir = tempdir();
        fs::write(
            dir.join("mixed.profile"),
            format!(
                "{FILE_HEADER}\nwheel_bogus=1\nwheel_strength=200\nnot a line\nwheel_range=540\n"
            ),
        )
        .unwrap();
        let dev = other_wheel();
        let errors = apply_in(&dir, "mixed", &dev).unwrap();
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().any(|(a, _)| a == "wheel_bogus"));
        assert!(errors.iter().any(|(a, _)| a == "wheel_strength"));
        // The good line after the bad ones still applied.
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(540));
        // The out-of-range write never reached the device.
        assert_eq!(dev.read("wheel_strength").unwrap(), Value::Percent(100));
    }

    #[test]
    fn apply_collects_rejected_writes() {
        let dir = tempdir();
        save_in(&dir, "race", &wheel()).unwrap();
        // Make one attr's write fail at the sysfs layer (EINVAL).
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "10");
        fs.set("wheel_range", "270");
        fs.set_errno("wheel_range", 22);
        let dev = Device::with_io(fs);
        let errors = apply_in(&dir, "race", &dev).unwrap();
        assert!(errors.iter().any(|(a, _)| a == "wheel_range"), "{errors:?}");
        assert_eq!(dev.read("wheel_strength").unwrap(), Value::Percent(62), "others applied");
    }

    #[test]
    fn apply_of_a_missing_profile_is_an_error() {
        let dir = tempdir();
        assert!(matches!(apply_in(&dir, "nope", &wheel()), Err(Error::Io(_))));
    }

    #[test]
    fn delete_removes_the_file_and_list_sorts() {
        let dir = tempdir();
        let dev = wheel();
        save_in(&dir, "zeta", &dev).unwrap();
        save_in(&dir, "alpha", &dev).unwrap();
        assert_eq!(list_in(&dir), vec!["alpha".to_string(), "zeta".to_string()]);
        delete_in(&dir, "zeta").unwrap();
        assert_eq!(list_in(&dir), vec!["alpha".to_string()]);
        assert!(matches!(delete_in(&dir, "zeta"), Err(Error::Io(_))), "double delete errors");
    }

    #[test]
    fn list_of_a_missing_dir_is_empty() {
        assert!(list_in(Path::new("/nonexistent-logi-wheel-profiles")).is_empty());
    }

    #[test]
    fn names_are_validated() {
        assert_eq!(validate_name("  race  ").unwrap(), "race");
        assert_eq!(validate_name("GT7 wet").unwrap(), "GT7 wet");
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name(&"x".repeat(33)).is_err());
        assert!(validate_name(&"x".repeat(32)).is_ok());
        assert!(matches!(save_in(Path::new("/tmp"), "a/b", &wheel()), Err(Error::Invalid)));
    }

    #[test]
    fn default_dir_honors_xdg_config_home() {
        // The only test that touches the environment; nothing else in this
        // crate reads XDG_CONFIG_HOME or HOME, so this cannot race another
        // test.
        let dir = tempdir();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert_eq!(default_dir(), dir.join("logi-wheel").join("profiles"));
        // And the public wrappers work against it end to end.
        save("envtest", &wheel()).unwrap();
        assert_eq!(list(), vec!["envtest".to_string()]);
        let errors = apply("envtest", &other_wheel()).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        delete("envtest").unwrap();
        assert!(list().is_empty());
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// Fresh install: neither directory exists yet. [`resolve_subdir_in`]
    /// returns the new path outright and never even creates a `logi-dd`
    /// directory to check.
    #[test]
    fn resolve_subdir_in_fresh_install_is_the_new_path_untouched() {
        let root = tempdir();
        let dir = resolve_subdir_in(&root, "profiles");
        assert_eq!(dir, root.join("logi-wheel").join("profiles"));
        assert!(!dir.exists(), "nothing to migrate, nothing created");
        assert!(!root.join("logi-dd").exists(), "the old directory was never touched");
    }

    /// Old-only: a pre-rename install with no new directory yet. Every
    /// profile is copied to the new directory, the resolved path is the new
    /// one, and the originals are left in place as a safety net.
    #[test]
    fn resolve_subdir_in_migrates_every_profile_once() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd").join("profiles")).unwrap();
        fs::write(root.join("logi-dd").join("profiles").join("legacy.profile"), FILE_HEADER)
            .unwrap();
        fs::write(root.join("logi-dd").join("profiles").join("second.profile"), FILE_HEADER)
            .unwrap();

        let dir = resolve_subdir_in(&root, "profiles");
        assert_eq!(dir, root.join("logi-wheel").join("profiles"), "the new directory wins after migrating");
        assert_eq!(list_in(&dir), vec!["legacy".to_string(), "second".to_string()], "every profile migrated");
        assert_eq!(
            list_in(&root.join("logi-dd").join("profiles")),
            vec!["legacy".to_string(), "second".to_string()],
            "the originals are left in place"
        );

        // A second resolution finds the new directory directly and does
        // not need to migrate again.
        assert_eq!(resolve_subdir_in(&root, "profiles"), root.join("logi-wheel").join("profiles"));
    }

    /// Both exist: the new directory wins outright, and the old one is left
    /// completely alone (no copy is even attempted).
    #[test]
    fn resolve_subdir_in_prefers_the_new_directory_when_both_exist() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd").join("profiles")).unwrap();
        fs::write(root.join("logi-dd").join("profiles").join("old.profile"), FILE_HEADER).unwrap();
        fs::create_dir_all(root.join("logi-wheel").join("profiles")).unwrap();
        fs::write(root.join("logi-wheel").join("profiles").join("new.profile"), FILE_HEADER).unwrap();

        let dir = resolve_subdir_in(&root, "profiles");
        assert_eq!(dir, root.join("logi-wheel").join("profiles"));
        assert_eq!(list_in(&dir), vec!["new".to_string()], "the new directory wins");
        assert_eq!(
            list_in(&root.join("logi-dd").join("profiles")),
            vec!["old".to_string()],
            "the old directory is untouched, not overwritten by the new one's content"
        );
    }

    /// A file with the same name already sitting at the destination (as if
    /// a concurrent migration, or an earlier partial attempt, got there
    /// first) is left alone, while every other profile is still copied
    /// over. Exercises [`migrate_dir`] directly: going through
    /// [`resolve_subdir_in`] would short-circuit on its own "new directory
    /// already exists" gate before ever reaching the copy step, which is
    /// exactly why this per-file protection lives in the copy step itself
    /// rather than the gate.
    #[test]
    fn migrate_dir_never_overwrites_an_existing_destination_file() {
        let root = tempdir();
        let old_dir = root.join("logi-dd").join("profiles");
        let new_dir = root.join("logi-wheel").join("profiles");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("race.profile"), "old-content").unwrap();
        fs::write(old_dir.join("wet.profile"), FILE_HEADER).unwrap();
        // Pre-seed the destination with a same-named file the migration
        // must not clobber.
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("race.profile"), "new-content").unwrap();

        migrate_dir(&old_dir, &new_dir).unwrap();
        assert_eq!(list_in(&new_dir), vec!["race".to_string(), "wet".to_string()]);
        assert_eq!(
            fs::read_to_string(new_dir.join("race.profile")).unwrap(),
            "new-content",
            "the pre-existing destination file is never overwritten"
        );
        assert_eq!(fs::read_to_string(new_dir.join("wet.profile")).unwrap(), FILE_HEADER, "the other file still migrated");
    }

    /// Migration failure (the new location cannot be created, simulated
    /// cheaply by putting a plain file where the `logi-wheel` directory
    /// needs to go): resolution still falls back to the old directory
    /// rather than panicking or losing the originals.
    #[test]
    fn resolve_subdir_in_falls_back_to_the_old_directory_when_migration_fails() {
        let root = tempdir();
        fs::create_dir_all(root.join("logi-dd").join("profiles")).unwrap();
        fs::write(root.join("logi-dd").join("profiles").join("legacy.profile"), FILE_HEADER).unwrap();
        // Block `<root>/logi-wheel` from ever becoming a directory.
        fs::write(root.join("logi-wheel"), "not a directory").unwrap();

        let dir = resolve_subdir_in(&root, "profiles");
        assert_eq!(dir, root.join("logi-dd").join("profiles"), "falls back to the old directory");
        assert_eq!(list_in(&dir), vec!["legacy".to_string()], "the original is still usable");
    }

    /// A fake G923 (classic engine): no `wheel_*` attrs, no onboard profile
    /// store or mode split at all, just its own four settings, set through
    /// `Device::write` (not a raw sysfs string) so the seeded values are
    /// exactly what `Kind::ScaledPercent`'s round trip produces.
    fn g923_wheel() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        fs.set("gain", "0");
        fs.set("autocenter", "0");
        fs.set("combine_pedals", "0");
        let dev = Device::with_io_and_model(fs, crate::device::WheelModel::G923);
        dev.write("gain", &Value::Percent(80)).unwrap();
        dev.write("autocenter", &Value::Percent(50)).unwrap();
        dev
    }

    #[test]
    fn g923_save_apply_round_trips_its_classic_settings() {
        // A wheel with no onboard profile store at all must still get a
        // real computer-side snapshot of what it does have, and applying it
        // back after a drift must restore every value - the same contract
        // `save_apply_round_trips_the_snapshot` proves for a DD wheel.
        let dir = tempdir();
        let a = g923_wheel();
        let (range, gain, autocenter) =
            (a.read("range").unwrap(), a.read("gain").unwrap(), a.read("autocenter").unwrap());
        save_in(&dir, "classic", &a).unwrap();
        assert_eq!(list_in(&dir), vec!["classic".to_string()]);

        // Drift every saved value away from what was snapshotted.
        a.write("range", &Value::Int(540)).unwrap();
        a.write("gain", &Value::Percent(20)).unwrap();
        a.write("autocenter", &Value::Percent(10)).unwrap();
        a.write("combine_pedals", &Value::Enum(1)).unwrap();

        let errors = apply_in(&dir, "classic", &a).unwrap();
        assert_eq!(errors, Vec::new(), "clean apply: {errors:?}");
        assert_eq!(a.read("range").unwrap(), range);
        assert_eq!(a.read("gain").unwrap(), gain);
        assert_eq!(a.read("autocenter").unwrap(), autocenter);
        assert_eq!(a.read("combine_pedals").unwrap(), Value::Enum(0));
    }

    #[test]
    fn g923_saved_file_has_its_own_four_attrs_never_the_dd_wheel_set() {
        let dir = tempdir();
        save_in(&dir, "classic", &g923_wheel()).unwrap();
        let text = fs::read_to_string(dir.join("classic.profile")).unwrap();
        assert!(text.contains("range=900\n"));
        assert!(text.contains("combine_pedals=0\n"));
        assert!(text.contains("gain="));
        assert!(text.contains("autocenter="));
        // Never a DD wheel_* line: this is a different device model, not
        // "DD with everything missing" (see `save_in`'s doc comment).
        assert!(!text.contains("wheel_"));
    }
}
