//! Computer-side profile store for desktop mode.
//!
//! In desktop mode the wheel is host-driven (onboard slot 0), so "profiles"
//! live on the computer, not in the wheel: one plain-text file per profile
//! under `$XDG_CONFIG_HOME/logi-dd/profiles` (falling back to
//! `~/.config/logi-dd/profiles`). A profile is a snapshot of every writable,
//! currently-available setting, taken with [`save`] and replayed with
//! [`apply`]. The format is deliberately trivial (a header line, then one
//! `attr=<raw sysfs value>` line per setting, encoded by each setting's own
//! [`Kind`]) so a profile survives hand-editing and version drift: unknown
//! or unparsable lines fail individually on apply, never the whole file.
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
use std::path::{Path, PathBuf};

use crate::device::Device;
use crate::error::Error;
use crate::kind::Kind;
use crate::registry::REGISTRY;
use crate::setting::{Access, ModeReq, SettingSpec};
use crate::sysfs::SysfsIo;

/// The first line of every profile file.
pub const FILE_HEADER: &str = "# logi-dd profile";

/// The store directory: `$XDG_CONFIG_HOME/logi-dd/profiles`, falling back
/// to `~/.config/logi-dd/profiles` when the variable is unset or empty.
pub fn default_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("logi-dd").join("profiles");
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("logi-dd").join("profiles")
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

fn profile_path(dir: &Path, name: &str) -> Result<PathBuf, Error> {
    Ok(dir.join(format!("{}.profile", validate_name(name)?)))
}

/// Whether `spec` belongs in a snapshot; see the module doc for the list.
fn snapshotted(spec: &SettingSpec) -> bool {
    spec.access == Access::ReadWrite
        && !matches!(spec.kind, Kind::SlotText { .. })
        && !matches!(spec.mode_req, ModeReq::OnboardOnly)
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
pub fn save_in<S: SysfsIo>(dir: &Path, name: &str, dev: &Device<S>) -> Result<(), Error> {
    let path = profile_path(dir, name)?;
    let mut out = String::from(FILE_HEADER);
    out.push('\n');
    for spec in REGISTRY.iter().filter(|s| snapshotted(s)) {
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
pub fn apply_in<S: SysfsIo>(
    dir: &Path,
    name: &str,
    dev: &Device<S>,
) -> Result<Vec<(String, String)>, Error> {
    let path = profile_path(dir, name)?;
    let text = fs::read_to_string(path).map_err(|e| Error::Io(e.to_string()))?;
    let mut errors = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((attr, raw)) = line.split_once('=') else {
            errors.push((line.to_string(), "not an attr=value line".to_string()));
            continue;
        };
        let Some(spec) = Device::<S>::spec(attr) else {
            errors.push((attr.to_string(), "unknown setting".to_string()));
            continue;
        };
        let result = spec.kind.parse(raw).and_then(|v| dev.write(attr, &v));
        if let Err(e) = result {
            errors.push((attr.to_string(), e.to_string()));
        }
    }
    Ok(errors)
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
            "logi-dd-profiles-test-{}-{}",
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
        assert!(list_in(Path::new("/nonexistent-logi-dd-profiles")).is_empty());
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
        assert_eq!(default_dir(), dir.join("logi-dd").join("profiles"));
        // And the public wrappers work against it end to end.
        save("envtest", &wheel()).unwrap();
        assert_eq!(list(), vec!["envtest".to_string()]);
        let errors = apply("envtest", &other_wheel()).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        delete("envtest").unwrap();
        assert!(list().is_empty());
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
