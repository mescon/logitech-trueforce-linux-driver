// SPDX-License-Identifier: GPL-2.0-only
//! The build identity every app prints and the doctor compares.
//!
//! Its own crate so the proxy, which does not use the core crate, carries
//! the same stamp from the same build script; the core crate re-exports it
//! as `logi_wheel_core::version`.
//!
//! `BUILD_ID` is the checkout's `git describe --tags --always --dirty`
//! (see build.rs), the same stamp the kernel module carries in
//! `/sys/module/hid_logitech_dd/version`, or the Cargo version for a build
//! from a tarball, where the module's stamp is the tagged version too. An
//! app and a module that print different ids were not built from the same
//! source, and that is a fault to report, not a curiosity: the pair only
//! works as designed when they match.

/// The Cargo package version (the last release tag).
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build identity: `git describe` of the checkout, or [`PKG_VERSION`].
pub const BUILD_ID: &str = env!("LOGI_BUILD_ID");

/// The `--version` line for `app`: `logi-tf-sim 0.40.3 (v0.40.3-19-g044d945)`.
pub fn banner(app: &str) -> String {
    format!("{app} {PKG_VERSION} ({BUILD_ID})")
}

/// A stamp with the module's leading `v` removed, so `v0.40.3` and a
/// tarball build's `0.40.3` compare equal.
pub fn normalise(id: &str) -> &str {
    id.trim().trim_start_matches('v')
}

/// How this build relates to the loaded module's stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleMatch {
    /// Same source.
    Same,
    /// Built from different sources; carries the module's stamp.
    Differs(String),
    /// The module is not loaded, or was built without a stamp ("unknown").
    Unknown,
}

/// Compare [`BUILD_ID`] with the module's version text, as read from
/// `/sys/module/hid_logitech_dd/version` or `modinfo`.
pub fn against_module(module: Option<&str>) -> ModuleMatch {
    against_module_ids(BUILD_ID, module)
}

fn against_module_ids(build: &str, module: Option<&str>) -> ModuleMatch {
    match module.map(str::trim) {
        None | Some("") | Some("unknown") => ModuleMatch::Unknown,
        Some(m) if normalise(m) == normalise(build) => ModuleMatch::Same,
        Some(m) => ModuleMatch::Differs(m.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_id_is_stamped_and_the_banner_carries_both() {
        assert!(!BUILD_ID.is_empty());
        let b = banner("logi-tf-sim");
        assert!(b.starts_with("logi-tf-sim "));
        assert!(b.ends_with(&format!("({BUILD_ID})")));
        assert!(b.contains(PKG_VERSION));
    }

    #[test]
    fn a_tarball_build_matches_a_tagged_module() {
        assert_eq!(against_module_ids("0.40.3", Some("v0.40.3")), ModuleMatch::Same);
        assert_eq!(against_module_ids("v0.40.3-19-g044d945", Some("v0.40.3-19-g044d945\n")), ModuleMatch::Same);
    }

    #[test]
    fn a_release_app_next_to_a_dev_module_differs() {
        assert_eq!(
            against_module_ids("0.40.3", Some("v0.40.3-19-g044d945")),
            ModuleMatch::Differs("v0.40.3-19-g044d945".into())
        );
    }

    #[test]
    fn an_unstamped_or_absent_module_is_not_a_verdict() {
        assert_eq!(against_module_ids("0.40.3", None), ModuleMatch::Unknown);
        assert_eq!(against_module_ids("0.40.3", Some("unknown")), ModuleMatch::Unknown);
        assert_eq!(against_module_ids("0.40.3", Some("")), ModuleMatch::Unknown);
    }
}
