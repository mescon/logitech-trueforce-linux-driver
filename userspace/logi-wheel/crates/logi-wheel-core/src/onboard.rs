// SPDX-License-Identifier: GPL-2.0-only
//! Onboard profile slot authoring: editing one of the wheel's five saved
//! slots directly, as opposed to [`crate::profiles`]'s computer-side store.
//!
//! There is no block-write protocol for a slot. A slot is authored by
//! activating it (`wheel_profile`) and then sending the same per-setting
//! sysfs writes every other view uses; the wheel persists each value to the
//! active slot's own storage immediately, with no separate commit step. See
//! `docs/PROTOCOL_SPECIFICATION.md`'s "Onboard profile authoring" section
//! for the wire-level evidence this module is built on.
//!
//! That wire model has three consequences [`OnboardEditor`] exists to
//! manage:
//!
//! - **Activating a slot changes the wheel's live feel right away**: the
//!   motor immediately runs whatever strength/damping/range that slot
//!   already has stored. Callers must warn about this before `begin`, not
//!   after.
//! - **A write can land on the wrong slot** if the wheel gets switched at
//!   its own OLED mid-flow. Every write here re-reads `wheel_profile`
//!   first ([`OnboardEditor::verify_still_active`]) and refuses to proceed
//!   if it has moved, rather than silently authoring whatever slot happens
//!   to be active now.
//! - **There is no transactional write**: a mid-flow failure (or a user
//!   change of mind) leaves a half-edited slot with no way to roll it back
//!   on the wire. [`OnboardEditor::begin`] snapshots every editable value
//!   up front so [`OnboardEditor::revert`] can replay it.

use crate::device::Device;
use crate::error::Error;
use crate::setting::SettingSpec;
use crate::sysfs::SysfsIo;
use crate::value::Value;
use std::fmt;
use std::path::Path;

/// The plain (non-slot-text) attrs an onboard slot stores, per the
/// protocol-authoring report's section 3: rotation range, FFB strength,
/// TrueForce intensity, damping, the FFB filter pair, brake force (onboard-
/// only on the wheel itself, which is exactly the mode this flow runs in),
/// and the LED effect/brightness pair. The slot's name (`wheel_profile_names`,
/// `Kind::SlotText`) is handled separately by [`slot_name`]/
/// [`OnboardEditor::set_name`], since it reads back as the whole 5-slot list
/// but writes one slot at a time.
///
/// Deliberately excludes attrs the report found are NOT slot content:
/// `wheel_sensitivity`/`wheel_response_curve` (desktop-only, G Hub actively
/// reverts the steering curve to linear in every onboard burst),
/// `wheel_combined_pedals` (a desktop-only runtime transform), the pedal/
/// handbrake curves (ignored in native PC mode), and every LED attr besides
/// effect/brightness (colors/slot/apply are a separate staged-then-applied
/// surface, out of scope here).
pub const ONBOARD_ATTRS: &[&str] = &[
    "wheel_range",
    "wheel_strength",
    "wheel_trueforce",
    "wheel_damping",
    "wheel_ffb_filter",
    "wheel_ffb_filter_auto",
    "wheel_brake_force",
    "wheel_led_effect",
    "wheel_led_brightness",
];

/// This crate's own errors, plus the flow-specific failure modes: an
/// out-of-range slot argument, and the wheel having been switched to a
/// different slot mid-flow (the OLED-drift guard every write goes through).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardError {
    /// A `Device`/sysfs error from the underlying read or write.
    Device(Error),
    /// The slot argument to [`OnboardEditor::begin`] was outside 1-5 (0 is
    /// the desktop state, not an onboard slot).
    InvalidSlot(u8),
    /// `wheel_profile` no longer reads back as the slot this editor was
    /// authoring: the wheel was switched (at its own OLED, or by another
    /// tool) since `begin` or the last successful write. Nothing was
    /// written; the caller should tell the user and let them re-pick a
    /// slot rather than silently continuing to author whatever is active
    /// now.
    SlotChanged { expected: u8, actual: u8 },
}

impl fmt::Display for OnboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OnboardError::Device(e) => write!(f, "{e}"),
            OnboardError::InvalidSlot(n) => write!(f, "slot {n} is out of range (1-5)"),
            OnboardError::SlotChanged { expected, actual } => write!(
                f,
                "the wheel switched to slot {actual} (was editing slot {expected}); pick a slot again"
            ),
        }
    }
}
impl std::error::Error for OnboardError {}

impl From<Error> for OnboardError {
    fn from(e: Error) -> Self {
        OnboardError::Device(e)
    }
}

/// The current name of `slot` (1-based), read off `wheel_profile_names`, or
/// `None` when the attr is absent or unreadable. Used to seed the snapshot
/// and to show the slot picker's labels.
pub fn slot_name<S: SysfsIo>(dev: &Device<S>, slot: u8) -> Option<String> {
    match dev.read("wheel_profile_names") {
        Ok(Value::SlotNames(names)) => names.get(slot.saturating_sub(1) as usize).cloned(),
        _ => None,
    }
}

/// The registry rows this flow edits: every [`ONBOARD_ATTRS`] entry the
/// connected wheel actually exposes (`dev.settings()`, not the bare
/// registry, so a G923 - which has no onboard slots at all and never
/// reaches this flow in practice - still resolves to nothing rather than
/// the DD wheel's rows). In registry order, same as every other page.
pub fn editable_specs<S: SysfsIo>(dev: &Device<S>) -> Vec<&'static SettingSpec> {
    dev.settings().iter().filter(|s| ONBOARD_ATTRS.contains(&s.attr)).collect()
}

/// One editable attr's value at the moment `begin` snapshotted it, for
/// [`OnboardEditor::revert`] to replay.
#[derive(Debug)]
struct Snapshot {
    values: Vec<(&'static str, Value)>,
    /// The slot's name, if it was readable at snapshot time.
    name: Option<String>,
}

/// The state machine for authoring one onboard slot: select it, edit its
/// values (each a normal, immediately-persisted sysfs write), optionally
/// revert to how it was found, optionally copy a saved computer profile in,
/// then leave. Holds no `Device`/`SysfsIo` itself - every method takes the
/// device explicitly, so a caller can freely interleave onboard-editor
/// calls with any other device use between them (a frontend's normal
/// per-request device handle keeps working unchanged).
#[derive(Debug)]
pub struct OnboardEditor {
    slot: u8,
    /// `wheel_profile`'s value when `begin` was called, 0-5. Restored by
    /// [`OnboardEditor::finish`] when the caller asks to leave that way.
    previous_slot: u8,
    snapshot: Snapshot,
}

impl OnboardEditor {
    /// Begin authoring `slot` (1-5): remember the currently active slot
    /// (for an optional restore on the way out), switch the wheel to
    /// `slot`, verify the switch actually took by reading `wheel_profile`
    /// back, then snapshot every editable attr's now-current (i.e. `slot`'s
    /// stored) value for [`revert`](Self::revert).
    ///
    /// This is the moment the wheel's motor starts running `slot`'s stored
    /// strength/damping/range; callers must warn about that before calling
    /// this, not after.
    pub fn begin<S: SysfsIo>(dev: &Device<S>, slot: u8) -> Result<Self, OnboardError> {
        if !(1..=5).contains(&slot) {
            return Err(OnboardError::InvalidSlot(slot));
        }
        let previous_slot = read_slot(dev)?;
        dev.write("wheel_profile", &Value::Int(i32::from(slot)))?;
        let actual = read_slot(dev)?;
        if actual != slot {
            return Err(OnboardError::SlotChanged { expected: slot, actual });
        }
        // On real hardware `wheel_mode` already reads "onboard" the instant
        // `wheel_profile` is nonzero (the two attrs read the same
        // underlying active-profile state; see PROTOCOL_SPECIFICATION.md's
        // Profile/Mode Switch section). This best-effort write is defense
        // in depth for the OnboardOnly attrs below (`wheel_brake_force`)
        // rather than something the wire capture ever showed G Hub sending
        // separately: if the wheel is already reporting onboard, it is a
        // harmless no-op; if `wheel_mode` is absent on this wheel, or the
        // write is rejected, editing continues anyway (errors here must
        // never abort `begin`, since the slot switch itself already
        // verified above is what actually matters).
        if dev.available("wheel_mode") {
            let _ = dev.write("wheel_mode", &Value::Enum(1));
        }
        let snapshot = snapshot_slot(dev);
        Ok(OnboardEditor { slot, previous_slot, snapshot })
    }

    /// The slot this editor is authoring (1-5).
    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// The slot that was active when [`begin`](Self::begin) was called
    /// (0-5; 0 is desktop).
    pub fn previous_slot(&self) -> u8 {
        self.previous_slot
    }

    /// Re-read `wheel_profile` and confirm it still matches this editor's
    /// slot. Every write below calls this first; also exposed directly so a
    /// frontend can proactively warn ("the wheel switched slots") before
    /// the user even tries to edit something.
    pub fn verify_still_active<S: SysfsIo>(&self, dev: &Device<S>) -> Result<(), OnboardError> {
        let actual = read_slot(dev)?;
        if actual != self.slot {
            return Err(OnboardError::SlotChanged { expected: self.slot, actual });
        }
        Ok(())
    }

    /// Write one attr to the active slot: the ordinary immediate-persist
    /// sysfs write, guarded by [`verify_still_active`](Self::verify_still_active).
    pub fn set<S: SysfsIo>(&self, dev: &Device<S>, attr: &str, value: &Value) -> Result<(), OnboardError> {
        self.verify_still_active(dev)?;
        dev.write(attr, value)?;
        Ok(())
    }

    /// Rename this slot, guarded the same way as [`set`](Self::set).
    pub fn set_name<S: SysfsIo>(&self, dev: &Device<S>, name: &str) -> Result<(), OnboardError> {
        self.verify_still_active(dev)?;
        dev.write("wheel_profile_names", &Value::SlotName { slot: self.slot, name: name.to_string() })?;
        Ok(())
    }

    /// Replay the snapshot [`begin`](Self::begin) took, restoring every
    /// editable value (and the name, if it was readable) this slot had
    /// before editing started. Guarded by
    /// [`verify_still_active`](Self::verify_still_active) once up front,
    /// same as a single [`set`](Self::set) call would be for one write:
    /// this is one user-triggered action, not several independent ones.
    ///
    /// Per-attr write failures are collected rather than aborting the rest
    /// (mirrors [`crate::profiles::apply_in`]), since a wheel/firmware
    /// quirk on one attr should not leave every other attr un-reverted.
    pub fn revert<S: SysfsIo>(&self, dev: &Device<S>) -> Result<Vec<(String, String)>, OnboardError> {
        self.verify_still_active(dev)?;
        let mut errors = Vec::new();
        for (attr, value) in &self.snapshot.values {
            if let Err(e) = dev.write(attr, value) {
                errors.push(((*attr).to_string(), e.to_string()));
            }
        }
        if let Some(name) = &self.snapshot.name {
            let v = Value::SlotName { slot: self.slot, name: name.clone() };
            if let Err(e) = dev.write("wheel_profile_names", &v) {
                errors.push(("wheel_profile_names".to_string(), e.to_string()));
            }
        }
        Ok(errors)
    }

    /// Copy a saved computer profile into the active slot: read
    /// `<dir>/<name>.profile` (the same file [`crate::profiles::apply_in`]
    /// reads) and replay only the lines whose attr is one of
    /// [`ONBOARD_ATTRS`] - the attrs an onboard slot actually stores. Every
    /// other line in the file (desktop-only attrs like
    /// `wheel_sensitivity`/`wheel_combined_pedals`, which a computer
    /// profile does carry) is skipped outright rather than attempted: this
    /// wheel is in onboard mode for the whole flow, so writing a
    /// `DesktopOnly` attr would only fail with `WrongMode` and clutter the
    /// error list with something the profile format itself makes
    /// unreachable here. `wheel_brake_force` and the LED effect/brightness
    /// pair DO get copied in when the saved profile happens to carry a
    /// value for them (a G923-only profile format quirk aside, a desktop
    /// profile never actually saves `wheel_brake_force`, since
    /// `profiles::snapshotted` excludes `OnboardOnly` attrs - see that
    /// module's doc comment - but the filter here is by attr name, not by
    /// which profiles happen to include it, so it copies through cleanly
    /// if a hand-edited or future profile ever does carry it).
    ///
    /// Guarded once up front like [`revert`](Self::revert); per-attr
    /// failures are collected the same way.
    pub fn copy_from_computer_profile<S: SysfsIo>(
        &self,
        dev: &Device<S>,
        dir: &Path,
        name: &str,
    ) -> Result<Vec<(String, String)>, OnboardError> {
        self.verify_still_active(dev)?;
        let path = crate::profiles::profile_path(dir, name)?;
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Io(e.to_string()))?;
        let mut errors = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((attr, raw)) = line.split_once('=') else { continue };
            if !ONBOARD_ATTRS.contains(&attr) {
                continue;
            }
            let Some(spec) = Device::<S>::spec(attr) else { continue };
            let result = spec.kind.parse(raw).and_then(|v| dev.write(attr, &v));
            if let Err(e) = result {
                errors.push((attr.to_string(), e.to_string()));
            }
        }
        Ok(errors)
    }

    /// Leave the flow. When `restore_previous` is true AND the wheel is
    /// still on this editor's slot (nobody switched it away at the OLED
    /// meanwhile - if they did, that is now their own deliberate choice,
    /// not something to overwrite), switch back to whatever slot was
    /// active before [`begin`](Self::begin). A no-op either way when the
    /// previous slot IS this editor's slot (nothing to restore).
    pub fn finish<S: SysfsIo>(&self, dev: &Device<S>, restore_previous: bool) -> Result<(), OnboardError> {
        if !restore_previous || self.previous_slot == self.slot {
            return Ok(());
        }
        if read_slot(dev)? != self.slot {
            return Ok(());
        }
        dev.write("wheel_profile", &Value::Int(i32::from(self.previous_slot)))?;
        Ok(())
    }
}

fn read_slot<S: SysfsIo>(dev: &Device<S>) -> Result<u8, OnboardError> {
    match dev.read("wheel_profile")? {
        Value::Int(n) if (0..=5).contains(&n) => Ok(n as u8),
        other => Err(OnboardError::Device(Error::Parse(format!("{other:?}")))),
    }
}

/// Read every [`ONBOARD_ATTRS`] entry this wheel exposes, plus the active
/// slot's name, skipping anything unreadable rather than failing the whole
/// snapshot (mirrors [`crate::profiles::save_in`]'s per-attr tolerance): a
/// slot missing one attr (e.g. no brake force on a wheel without a
/// load-cell pedal) must not block editing everything else.
fn snapshot_slot<S: SysfsIo>(dev: &Device<S>) -> Snapshot {
    let values = editable_specs(dev)
        .into_iter()
        .filter(|s| dev.available(s.attr))
        .filter_map(|s| dev.read(s.attr).ok().map(|v| (s.attr, v)))
        .collect();
    // `slot` is read fresh rather than trusted from a caller-supplied
    // argument, so this stays correct even if it is ever called before
    // `wheel_profile` settles.
    let name = match read_slot(dev) {
        Ok(slot) if slot >= 1 => slot_name(dev, slot),
        _ => None,
    };
    Snapshot { values, name }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::FakeSysfs;

    /// A fresh onboard-capable wheel: slot 1 active, 5 named slots, every
    /// [`ONBOARD_ATTRS`] present with a distinct value per slot so tests can
    /// tell slots apart.
    fn wheel() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        fs.set("wheel_profile_names", "1: AC EVO\n2: GT7\n3: PROFILE 3\n4: PROFILE 4\n5: PROFILE 5");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        fs.set("wheel_trueforce", "50");
        fs.set("wheel_damping", "10");
        fs.set("wheel_ffb_filter", "7");
        fs.set("wheel_ffb_filter_auto", "0");
        fs.set("wheel_brake_force", "60");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_led_brightness", "100");
        Device::with_io(fs)
    }

    #[test]
    fn begin_switches_and_verifies_the_slot() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 3).unwrap();
        assert_eq!(editor.slot(), 3);
        assert_eq!(editor.previous_slot(), 1);
        assert_eq!(dev.read("wheel_profile").unwrap(), Value::Int(3));
    }

    #[test]
    fn begin_ensures_wheel_mode_reports_onboard_even_if_it_had_not_caught_up_yet() {
        // The flow only ever starts from the desktop Profiles page (see
        // `logi-wheel-tui`/`logi-wheel-gui`'s entry gating), so `wheel_mode`
        // legitimately starts at "desktop" here; on real hardware it would
        // already read "onboard" the instant `wheel_profile` moves off 0
        // (same underlying state), but this proves `begin` does not rely on
        // that: an OnboardOnly attr (`wheel_brake_force`) must be writable
        // immediately after `begin` returns, not only once some other read
        // catches up.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_brake_force", "50");
        let dev = Device::with_io(fs);
        let editor = OnboardEditor::begin(&dev, 2).unwrap();
        assert_eq!(dev.read("wheel_mode").unwrap(), Value::Enum(1), "onboard");
        editor.set(&dev, "wheel_brake_force", &Value::Percent(70)).unwrap();
        assert_eq!(dev.read("wheel_brake_force").unwrap(), Value::Percent(70));
    }

    #[test]
    fn begin_rejects_slot_0_and_slot_6() {
        let dev = wheel();
        assert_eq!(OnboardEditor::begin(&dev, 0).unwrap_err(), OnboardError::InvalidSlot(0));
        assert_eq!(OnboardEditor::begin(&dev, 6).unwrap_err(), OnboardError::InvalidSlot(6));
    }

    /// A `SysfsIo` wrapper whose writes to `stuck_attr` are silently
    /// swallowed (accepted, but never actually stored): simulates a wheel
    /// that acks a select but does not actually switch (a stale HID++
    /// cache, or a rejected fn2 whose ack does not surface as an error).
    struct StaleWrite<'a> {
        inner: &'a FakeSysfs,
        stuck_attr: &'static str,
    }
    impl SysfsIo for StaleWrite<'_> {
        fn read(&self, attr: &str) -> std::io::Result<String> {
            self.inner.read(attr)
        }
        fn write(&self, attr: &str, val: &str) -> std::io::Result<()> {
            if attr == self.stuck_attr {
                return Ok(());
            }
            self.inner.write(attr, val)
        }
        fn exists(&self, attr: &str) -> bool {
            self.inner.exists(attr)
        }
    }

    #[test]
    fn begin_reports_a_wheel_that_acks_the_select_but_never_actually_switches() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        let dev = Device::with_io(StaleWrite { inner: &fs, stuck_attr: "wheel_profile" });
        let err = OnboardEditor::begin(&dev, 4).unwrap_err();
        assert_eq!(err, OnboardError::SlotChanged { expected: 4, actual: 1 });
    }

    #[test]
    fn begin_propagates_a_write_error_from_the_wheel() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        fs.set_errno("wheel_profile", 22); // EINVAL
        let dev = Device::with_io(fs);
        assert!(matches!(OnboardEditor::begin(&dev, 4), Err(OnboardError::Device(_))));
    }

    #[test]
    fn set_writes_the_attr_when_the_slot_is_still_active() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 2).unwrap();
        editor.set(&dev, "wheel_strength", &Value::Percent(42)).unwrap();
        assert_eq!(dev.read("wheel_strength").unwrap(), Value::Percent(42));
    }

    #[test]
    fn set_refuses_once_the_wheel_switched_slots_underneath_it() {
        // The user's own action mid-flow (or another tool): the guard must
        // catch it before the write, not after.
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 2).unwrap();
        dev.write("wheel_profile", &Value::Int(5)).unwrap(); // simulate an OLED switch
        let err = editor.set(&dev, "wheel_strength", &Value::Percent(42)).unwrap_err();
        assert_eq!(err, OnboardError::SlotChanged { expected: 2, actual: 5 });
        // Nothing was written: slot 5's strength is whatever it already was.
        assert_ne!(dev.read("wheel_strength").unwrap(), Value::Percent(42));
    }

    #[test]
    fn set_name_writes_a_slot_name() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 3).unwrap();
        editor.set_name(&dev, "Race nite").unwrap();
        match dev.read("wheel_profile_names").unwrap() {
            Value::SlotNames(names) => assert_eq!(names[2], "Race nite"),
            other => panic!("expected SlotNames, got {other:?}"),
        }
    }

    #[test]
    fn revert_replays_the_snapshot_taken_at_begin() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        editor.set(&dev, "wheel_strength", &Value::Percent(1)).unwrap();
        editor.set(&dev, "wheel_range", &Value::Int(360)).unwrap();
        editor.set_name(&dev, "SCRATCH").unwrap();
        let errors = editor.revert(&dev).unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(dev.read("wheel_strength").unwrap(), Value::Percent(80));
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(900));
        match dev.read("wheel_profile_names").unwrap() {
            Value::SlotNames(names) => assert_eq!(names[0], "AC EVO"),
            other => panic!("expected SlotNames, got {other:?}"),
        }
    }

    #[test]
    fn revert_collects_per_attr_errors_without_aborting() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        fs.set_errno("wheel_strength", 22); // every write to wheel_strength fails
        let dev = Device::with_io(fs);
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        editor.set(&dev, "wheel_range", &Value::Int(360)).unwrap();
        let errors = editor.revert(&dev).unwrap();
        assert!(errors.iter().any(|(a, _)| a == "wheel_strength"), "{errors:?}");
        // The rest of the revert still went through despite that one failure.
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(900));
    }

    #[test]
    fn revert_refuses_once_the_slot_changed() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        dev.write("wheel_profile", &Value::Int(2)).unwrap();
        assert!(matches!(editor.revert(&dev), Err(OnboardError::SlotChanged { .. })));
    }

    #[test]
    fn snapshot_tolerates_a_missing_attr() {
        // A wheel with no brake-force load cell (or any other ONBOARD_ATTRS
        // entry absent): begin must still succeed, and revert must still
        // restore every attr that WAS there.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        let dev = Device::with_io(fs);
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        editor.set(&dev, "wheel_range", &Value::Int(540)).unwrap();
        let errors = editor.revert(&dev).unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(900));
    }

    #[test]
    fn finish_without_restore_leaves_the_authored_slot_active() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 3).unwrap();
        editor.finish(&dev, false).unwrap();
        assert_eq!(dev.read("wheel_profile").unwrap(), Value::Int(3));
    }

    #[test]
    fn finish_with_restore_puts_the_previous_slot_back() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 3).unwrap();
        editor.finish(&dev, true).unwrap();
        assert_eq!(dev.read("wheel_profile").unwrap(), Value::Int(1), "restored to what was active before begin");
    }

    #[test]
    fn finish_with_restore_is_a_no_op_when_the_previous_slot_is_the_same_slot() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 1).unwrap(); // already active
        editor.finish(&dev, true).unwrap();
        assert_eq!(dev.read("wheel_profile").unwrap(), Value::Int(1));
    }

    #[test]
    fn finish_with_restore_does_not_override_a_slot_the_user_already_switched_to() {
        // The OLED-drift case: the user picked their own slot before
        // leaving the flow. Restoring the pre-flow slot over that would
        // silently undo a deliberate choice.
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 3).unwrap();
        dev.write("wheel_profile", &Value::Int(5)).unwrap();
        editor.finish(&dev, true).unwrap();
        assert_eq!(dev.read("wheel_profile").unwrap(), Value::Int(5), "the user's own switch is left alone");
    }

    #[test]
    fn copy_from_computer_profile_maps_only_onboard_attrs() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-onboard-test-copy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // A realistic desktop-profile file: onboard-editable attrs
        // (range/strength/led_effect) alongside desktop-only ones
        // (sensitivity/combined_pedals) a real computer profile does save
        // (see `profiles::snapshotted`), plus an attr this wheel does not
        // even have.
        std::fs::write(
            dir.join("race.profile"),
            "# logi-wheel profile\nwheel_range=540\nwheel_strength=33\nwheel_sensitivity=70\nwheel_combined_pedals=1\nwheel_led_effect=3\nwheel_bogus=1\n",
        )
        .unwrap();

        // `Rc<FakeSysfs>` so the test keeps its own handle to the write log
        // after handing a clone to `Device` (see `sysfs::SysfsIo for Rc<T>`).
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        fs.set("wheel_led_effect", "1");
        let dev = Device::with_io(fs.clone());
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        let errors = editor.copy_from_computer_profile(&dev, &dir, "race").unwrap();
        assert_eq!(errors, Vec::new(), "{errors:?}");
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(540));
        assert_eq!(dev.read("wheel_strength").unwrap(), Value::Percent(33));
        assert_eq!(dev.read("wheel_led_effect").unwrap(), Value::Int(3));
        // The desktop-only attr was never even attempted; onboard mode
        // would reject it, and it is not a slot field anyway.
        assert!(fs.writes().iter().all(|(a, _)| a != "wheel_sensitivity"));
        assert!(fs.writes().iter().all(|(a, _)| a != "wheel_combined_pedals"));
        assert!(fs.writes().iter().all(|(a, _)| a != "wheel_bogus"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_from_computer_profile_refuses_once_the_slot_changed() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-onboard-test-copy-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("race.profile"), "wheel_range=540\n").unwrap();

        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        dev.write("wheel_profile", &Value::Int(2)).unwrap();
        assert!(matches!(
            editor.copy_from_computer_profile(&dev, &dir, "race"),
            Err(OnboardError::SlotChanged { .. })
        ));
        assert_eq!(dev.read("wheel_range").unwrap(), Value::Int(900), "nothing was copied in");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_from_computer_profile_of_a_missing_file_errors() {
        let dev = wheel();
        let editor = OnboardEditor::begin(&dev, 1).unwrap();
        assert!(editor
            .copy_from_computer_profile(&dev, std::path::Path::new("/nonexistent-logi-wheel-dir"), "nope")
            .is_err());
    }

    #[test]
    fn editable_specs_lists_every_onboard_attrs_entry_present_on_this_wheel() {
        let dev = wheel();
        let attrs: Vec<&str> = editable_specs(&dev).iter().map(|s| s.attr).collect();
        for a in ONBOARD_ATTRS {
            assert!(attrs.contains(a), "{a}");
        }
    }

    #[test]
    fn slot_name_reads_the_requested_1_based_slot() {
        let dev = wheel();
        assert_eq!(slot_name(&dev, 1).as_deref(), Some("AC EVO"));
        assert_eq!(slot_name(&dev, 2).as_deref(), Some("GT7"));
    }

    #[test]
    fn onboard_error_display_is_readable() {
        assert_eq!(OnboardError::InvalidSlot(9).to_string(), "slot 9 is out of range (1-5)");
        assert!(OnboardError::SlotChanged { expected: 1, actual: 2 }.to_string().contains("slot 2"));
    }
}
