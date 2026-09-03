//! The base's screen as a dashboard, driven from the same telemetry as the
//! rev lights.
//!
//! The driver owns the panel's rules (`wheel_oled`: layouts, widths,
//! refresh, handback), so this writes one string: a template such as
//! `G|{gear}|{speed}` with its placeholders filled from the latest
//! [`Telemetry`]. Written only when the rendered text changes, paced so a
//! 60 Hz feed does not become 60 sysfs writes a second, and `off` on stop
//! so the wheel gets its menu back at once rather than after its own
//! timeout.
//!
//! Placeholders: `{gear}` (R, N, 1..), `{speed}` km/h, `{speed_mph}`,
//! `{rpm}`, and for the gauge layouts' number fields `{rpm_pct}`,
//! `{throttle_pct}`, `{brake_pct}` as 0..255. Anything else is left as
//! written, so a template can carry literal text.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use logi_wheel_core::telemetry::Telemetry;

pub const ATTR: &str = "wheel_oled";
const SYSFS_ROOT: &str = "/sys/bus/hid/devices";

/// Floor between writes. The driver refreshes the panel itself; this only
/// has to keep up with a human reading numbers.
const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(100);

/// The default template: the gear-and-speed screen, layout G.
pub const DEFAULT_TEMPLATE: &str = "G|{gear}|{speed}";

pub struct Screen {
    attr: PathBuf,
    last: Option<String>,
    last_write: Option<Instant>,
    warned: bool,
}

impl Screen {
    /// The first direct-drive wheel exposing `wheel_oled`, honouring the
    /// same sysfs override the rev lights use.
    pub fn discover() -> Option<Screen> {
        if let Some(dir) = crate::leds::sysfs_dir_override() {
            let attr = PathBuf::from(dir).join(ATTR);
            return attr.exists().then(|| Screen::at(attr));
        }
        for entry in std::fs::read_dir(SYSFS_ROOT).ok()?.flatten() {
            let attr = entry.path().join(ATTR);
            if attr.exists() {
                return Some(Screen::at(attr));
            }
        }
        None
    }

    /// A screen at an explicit `wheel_oled` path (tests use a plain file).
    pub fn at(attr: PathBuf) -> Screen {
        Screen { attr, last: None, last_write: None, warned: false }
    }

    pub fn path(&self) -> &Path {
        &self.attr
    }

    /// Render `template` from `tel` and write it if it changed and the
    /// pacing floor has passed. Errors are reported once per streak: a
    /// template wider than its layout is refused by the driver with
    /// `EMSGSIZE`, and saying so once is enough.
    pub fn update(&mut self, tel: &Telemetry, template: &str, now: Instant) {
        let text = render(template, tel);
        if self.last.as_deref() == Some(text.as_str()) {
            return;
        }
        if let Some(t) = self.last_write {
            if now.duration_since(t) < MIN_WRITE_INTERVAL {
                return;
            }
        }
        self.last_write = Some(now);
        match std::fs::write(&self.attr, format!("{text}\n")) {
            Ok(()) => {
                self.last = Some(text);
                self.warned = false;
            }
            Err(e) => {
                if !self.warned {
                    eprintln!("logi-tf-sim: screen write failed ({e}); template {template:?}");
                    self.warned = true;
                }
            }
        }
    }

    /// Hand the screen back to the wheel.
    pub fn stop(&mut self) {
        let _ = std::fs::write(&self.attr, "off\n");
        self.last = None;
    }
}

fn gear_text(gear: i8) -> String {
    match gear {
        g if g < 0 => "R".to_string(),
        0 => "N".to_string(),
        g => g.to_string(),
    }
}

fn pct255(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// Fill a template's placeholders from a telemetry sample. Unknown
/// placeholders are left as written.
pub fn render(template: &str, tel: &Telemetry) -> String {
    let rpm_frac = if tel.max_rpm > 0.0 { tel.rpm / tel.max_rpm } else { 0.0 };
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = &after[..end];
        let value = match key {
            "gear" => Some(gear_text(tel.gear)),
            "speed" => Some(((tel.speed * 3.6).round() as i64).to_string()),
            "speed_mph" => Some(((tel.speed * 2.236_94).round() as i64).to_string()),
            "rpm" => Some((tel.rpm.round() as i64).to_string()),
            "rpm_pct" => Some(pct255(rpm_frac).to_string()),
            "throttle_pct" => Some(pct255(tel.throttle).to_string()),
            "brake_pct" => Some(pct255(tel.brake).to_string()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tel() -> Telemetry {
        Telemetry {
            rpm: 6000.0,
            max_rpm: 8000.0,
            speed: 39.44, // m/s, so 142 km/h
            throttle: 0.5,
            brake: 1.0,
            gear: 3,
            ..Telemetry::default()
        }
    }

    #[test]
    fn the_default_template_is_gear_and_speed() {
        assert_eq!(render(DEFAULT_TEMPLATE, &tel()), "G|3|142");
    }

    #[test]
    fn every_placeholder_renders_and_unknown_ones_survive() {
        let t = tel();
        assert_eq!(render("{gear} {speed} {speed_mph} {rpm}", &t), "3 142 88 6000");
        assert_eq!(render("C|{rpm_pct}", &t), "C|191");
        assert_eq!(render("D|{throttle_pct}|{brake_pct}|x", &t), "D|128|255|x");
        assert_eq!(render("{nope} {gear", &t), "{nope} {gear");
        assert_eq!(render("R: {gear}", &Telemetry { gear: -1, ..t }), "R: R");
        assert_eq!(render("N: {gear}", &Telemetry { gear: 0, ..t }), "N: N");
    }

    #[test]
    fn writes_only_on_change_and_hands_back_on_stop() {
        let dir = std::env::temp_dir().join(format!("tfsim-screen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let attr = dir.join(ATTR);
        std::fs::write(&attr, "off\n").unwrap();
        let mut s = Screen::at(attr.clone());
        let t0 = Instant::now();

        s.update(&tel(), DEFAULT_TEMPLATE, t0);
        assert_eq!(std::fs::read_to_string(&attr).unwrap(), "G|3|142\n");

        // Changed text 50 ms after the last write: held by the floor.
        let faster = Telemetry { speed: 50.0, ..tel() };
        s.update(&faster, DEFAULT_TEMPLATE, t0 + Duration::from_millis(50));
        assert_eq!(std::fs::read_to_string(&attr).unwrap(), "G|3|142\n");
        // Past the floor: written.
        s.update(&faster, DEFAULT_TEMPLATE, t0 + Duration::from_millis(200));
        assert_eq!(std::fs::read_to_string(&attr).unwrap(), "G|3|180\n");

        // Same text again, long after: nothing rewritten at all.
        std::fs::write(&attr, "sentinel\n").unwrap();
        s.update(&faster, DEFAULT_TEMPLATE, t0 + Duration::from_secs(2));
        assert_eq!(std::fs::read_to_string(&attr).unwrap(), "sentinel\n");

        s.stop();
        assert_eq!(std::fs::read_to_string(&attr).unwrap(), "off\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
