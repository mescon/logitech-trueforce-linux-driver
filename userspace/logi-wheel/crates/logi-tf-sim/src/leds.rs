// SPDX-License-Identifier: GPL-2.0-only
//! Rev-display feeder: mirrors telemetry RPM onto the wheel's rev LEDs.
//!
//! To be unambiguous, since the wheel has two light-emitting surfaces:
//! this drives the **10-LED rev strip across the rim** (HID++ `0x807A`),
//! never the **OLED screen on the wheel base** (HID++ `0x8130`, which
//! nothing in this project writes). See `docs/PROTOCOL_SPECIFICATION.md`
//! 12.3 and 12.4.
//!
//! Two backends, chosen at [`RevLeds::discover`] time:
//! - The DD wheels (RS50, real G PRO) expose a single driver attribute,
//!   `wheel_rev_level` (0-10 LEDs lit; on the RS50 the fill uses the
//!   active LIGHTSYNC slot's colours and direction, on a real G PRO rim
//!   the onboard profile owns the colours).
//! - The G923 has no such attribute: its rev strip is 5 standard Linux
//!   LED classdevs, `<hiddev>::RPM1`.."RPM5"`, each a plain on/off
//!   brightness file (RPM1 outermost pair, RPM5 innermost, matching the
//!   classic lg4ff convention the kernel driver ports). The same 0-10
//!   level is mapped onto the 5 pairs and each brightness file is
//!   written only when its own on/off state changes.
//!
//! Pacing: writes are rate-limited to match G HUB's measured rev cadence
//! (~60 Hz, ~16 ms per level update in the issue #20 iRacing capture; the
//! driver coalesces writes and enforces its own ~10 ms floor), so
//! [`RevLeds::update`] rate-limits itself and only writes when the level
//! actually changed. Everything here is
//! best-effort: a wheel without either backend, a failed write or a
//! missing driver never disturbs the TrueForce stream that rides alongside.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The driver attribute the DD-wheel backend writes.
pub const ATTR: &str = "wheel_rev_level";

/// The sibling attribute whose re-write restores the DD wheels' idle
/// pattern.
const IDLE_ATTR: &str = "wheel_led_effect";

/// Where the driver's per-device attribute directories live.
const SYSFS_ROOT: &str = "/sys/bus/hid/devices";

/// Where LED classdevs live.
const LEDS_ROOT: &str = "/sys/class/leds";

/// The dev-override sysfs directory: `LOGI_WHEEL_SYSFS_DIR`, falling back to
/// the pre-rename `LOGI_DD_SYSFS_DIR` (deprecated alias). The new name wins
/// when both are set. Duplicated from `logi-wheel-core`'s own helper rather
/// than linked (this crate is GPL-2.0-only and does not depend on core at
/// runtime; see the module doc for the license boundary).
fn sysfs_dir_override() -> Option<String> {
    std::env::var("LOGI_WHEEL_SYSFS_DIR").ok().or_else(|| std::env::var("LOGI_DD_SYSFS_DIR").ok())
}

/// Classdev name suffixes for the G923's rev strip, outermost pair
/// (RPM1) to innermost (RPM5), matching the kernel driver's naming
/// (`dd-lg4ff.c`, ported from lg4ff).
const RPM_SUFFIXES: [&str; 5] = ["::RPM1", "::RPM2", "::RPM3", "::RPM4", "::RPM5"];

/// Minimum spacing between two rev-level writes. ~16 ms (~60 Hz) mirrors
/// G HUB's measured rev cadence and clears the driver's own ~10 ms floor;
/// the old 160 ms was a protocol-doc misread that made a full 0->10 sweep
/// crawl (~1.6 s).
pub const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(16);

/// How long the rev strip stays full, then dark, while the pit limiter is
/// engaged. The strip, not the base's OLED screen: the limiter is rendered
/// purely as rev-level 10/0, which is why no display support is needed. G Hub renders the limiter with no device-side effect at all:
/// it just alternates the ordinary rev level between 10 and 0, measured at
/// ~416.7 ms per half cycle (about 1.2 Hz) in the issue #20 iRacing
/// capture. See `docs/PROTOCOL_SPECIFICATION.md` 12.4.
///
/// Reproduced on an RS50 on 2026-07-29, driving the real strip from
/// synthetic OutGauge packets: 28 transitions, strictly 10 and 0, mean gap
/// 418 ms against the captured 417 ms, with the rev level the same RPM
/// would otherwise show (5) never appearing.
pub const PIT_FLASH_HALF_PERIOD: Duration = Duration::from_micros(416_700);

/// The rev level for a pit-limiter flash `elapsed` into it: full strip for
/// the first [`PIT_FLASH_HALF_PERIOD`], dark for the next, repeating. RPM
/// is ignored entirely while the limiter is engaged, which is what G Hub
/// does: on a limiter the strip carries no rev information at all.
fn pit_flash_level(elapsed: Duration) -> u8 {
    let half = PIT_FLASH_HALF_PERIOD.as_micros();
    // half is a non-zero constant, so this cannot divide by zero.
    if (elapsed.as_micros() / half) % 2 == 0 { 10 } else { 0 }
}

/// The rev level for `rpm` out of `max_rpm`: `round(10 * rpm / max_rpm)`
/// clamped to 0-10. A zero (or negative, or NaN) `max_rpm` reads as 0 so
/// a car that never reported its limiter shows a dark strip instead of a
/// division artifact.
pub fn rev_level(rpm: f32, max_rpm: f32) -> u8 {
    if max_rpm.is_nan() || max_rpm <= 0.0 {
        return 0;
    }
    // NaN rpm falls out as 0 through the `as` cast's saturation.
    (10.0 * rpm / max_rpm).round().clamp(0.0, 10.0) as u8
}

/// How many of the 5 LED pairs are lit for `level` (0-10): `round(level /
/// 2)`, computed as `level.div_ceil(2)` (equivalent to
/// round-half-away-from-zero, the same convention [`rev_level`] itself
/// uses, since `level` is never negative). Pair 0 (RPM1, outermost) fills
/// first.
fn lit_count(level: u8) -> u8 {
    level.div_ceil(2)
}

/// Find one classdev quintet under `root`: 5 entries sharing a common
/// name prefix before `::RPM1`.."RPM5"`, each holding a `brightness`
/// file. Returns the 5 brightness paths in RPM1..RPM5 order.
///
/// Deliberately does not check the parent HID device's vendor/product:
/// any wheel exposing this exact 5-classdev shape gets the same
/// treatment, which is both simpler and more robust than matching the
/// G923's specific PIDs (a future wheel with the same rev-strip layout
/// needs no code change here).
fn discover_classdevs_at(root: &Path) -> Option<[PathBuf; 5]> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(prefix) = name.strip_suffix(RPM_SUFFIXES[0]) else { continue };
        let mut brightness = Vec::with_capacity(RPM_SUFFIXES.len());
        let mut complete = true;
        for suffix in RPM_SUFFIXES {
            let path = root.join(format!("{prefix}{suffix}")).join("brightness");
            if !path.exists() {
                complete = false;
                break;
            }
            brightness.push(path);
        }
        if complete {
            return brightness.try_into().ok();
        }
    }
    None
}

/// Which sysfs surface a [`RevLeds`] drives.
enum Backend {
    /// The DD wheels: a single `wheel_rev_level` attribute.
    Attr(PathBuf),
    /// The G923: 5 discrete brightness files (RPM1..RPM5), with each
    /// LED's last-written on/off state so [`RevLeds::update`] only
    /// rewrites the ones that actually change.
    Classdevs { brightness: [PathBuf; 5], lit: [bool; 5] },
}

/// One wheel's rev display, found at stream start and driven while
/// telemetry flows.
pub struct RevLeds {
    backend: Backend,
    /// The last level actually written; writes are skipped while the
    /// level is unchanged.
    last_level: Option<u8>,
    /// When the last write landed, for the pacing floor.
    last_write: Option<Instant>,
    /// When the current pit-limiter flash began, so the phase is measured
    /// from the moment the limiter engaged rather than from process start.
    /// Cleared as soon as the limiter disengages, so the next one starts
    /// lit rather than wherever a free-running clock happened to be.
    pit_flash_since: Option<Instant>,
    /// Whether a failed write has already been reported. The display is
    /// driven at up to 60 Hz, so the complaint is made once and then the
    /// daemon stops talking about it.
    warned: bool,
}

impl RevLeds {
    /// Scan for a wheel's rev display: a DD wheel exposing [`ATTR`] wins
    /// first (`LOGI_WHEEL_SYSFS_DIR`, when set - or the deprecated
    /// `LOGI_DD_SYSFS_DIR` alias, see [`sysfs_dir_override`] - overrides
    /// that scan with a single device directory - the same development/test
    /// override the logi-wheel front-ends honor, and additionally checked
    /// for a `leds` subdirectory shaped like a classdev quintet); otherwise
    /// the first classdev quintet found under `/sys/class/leds` (the G923's
    /// rev strip) is used.
    /// The rev display belonging to one specific wheel, named by its HID
    /// device id (`0003:046D:C266.0004`).
    ///
    /// [`RevLeds::discover`] takes the first rev display it finds anywhere in
    /// sysfs, which is wrong the moment two wheels are attached: on a rig
    /// with a G923 and an RS50 it drove the RS50's lights while the haptic
    /// stream was driving the G923, and reported success while doing it.
    /// Caught on hardware 2026-08-06, by the lights simply not coming on.
    ///
    /// There is deliberately no fallback to the global scan. If the wheel
    /// being driven has no rev display, the answer is that it has none, not
    /// somebody else's.
    pub fn discover_for(hid_id: &str) -> Option<RevLeds> {
        RevLeds::discover_for_at(hid_id, Path::new(SYSFS_ROOT), Path::new(LEDS_ROOT))
    }

    /// [`RevLeds::discover_for`] against caller-supplied sysfs roots, so the
    /// scoping can be tested against a fabricated two-wheel tree rather than
    /// only on a rig that happens to have two wheels plugged in.
    pub fn discover_for_at(hid_id: &str, sysfs_root: &Path, leds_root: &Path) -> Option<RevLeds> {
        let attr = sysfs_root.join(hid_id).join(ATTR);
        if attr.exists() {
            return Some(RevLeds::at(attr));
        }
        let mut brightness = Vec::with_capacity(RPM_SUFFIXES.len());
        for suffix in RPM_SUFFIXES {
            let path = leds_root.join(format!("{hid_id}{suffix}")).join("brightness");
            if !path.exists() {
                return None;
            }
            brightness.push(path);
        }
        brightness.try_into().ok().map(RevLeds::at_classdevs)
    }

    pub fn discover() -> Option<RevLeds> {
        if let Some(dir) = sysfs_dir_override() {
            let dir = PathBuf::from(dir);
            let attr = dir.join(ATTR);
            if attr.exists() {
                return Some(RevLeds::at(attr));
            }
            return discover_classdevs_at(&dir.join("leds")).map(RevLeds::at_classdevs);
        }
        for entry in std::fs::read_dir(SYSFS_ROOT).ok()?.flatten() {
            let attr = entry.path().join(ATTR);
            if attr.exists() {
                return Some(RevLeds::at(attr));
            }
        }
        discover_classdevs_at(Path::new(LEDS_ROOT)).map(RevLeds::at_classdevs)
    }

    /// A rev display at an explicit `wheel_rev_level` attribute path
    /// (tests point this at a plain file in a temp directory).
    pub fn at(attr: PathBuf) -> RevLeds {
        RevLeds {
            backend: Backend::Attr(attr),
            last_level: None,
            last_write: None,
            pit_flash_since: None,
            warned: false,
        }
    }

    /// A rev display driven by 5 discrete LED brightness files, RPM1
    /// (outermost pair) through RPM5 (innermost) (tests point these at
    /// plain files in a temp directory).
    pub fn at_classdevs(brightness: [PathBuf; 5]) -> RevLeds {
        RevLeds {
            backend: Backend::Classdevs { brightness, lit: [false; 5] },
            last_level: None,
            last_write: None,
            pit_flash_since: None,
            warned: false,
        }
    }

    /// Feed one telemetry sample at time `now` (injected so tests control
    /// the clock). Writes the new level only when it CHANGED and the last
    /// write is at least [`MIN_WRITE_INTERVAL`] old; a skipped change is
    /// picked up by a later call since `last_level` still differs. Write
    /// failures are ignored (and not recorded, so the level is retried).
    pub fn update(&mut self, rpm: f32, max_rpm: f32, pit_limiter: bool, now: Instant) {
        let level = if pit_limiter {
            let since = *self.pit_flash_since.get_or_insert(now);
            pit_flash_level(now.duration_since(since))
        } else {
            self.pit_flash_since = None;
            rev_level(rpm, max_rpm)
        };
        if self.last_level == Some(level) {
            return;
        }
        if self.last_write.is_some_and(|t| now.duration_since(t) < MIN_WRITE_INTERVAL) {
            return;
        }
        match self.write_level(level) {
            Ok(()) => {
                self.last_level = Some(level);
                self.last_write = Some(now);
            }
            Err((path, err)) if !self.warned => {
                // Say it once, then stop: this runs at up to 60 Hz and a
                // per-write message would bury everything else.
                //
                // Silence here has cost two separate investigations. A
                // G923's five LED classdevs come up 0644 root:root, and a
                // DD wheel's `wheel_rev_level` is 0644 until the udev rule
                // chmods it, so in both cases every write failed while the
                // daemon reported that it was driving the display, and the
                // only symptom anyone could report was "the lights do not
                // move". Nothing distinguished that from the wheel not
                // being fed any RPM at all.
                eprintln!("logi-tf-sim: cannot write the rev display at {}: {err}", path.display());
                eprintln!(
                    "logi-tf-sim: the rev lights will not move. This is usually a permissions \
                     problem: the file should be writable by you (mode 0666), which the udev \
                     rule shipped with the driver sets. Replug the wheel after installing or \
                     updating the driver, or run: sudo udevadm trigger"
                );
                self.warned = true;
            }
            Err(_) => {}
        }
    }

    /// Push `level` (0-10) to the backend. For [`Backend::Attr`] this is
    /// one write of the level itself; for [`Backend::Classdevs`] it maps
    /// `level` to a lit-pair count via [`lit_count`] and writes only the
    /// brightness files whose on/off state actually changed.
    ///
    /// The error carries the path that failed as well as the reason,
    /// because on the classdev backend there are five candidates and
    /// "permission denied" on its own does not say which. A partial
    /// classdev failure still keeps the LEDs that did land, and the
    /// unwritten ones are retried on the next call since they are left out
    /// of `lit`.
    fn write_level(&mut self, level: u8) -> Result<(), (PathBuf, std::io::Error)> {
        match &mut self.backend {
            Backend::Attr(attr) => {
                std::fs::write(&attr, level.to_string()).map_err(|e| (attr.clone(), e))
            }
            Backend::Classdevs { brightness, lit } => {
                let target = lit_count(level) as usize;
                let mut failure = None;
                for i in 0..RPM_SUFFIXES.len() {
                    let on = i < target;
                    if lit[i] == on {
                        continue;
                    }
                    match std::fs::write(&brightness[i], if on { "1" } else { "0" }) {
                        Ok(()) => lit[i] = on,
                        Err(e) => {
                            failure.get_or_insert((brightness[i].clone(), e));
                        }
                    }
                }
                match failure {
                    Some(f) => Err(f),
                    None => Ok(()),
                }
            }
        }
    }

    /// Blank the display and hand the strip back. For [`Backend::Attr`]:
    /// write level 0, then restore the DD wheels' idle pattern by reading
    /// the sibling `wheel_led_effect` and writing the same value back
    /// (the driver re-applies the effect, which exits the rev fill). For
    /// [`Backend::Classdevs`]: there is no idle-pattern concept (the
    /// classdevs are plain on/off LEDs), so this just writes 0 to all 5.
    /// All writes are best-effort; called on telemetry silence and on
    /// shutdown.
    pub fn stop(&mut self) {
        match &mut self.backend {
            Backend::Attr(attr) => {
                let _ = std::fs::write(&attr, "0");
                if let Some(dir) = attr.parent() {
                    let idle = dir.join(IDLE_ATTR);
                    if let Ok(current) = std::fs::read_to_string(&idle) {
                        let _ = std::fs::write(&idle, current.trim());
                    }
                }
            }
            Backend::Classdevs { brightness, lit } => {
                for (path, on) in brightness.iter().zip(lit.iter_mut()) {
                    let _ = std::fs::write(path, "0");
                    *on = false;
                }
            }
        }
        self.last_level = None;
        self.last_write = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, unique temp directory per test (std only, same pattern as
    /// the config tests).
    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-sim-leds-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a two-wheel rig: a DD wheel exposing `wheel_rev_level` and a
    /// G923 exposing the five RPM classdevs, exactly the shape of the
    /// development machine on 2026-08-06.
    fn two_wheel_rig() -> (PathBuf, PathBuf, &'static str, &'static str) {
        let root = tempdir();
        let sysfs = root.join("hid");
        let leds = root.join("leds");
        let dd = "0003:046D:C276.0030";
        let g923 = "0003:046D:C266.0004";
        fs::create_dir_all(sysfs.join(dd)).unwrap();
        fs::write(sysfs.join(dd).join(ATTR), "0").unwrap();
        for suffix in RPM_SUFFIXES {
            let d = leds.join(format!("{g923}{suffix}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("brightness"), "0").unwrap();
        }
        (sysfs, leds, dd, g923)
    }

    /// The bug this exists for, found on hardware: with two wheels attached
    /// the daemon drove the G923's haptics and the RS50's rev lights, and
    /// said it had found "the wheel's rev display" while doing it. Asking
    /// for one wheel must never return the other's display.
    #[test]
    fn a_rev_display_is_scoped_to_the_wheel_that_was_asked_for() {
        let (sysfs, leds, dd, g923) = two_wheel_rig();

        let found = RevLeds::discover_for_at(g923, &sysfs, &leds).expect("the G923 has classdevs");
        match found.backend {
            Backend::Classdevs { brightness, .. } => {
                for path in &brightness {
                    let s = path.to_string_lossy();
                    assert!(s.contains(g923), "took another wheel's LED: {s}");
                    assert!(!s.contains(dd), "took the DD wheel's LED: {s}");
                }
            }
            Backend::Attr(p) => panic!("the G923 has no wheel_rev_level, got {}", p.display()),
        }

        let found = RevLeds::discover_for_at(dd, &sysfs, &leds).expect("the DD wheel has the attr");
        match found.backend {
            Backend::Attr(path) => assert!(path.to_string_lossy().contains(dd)),
            Backend::Classdevs { .. } => panic!("the DD wheel must use its own attribute"),
        }
    }

    /// A wheel with no rev display of its own gets none. Falling back to a
    /// global scan is what produced the cross-wired bug in the first place,
    /// so the absence of a fallback is the fix and needs holding in place.
    #[test]
    fn a_wheel_without_a_rev_display_does_not_borrow_someone_elses() {
        let (sysfs, leds, _dd, _g923) = two_wheel_rig();
        assert!(
            RevLeds::discover_for_at("0003:046D:CAFE.0001", &sysfs, &leds).is_none(),
            "an unknown wheel must get nothing, not the first display in sysfs"
        );
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// Build a fake classdev quintet under `root`, named
    /// `"<prefix>::RPM1".."RPM5"`, each with a `brightness` file
    /// initialized to `"0"`. Returns the 5 brightness paths in
    /// RPM1..RPM5 order.
    fn make_classdevs(root: &Path, prefix: &str) -> [PathBuf; 5] {
        let mut paths = Vec::with_capacity(RPM_SUFFIXES.len());
        for suffix in RPM_SUFFIXES {
            let dir = root.join(format!("{prefix}{suffix}"));
            fs::create_dir_all(&dir).unwrap();
            let brightness = dir.join("brightness");
            fs::write(&brightness, "0").unwrap();
            paths.push(brightness);
        }
        paths.try_into().unwrap()
    }

    #[test]
    fn pit_flash_alternates_full_and_dark_at_the_captured_period() {
        let half = PIT_FLASH_HALF_PERIOD;
        // Lit for the first half period, from the instant it engages.
        assert_eq!(pit_flash_level(Duration::ZERO), 10);
        assert_eq!(pit_flash_level(half - Duration::from_millis(1)), 10);
        // Dark for the second.
        assert_eq!(pit_flash_level(half), 0);
        assert_eq!(pit_flash_level(half + half / 2), 0);
        // And back, so it is a cycle rather than a one-shot.
        assert_eq!(pit_flash_level(half * 2), 10);
        assert_eq!(pit_flash_level(half * 3), 0);
        assert_eq!(pit_flash_level(half * 4), 10);
    }

    #[test]
    fn the_flash_period_matches_the_capture() {
        // ~416.7 ms per half cycle, i.e. a full cycle a little over 1.2 Hz.
        // Guards the constant itself against a careless edit.
        assert_eq!(PIT_FLASH_HALF_PERIOD.as_micros(), 416_700);
        let hz = 1.0 / (PIT_FLASH_HALF_PERIOD.as_secs_f64() * 2.0);
        assert!((hz - 1.2).abs() < 0.01, "full cycle should be ~1.2 Hz, got {hz}");
    }

    #[test]
    fn the_limiter_overrides_rpm_entirely() {
        // At redline the ordinary level is already 10, so the interesting
        // case is the dark half: a screaming engine must still go dark.
        assert_eq!(rev_level(8000.0, 8000.0), 10);
        assert_eq!(pit_flash_level(PIT_FLASH_HALF_PERIOD), 0);
        // And an idling engine must still light the whole strip.
        assert_eq!(rev_level(800.0, 8000.0), 1);
        assert_eq!(pit_flash_level(Duration::ZERO), 10);
    }

    #[test]
    fn rev_level_maps_the_rpm_range_onto_0_to_10() {
        assert_eq!(rev_level(0.0, 8000.0), 0);
        assert_eq!(rev_level(4000.0, 8000.0), 5);
        assert_eq!(rev_level(8000.0, 8000.0), 10);
        assert_eq!(rev_level(360.0, 8000.0), 0, "rounds down below half a step");
        assert_eq!(rev_level(440.0, 8000.0), 1, "rounds up above half a step");
        assert_eq!(rev_level(9000.0, 8000.0), 10, "over-rev clamps");
        assert_eq!(rev_level(-100.0, 8000.0), 0, "negative rpm clamps");
    }

    #[test]
    fn rev_level_guards_a_missing_limiter() {
        assert_eq!(rev_level(5000.0, 0.0), 0);
        assert_eq!(rev_level(5000.0, -1.0), 0);
        assert_eq!(rev_level(5000.0, f32::NAN), 0);
        assert_eq!(rev_level(f32::NAN, 8000.0), 0);
    }

    #[test]
    fn lit_count_maps_the_0_to_10_level_onto_0_to_5_pairs() {
        let expected = [0u8, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5];
        for (level, &want) in expected.iter().enumerate() {
            assert_eq!(lit_count(level as u8), want, "level {level}");
        }
        assert_eq!(lit_count(1), 1, "half a pair rounds up, not down to 0");
        assert_eq!(lit_count(9), 5, "half a pair rounds up to the full 5th pair");
    }

    #[test]
    fn update_writes_only_changed_levels() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();

        leds.update(0.0, 100.0, false, t0);
        assert_eq!(read(&attr), "0", "first sample always lands");

        // Same level again, well past the pacing floor: no write (pinned
        // via a sentinel the skipped write would have replaced).
        fs::write(&attr, "sentinel").unwrap();
        leds.update(1.0, 100.0, false, t0 + Duration::from_secs(1));
        assert_eq!(read(&attr), "sentinel", "unchanged level writes nothing");

        leds.update(50.0, 100.0, false, t0 + Duration::from_secs(2));
        assert_eq!(read(&attr), "5", "changed level lands");
    }

    /// A write that cannot land must not be mistaken for a level that has
    /// not changed, or the daemon retries nothing and reports nothing.
    ///
    /// This is the state two separate investigations ended up in. A G923's
    /// LED classdevs come up 0644 root:root and a DD wheel's
    /// `wheel_rev_level` is 0644 until udev chmods it, so every write
    /// failed while the daemon said it was driving the display. The only
    /// symptom reachable from a bug report was "the lights do not move",
    /// which is also what no telemetry at all looks like.
    #[test]
    fn a_write_that_fails_is_never_recorded_as_written() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        // Read-only for everyone, which is what the pre-udev mode amounts
        // to for a non-root daemon.
        let mut perms = fs::metadata(&attr).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&attr, perms).unwrap();

        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();
        leds.update(50.0, 100.0, false, t0);

        // Running as root defeats the permission bit entirely, so the
        // assertion below would be checking nothing. Skip rather than
        // pass: a test that cannot fail is worse than an absent one.
        if read(&attr) == "5" {
            return;
        }
        assert_eq!(leds.last_level, None, "a failed write must not be cached as the current level");
        assert!(leds.warned, "a failure the user cannot see is one they cannot report");

        // Made writable again: the level still wants writing, because it
        // was never recorded as landed.
        let mut perms = fs::metadata(&attr).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o666);
        }
        fs::set_permissions(&attr, perms).unwrap();
        leds.update(50.0, 100.0, false, t0 + Duration::from_secs(1));
        assert_eq!(read(&attr), "5", "the level must be retried once the write can land");
    }

    #[test]
    fn the_pit_limiter_flashes_the_strip_through_the_real_write_path() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();
        let half = PIT_FLASH_HALF_PERIOD;

        // Engage the limiter at an RPM that would otherwise show 5.
        leds.update(50.0, 100.0, true, t0);
        assert_eq!(read(&attr), "10", "limiter lights the whole strip, not the rev level");

        // Still lit just before the half period is up.
        leds.update(50.0, 100.0, true, t0 + half - Duration::from_millis(20));
        assert_eq!(read(&attr), "10");

        // Dark on the second half, despite RPM being unchanged.
        leds.update(50.0, 100.0, true, t0 + half);
        assert_eq!(read(&attr), "0", "off phase overrides a mid-range RPM");

        // Lit again on the third.
        leds.update(50.0, 100.0, true, t0 + half * 2);
        assert_eq!(read(&attr), "10");

        // Releasing the limiter hands the strip straight back to RPM.
        leds.update(50.0, 100.0, false, t0 + half * 3);
        assert_eq!(read(&attr), "5", "back to the rev level once the limiter clears");
    }

    #[test]
    fn a_second_pit_limiter_starts_lit_rather_than_mid_phase() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();
        let half = PIT_FLASH_HALF_PERIOD;

        leds.update(50.0, 100.0, true, t0);
        assert_eq!(read(&attr), "10");
        leds.update(50.0, 100.0, false, t0 + half / 2);

        // Re-engaging one and a half periods later must light the strip
        // again immediately. A phase measured from a free-running clock
        // would be in its dark half here and start the flash invisible.
        leds.update(50.0, 100.0, true, t0 + half + half / 2);
        assert_eq!(read(&attr), "10", "each limiter starts lit");
    }

    #[test]
    fn update_respects_the_pacing_floor() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        let t0 = Instant::now();

        leds.update(20.0, 100.0, false, t0);
        assert_eq!(read(&attr), "2");

        // A changed level inside the floor is skipped...
        leds.update(50.0, 100.0, false, t0 + Duration::from_millis(8));
        assert_eq!(read(&attr), "2", "no write 8 ms after the last one");

        // ...and picked up by the next call past it (the level still
        // differs from the last WRITTEN one).
        leds.update(50.0, 100.0, false, t0 + MIN_WRITE_INTERVAL);
        assert_eq!(read(&attr), "5");
    }

    #[test]
    fn stop_blanks_and_restores_the_idle_pattern() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        let idle = dir.join("wheel_led_effect");
        fs::write(&attr, "").unwrap();
        fs::write(&idle, "5\n").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        leds.update(100.0, 100.0, false, Instant::now());
        assert_eq!(read(&attr), "10");

        leds.stop();
        assert_eq!(read(&attr), "0", "display blanked");
        assert_eq!(read(&idle), "5", "current effect written back to exit the fill");

        // After a stop the feeder starts fresh: the next sample writes
        // regardless of what was last written before the stop.
        leds.update(0.0, 100.0, false, Instant::now());
        assert_eq!(read(&attr), "0");
    }

    #[test]
    fn stop_without_an_idle_attr_still_blanks() {
        let dir = tempdir();
        let attr = dir.join(ATTR);
        fs::write(&attr, "7").unwrap();
        let mut leds = RevLeds::at(attr.clone());
        leds.stop();
        assert_eq!(read(&attr), "0");
    }

    #[test]
    fn discover_honors_the_sysfs_dir_override() {
        // The only test here that touches the environment; nothing else
        // in this crate reads LOGI_WHEEL_SYSFS_DIR or LOGI_DD_SYSFS_DIR, so
        // it cannot race the other tests.
        let dir = tempdir();
        std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &dir);
        assert!(RevLeds::discover().is_none(), "no attribute file yet");
        fs::write(dir.join(ATTR), "0").unwrap();
        let leds = RevLeds::discover().expect("attribute file present");
        assert!(matches!(&leds.backend, Backend::Attr(p) if p == &dir.join(ATTR)));
        std::env::remove_var("LOGI_WHEEL_SYSFS_DIR");

        // The deprecated LOGI_DD_SYSFS_DIR alias works on its own too.
        std::env::set_var("LOGI_DD_SYSFS_DIR", &dir);
        let via_alias = RevLeds::discover().expect("attribute file present via alias");
        assert!(matches!(&via_alias.backend, Backend::Attr(p) if p == &dir.join(ATTR)));

        // And the new name wins when both are set, even pointing elsewhere.
        let other = tempdir();
        std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &other);
        assert!(RevLeds::discover().is_none(), "new dir has no attribute file, and it must win");
        std::env::remove_var("LOGI_WHEEL_SYSFS_DIR");
        std::env::remove_var("LOGI_DD_SYSFS_DIR");
    }

    // -- G923 classdev backend --------------------------------------------

    #[test]
    fn discover_classdevs_finds_a_complete_quintet_by_common_prefix() {
        let dir = tempdir();
        let expected = make_classdevs(&dir, "0003:046D:C266.0005");
        // An unrelated entry (e.g. another LED on the same box) must not
        // confuse the prefix match.
        fs::create_dir_all(dir.join("input3::capslock")).unwrap();

        let found = discover_classdevs_at(&dir).expect("quintet found");
        assert_eq!(found, expected);
    }

    #[test]
    fn discover_classdevs_ignores_an_incomplete_quintet() {
        let dir = tempdir();
        // Only 4 of the 5 required classdevs (RPM5 missing).
        for suffix in &RPM_SUFFIXES[..4] {
            let d = dir.join(format!("0003:046D:C266.0005{suffix}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("brightness"), "0").unwrap();
        }
        assert!(discover_classdevs_at(&dir).is_none());
    }

    #[test]
    fn discover_classdevs_requires_the_brightness_file_itself() {
        let dir = tempdir();
        for suffix in RPM_SUFFIXES {
            fs::create_dir_all(dir.join(format!("prefix{suffix}"))).unwrap();
        }
        // Classdev directories exist but none has a brightness file yet.
        assert!(discover_classdevs_at(&dir).is_none());
    }

    #[test]
    fn discover_classdevs_returns_paths_in_rpm1_to_rpm5_order() {
        let dir = tempdir();
        let found = discover_classdevs_at(&dir.join("missing")); // sanity: no panic on a missing root
        assert!(found.is_none());

        let paths = make_classdevs(&dir, "wheel");
        for (i, suffix) in RPM_SUFFIXES.iter().enumerate() {
            assert!(paths[i].starts_with(dir.join(format!("wheel{suffix}"))), "slot {i} is {suffix}");
        }
    }

    #[test]
    fn classdev_update_lights_the_right_number_of_pairs_outermost_first() {
        let dir = tempdir();
        let brightness = make_classdevs(&dir, "wheel");
        let mut leds = RevLeds::at_classdevs(brightness.clone());
        let t0 = Instant::now();

        leds.update(30.0, 100.0, false, t0); // level 3 -> 2 pairs lit
        let states: Vec<&str> = brightness.iter().map(|p| if read(p) == "1" { "1" } else { "0" }).collect();
        assert_eq!(states, vec!["1", "1", "0", "0", "0"], "outermost pairs (RPM1, RPM2) light first");
    }

    #[test]
    fn classdev_update_writes_only_the_leds_whose_state_changed() {
        let dir = tempdir();
        let brightness = make_classdevs(&dir, "wheel");
        let mut leds = RevLeds::at_classdevs(brightness.clone());
        let t0 = Instant::now();

        leds.update(30.0, 100.0, false, t0); // level 3 -> lit_count 2
        assert_eq!(read(&brightness[0]), "1");
        assert_eq!(read(&brightness[1]), "1");

        // Pin every file with a sentinel; a level whose lit_count is
        // unchanged (level 4 also maps to 2 pairs) must not rewrite any
        // of them.
        for path in &brightness {
            fs::write(path, "sentinel").unwrap();
        }
        leds.update(40.0, 100.0, false, t0 + Duration::from_secs(1)); // level 4 -> lit_count 2
        for path in &brightness {
            assert_eq!(read(path), "sentinel", "unchanged lit_count rewrites nothing");
        }

        // level 6 -> lit_count 3: only pair index 2 (RPM3) turns on.
        leds.update(60.0, 100.0, false, t0 + Duration::from_secs(2));
        assert_eq!(read(&brightness[2]), "1", "newly-lit pair is written");
        assert_eq!(read(&brightness[0]), "sentinel", "already-lit pairs are left alone");
        assert_eq!(read(&brightness[1]), "sentinel", "already-lit pairs are left alone");
        assert_eq!(read(&brightness[3]), "sentinel", "still-dark pairs are left alone");
        assert_eq!(read(&brightness[4]), "sentinel", "still-dark pairs are left alone");
    }

    #[test]
    fn classdev_stop_blanks_every_pair() {
        let dir = tempdir();
        let brightness = make_classdevs(&dir, "wheel");
        let mut leds = RevLeds::at_classdevs(brightness.clone());
        leds.update(100.0, 100.0, false, Instant::now()); // level 10 -> all 5 lit
        for path in &brightness {
            assert_eq!(read(path), "1");
        }

        leds.stop();
        for path in &brightness {
            assert_eq!(read(path), "0", "every pair blanked on stop");
        }

        // After a stop the feeder starts fresh: the next sample rewrites
        // regardless of what was last written before the stop.
        leds.update(0.0, 100.0, false, Instant::now());
        for path in &brightness {
            assert_eq!(read(path), "0");
        }
    }
}
