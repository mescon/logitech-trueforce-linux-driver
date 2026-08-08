// SPDX-License-Identifier: GPL-2.0-only
//! Why is there no wheel?
//!
//! The apps used to answer that with one line and a Retry button, which is
//! no help to the person most likely to be reading it: someone who has just
//! installed, has nothing working, and does not yet know what a udev rule
//! is. `tools/setup.sh doctor` knows all of this, but it is a shell script
//! in a checkout, and someone who installed a package does not have it.
//!
//! So the checks live here instead, and the apps run them. The output is
//! deliberately not a list of seven PASS/FAIL lines: it is the FIRST thing
//! that is wrong, said plainly, with the one command that fixes it. The
//! full list is available behind that for anyone who wants it, because it
//! is also what belongs in a bug report.
//!
//! Ordered by what has to be true first. A wheel that is not plugged in
//! makes every later check meaningless, and telling someone their udev
//! rules are missing when the wheel is in a drawer is how diagnostics lose
//! people's trust.

use std::fs;
use std::path::Path;

use crate::device::{is_g923_pid, DD_PIDS, G923_PIDS, LOGITECH_VID};

/// The Xbox G923 before its mode switch. It enumerates as a console device
/// that speaks nothing we can use, and no amount of driver debugging helps
/// until it is switched.
const G923_XBOX_CONSOLE_PID: u16 = 0xc26d;

/// How much a finding blocks the wheel working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Nothing will work until this is dealt with.
    Blocking,
    /// Worth fixing, but the wheel may still be usable.
    Warning,
    /// Confirmation that a layer is fine.
    Ok,
}

/// A command that would fix a finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// The command, exactly as it should be run.
    pub command: String,
    /// Whether it needs root. Drives whether an app offers to run it
    /// through pkexec, and whether the copyable form carries `sudo`.
    pub needs_root: bool,
}

/// One check's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// One short sentence naming what is wrong, in the user's terms.
    pub title: String,
    /// What it means and why it stops the wheel working. Plain language:
    /// the reader may never have heard of a kernel module.
    pub detail: String,
    pub fix: Option<Fix>,
}

impl Finding {
    fn ok(title: impl Into<String>, detail: impl Into<String>) -> Finding {
        Finding {
            severity: Severity::Ok,
            title: title.into(),
            detail: detail.into(),
            fix: None,
        }
    }
}

/// Run every check, in order, against the real system.
pub fn diagnose() -> Vec<Finding> {
    diagnose_in(Path::new("/sys"), Path::new("/"))
}

/// `diagnose`, rooted at `sys` and `root` so it can be tested against a
/// fixture instead of whatever hardware happens to be plugged in.
pub fn diagnose_in(sys: &Path, root: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let attached = attached_wheels(sys);

    // 1. Is there a wheel at all? Everything else is meaningless without
    //    one, and this is the single most common answer.
    if attached.is_empty() {
        if console_mode_g923(sys) {
            out.push(Finding {
                severity: Severity::Blocking,
                title: "Your G923 is in console mode".to_string(),
                detail: "The Xbox edition starts up pretending to be a console \
                         controller and has to be switched into PC mode before \
                         anything can talk to it. This normally happens by \
                         itself when you plug it in; when it does not, running \
                         the switch by hand fixes it until the next replug."
                    .to_string(),
                fix: Some(Fix {
                    command: "logi-g923-modeswitch".to_string(),
                    needs_root: true,
                }),
            });
            return out;
        }
        out.push(Finding {
            severity: Severity::Blocking,
            title: "No Logitech wheel is plugged in".to_string(),
            detail: "Nothing on USB identifies itself as one of the wheels this \
                     driver supports. Check the cable and the wheel's own power \
                     supply, then press Retry. If the wheel is on a USB hub, \
                     try it directly on the machine."
                .to_string(),
            fix: None,
        });
        return out;
    }
    out.push(Finding::ok(
        "Wheel detected",
        format!("{} attached over USB.", describe(&attached)),
    ));

    // 2. Is the driver even running? A wheel that is plugged in but has no
    //    driver is the second most common answer, and the fix is one line.
    let module_loaded = sys.join("module/hid_logitech_dd").is_dir();
    if !module_loaded {
        let installed = dkms_installed(root);
        out.push(Finding {
            severity: Severity::Blocking,
            title: "The driver is not running".to_string(),
            detail: if installed {
                "The driver is installed but not loaded. Loading it should be \
                 all that is needed. If it refuses to load on a machine with \
                 Secure Boot turned on, the module needs to be signed and \
                 enrolled, which the installation guide covers."
                    .to_string()
            } else {
                "The driver does not appear to be installed. Install the \
                 package for your distribution, or run the setup script from a \
                 checkout, then plug the wheel in again."
                    .to_string()
            },
            fix: Some(Fix {
                command: "modprobe hid-logitech-dd".to_string(),
                needs_root: true,
            }),
        });
        return out;
    }
    out.push(Finding::ok("Driver loaded", "The kernel driver is running."));

    // 3. Loaded is not the same as in charge. When the wheel enumerates
    //    before the driver does, the generic HID driver claims it and
    //    everything looks present while nothing works.
    let bound = binding(sys);
    match bound {
        Binding::Ours => {
            out.push(Finding::ok("Wheel claimed", "The driver is managing your wheel."));
        }
        Binding::Other(ref other) => {
            out.push(Finding {
                severity: Severity::Blocking,
                title: format!("Another driver has your wheel ({other})"),
                detail: "The wheel was picked up by a different driver before \
                         this one was ready, which usually means it was plugged \
                         in before the system finished starting. Unplugging it \
                         and plugging it back in is the simplest fix, and the \
                         rebind command below does the same thing without \
                         reaching behind the desk."
                    .to_string(),
                fix: Some(Fix {
                    command: "logi-rebind-wheel".to_string(),
                    needs_root: true,
                }),
            });
            return out;
        }
        Binding::None => {
            out.push(Finding {
                severity: Severity::Blocking,
                title: "Your wheel has no driver attached".to_string(),
                detail: "The wheel is plugged in and the driver is running, but \
                         the two have not been introduced. Replugging the wheel \
                         normally settles it."
                    .to_string(),
                fix: Some(Fix {
                    command: "logi-rebind-wheel".to_string(),
                    needs_root: true,
                }),
            });
            return out;
        }
    }

    // 4. Permissions. The wheel works, but the settings app cannot write to
    //    it, which looks like the app being broken rather than a rule being
    //    absent. A warning rather than blocking: reads still work.
    if !udev_rules_present(root) {
        out.push(Finding {
            severity: Severity::Warning,
            title: "Permission rules are missing".to_string(),
            detail: "Without them, changing settings needs root and this app \
                     will report writes it cannot make. They are installed by \
                     the package and by the setup script."
                .to_string(),
            fix: Some(Fix {
                command: "udevadm control --reload-rules && udevadm trigger".to_string(),
                needs_root: true,
            }),
        });
    } else {
        out.push(Finding::ok("Permissions in place", "The udev rules are installed."));
    }

    out
}

/// The first thing that actually needs attention, or `None` when every
/// check passed.
pub fn first_problem(findings: &[Finding]) -> Option<&Finding> {
    findings
        .iter()
        .find(|f| f.severity == Severity::Blocking)
        .or_else(|| findings.iter().find(|f| f.severity == Severity::Warning))
}

/// The copyable form of a fix: with `sudo` when it needs root, because
/// that is what someone pasting it into a terminal needs.
pub fn copyable(fix: &Fix) -> String {
    if fix.needs_root {
        format!("sudo {}", fix.command)
    } else {
        fix.command.clone()
    }
}

enum Binding {
    Ours,
    Other(String),
    None,
}

fn supported_pid(pid: u16) -> bool {
    DD_PIDS.contains(&pid) || G923_PIDS.contains(&pid)
}

/// Product ids of supported wheels currently on USB.
fn attached_wheels(sys: &Path) -> Vec<u16> {
    usb_pids(sys).into_iter().filter(|p| supported_pid(*p)).collect()
}

fn console_mode_g923(sys: &Path) -> bool {
    usb_pids(sys).contains(&G923_XBOX_CONSOLE_PID)
}

/// Every Logitech product id on the USB bus.
fn usb_pids(sys: &Path) -> Vec<u16> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(sys.join("bus/usb/devices")) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let vid = read_hex(&dir.join("idVendor"));
        let pid = read_hex(&dir.join("idProduct"));
        if let (Some(v), Some(p)) = (vid, pid) {
            if v == LOGITECH_VID {
                out.push(p);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn read_hex(path: &Path) -> Option<u16> {
    let raw = fs::read_to_string(path).ok()?;
    u16::from_str_radix(raw.trim(), 16).ok()
}

/// What, if anything, has claimed a supported wheel.
fn binding(sys: &Path) -> Binding {
    let Ok(entries) = fs::read_dir(sys.join("bus/hid/devices")) else {
        return Binding::None;
    };
    let mut other = None;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_uppercase();
        if !name.contains("046D") {
            continue;
        }
        // The hid device name carries the pid: 0003:046D:C276.0001
        let pid = name
            .split(':')
            .nth(2)
            .and_then(|s| s.split('.').next())
            .and_then(|s| u16::from_str_radix(s, 16).ok());
        if !pid.is_some_and(supported_pid) {
            continue;
        }
        match fs::read_link(e.path().join("driver")) {
            Ok(link) => {
                let drv = link
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if drv.contains("logitech-dd") {
                    return Binding::Ours;
                }
                other = Some(drv);
            }
            Err(_) => continue,
        }
    }
    match other {
        Some(d) => Binding::Other(d),
        None => Binding::None,
    }
}

fn udev_rules_present(root: &Path) -> bool {
    ["etc/udev/rules.d", "usr/lib/udev/rules.d", "lib/udev/rules.d"]
        .iter()
        .any(|d| root.join(d).join("70-logitech-trueforce.rules").exists())
}

fn dkms_installed(root: &Path) -> bool {
    root.join("usr/src").read_dir().is_ok_and(|mut e| {
        e.any(|x| {
            x.map(|x| x.file_name().to_string_lossy().starts_with("logitech-trueforce"))
                .unwrap_or(false)
        })
    })
}

fn describe(pids: &[u16]) -> String {
    let names: Vec<&str> = pids
        .iter()
        .map(|p| {
            if is_g923_pid(*p) {
                "G923"
            } else if *p == 0xc276 {
                "RS50"
            } else {
                "G PRO"
            }
        })
        .collect();
    names.join(" and ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake /sys and / with the pieces each test needs.
    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let dir = std::env::temp_dir().join(format!("lw-diag-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Fixture(dir)
        }
        fn sys(&self) -> std::path::PathBuf {
            self.0.join("sys")
        }
        fn root(&self) -> std::path::PathBuf {
            self.0.join("root")
        }
        fn usb(&self, pid: u16) {
            let d = self.sys().join("bus/usb/devices").join(format!("1-{pid:x}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("idVendor"), "046d").unwrap();
            fs::write(d.join("idProduct"), format!("{pid:04x}")).unwrap();
        }
        fn module(&self) {
            fs::create_dir_all(self.sys().join("module/hid_logitech_dd")).unwrap();
        }
        fn hid(&self, pid: u16, driver: Option<&str>) {
            let d = self.sys().join("bus/hid/devices").join(format!("0003:046D:{pid:04X}.0001"));
            fs::create_dir_all(&d).unwrap();
            if let Some(drv) = driver {
                let target = self.sys().join("bus/hid/drivers").join(drv);
                fs::create_dir_all(&target).unwrap();
                std::os::unix::fs::symlink(&target, d.join("driver")).unwrap();
            }
        }
        fn udev(&self) {
            let d = self.root().join("usr/lib/udev/rules.d");
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("70-logitech-trueforce.rules"), "# rule").unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn no_wheel_is_reported_before_anything_else() {
        let f = Fixture::new("nowheel");
        fs::create_dir_all(f.sys()).unwrap();
        let out = diagnose_in(&f.sys(), &f.root());
        let first = first_problem(&out).expect("a problem");
        assert_eq!(first.severity, Severity::Blocking);
        assert!(first.title.contains("No Logitech wheel"), "{}", first.title);
        // Nothing about modules or udev: with no wheel those are noise, and
        // telling someone their rules are missing while the wheel is in a
        // drawer is how diagnostics lose trust.
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_console_mode_g923_is_named_rather_than_called_missing() {
        let f = Fixture::new("console");
        f.usb(G923_XBOX_CONSOLE_PID);
        let out = diagnose_in(&f.sys(), &f.root());
        let first = first_problem(&out).expect("a problem");
        assert!(first.title.contains("console mode"), "{}", first.title);
        assert!(first.fix.as_ref().unwrap().needs_root);
    }

    #[test]
    fn a_wheel_with_no_driver_running_says_so() {
        let f = Fixture::new("nomodule");
        f.usb(0xc276);
        let out = diagnose_in(&f.sys(), &f.root());
        let first = first_problem(&out).expect("a problem");
        assert!(first.title.contains("not running"), "{}", first.title);
        assert_eq!(first.fix.as_ref().unwrap().command, "modprobe hid-logitech-dd");
    }

    #[test]
    fn a_wheel_claimed_by_another_driver_names_that_driver() {
        let f = Fixture::new("hijacked");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, Some("hid-generic"));
        let out = diagnose_in(&f.sys(), &f.root());
        let first = first_problem(&out).expect("a problem");
        assert!(first.title.contains("hid-generic"), "{}", first.title);
    }

    #[test]
    fn everything_in_place_reports_no_problem() {
        let f = Fixture::new("healthy");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, Some("logitech-dd"));
        f.udev();
        let out = diagnose_in(&f.sys(), &f.root());
        assert!(first_problem(&out).is_none(), "{out:?}");
        assert!(out.iter().all(|x| x.severity == Severity::Ok));
    }

    #[test]
    fn missing_udev_rules_warn_without_blocking() {
        let f = Fixture::new("noudev");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, Some("logitech-dd"));
        let out = diagnose_in(&f.sys(), &f.root());
        let first = first_problem(&out).expect("a problem");
        // The wheel still works for reading, so this must not claim to be
        // the reason nothing is happening.
        assert_eq!(first.severity, Severity::Warning);
    }

    #[test]
    fn a_root_fix_is_copyable_with_sudo() {
        let fix = Fix { command: "modprobe hid-logitech-dd".into(), needs_root: true };
        assert_eq!(copyable(&fix), "sudo modprobe hid-logitech-dd");
        let plain = Fix { command: "logi-wheel --report".into(), needs_root: false };
        assert_eq!(copyable(&plain), "logi-wheel --report");
    }

    #[test]
    fn every_finding_says_something_useful() {
        // A blank title or detail would render as an empty banner, which is
        // worse than the one-line message this replaces.
        let f = Fixture::new("prose");
        f.usb(0xc276);
        let out = diagnose_in(&f.sys(), &f.root());
        for finding in &out {
            assert!(!finding.title.trim().is_empty());
            assert!(finding.detail.trim().len() > 20, "{:?}", finding.detail);
        }
    }

    /// Collect every command any finding can offer, by driving `diagnose_in`
    /// through each failure state in turn.
    fn all_offered_commands() -> Vec<String> {
        let mut cmds = Vec::new();
        let mut collect = |f: &Fixture| {
            for finding in diagnose_in(&f.sys(), &f.root()) {
                if let Some(fix) = finding.fix {
                    cmds.push(fix.command);
                }
            }
        };

        collect(&Fixture::new("no-wheel"));

        let f = Fixture::new("console");
        f.usb(G923_XBOX_CONSOLE_PID);
        collect(&f);

        let f = Fixture::new("no-module");
        f.usb(0xc276);
        collect(&f);

        let f = Fixture::new("misbound");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, Some("hid-generic"));
        collect(&f);

        let f = Fixture::new("unbound");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, None);
        collect(&f);

        let f = Fixture::new("no-rules");
        f.usb(0xc276);
        f.module();
        f.hid(0xc276, Some("logitech-dd"));
        collect(&f);

        cmds
    }

    #[test]
    fn every_offered_command_is_one_we_ship() {
        // The bug this guards: a fix offered `logi-rebind-wheel` when no
        // package installed anything by that name, so the button ran a
        // command that did not exist. A fix that cannot work is worse than
        // no fix, because it moves the blame onto the user's system.
        //
        // Anything not on this list is either a system tool every distro
        // has, or one of our helpers. Adding a helper here means adding it
        // to the packaging too, which the next test checks.
        const SYSTEM: &[&str] = &["modprobe", "udevadm"];
        for cmd in all_offered_commands() {
            let prog = cmd.split_whitespace().next().unwrap_or_default();
            assert!(
                SYSTEM.contains(&prog) || OUR_HELPERS.contains(&prog),
                "{cmd:?} offers {prog:?}, which is neither a system tool nor a helper we ship"
            );
        }
    }

    /// Helpers we install ourselves. Kept beside the check that every one of
    /// them is actually installed by every packaging path.
    const OUR_HELPERS: &[&str] = &["logi-g923-modeswitch", "logi-rebind-wheel"];

    #[test]
    fn every_helper_we_offer_is_installed_by_every_package() {
        // Reaches out of the crate to the packaging, because that is where
        // the invariant actually lives: the apps may only name a command
        // that all five install paths put on PATH. Skipped when the crate is
        // built outside the repo (a vendored or packaged source tree).
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let paths = [
            "packaging/debian/rules",
            "packaging/akmods/logitech-trueforce-kmod.spec",
            "packaging/obs/logitech-trueforce-dkms.spec",
            "packaging/aur/logitech-trueforce-dkms/PKGBUILD",
            "tools/dkms-update.sh",
        ];
        if !repo.join(paths[0]).is_file() {
            return;
        }
        for rel in paths {
            let text = fs::read_to_string(repo.join(rel)).unwrap_or_default();
            for helper in OUR_HELPERS {
                assert!(
                    text.contains(helper),
                    "{rel} does not install {helper}, but a diagnosis offers it"
                );
            }
        }
    }
}
