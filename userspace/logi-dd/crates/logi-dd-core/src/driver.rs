//! Reading the loaded kernel module's version stamp, so the front-ends can
//! show which driver build is actually running (the Info identity block's
//! Driver row). The module stamps `MODULE_VERSION` with `git describe` at
//! build time; a missing file simply means the module is not loaded.

use std::path::Path;

/// Where the loaded module exposes its `MODULE_VERSION` stamp.
pub const MODULE_VERSION_PATH: &str = "/sys/module/hid_logitech_dd/version";

/// The version stamp at `path`: the trimmed file content, or `None` when
/// the file is absent, unreadable or empty (the module is not loaded).
/// Parameterized over the path so tests can point it at a fixture file.
pub fn module_version_at(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// [`module_version_at`] over the real sysfs path.
pub fn module_version() -> Option<String> {
    module_version_at(Path::new(MODULE_VERSION_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique fixture path under the system temp dir, removed on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("logi-dd-driver-test-{}-{name}", std::process::id()));
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn reads_and_trims_the_stamp() {
        let f = TempFile::new("stamp");
        std::fs::write(&f.0, "v0.16.0\n").unwrap();
        assert_eq!(module_version_at(&f.0), Some("v0.16.0".to_string()));
        std::fs::write(&f.0, "v0.15.0-231-g8beb949-dirty\n").unwrap();
        assert_eq!(module_version_at(&f.0), Some("v0.15.0-231-g8beb949-dirty".to_string()));
    }

    #[test]
    fn absent_or_empty_file_is_none() {
        let f = TempFile::new("absent");
        assert_eq!(module_version_at(&f.0), None, "module not loaded");
        std::fs::write(&f.0, "\n").unwrap();
        assert_eq!(module_version_at(&f.0), None, "empty stamp");
    }
}
