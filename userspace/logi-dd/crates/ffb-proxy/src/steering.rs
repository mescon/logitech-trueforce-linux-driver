//! Per-Wine-prefix enumeration steering.
//!
//! Games see two look-alike wheels once the virtual proxy device exists
//! alongside the real one: the real Logitech wheel and the uhid clone this
//! proxy creates. This module computes (and applies) the environment
//! variables and Wine registry entry that hide the real device from the
//! game, so it binds only to the virtual proxy.
//!
//! Two independent mechanisms are covered because games probe for joysticks
//! through different layers:
//! - SDL-based titles read `SDL_GAMECONTROLLER_IGNORE_DEVICES` to skip a
//!   device by vid/pid before it is ever opened.
//! - Wine's DirectInput enumerates HID joysticks by name and consults a
//!   per-user registry key (`HKCU\Software\Wine\DirectInput\Joysticks`) for
//!   devices explicitly marked "disabled".
//!
//! The exact discriminator (vid/pid vs. name) that reliably hides only the
//! real device without also hiding the virtual clone is validated against
//! real hardware in a later task. Keeping the computed values in one place
//! (`plan_for`) means that adjustment stays localized to this module.

use std::fs;
use std::path::Path;

use crate::{Error, Result};

/// The Wine registry section that lists DirectInput joysticks the user (or
/// we) has disabled.
const WINE_DINPUT_JOYSTICKS_SECTION: &str = "[Software\\\\Wine\\\\DirectInput\\\\Joysticks]";

/// A computed enumeration-steering plan for one real-wheel identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Environment variable pairs to inject into the game process.
    pub env: Vec<(String, String)>,
    /// The exact device name Wine's DirectInput disabled-devices list keys
    /// on (i.e. the real wheel's HID product name).
    pub reg_disable_name: String,
}

/// Compute the steering plan for hiding the real wheel identified by
/// `real_vendor`/`real_product`/`real_name` from the game.
///
/// Pure: does not touch the filesystem or environment.
pub fn plan_for(real_vendor: u16, real_product: u16, real_name: &str) -> Plan {
    let env = vec![(
        "SDL_GAMECONTROLLER_IGNORE_DEVICES".to_string(),
        format!("0x{real_vendor:04x}/0x{real_product:04x}"),
    )];

    Plan { env, reg_disable_name: real_name.to_string() }
}

/// The env pairs to inject into the game process for this plan.
pub fn child_env(plan: &Plan) -> Vec<(String, String)> {
    plan.env.clone()
}

/// Build the line that disables `name` in Wine's DirectInput joystick list.
fn disable_line(name: &str) -> String {
    format!("\"{name}\"=\"disabled\"")
}

/// Apply `plan` to a Wine prefix: if `wineprefix` is `Some(path)`, ensure
/// `<path>/user.reg` disables `plan.reg_disable_name` in DirectInput. If the
/// disable line is already present, nothing is written (idempotent). If
/// `wineprefix` is `None`, this is a no-op (env-only steering).
pub fn apply(plan: &Plan, wineprefix: Option<&str>) -> Result<()> {
    let Some(prefix) = wineprefix else {
        return Ok(());
    };

    let reg_path = Path::new(prefix).join("user.reg");
    let line = disable_line(&plan.reg_disable_name);

    let existing = match fs::read_to_string(&reg_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::Io(format!("read {}", reg_path.display()), e)),
    };

    if existing.lines().any(|l| l == line) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(WINE_DINPUT_JOYSTICKS_SECTION);
    updated.push('\n');
    updated.push_str(&line);
    updated.push('\n');

    fs::write(&reg_path, updated).map_err(|e| Error::Io(format!("write {}", reg_path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_targets_real_device_vidpid_and_name() {
        let p = plan_for(0x046d, 0xc276, "Logitech RS50 Base for PlayStation/PC");
        assert!(p.env.iter().any(|(k, v)| k == "SDL_GAMECONTROLLER_IGNORE_DEVICES" && v.contains("0x046d/0xc276")));
        assert_eq!(p.reg_disable_name, "Logitech RS50 Base for PlayStation/PC");
    }

    #[test]
    fn child_env_returns_plan_env() {
        let p = plan_for(0x046d, 0xc262, "Logitech G PRO Racing Wheel");
        assert_eq!(child_env(&p), p.env);
    }

    #[test]
    fn apply_is_noop_without_a_wineprefix() {
        let p = plan_for(0x046d, 0xc276, "Logitech RS50 Base for PlayStation/PC");
        assert!(apply(&p, None).is_ok());
    }

    #[test]
    fn apply_is_idempotent_on_an_existing_prefix() {
        let dir = std::env::temp_dir().join(format!("logi-ffb-steering-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let reg_path = dir.join("user.reg");
        // Seed a user.reg with unrelated content to make sure apply appends
        // rather than clobbers.
        fs::write(&reg_path, "[Software\\\\Wine\\\\Some\\\\Other]\n\"Key\"=\"Value\"\n").unwrap();

        let p = plan_for(0x046d, 0xc276, "Logitech RS50 Base for PlayStation/PC");
        let prefix = dir.to_str().unwrap();

        apply(&p, Some(prefix)).unwrap();
        apply(&p, Some(prefix)).unwrap();

        let contents = fs::read_to_string(&reg_path).unwrap();
        let disable = disable_line(&p.reg_disable_name);
        let occurrences = contents.lines().filter(|l| *l == disable).count();
        assert_eq!(occurrences, 1, "disable line should appear exactly once, got:\n{contents}");
        assert!(contents.contains("\"Key\"=\"Value\""), "pre-existing content should be preserved");

        fs::remove_dir_all(&dir).ok();
    }
}
