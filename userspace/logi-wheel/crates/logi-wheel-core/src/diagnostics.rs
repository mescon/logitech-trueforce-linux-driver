// SPDX-License-Identifier: GPL-2.0-only
//! One pasteable diagnostic report, shared by every front-end.
//!
//! Lives here rather than in a binary because all three surfaces need it and
//! a command-line flag is not discoverable by the people most likely to need
//! one: somebody whose wheel is misbehaving is in the app, not reading
//! `--help`.

/// Everything a bug report needs, in one pasteable block, with the parts
/// that identify a person left out.
///
/// This exists because the alternative advice is "paste your dmesg", and
/// that publishes the wheel's serial number: the driver logs it at probe,
/// and `wheel_serial` sits in sysfs next to the settings worth reading.
/// `wheel_profile_names` is worse, being whatever the owner called their
/// profiles. Neither helps diagnose anything, so neither is collected, and
/// the dmesg command suggested at the end filters the serial line out.
/// Settings whose VALUE is the owner's, not the wheel's. Their presence is
/// worth reporting; their contents are not.
///
/// Add to this rather than removing from it: a field wrongly withheld costs
/// one round trip in a bug report, a field wrongly published cannot be taken
/// back.
pub const WITHHELD: &[&str] = &["wheel_serial", "wheel_profile_names", "wheel_led_slot_name"];

/// The driver's own kernel-log lines, serial withheld, or `None` when the
/// log cannot be read (the usual case for a non-root user under
/// `dmesg_restrict`). Capped because a long-running machine can hold
/// hundreds of replug cycles and the tail is what matters.
fn kernel_log_lines() -> Option<Vec<String>> {
    let out = std::process::Command::new("dmesg").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.to_ascii_lowercase().contains("logitech"))
        .filter(|l| !l.to_ascii_lowercase().contains("serial"))
        .map(|l| l.to_string())
        .collect();
    if lines.len() > 40 {
        lines = lines.split_off(lines.len() - 40);
    }
    Some(lines)
}

pub fn report() -> String {
    use std::fmt::Write as _;
    use std::fs;

    /// Read a file, trimmed, or None.
    fn slurp(p: impl AsRef<std::path::Path>) -> Option<String> {
        fs::read_to_string(p).ok().map(|s| s.trim().to_string())
    }

    let mut out = String::new();
    let _ = writeln!(out, "## logitech-trueforce diagnostic report");
    let _ = writeln!(out);
    let _ = writeln!(out, "app        {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "kernel     {}", slurp("/proc/sys/kernel/osrelease").unwrap_or_default());
    let _ = writeln!(out, "module     {}",
             slurp("/sys/module/hid_logitech_dd/version")
                 .unwrap_or_else(|| "not loaded".into()));
    // A stale driver next to current apps is a real and common state, and
    // one that changes what a report means. It was visible here as two
    // version lines nobody was asked to compare.
    if let Some(m) = slurp("/sys/module/hid_logitech_dd/version") {
        let app = env!("CARGO_PKG_VERSION");
        if !m.trim_start_matches('v').starts_with(app) {
            let _ = writeln!(out, "           NOTE: driver {m} and apps {app} differ; the driver may need reinstalling");
        }
    }
    if let Some(os) = slurp("/etc/os-release") {
        if let Some(line) = os.lines().find(|l| l.starts_with("PRETTY_NAME=")) {
            let _ = writeln!(out, "distro     {}", line.trim_start_matches("PRETTY_NAME=").trim_matches('"'));
        }
    }

    let _ = writeln!(out, "\n### wheels");
    let base = std::path::Path::new("/sys/bus/hid/devices");
    let mut found = 0;
    if let Ok(entries) = fs::read_dir(base) {
        let mut names: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        names.sort();
        for name in names {
            let n = name.to_string_lossy().into_owned();
            // Logitech wheels only; other HID devices are not ours to report.
            if !n.contains("046D:C2") {
                continue;
            }
            found += 1;
            let dir = base.join(&name);
            let drv = fs::read_link(dir.join("driver")).ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "none".into());
            // Named, because a raw HID id does not tell a reader which of
            // their wheels this block is. The pid is in the name: a G923
            // and an RS50 do not collide in sysfs, they each get their own
            // directory, but the report gave no way to tell them apart.
            let model = match n.split(':').nth(2).and_then(|t| t.split('.').next()) {
                Some("C276") => " (RS50)",
                Some("C272") | Some("C268") => " (G PRO)",
                Some("C266") | Some("C267") | Some("C26E") => " (G923)",
                Some("C26D") => " (G923 Xbox, console mode)",
                _ => "",
            };
            let _ = writeln!(out, "\n{n}{model}  driver={drv}");
            let mut attrs: Vec<String> = fs::read_dir(&dir).ok()
                .map(|rd| rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    // The classic set as well as the wheel_* one. Matching
                    // only "range" meant a G923 reported one attribute and
                    // silently dropped gain, autocenter and combined
                    // pedals, which is most of what that wheel has.
                    .filter(|a| {
                        a.starts_with("wheel_")
                            || matches!(a.as_str(), "range" | "gain" | "autocenter" | "combine_pedals")
                    })
                    .collect())
                .unwrap_or_default();
            attrs.sort();
            for a in attrs {
                if WITHHELD.contains(&a.as_str()) {
                    let _ = writeln!(out, "  {a:<26} <withheld: identifies you, not the wheel>");
                    continue;
                }
                if let Some(v) = slurp(dir.join(&a)) {
                    let v = v.replace('\n', " | ");
                    // char_indices, not a byte slice: a sysfs value with a
                    // multi-byte character straddling byte 70 would panic,
                    // and the GUI calls this from a spawned thread where a
                    // panic just makes the Collect button never respond.
                    let v = match v.char_indices().nth(70) {
                        Some((cut, _)) => format!("{}...", &v[..cut]),
                        None => v,
                    };
                    let _ = writeln!(out, "  {a:<26} {v}");
                }
            }
        }
    }
    if found == 0 {
        let _ = writeln!(out, "  no Logitech wheel bound");
    }

    // The HID++ feature map, per wheel. Worth collecting automatically
    // because it is what a protocol question actually turns on, and
    // because asking for it by hand has cost issue #27 months: a wheel
    // that answers nothing and a wheel nobody addressed correctly look
    // identical in every other kind of report. Each line is one round trip
    // to the wheel, so this runs once per report and never per frame.
    let _ = writeln!(out, "\n### HID++ features");
    let wheels = crate::Device::discover_all();
    if wheels.is_empty() {
        let _ = writeln!(out, "no wheel to ask");
    } else {
        let models: Vec<_> = wheels.iter().map(|d| d.model()).collect();
        let labels = crate::device::short_labels(&models);
        for (i, wheel) in wheels.iter().enumerate() {
            let label = labels.get(i).cloned().unwrap_or_else(|| format!("{:?}", wheel.model()));
            match wheel.hidpp_features() {
                None => {
                    let _ = writeln!(out, "{label}: no HID++ interface could be opened");
                }
                Some(rows) => {
                    // What the wheel says it has, alongside what we knew to
                    // ask for. The gap between the two is where an
                    // undecoded capability hides, and it is large: an RS50
                    // lists 37 features and this project documents 14.
                    if let Some(all) = wheel.hidpp_all_features() {
                        let extra: Vec<String> = all
                            .iter()
                            .filter(|(_, _, name)| name.is_none())
                            .map(|(i, id, _)| format!("{id:04X}@{i:02X}"))
                            .collect();
                        let _ = writeln!(out, "{label}: {} features total", all.len());
                        if !extra.is_empty() {
                            let _ = writeln!(out, "{label}: undocumented {}", extra.join(" "));
                        }
                    }
                    let implemented: Vec<String> = rows
                        .iter()
                        .filter_map(|(id, _, idx)| idx.map(|i| format!("{id:04X}@{i:02X}")))
                        .collect();
                    let _ = writeln!(
                        out,
                        "{label}: {}",
                        if implemented.is_empty() {
                            "none of the known features".to_string()
                        } else {
                            implemented.join(" ")
                        }
                    );
                }
            }
        }
    }

    let _ = writeln!(out, "\n### simulated TrueForce config");
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("logi-wheel/tf-sim.conf");
    match slurp(&cfg) {
        Some(c) if !c.is_empty() => {
            for line in c.lines().filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty()) {
                let _ = writeln!(out, "  {line}");
            }
        }
        _ => { let _ = writeln!(out, "  none (defaults apply)"); }
    }

    let _ = writeln!(out, "\n### udev rules installed");
    // Deduped by canonical path: /lib is a symlink to /usr/lib on most
    // distributions now, so listing both directories reported every
    // packaged rule twice. That reads as a duplicate install, which is
    // exactly the thing someone runs this report to rule out.
    let mut any_rule = false;
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    for d in ["/etc/udev/rules.d", "/usr/lib/udev/rules.d", "/lib/udev/rules.d"] {
        if let Ok(rd) = fs::read_dir(d) {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("logi") || n.contains("trueforce"))
                .collect();
            names.sort();
            for n in names {
                let path = std::path::Path::new(d).join(&n);
                let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if seen.contains(&real) {
                    continue;
                }
                seen.push(real);
                let _ = writeln!(out, "  {d}/{n}");
                any_rule = true;
            }
        }
    }
    if !any_rule {
        let _ = writeln!(out, "  none found (force feedback and LEDs will need root)");
    }

    // Read it when we can rather than always claiming we cannot: the old
    // text told someone running as root that root was required, which is
    // both wrong and the sort of thing that makes a reader distrust the
    // rest of the report.
    //
    // The serial line is dropped here for the same reason it is withheld
    // above. It identifies the owner, not the wheel, and the whole point of
    // this report is that it can be pasted into a public issue.
    let _ = writeln!(out, "\n### kernel log");
    match kernel_log_lines() {
        Some(lines) if !lines.is_empty() => {
            for l in lines {
                let _ = writeln!(out, "  {l}");
            }
        }
        Some(_) => {
            let _ = writeln!(out, "  readable, but nothing from this driver is in it");
        }
        None => {
            let _ = writeln!(out, "  Not readable as this user. Add it with:");
            let _ = writeln!(out);
            let _ = writeln!(out, "    sudo dmesg | grep -i logitech | grep -v serial");
            let _ = writeln!(out);
            let _ = writeln!(out, "  The grep drops the line carrying your wheel's serial number.");
            let _ = writeln!(out, "  The lines worth having are \"HID++ features\", \"Effect timer\",");
            let _ = writeln!(out, "  and anything saying failed or error.");
        }
    }

    out
}

/// Write [`report`] somewhere the user can find it, and say where.
///
/// A file rather than the clipboard: the front-ends run on X11, Wayland and
/// a terminal that may be over SSH, clipboard support differs across all
/// three, and a path can be pasted into a bug report by hand when the
/// clipboard cannot be. Overwrites any previous one, since a stale report is
/// worse than none.
pub fn write_report() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
                .join(".cache")
        });
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("logi-wheel-report.txt");
    std::fs::write(&path, report())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_is_never_empty_and_names_its_sections() {
        // It has to be useful on a machine with no wheel attached, which is
        // exactly the machine somebody files "it does not detect my wheel"
        // from.
        let r = report();
        for section in ["diagnostic report", "### wheels", "### udev rules installed"] {
            assert!(r.contains(section), "missing {section} in:\n{r}");
        }
    }

    /// The withheld list must contain what it claims to.
    ///
    /// A version of this guard lived in the TUI and asserted that the TUI's
    /// source contained the literals, three lines below where the test
    /// itself declared them: it passed no matter what this module did.
    /// Reading the real constant is the whole point.
    #[test]
    fn the_withheld_list_covers_every_identifying_setting() {
        for field in ["wheel_serial", "wheel_profile_names", "wheel_led_slot_name"] {
            assert!(
                WITHHELD.contains(&field),
                "{field} identifies the owner and must stay withheld",
            );
        }
    }

    #[test]
    fn identifying_settings_never_reach_the_report() {
        // The report exists so people do not paste dmesg, which carries the
        // wheel's serial. Publishing it here instead would defeat the point.
        let r = report();
        for withheld in ["wheel_serial", "wheel_profile_names", "wheel_led_slot_name"] {
            if let Some(i) = r.find(withheld) {
                let line = r[i..].lines().next().unwrap_or("");
                assert!(
                    line.contains("withheld"),
                    "{withheld} printed a value rather than being withheld: {line}",
                );
            }
        }
    }
}
