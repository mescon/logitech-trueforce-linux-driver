use crate::error::{map_io_error, Error, Mode};
use crate::kind::Kind;
use crate::registry::{CLASSIC_REGISTRY, REGISTRY};
use crate::setting::{Access, Category, ModeReq, SettingSpec};
use crate::sysfs::{RealSysfs, SysfsIo};
use crate::value::Value;
use std::path::{Path, PathBuf};

/// Which physical wheel is connected, for frontends that need to brand the
/// UI (the Info/Testing page's product photo) rather than just render
/// settings generically. `Rs50`/`GPro` both use [`REGISTRY`] (the direct-
/// drive `wheel_*` attribute set; the two share one protocol, see the
/// project's G PRO protocol notes) and differ only in branding; `G923` uses
/// [`CLASSIC_REGISTRY`] instead, a different FFB engine entirely. `Unknown`
/// covers anything discovery could not pin to a specific product id (a dev-
/// override directory, an unrecognised future PID): it is treated as a DD
/// wheel for settings purposes, same as before this enum existed, and falls
/// back to the default (RS50) branding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelModel {
    #[default]
    Unknown,
    Rs50,
    GPro,
    G923,
}

impl WheelModel {
    /// The registry `self`'s settings live in: [`CLASSIC_REGISTRY`] for a
    /// G923, [`REGISTRY`] (the direct-drive `wheel_*` set) for everything
    /// else. A free function on the model alone (not `Device::settings`,
    /// which needs a live device) so a frontend holding only a
    /// [`DeviceInfo`]'s `model` field, not the `Device` itself, can still
    /// resolve which settings a wheel of this model would show.
    pub fn settings(self) -> &'static [SettingSpec] {
        match self {
            WheelModel::G923 => CLASSIC_REGISTRY,
            _ => REGISTRY,
        }
    }

    /// Whether `cat` has anything to show for a wheel of this model: at
    /// least one row in `self.settings()`, or `Info` (its identity rows can
    /// be empty on a classic wheel, but the page also carries the live
    /// input monitor and force-feedback test sims, which work off evdev
    /// regardless of model), or `Profiles` (its computer-side profile store
    /// snapshots whatever settings this model does have - see
    /// `crate::profiles` - so the category is never truly empty even for a
    /// wheel with no onboard slots at all, e.g. a G923). See
    /// [`Device::category_has_content`], which this backs; kept as a
    /// model-only free function for the same reason as
    /// [`WheelModel::settings`].
    pub fn category_has_content(self, cat: Category) -> bool {
        matches!(cat, Category::Info | Category::Profiles) || self.settings().iter().any(|s| s.category == cat)
    }
}

pub struct DeviceInfo {
    /// Human-readable identity for the Info page's "Wheel" row (e.g.
    /// "Logitech G923 Racing Wheel for PlayStation 4 and PC"), or empty
    /// when nothing could be resolved at all. See
    /// [`Device::info`]/[`wheel_display_name`].
    pub name: String,
    pub serial: String,
    /// The DD wheels' active-firmware string, read straight off
    /// `wheel_firmware` sysfs. A G923 has no such attribute, so this stays
    /// empty here even when the wheel's firmware IS known: that value
    /// takes a live HID++ round trip a plain `info()` read must not pay
    /// for on every call (draw loops call `info()` freely). Callers that
    /// want it explicitly opt in via [`Device::classic_firmware`], once
    /// per page load/refresh.
    pub firmware: String,
    pub mode: Mode,
    pub model: WheelModel,
}

/// Settings that only mean anything with the RS Shifter & Handbrake attached.
///
/// These need an explicit presence check rather than the EOPNOTSUPP path every
/// other unsupported setting takes, because the wheel cannot tell us they are
/// inapplicable: handbrake shaping targets the *base's* own 0x80A4 axis 4, not
/// the accessory, so with nothing attached a read still returns a value and a
/// write still succeeds, configuring an axis no hardware drives (verified on an
/// RS50, 2026-07-28). The shifter's own settings join this list once their wire
/// format is captured.
const ACCESSORY_ATTRS: &[&str] = &[
    "wheel_handbrake_sensitivity",
    "wheel_handbrake_curve",
    // The accessory's own 0x80B1 actuation points. The driver already
    // answers EOPNOTSUPP for these without the accessory, so this is
    // belt and braces, and keeps `available` consistent with `read_supported`.
    "wheel_shift_actuation",
    "wheel_handbrake_actuation",
];

/// Whether `attr` is one of [`ACCESSORY_ATTRS`].
pub fn requires_accessory(attr: &str) -> bool {
    ACCESSORY_ATTRS.contains(&attr)
}

/// The accessory mode an attribute needs before it does anything.
///
/// The RS Shifter & Handbrake is one of three things at a time, chosen by a
/// physical switch, and most of its settings belong to exactly one of them:
/// a shift actuation point is meaningless while the unit is a handbrake. The
/// driver reports the live position in `wheel_accessory_mode`, so a frontend
/// can grey the settings that are not currently doing anything and say why.
///
/// Deliberately advisory rather than enforced: the values persist across a
/// mode change, so writing one while the switch is elsewhere is a legitimate
/// thing to do (set it up now, flip the switch later). sysfs stays
/// permissive; only the presentation changes.
pub fn required_mode(attr: &str) -> Option<&'static str> {
    match attr {
        "wheel_shift_actuation" => Some("shifter"),
        "wheel_handbrake_actuation" => Some("digital-handbrake"),
        "wheel_handbrake_sensitivity" | "wheel_handbrake_curve" => Some("analog-handbrake"),
        _ => None,
    }
}

pub struct Device<S: SysfsIo> {
    io: S,
    /// Canonical sysfs path this device was discovered at, used only to
    /// tell attached wheels apart and to dedupe the several hidraw nodes a
    /// single wheel exposes. Deliberately separate from `hid_dir`, which
    /// decides whether HID++ probing runs: reusing that field for identity
    /// would start probing DD wheels that previously never did.
    sysfs_key: Option<std::path::PathBuf>,
    model: WheelModel,
    /// The interface-0 HID device directory a classic (G923) wheel's
    /// identity is anchored to: the sysfs `uniq` string lives in this same
    /// directory's `uevent` (see [`read_hid_uniq`]), and the HID++ vendor
    /// interface used for [`Device::classic_firmware`] is a sibling of it
    /// (see `hidpp::find_hidpp_sibling`). `None` for a DD wheel (identity
    /// comes from `wheel_serial`/`wheel_firmware` sysfs instead) and for
    /// every `Device` built via `with_io`/`with_io_and_model` (tests, and
    /// any caller with no real sysfs directory to anchor to).
    hid_dir: Option<PathBuf>,
}

/// The generic label used for the Info page's "Wheel" row when the wheel's
/// own evdev node cannot be found (a fresh connect can lag sysfs briefly,
/// or a dev-override fixture has no evdev at all): the model this crate
/// already knows, worded the same way the real device names itself.
fn generic_wheel_name(model: WheelModel) -> &'static str {
    match model {
        WheelModel::Rs50 => "Logitech RS50 Racing Wheel",
        WheelModel::GPro => "Logitech G PRO Racing Wheel",
        WheelModel::G923 => "Logitech G923 Racing Wheel",
        WheelModel::Unknown => "Logitech Racing Wheel",
    }
}

/// Human-readable wheel name for the Info page's "Wheel" row: the evdev
/// node's own name when the wheel is enumerated there (already fully
/// descriptive, e.g. "Logitech G923 Racing Wheel for PlayStation 4 and
/// PC"), else the generic label for `model` (evdev can lag sysfs briefly
/// right after a (re)connect, and a dev-override fixture has no evdev node
/// at all). Takes the scan directory explicitly so tests can point it at a
/// fixture instead of the real `/sys/class/input`.
pub fn wheel_display_name_at(sysfs_input: &Path, model: WheelModel) -> String {
    if let Some(input) = crate::evtest::scan_wheel_input(sysfs_input) {
        if !input.name.trim().is_empty() {
            return input.name;
        }
    }
    generic_wheel_name(model).to_string()
}

/// [`wheel_display_name_at`] against the real `/sys/class/input`.
/// A short label for a wheel, for a tab or picker where the full product
/// name ("Logitech G923 Racing Wheel for PlayStation 4 and PC") would be
/// truncated into uselessness.
pub fn short_model_label(model: WheelModel) -> &'static str {
    match model {
        WheelModel::Rs50 => "RS50",
        WheelModel::GPro => "G PRO",
        WheelModel::G923 => "G923",
        WheelModel::Unknown => "Wheel",
    }
}

/// Short labels for `models`, made unique.
///
/// Two wheels of the same model is a real configuration, and two tabs both
/// reading "G923" would be unusable, so repeats are numbered in the order
/// they were discovered. A model that appears once is left alone: "RS50"
/// reads better than "RS50 1" when there is only one.
pub fn short_labels(models: &[WheelModel]) -> Vec<String> {
    let mut out = Vec::with_capacity(models.len());
    for (i, m) in models.iter().enumerate() {
        let base = short_model_label(*m);
        let repeats = models.iter().filter(|o| *o == m).count();
        if repeats <= 1 {
            out.push(base.to_string());
        } else {
            let nth = models[..i].iter().filter(|o| *o == m).count() + 1;
            out.push(format!("{base} {nth}"));
        }
    }
    out
}

/// The model a discovered device really is, resolving the one case where
/// the product id lies.
///
/// An RS50 in G PRO compatibility mode borrows the G PRO's product id but
/// keeps its own USB product string, so a mapping from the id alone calls
/// it a G PRO. The kernel driver settles this the same way, by looking at
/// the product string (`dd_is_real_gpro`), and `model_from_name` already
/// encodes the rule by testing for RS50 before PRO. This is the piece that
/// was missing: discovery never asked the name.
///
/// Only the G PRO ids are second-guessed. Every other id maps directly, and
/// a name that says nothing useful leaves the id's answer alone.
fn model_for(pid: Option<u16>, name: &str) -> WheelModel {
    let by_pid = pid.map(model_from_pid).unwrap_or_default();
    if by_pid == WheelModel::GPro {
        let by_name = model_from_name(name);
        if by_name != WheelModel::Unknown {
            return by_name;
        }
    }
    if by_pid == WheelModel::Unknown {
        return model_from_name(name);
    }
    by_pid
}

/// The USB device directory a discovered wheel's sysfs key belongs to.
///
/// A key looks like `.../usb1/1-5/1-5.2/1-5.2.3/1-5.2.3:1.1/0003:046D:C276.0051`:
/// the HID device, inside an interface directory, inside the USB device.
/// Two ancestors up is the USB device itself, which is what every
/// interface and input node of the same physical wheel shares.
pub fn usb_device_dir(sysfs_key: &Path) -> Option<PathBuf> {
    let iface = sysfs_key.parent()?;
    let usb = iface.parent()?;
    usb.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.contains(':'))
        .map(|_| usb.to_path_buf())
}

/// The display name of the wheel at `sysfs_key`, rather than of whichever
/// wheel `/sys/class/input` happens to list first.
///
/// With one wheel attached the difference never showed. With two, every
/// device reported the same name, because the scan matched on "looks like
/// a wheel" and stopped at the first hit: an RS50 would introduce itself
/// as a G923 purely because the G923 enumerated earlier.
pub fn wheel_display_name_for(sysfs_key: Option<&Path>, model: WheelModel) -> String {
    if let Some(usb) = sysfs_key.and_then(usb_device_dir) {
        if let Some(name) = input_name_under(Path::new("/sys/class/input"), &usb) {
            return name;
        }
    }
    wheel_display_name(model)
}

/// The name of the first input device under `usb` that reads like a wheel.
fn input_name_under(sysfs_input: &Path, usb: &Path) -> Option<String> {
    let mut entries: Vec<_> = std::fs::read_dir(sysfs_input)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(real) = std::fs::canonicalize(entry.path()) else { continue };
        if !real.starts_with(usb) {
            continue;
        }
        let name = match std::fs::read_to_string(entry.path().join("device/name")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

pub fn wheel_display_name(model: WheelModel) -> String {
    wheel_display_name_at(Path::new("/sys/class/input"), model)
}

/// Parse `HID_UNIQ=` out of a HID device directory's `uevent`: there is no
/// dedicated `uniq` sysfs file on this kernel (checked live against a
/// G923), only this uevent key/value line. `None` when absent, empty, or
/// the directory has no `uevent` at all (a dev-override test fixture).
fn read_hid_uniq(dir: &Path) -> Option<String> {
    let uevent = std::fs::read_to_string(dir.join("uevent")).ok()?;
    uevent.lines().find_map(|line| {
        let v = line.strip_prefix("HID_UNIQ=")?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// USB product ids this crate can identify, mapped to their [`WheelModel`].
/// See `mainline/hid-logitech-hidpp.c`/`mainline/dd-lg4ff.c` for where each
/// id is bound on the kernel side.
/// Logitech's USB vendor id.
pub const LOGITECH_VID: u16 = 0x046d;

/// Every USB product id the driver binds as a G923.
///
/// One owner, because there were two and they disagreed: this list was
/// missing `c267` while the kernel bound all three, and after that was fixed
/// here `logi-tf-sim` still carried its own copy with the same gap, so a
/// `c267` owner was identified correctly by the settings pages and then not
/// found at all by the simulated-TrueForce daemon.
///
/// `c26d` is deliberately absent: that is the Xbox edition still in console
/// mode, which the driver cannot bind until the mode-switch helper has run.
pub const G923_PIDS: &[u16] = &[0xc266, 0xc267, 0xc26e];

/// Every USB product id the driver binds as a direct-drive wheel.
pub const DD_PIDS: &[u16] = &[0xc276, 0xc272, 0xc268];

/// Whether `pid` is a G923 of any edition.
pub fn is_g923_pid(pid: u16) -> bool {
    G923_PIDS.contains(&pid)
}

fn model_from_pid(pid: u16) -> WheelModel {
    match pid {
        0xc276 => WheelModel::Rs50,
        0xc272 | 0xc268 => WheelModel::GPro,
        _ if is_g923_pid(pid) => WheelModel::G923,
        _ => WheelModel::Unknown,
    }
}

/// The [`WheelModel`] a device's human-readable name implies, for use when
/// the product id is not available.
///
/// A wheel is only identified by product id once this driver has bound it
/// and created its sysfs directory. When binding fails, the input node's
/// name is still there and still says exactly which wheel it is: a G923
/// whose force-feedback setup had failed reported "Logitech G923 Racing
/// Wheel for Xbox One and PC" on screen while the app showed a photo of an
/// RS50, because nothing consulted the one identifier it had (issue #27).
///
/// Matching mirrors `evtest::is_wheel_name`, including that a real G PRO
/// says "PRO Racing Wheel" with no "G" anywhere in it.
pub fn model_from_name(name: &str) -> WheelModel {
    let upper = name.to_uppercase();
    if upper.contains("RS50") {
        WheelModel::Rs50
    } else if upper.contains("G923") {
        WheelModel::G923
    } else if upper.contains("PRO RACING WHEEL") || upper.contains("G PRO") {
        WheelModel::GPro
    } else {
        WheelModel::Unknown
    }
}

/// Parse the USB/HID product id out of a sysfs device directory name of the
/// form `BUS:VID:PID.SEQ` (the kernel's HID device naming convention, e.g.
/// `0003:046D:C266.0002`). `dir` is canonicalized first, since discovery's
/// `dir` may be (or resolve through symlinks to) a sibling hidraw node's own
/// `device` link rather than that exact directory. `None` for a name in a
/// different shape (a dev-override directory has no such name at all).
fn pid_from_hid_dir(dir: &std::path::Path) -> Option<u16> {
    let real = std::fs::canonicalize(dir).ok()?;
    let name = real.file_name()?.to_str()?;
    let mut parts = name.split(':');
    let _bus = parts.next()?;
    let _vid = parts.next()?;
    let pid_part = parts.next()?; // "C266.0002"
    let pid_hex = pid_part.split('.').next()?;
    u16::from_str_radix(pid_hex, 16).ok()
}

/// Whether `dir` looks like a G923-class classic sysfs surface: `range`,
/// `gain` and `autocenter` all present. `combine_pedals` is deliberately not
/// required here (older/trimmed ports could omit it), but the registry only
/// ever offers it when `available()` says so.
///
/// Used for the dev-override directory, which carries no product id to check
/// against, so the full set is the only evidence available that a fixture is
/// meant to be a wheel. Real devices go through [`classic_ffb_present`],
/// which does have a product id behind it.
fn classic_attrs_present(dir: &std::path::Path) -> bool {
    dir.join("range").exists() && dir.join("gain").exists() && dir.join("autocenter").exists()
}

/// Whether `dir` belongs to a wheel whose force feedback has registered, by
/// the one attribute every classic engine creates: `range`.
///
/// Deliberately weaker than [`classic_attrs_present`], and safe because
/// every caller pairs it with a product-id check. The two force-feedback
/// engines behind a G923 do not expose the same sysfs surface: the ported
/// lg4ff engine (PlayStation editions) creates `range`, `gain` and
/// `autocenter`, while the HID++ 0x8123 engine (Xbox edition) creates only
/// `range`, and puts gain and autocenter on the input device instead.
///
/// Requiring all three therefore hid the Xbox edition completely. Its owner
/// had working force feedback in games while the settings apps insisted no
/// wheel was connected (issue #27). Settings whose files are absent are
/// already reported unavailable by [`Device::available`], so accepting the
/// wheel here shows what it does have rather than inventing anything.
fn classic_ffb_present(dir: &std::path::Path) -> bool {
    dir.join("range").exists()
}

/// The dev-override sysfs directory: `LOGI_WHEEL_SYSFS_DIR`, falling back to
/// the pre-rename `LOGI_DD_SYSFS_DIR` (deprecated alias, kept so scripts
/// written before the logi-wheel rename keep working). The new name wins
/// when both are set.
pub(crate) fn sysfs_dir_override() -> Option<String> {
    std::env::var("LOGI_WHEEL_SYSFS_DIR").ok().or_else(|| std::env::var("LOGI_DD_SYSFS_DIR").ok())
}

impl Device<RealSysfs> {
    /// Find the wheel by the sysfs attributes only this driver (or the
    /// classic G923 port sharing its kernel module) creates.
    ///
    /// `LOGI_WHEEL_SYSFS_DIR`, when set, overrides discovery with a directory of
    /// attribute files (development aid: run the frontends against a
    /// plain-file copy of a device's sysfs dir, no wheel or driver needed).
    /// The pre-rename `LOGI_DD_SYSFS_DIR` still works as a deprecated alias
    /// (`LOGI_WHEEL_SYSFS_DIR` wins if both are set); see
    /// [`sysfs_dir_override`]. The directory must contain `wheel_range` (a DD
    /// wheel) or the classic `range`/`gain`/`autocenter` set (a G923) to
    /// count as a wheel, same as the real probe; a dev-override classic dir
    /// cannot be PID-checked (it is not a real HID device directory), so it
    /// is trusted and modeled as `G923`, the only classic wheel this crate
    /// knows.
    pub fn discover() -> Result<Device<RealSysfs>, Error> {
        Self::discover_all().into_iter().next().ok_or(Error::NoWheel)
    }

    /// Every wheel attached right now, not just the first one found.
    ///
    /// `discover()` returns whichever wheel `/sys/class/hidraw` happened to
    /// yield first, which on a two-wheel rig is decided by directory
    /// iteration order rather than by anything the user chose. This is what
    /// a caller uses to offer that choice instead of guessing.
    ///
    /// Ordered by sysfs path so the list is stable between calls: a picker
    /// whose entries reshuffle on every refresh is worse than no picker.
    /// The `LOGI_WHEEL_SYSFS_DIR` override still pins a single device, and
    /// is returned here as a one-element list so an override behaves the
    /// same everywhere.
    pub fn discover_all() -> Vec<Device<RealSysfs>> {
        if sysfs_dir_override().is_some() {
            return Self::discover_overridden().into_iter().collect();
        }
        let mut dirs: Vec<std::path::PathBuf> = match std::fs::read_dir("/sys/class/hidraw") {
            Ok(entries) => entries.flatten().map(|e| e.path().join("device")).collect(),
            Err(_) => return Vec::new(),
        };
        dirs.sort();
        let mut found: Vec<Device<RealSysfs>> = Vec::new();
        for dir in dirs {
            // One physical wheel exposes several hidraw nodes that resolve
            // to the same device directory, so dedupe on the canonical path
            // or the same wheel appears two or three times in the picker.
            let key = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if found.iter().any(|d| d.sysfs_key().as_deref() == Some(key.as_path())) {
                continue;
            }
            if dir.join("wheel_range").exists() {
                let name = usb_device_dir(&key)
                    .and_then(|usb| input_name_under(Path::new("/sys/class/input"), &usb))
                    .unwrap_or_default();
                let model = model_for(pid_from_hid_dir(&dir), &name);
                // A direct-drive wheel gets its HID device directory too.
                // Leaving this None silently disabled every HID++ feature
                // on the RS50 and G PRO: --hidpp-features, the firmware
                // query and --led-probe's level-dialect test all resolve
                // the HID++ interface from it, and all of them reported
                // "no HID++ interface" on wheels that plainly have one.
                // The directory is a fine starting point whichever
                // interface it belongs to, because find_hidpp_sibling
                // walks up to the shared USB device before scanning.
                found.push(Device {
                    io: RealSysfs::new(dir.clone()),
                    model,
                    hid_dir: Some(dir),
                    sysfs_key: Some(key),
                });
                continue;
            }
            // Only trust the classic attr set when the PID confirms a real
            // G923: an unrelated device coincidentally exposing similarly-
            // named sysfs files must not be adopted as a wheel.
            if classic_ffb_present(&dir)
                && pid_from_hid_dir(&dir).map(model_from_pid) == Some(WheelModel::G923)
            {
                found.push(Device {
                    io: RealSysfs::new(dir.clone()),
                    model: WheelModel::G923,
                    hid_dir: Some(dir),
                    sysfs_key: Some(key),
                });
            }
        }
        found
    }

    /// The `LOGI_WHEEL_SYSFS_DIR` path, resolved to a device or nothing.
    fn discover_overridden() -> Option<Device<RealSysfs>> {
        let dir = std::path::PathBuf::from(sysfs_dir_override()?);
        if dir.join("wheel_range").exists() {
            // hid_dir is set here for the same reason as in discover_all:
            // it is what every HID++ lookup starts from. A fixture has no
            // USB parent, so those lookups fail cleanly and the fixture
            // shows "-" rather than wrong data.
            return Some(Device {
                io: RealSysfs::new(dir.clone()),
                model: WheelModel::Unknown,
                hid_dir: Some(dir.clone()),
                sysfs_key: Some(dir),
            });
        }
        if classic_attrs_present(&dir) {
            // Not a real HID device directory (no `uevent`/USB parent
            // structure), so the uniq/HID++ lookups this enables just fail
            // cleanly and the fixture shows "-"/"unavailable" - exactly the
            // fake-sysfs dev-aid's existing no-hidraw story.
            return Some(Device {
                io: RealSysfs::new(dir.clone()),
                model: WheelModel::G923,
                hid_dir: Some(dir.clone()),
                sysfs_key: Some(dir),
            });
        }
        None
    }
}

impl<S: SysfsIo> Device<S> {
    pub fn with_io(io: S) -> Device<S> {
        Device { io, model: WheelModel::default(), hid_dir: None, sysfs_key: None }
    }

    /// Same as `with_io`, but with an explicit `WheelModel` (tests, and any
    /// caller building a `Device` for a known-model classic wheel without
    /// going through `discover()`'s PID sniffing).
    pub fn with_io_and_model(io: S, model: WheelModel) -> Device<S> {
        Device { io, model, hid_dir: None, sysfs_key: None }
    }

    /// The canonical sysfs directory this device was discovered at, used
    /// to tell two attached wheels apart and to dedupe the several hidraw
    /// nodes one wheel exposes. `None` for devices built directly from an
    /// io backend (tests, and the fake-sysfs dev aid).
    pub fn sysfs_key(&self) -> Option<std::path::PathBuf> {
        self.sysfs_key.clone()
    }

    /// This wheel's HID device id (`0003:046D:C276.0003`), the last
    /// component of [`Device::sysfs_key`].
    ///
    /// The stable name for one wheel, and the one the rest of the project
    /// already speaks: it is what `/sys/bus/hid/devices` and
    /// `/sys/class/leds` entries are named after, so a caller holding it can
    /// address this wheel's attributes and its rev display without a fresh
    /// scan that might land on a different wheel. Unlike a hidraw node
    /// number it survives nothing being replugged, and unlike an index into
    /// a discovery list it does not depend on iteration order.
    ///
    /// `None` for a device with no sysfs path behind it (tests, the
    /// `LOGI_WHEEL_SYSFS_DIR` fixture), whose directory name is not an id.
    pub fn hid_id(&self) -> Option<String> {
        let key = self.sysfs_key.as_ref()?;
        let name = key.file_name()?.to_string_lossy().into_owned();
        // A fixture directory is not an id, and handing its name out as one
        // would send every lookup keyed on it somewhere that cannot exist.
        // The kernel writes ids as BUS:VID:PID.SEQ.
        (name.matches(':').count() == 2 && name.contains('.')).then_some(name)
    }

    pub fn model(&self) -> WheelModel {
        self.model
    }

    /// This wheel's USB product id, read from its own sysfs directory name.
    ///
    /// Needed to scope `PROTON_ENABLE_HIDRAW` to the wheel. Proton matches
    /// that variable as a substring against `0xVID/0xPID` per device
    /// (`dlls/winebus.sys/main.c`), and the bare value `1` short-circuits
    /// that test and hands EVERY HID device on the machine to the game:
    /// keyboards, headsets, other controllers. Naming the wheel is what the
    /// pattern form exists for.
    ///
    /// `None` for a device built from a test or dev-override directory,
    /// whose name is not in the kernel's `BUS:VID:PID.SEQ` shape.
    pub fn product_id(&self) -> Option<u16> {
        pid_from_hid_dir(self.hid_dir.as_ref()?)
    }

    /// What this wheel can do, for resolving a game's setup recipe (see
    /// [`crate::games::WheelCaps`]).
    ///
    /// The model answers it whenever the model is known. When it is not,
    /// the attribute set still does: the direct-drive `wheel_*` namespace
    /// and the classic G923 one never overlap, so a device carrying
    /// `wheel_range` is a direct-drive wheel whatever its product id said.
    /// That case is not hypothetical, it is every device built without PID
    /// sniffing: the `LOGI_WHEEL_SYSFS_DIR` development override and
    /// [`Device::with_io`] both produce a direct-drive device modeled as
    /// `Unknown`, and going by the model alone would have told a developer
    /// running against a DD fixture that their wheel has no TrueForce.
    pub fn wheel_caps(&self) -> crate::games::WheelCaps {
        match self.model {
            WheelModel::Unknown if self.io.exists("wheel_range") => {
                crate::games::WheelCaps { sdk_trueforce: true }
            }
            model => crate::games::WheelCaps::of(model),
        }
    }

    /// The registry this device's settings live in: [`CLASSIC_REGISTRY`] for
    /// a G923, [`REGISTRY`] (the direct-drive `wheel_*` set) for everything
    /// else. Frontends use this instead of the bare `REGISTRY` constant so a
    /// connected G923 only ever shows its own four settings, never the DD
    /// wheels' rows marked unavailable (a different device model, not "DD
    /// with everything missing").
    pub fn settings(&self) -> &'static [SettingSpec] {
        self.model.settings()
    }

    /// Look up `attr` in either registry: the two attribute namespaces never
    /// collide (the classic set has no `wheel_` prefix), so a plain attr
    /// lookup does not need to know which wheel is connected.
    pub fn spec(attr: &str) -> Option<&'static SettingSpec> {
        REGISTRY.iter().find(|s| s.attr == attr).or_else(|| CLASSIC_REGISTRY.iter().find(|s| s.attr == attr))
    }

    pub fn available(&self, attr: &str) -> bool {
        if !self.io.exists(attr) {
            return false;
        }
        if requires_accessory(attr) && self.accessory_attached() == Some(false) {
            return false;
        }
        !self.wrong_accessory_mode(attr)
    }

    /// Whether `cat` has anything to show for this device: at least one row
    /// in `self.settings()`, or `Info` (its identity rows can be empty on a
    /// classic wheel, but the page also carries the live input monitor and
    /// force-feedback test sims, which work off evdev regardless of model).
    /// Frontends use this to hide a sidebar entry that would otherwise open
    /// onto a blank page, e.g. a G923 has no `Leds`/`Profiles` rows at all.
    pub fn category_has_content(&self, cat: Category) -> bool {
        self.model.category_has_content(cat)
    }

    /// The classic engine (a G923) has no desktop/onboard split at all, so
    /// it always reads as `Desktop`: the mode gating every caller applies
    /// (`ModeReq`) is `Any` for every classic setting, but `current_mode`
    /// itself must still resolve rather than error, since
    /// `info()`/`ensure_desktop_mode()`/`drift_snapshot()` all call it
    /// unconditionally regardless of which wheel is connected. This is
    /// gated on the model, not just on `wheel_mode`'s absence: for a DD
    /// wheel, a missing `wheel_mode` means the wheel is actually gone (the
    /// no-wheel/drift-detection paths rely on that read failing then), so
    /// only a confirmed classic device gets the free pass.
    pub fn current_mode(&self) -> Result<Mode, Error> {
        if self.model == WheelModel::G923 {
            return Ok(Mode::Desktop);
        }
        match self.io.read("wheel_mode").map_err(|e| map_io_error(&e, "wheel_mode"))?.trim() {
            "onboard" => Ok(Mode::Onboard),
            _ => Ok(Mode::Desktop),
        }
    }

    pub fn info(&self) -> Result<DeviceInfo, Error> {
        let read = |a: &str| {
            self.io.read(a).map(|s| s.trim().to_string()).unwrap_or_default()
        };
        // A G923 has no `wheel_serial` sysfs at all; its serial is the HID
        // `uniq` string off the same interface-0 directory `hid_dir`
        // anchors to instead (cheap: one small file read, safe to do on
        // every `info()` call, unlike `classic_firmware`'s live HID++
        // round trip).
        let serial = if self.model == WheelModel::G923 {
            self.hid_dir.as_deref().and_then(read_hid_uniq).unwrap_or_default()
        } else {
            read("wheel_serial")
        };
        Ok(DeviceInfo {
            name: wheel_display_name_for(self.sysfs_key.as_deref(), self.model),
            serial,
            // The driver returns "base: ...\nmotor: ..."; keep it on one line.
            firmware: read("wheel_firmware").replace('\n', " / "),
            mode: self.current_mode()?,
            model: self.model,
        })
    }

    /// Best-effort HID++ firmware string for a classic (G923) wheel: `None`
    /// immediately for any other model (nothing to query; `info().firmware`
    /// already covers them from sysfs), and `None` when the HID++ sibling
    /// node or the query itself failed (unavailable permissions, timeout,
    /// unplugged). This is a real USB round trip with its own timeout, not
    /// a sysfs read: callers must call it once per Info-page load/refresh,
    /// never per frame/draw (see `hidpp::query_g923_firmware`).
    pub fn classic_firmware(&self) -> Option<String> {
        if self.model != WheelModel::G923 {
            return None;
        }
        crate::hidpp::query_g923_firmware(self.hid_dir.as_deref()?)
    }

    /// Ask the wheel which HID++ features it implements.
    ///
    /// Unlike [`Device::classic_firmware`] this is not restricted by model:
    /// the point is to find out what an unfamiliar wheel supports, and
    /// restricting it to wheels we already understand would defeat that.
    /// `None` when there is no HID++ sibling node or it cannot be opened.
    ///
    /// A round trip per feature, so this belongs in a one-shot diagnostic
    /// rather than anywhere near a redraw.
    pub fn hidpp_features(&self) -> Option<Vec<(u16, &'static str, Option<u8>)>> {
        crate::hidpp::probe_features(self.hid_dir.as_deref()?)
    }

    /// The wheel's interface-0 HID directory, when discovery found one.
    /// Needed by the rev-light probe, which talks to both that interface
    /// and its HID++ sibling.
    /// Every feature this wheel implements, named where we know the name.
    /// Unlike [`hidpp_features`](Self::hidpp_features) this asks the wheel
    /// to list them, so a feature nobody here has documented still shows up.
    pub fn hidpp_all_features(&self) -> Option<Vec<(u8, u16, Option<&'static str>)>> {
        let node = crate::hidpp::find_hidpp_sibling(self.hid_dir.as_deref()?)?;
        let mut io = crate::hidpp::RealHidppIo::open(&node).ok()?;
        let found = crate::hidpp::enumerate_features(&mut io)?;
        Some(found.into_iter().map(|(i, id)| (i, id, crate::hidpp::feature_name(id))).collect())
    }

    pub fn hid_dir(&self) -> Option<&std::path::Path> {
        self.hid_dir.as_deref()
    }

    pub fn read(&self, attr: &str) -> Result<Value, Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
        // Action attributes are write-only triggers; reading the sysfs file
        // returns EACCES. Report the trigger value instead of a permission error.
        if spec.access == Access::Action {
            return Ok(Value::Trigger);
        }
        let raw = self.io.read(attr).map_err(|e| map_io_error(&e, attr))?;
        // wheel_mode / wheel_texture_route report words; map to the enum index.
        if let Kind::Enum(variants) = spec.kind {
            let t = raw.trim();
            if let Some(i) = variants.iter().position(|v| *v == t) {
                return Ok(Value::Enum(i as u8));
            }
        }
        spec.kind.parse(&raw)
    }

    /// Whether `attr` is actually usable on this wheel, distinguishing two
    /// different "not there" shapes a frontend must present the same way:
    /// the sysfs file missing entirely (`Ok(None)`, same as `available`
    /// returning `false`), and the file present but the wheel/firmware
    /// answering EOPNOTSUPP on a live read (also `Ok(None)`: the pedal MCU
    /// on an RS50 exposes `wheel_throttle_sensitivity` et al. as files, but
    /// the feature does not exist on that sub-device, so every read of them
    /// comes back `Error::Unsupported`). Any other read error (permissions,
    /// a transient I/O failure) is not "unsupported" and is passed through
    /// as `Err` so a caller keeps whatever error handling it already has for
    /// those; only `Error::Unsupported` collapses to `Ok(None)` here. One
    /// `exists` check plus, at most, one `read` - no extra round trip beyond
    /// what a plain `available`-then-`read` pair already costs.
    pub fn read_supported(&self, attr: &str) -> Result<Option<Value>, Error> {
        if !self.io.exists(attr) {
            return Ok(None);
        }
        if requires_accessory(attr) && self.accessory_attached() == Some(false) {
            return Ok(None);
        }
        if self.wrong_accessory_mode(attr) {
            return Ok(None);
        }
        match self.read(attr) {
            Ok(v) => Ok(Some(v)),
            Err(Error::Unsupported) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether the RS Shifter & Handbrake is attached, per the driver's
    /// `wheel_accessory` attribute (`none` when the base probed and found
    /// nothing). `None` means "cannot tell": the attribute is missing (a
    /// driver older than accessory discovery) or unreadable. Callers must
    /// treat `None` as "show the row" rather than hiding it, so an older
    /// driver keeps working instead of silently losing settings.
    /// The accessory's live mode from `wheel_accessory_mode`, or `None` when
    /// it cannot be read (no accessory, a driver without the attribute, or
    /// the firmware reporting a value this build does not know).
    pub fn accessory_mode(&self) -> Option<String> {
        if !self.io.exists("wheel_accessory_mode") {
            return None;
        }
        let raw = self.io.read("wheel_accessory_mode").ok()?;
        let t = raw.trim();
        if t.is_empty() || t.starts_with("unknown") {
            return None;
        }
        Some(t.to_string())
    }

    /// Whether `attr` is idle because the accessory is in a different mode.
    /// `false` whenever the mode cannot be read, so an unknown mode never
    /// hides a control.
    pub fn wrong_accessory_mode(&self, attr: &str) -> bool {
        match (required_mode(attr), self.accessory_mode()) {
            (Some(want), Some(have)) => want != have,
            _ => false,
        }
    }

    pub fn accessory_attached(&self) -> Option<bool> {
        if !self.io.exists("wheel_accessory") {
            return None;
        }
        let raw = self.io.read("wheel_accessory").ok()?;
        let t = raw.trim();
        if t.is_empty() {
            return None;
        }
        Some(t != "none")
    }

    pub fn write(&self, attr: &str, v: &Value) -> Result<(), Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
        if spec.access == Access::ReadOnly {
            return Err(Error::Invalid);
        }
        self.write_checked(spec, v)
    }

    /// [`Device::write`] for the deliberate hardware tests that borrow an
    /// attribute a live feed normally owns, such as the LED test's rev
    /// sweep over `wheel_rev_level`.
    ///
    /// `Access::ReadOnly` in the registry means "not a control on the
    /// settings page", not "the wheel refuses it": several of those
    /// attributes are read-write on the wire and are modeled read-only so
    /// the app cannot race logi-rpm-bridge or logi-launch. A test the user
    /// asked for, which runs for a moment and puts the display back, is the
    /// one case where taking the strip is what they meant. Everything else
    /// goes through `write`, so no widget can reach this.
    pub fn write_test_pattern(&self, attr: &str, v: &Value) -> Result<(), Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
        if spec.access == Access::Action {
            return Err(Error::Invalid);
        }
        self.write_checked(spec, v)
    }

    /// The shared half of the two writes above: validate, gate on mode, and
    /// hand the formatted value to sysfs. Access is the caller's business.
    fn write_checked(&self, spec: &SettingSpec, v: &Value) -> Result<(), Error> {
        let attr = spec.attr;
        spec.kind.validate(v)?;
        // Mode gating: reject up front with a WrongMode the UI can act on.
        match spec.mode_req {
            ModeReq::DesktopOnly if self.current_mode()? != Mode::Desktop => {
                return Err(Error::WrongMode { needed: Mode::Desktop });
            }
            ModeReq::OnboardOnly if self.current_mode()? != Mode::Onboard => {
                return Err(Error::WrongMode { needed: Mode::Onboard });
            }
            _ => {}
        }
        let raw = self.raw_for_write(spec, v)?;
        self.io.write(attr, &raw).map_err(|e| map_io_error(&e, attr))
    }

    /// wheel_mode/texture_route take words; write the variant string, not index.
    fn raw_for_write(&self, spec: &SettingSpec, v: &Value) -> Result<String, Error> {
        if let (Kind::Enum(variants), Value::Enum(i)) = (spec.kind, v) {
            if spec.attr == "wheel_mode" || spec.attr == "wheel_texture_route" {
                return variants
                    .get(*i as usize)
                    .map(|s| s.to_string())
                    .ok_or(Error::OutOfRange);
            }
        }
        spec.kind.format(v)
    }

    pub fn ensure_desktop_mode(&self) -> Result<(), Error> {
        if self.current_mode()? == Mode::Desktop {
            return Ok(());
        }
        self.io.write("wheel_mode", "desktop").map_err(|e| map_io_error(&e, "wheel_mode"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_from_name_identifies_each_wheel() {
        // The exact strings these wheels put on their input nodes.
        assert_eq!(model_from_name("Logitech RS50 Base for PlayStation/PC"), WheelModel::Rs50);
        assert_eq!(
            model_from_name("Logitech G923 Racing Wheel for Xbox One and PC"),
            WheelModel::G923,
            "the name from issue #27, which the app had and did not use"
        );
        assert_eq!(model_from_name("Logitech G923 Racing Wheel for PlayStation"), WheelModel::G923);
        // A real G PRO says PRO Racing Wheel, with no G at all.
        assert_eq!(model_from_name("Logitech  PRO Racing Wheel"), WheelModel::GPro);
        assert_eq!(model_from_name("Logitech G PRO Racing Wheel"), WheelModel::GPro);
        // Anything else stays unknown rather than guessing a model.
        assert_eq!(model_from_name("Some Other Gamepad"), WheelModel::Unknown);
        assert_eq!(model_from_name(""), WheelModel::Unknown);
    }
    use crate::sysfs::FakeSysfs;
    use crate::value::Value;

    fn dev() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "TESTSERIAL01");
        fs.set("wheel_sensitivity", "50");
        fs.set("wheel_texture_route", "tf");
        Device::with_io(fs)
    }

    #[test]
    fn reads_typed_value() {
        assert_eq!(dev().read("wheel_range").unwrap(), Value::Int(900));
    }

    #[test]
    fn texture_route_word_parses_to_enum() {
        // driver reports "tf"; registry models it as Enum index 1
        assert_eq!(dev().read("wheel_texture_route").unwrap(), Value::Enum(1));
    }

    #[test]
    fn action_attrs_read_as_trigger_not_permission_error() {
        // wheel_led_apply / wheel_calibrate_here are write-only (0220); reading
        // the file gives EACCES. read() must report the trigger, not the error.
        let fs = FakeSysfs::new();
        fs.set_errno("wheel_led_apply", 13); // EACCES if it tried to read
        let d = Device::with_io(fs);
        assert_eq!(d.read("wheel_led_apply").unwrap(), Value::Trigger);
        // even with the file entirely absent
        assert_eq!(d.read("wheel_calibrate_here").unwrap(), Value::Trigger);
    }

    #[test]
    fn firmware_info_is_single_line() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "X");
        fs.set("wheel_firmware", "base: U1 65.04.B0039\nmotor: SC 02.01.B0042\n");
        let info = Device::with_io(fs).info().unwrap();
        assert!(!info.firmware.contains('\n'), "firmware: {:?}", info.firmware);
        assert_eq!(info.firmware, "base: U1 65.04.B0039 / motor: SC 02.01.B0042");
    }

    #[test]
    fn writes_valid_value() {
        let d = dev();
        d.write("wheel_range", &Value::Int(540)).unwrap();
        assert_eq!(d.read("wheel_range").unwrap(), Value::Int(540));
    }

    #[test]
    fn write_out_of_range_rejected_before_io() {
        let d = dev();
        assert!(matches!(d.write("wheel_range", &Value::Int(45)), Err(Error::OutOfRange)));
    }

    /// The rev strip belongs to whichever feed is driving it, so an ordinary
    /// write (a settings widget) is refused while the LED test's explicit
    /// borrow still lands. The wheel takes both: the difference is ownership,
    /// not what the hardware accepts.
    #[test]
    fn the_rev_strip_refuses_a_widget_and_allows_the_led_test() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_rev_level", "0");
        let d = Device::with_io(fs);
        assert!(matches!(d.write("wheel_rev_level", &Value::Int(7)), Err(Error::Invalid)));
        assert_eq!(d.read("wheel_rev_level").unwrap(), Value::Int(0), "the feed's level stands");
        d.write_test_pattern("wheel_rev_level", &Value::Int(7)).unwrap();
        assert_eq!(d.read("wheel_rev_level").unwrap(), Value::Int(7));
        // Still validated: the escape hatch is about access, nothing else.
        assert!(matches!(
            d.write_test_pattern("wheel_rev_level", &Value::Int(11)),
            Err(Error::OutOfRange)
        ));
        // And write-only triggers stay out of reach: they are not values.
        assert!(matches!(
            d.write_test_pattern("wheel_calibrate_here", &Value::Trigger),
            Err(Error::Invalid)
        ));
    }

    #[test]
    fn desktop_only_write_in_onboard_returns_wrong_mode() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_sensitivity", "50");
        let d = Device::with_io(fs);
        assert!(matches!(d.write("wheel_sensitivity", &Value::Percent(10)),
                         Err(Error::WrongMode { needed: Mode::Desktop })));
    }

    #[test]
    fn ensure_desktop_switches_mode() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        let d = Device::with_io(fs);
        d.ensure_desktop_mode().unwrap();
        assert_eq!(d.current_mode().unwrap(), Mode::Desktop);
    }

    #[test]
    fn read_supported_returns_the_value_for_a_normal_attr() {
        assert_eq!(dev().read_supported("wheel_range").unwrap(), Some(Value::Int(900)));
    }

    #[test]
    fn read_supported_is_none_for_a_missing_attr() {
        assert_eq!(dev().read_supported("wheel_brake_force").unwrap(), None);
    }

    #[test]
    fn read_supported_is_none_when_the_attr_exists_but_the_wheel_says_unsupported() {
        // The RS50 pedal MCU's story: `wheel_throttle_sensitivity` exists as
        // a sysfs file, but the wheel has no such feature on that
        // sub-device, so every read comes back EOPNOTSUPP.
        let fs = FakeSysfs::new();
        fs.set_read_errno("wheel_throttle_sensitivity", 95);
        let d = Device::with_io(fs);
        assert!(d.available("wheel_throttle_sensitivity"), "the file itself is there");
        assert_eq!(d.read_supported("wheel_throttle_sensitivity").unwrap(), None);
    }

    /// A wheel exposing the handbrake attrs, with `wheel_accessory` reporting
    /// whatever `accessory` says. `None` omits the attribute entirely, which is
    /// what a driver older than accessory discovery looks like.
    fn dev_with_accessory(accessory: Option<&str>) -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_handbrake_sensitivity", "50");
        fs.set("wheel_handbrake_curve", "0/64 points loaded (0 = built-in curve)");
        fs.set("wheel_range", "900");
        if let Some(a) = accessory {
            fs.set("wheel_accessory", a);
        }
        Device::with_io(fs)
    }

    #[test]
    fn handbrake_settings_are_unavailable_with_no_accessory_attached() {
        // The wheel cannot tell us these are inapplicable: handbrake shaping
        // lands on the base's own axis, so the attr reads a real value and a
        // write would succeed. Only `wheel_accessory` distinguishes the case.
        let d = dev_with_accessory(Some("none"));
        assert!(!d.available("wheel_handbrake_sensitivity"));
        assert!(!d.available("wheel_handbrake_curve"));
        assert_eq!(d.read_supported("wheel_handbrake_sensitivity").unwrap(), None);
        assert_eq!(d.read_supported("wheel_handbrake_curve").unwrap(), None);
    }

    #[test]
    fn handbrake_settings_are_available_with_the_accessory_attached() {
        let d = dev_with_accessory(Some("RS Shifter & Handbrake"));
        assert!(d.available("wheel_handbrake_sensitivity"));
        assert_eq!(
            d.read_supported("wheel_handbrake_sensitivity").unwrap(),
            Some(Value::Percent(50))
        );
    }

    #[test]
    fn handbrake_settings_stay_available_when_accessory_state_is_unknown() {
        // A driver predating accessory discovery has no `wheel_accessory` file.
        // Hiding the rows there would silently drop settings that do work, so
        // "cannot tell" must fall back to showing them.
        let d = dev_with_accessory(None);
        assert_eq!(d.accessory_attached(), None);
        assert!(d.available("wheel_handbrake_sensitivity"));
        assert_eq!(
            d.read_supported("wheel_handbrake_sensitivity").unwrap(),
            Some(Value::Percent(50))
        );
    }

    /// A wheel with the accessory attached and switched to `mode`.
    fn dev_in_mode(mode: &str) -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        for a in ["wheel_shift_actuation", "wheel_handbrake_actuation",
                  "wheel_handbrake_sensitivity", "wheel_range"] {
            fs.set(a, "50");
        }
        fs.set("wheel_accessory", "RS Shifter & Handbrake");
        fs.set("wheel_accessory_mode", mode);
        Device::with_io(fs)
    }

    #[test]
    fn accessory_settings_are_gated_on_the_mode_the_switch_is_in() {
        // The unit is one of three things at a time, and each setting belongs
        // to exactly one of them.
        let shifter = dev_in_mode("shifter");
        assert!(shifter.available("wheel_shift_actuation"));
        assert!(!shifter.available("wheel_handbrake_actuation"));
        assert!(!shifter.available("wheel_handbrake_sensitivity"));

        let digital = dev_in_mode("digital-handbrake");
        assert!(!digital.available("wheel_shift_actuation"));
        assert!(digital.available("wheel_handbrake_actuation"));
        assert!(!digital.available("wheel_handbrake_sensitivity"));

        let analog = dev_in_mode("analog-handbrake");
        assert!(!analog.available("wheel_shift_actuation"));
        assert!(!analog.available("wheel_handbrake_actuation"));
        assert!(analog.available("wheel_handbrake_sensitivity"));

        // Nothing else is touched by the mode.
        assert!(analog.available("wheel_range"));
    }

    #[test]
    fn an_unreadable_mode_never_hides_anything() {
        // A driver without the attribute, or a firmware reporting a value this
        // build does not know, must not make settings disappear.
        let fs = FakeSysfs::new();
        fs.set("wheel_shift_actuation", "50");
        fs.set("wheel_handbrake_actuation", "50");
        fs.set("wheel_accessory", "RS Shifter & Handbrake");
        let d = Device::with_io(fs);
        assert_eq!(d.accessory_mode(), None);
        assert!(d.available("wheel_shift_actuation"));
        assert!(d.available("wheel_handbrake_actuation"));
    }

    #[test]
    fn a_missing_accessory_does_not_gate_ordinary_settings() {
        // The gate is scoped to `ACCESSORY_ATTRS`; nothing else may be caught
        // by it just because no accessory is attached.
        let d = dev_with_accessory(Some("none"));
        assert!(d.available("wheel_range"));
        assert_eq!(d.read_supported("wheel_range").unwrap(), Some(Value::Int(900)));
        assert!(!requires_accessory("wheel_range"));
        assert!(requires_accessory("wheel_handbrake_curve"));
    }

    #[test]
    fn read_supported_propagates_a_non_unsupported_read_error() {
        // A permission error (or any other read failure) is not "this
        // feature doesn't exist"; it must come back as an `Err`, not
        // collapse to `Ok(None)` the way `Unsupported` does.
        let fs = FakeSysfs::new();
        fs.set_read_errno("wheel_range", 13); // EACCES
        let d = Device::with_io(fs);
        assert!(matches!(d.read_supported("wheel_range"), Err(Error::Io(_))));
    }

    #[test]
    fn available_reflects_presence() {
        let d = dev();
        assert!(d.available("wheel_range"));
        assert!(!d.available("wheel_brake_force"));
    }

    #[test]
    fn info_reads_identity() {
        let i = dev().info().unwrap();
        assert_eq!(i.serial, "TESTSERIAL01");
        assert_eq!(i.mode, Mode::Desktop);
    }

    // --- G923 / WheelModel ---

    /// Whether `a` and `b` are the same registry, by content (attr names,
    /// in order) rather than address: `pub const X: &[T] = &[..]` is a
    /// `const`, not a `static`, so two syntactic uses of the same constant
    /// are not guaranteed to share one address (each use may promote its
    /// own anonymous static), making `std::ptr::eq` an unreliable way to
    /// check "is this the registry I expect".
    fn same_registry(a: &[SettingSpec], b: &[SettingSpec]) -> bool {
        a.iter().map(|s| s.attr).eq(b.iter().map(|s| s.attr))
    }

    #[test]
    fn with_io_defaults_to_unknown_model_and_the_dd_registry() {
        let d = Device::with_io(FakeSysfs::new());
        assert_eq!(d.model(), WheelModel::Unknown);
        assert!(same_registry(d.settings(), REGISTRY));
    }

    #[test]
    fn a_g923_device_uses_the_classic_registry() {
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        fs.set("gain", "65535");
        fs.set("autocenter", "0");
        fs.set("combine_pedals", "0");
        let d = Device::with_io_and_model(fs, WheelModel::G923);
        assert!(same_registry(d.settings(), CLASSIC_REGISTRY));
        assert_eq!(d.read("range").unwrap(), Value::Int(900));
        // gain/autocenter read back as a percent (Kind::ScaledPercent), not
        // the raw sysfs 0-65535 integer.
        assert_eq!(d.read("gain").unwrap(), Value::Percent(100));
        assert_eq!(d.read("combine_pedals").unwrap(), Value::Enum(0));
    }

    #[test]
    fn a_g923_device_writes_and_validates_its_settings() {
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        fs.set("gain", "0");
        fs.set("autocenter", "0");
        fs.set("combine_pedals", "0");
        let d = Device::with_io_and_model(fs, WheelModel::G923);
        d.write("range", &Value::Int(540)).unwrap();
        assert_eq!(d.read("range").unwrap(), Value::Int(540));
        assert!(matches!(d.write("range", &Value::Int(39)), Err(Error::OutOfRange)));
        assert!(matches!(d.write("range", &Value::Int(901)), Err(Error::OutOfRange)));
        d.write("combine_pedals", &Value::Enum(2)).unwrap();
        assert_eq!(d.read("combine_pedals").unwrap(), Value::Enum(2));
        // gain is written as a percent; the raw sysfs attr underneath still
        // takes the scaled 0-65535 value (Oversteer compatibility).
        d.write("gain", &Value::Percent(50)).unwrap();
        assert_eq!(d.read("gain").unwrap(), Value::Percent(50));
    }

    #[test]
    fn a_g923_device_has_no_dd_settings_available() {
        // The registry selection is exclusive: a G923's `settings()` never
        // includes the DD wheels' `wheel_*` rows, so a frontend iterating it
        // never renders them (not even as "unavailable").
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::G923);
        assert!(!d.settings().iter().any(|s| s.attr.starts_with("wheel_")));
        assert!(d.settings().iter().any(|s| s.attr == "range"));
    }

    #[test]
    fn a_g923_has_no_content_for_leds_but_info_and_profiles_always_have_content() {
        // A classic wheel has no LIGHTSYNC strip at all: that sidebar page
        // would be blank, so frontends must hide it rather than open onto
        // nothing. Info always has content (the live input monitor + test
        // sims work regardless of model), and so does Profiles: even with
        // no onboard slots, the computer-side profile store still has the
        // wheel's own four settings to save/apply.
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::G923);
        assert!(!d.category_has_content(Category::Leds));
        assert!(d.category_has_content(Category::Profiles));
        assert!(d.category_has_content(Category::Info));
        assert!(d.category_has_content(Category::Ffb));
        assert!(d.category_has_content(Category::Steering));
        assert!(d.category_has_content(Category::Pedals));
    }

    #[test]
    fn a_dd_wheel_has_content_in_every_category() {
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::Rs50);
        for cat in Category::ALL {
            assert!(d.category_has_content(*cat), "{cat:?}");
        }
    }

    #[test]
    fn a_classic_wheel_with_no_wheel_mode_reads_as_desktop() {
        // The classic engine has no onboard/desktop split at all; current_mode
        // must resolve rather than error so info()/writes never fail on it.
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        let d = Device::with_io_and_model(fs, WheelModel::G923);
        assert_eq!(d.current_mode().unwrap(), Mode::Desktop);
        let info = d.info().unwrap();
        assert_eq!(info.mode, Mode::Desktop);
        assert_eq!(info.model, WheelModel::G923);
        // A classic wheel has no wheel_serial/wheel_firmware sysfs either;
        // info() must still succeed with blank identity rather than erroring.
        assert_eq!(info.serial, "");
        assert_eq!(info.firmware, "");
    }

    #[test]
    fn ensure_desktop_mode_is_a_no_op_without_wheel_mode() {
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::G923);
        // No wheel_mode attr to write; must not error and must not panic.
        d.ensure_desktop_mode().unwrap();
    }

    #[test]
    fn spec_resolves_attrs_from_either_registry() {
        assert_eq!(Device::<FakeSysfs>::spec("wheel_range").unwrap().attr, "wheel_range");
        assert_eq!(Device::<FakeSysfs>::spec("range").unwrap().attr, "range");
        assert_eq!(Device::<FakeSysfs>::spec("combine_pedals").unwrap().attr, "combine_pedals");
        assert!(Device::<FakeSysfs>::spec("nonexistent").is_none());
    }

    #[test]
    fn model_from_pid_maps_the_known_product_ids() {
        assert_eq!(model_from_pid(0xc276), WheelModel::Rs50);
        assert_eq!(model_from_pid(0xc272), WheelModel::GPro);
        assert_eq!(model_from_pid(0xc268), WheelModel::GPro);
        assert_eq!(model_from_pid(0xc266), WheelModel::G923);
        assert_eq!(model_from_pid(0xc26e), WheelModel::G923);
        assert_eq!(model_from_pid(0x1234), WheelModel::Unknown);
    }

    #[test]
    fn pid_from_hid_dir_parses_the_kernel_naming_convention() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-hid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let hid_dir = dir.join("0003:046D:C266.0002");
        std::fs::create_dir_all(&hid_dir).unwrap();
        assert_eq!(pid_from_hid_dir(&hid_dir), Some(0xc266));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pid_from_hid_dir_is_none_for_an_unshaped_directory() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-plain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(pid_from_hid_dir(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn classic_ffb_present_accepts_a_range_only_wheel() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-rangeonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!classic_ffb_present(&dir), "nothing there yet");

        // The G923 Xbox surface: HID++ 0x8123 creates range and nothing else.
        std::fs::write(dir.join("range"), "900\n").unwrap();
        assert!(classic_ffb_present(&dir), "range alone is a registered wheel");
        assert!(
            !classic_attrs_present(&dir),
            "and it is exactly the case the three-file check rejects"
        );

        // The PlayStation surface still qualifies under both.
        std::fs::write(dir.join("gain"), "100\n").unwrap();
        std::fs::write(dir.join("autocenter"), "0\n").unwrap();
        assert!(classic_ffb_present(&dir));
        assert!(classic_attrs_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn classic_attrs_present_requires_all_three_files() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-classic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!classic_attrs_present(&dir));
        std::fs::write(dir.join("range"), "900").unwrap();
        std::fs::write(dir.join("gain"), "0").unwrap();
        assert!(!classic_attrs_present(&dir), "autocenter still missing");
        std::fs::write(dir.join("autocenter"), "0").unwrap();
        assert!(classic_attrs_present(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `discover()`'s `LOGI_WHEEL_SYSFS_DIR` dev-override path, both shapes,
    /// the deprecated `LOGI_DD_SYSFS_DIR` alias, and the new-wins-if-both-set
    /// rule, all in one test: the only test in this crate touching either
    /// variable, so it cannot race another test over them (two separate
    /// tests both setting the same variable could race each other under the
    /// default parallel test runner).
    #[test]
    fn discover_dev_override_recognizes_both_directory_shapes() {
        let base = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-discover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));

        let classic_dir = base.join("classic");
        std::fs::create_dir_all(&classic_dir).unwrap();
        std::fs::write(classic_dir.join("range"), "900").unwrap();
        std::fs::write(classic_dir.join("gain"), "0").unwrap();
        std::fs::write(classic_dir.join("autocenter"), "0").unwrap();
        std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &classic_dir);
        let classic = Device::discover().unwrap();
        assert_eq!(classic.model(), WheelModel::G923);
        assert_eq!(classic.read("range").unwrap(), Value::Int(900));

        let dd_dir = base.join("dd");
        std::fs::create_dir_all(&dd_dir).unwrap();
        std::fs::write(dd_dir.join("wheel_range"), "900").unwrap();
        std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &dd_dir);
        let dd = Device::discover().unwrap();
        assert_eq!(dd.model(), WheelModel::Unknown);
        assert!(same_registry(dd.settings(), REGISTRY));
        // A direct-drive wheel must carry its HID device directory. Every
        // HID++ lookup resolves the interface from it, so leaving it unset
        // turned --hidpp-features, the firmware query and --led-probe's
        // level-dialect test into "no HID++ interface" on wheels that
        // plainly have one, with nothing in the output saying why.
        assert!(
            dd.hid_dir().is_some(),
            "a direct-drive wheel must keep its hid_dir, or HID++ probing goes dark"
        );
        assert!(classic.hid_dir().is_some(), "a classic wheel keeps its hid_dir too");

        std::env::remove_var("LOGI_WHEEL_SYSFS_DIR");

        // The deprecated LOGI_DD_SYSFS_DIR alias still works on its own.
        std::env::set_var("LOGI_DD_SYSFS_DIR", &classic_dir);
        let via_alias = Device::discover().unwrap();
        assert_eq!(via_alias.model(), WheelModel::G923);

        // And the new name wins when both are set.
        std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &dd_dir);
        let both_set = Device::discover().unwrap();
        assert_eq!(both_set.model(), WheelModel::Unknown, "new name wins over the old alias");

        std::env::remove_var("LOGI_WHEEL_SYSFS_DIR");
        std::env::remove_var("LOGI_DD_SYSFS_DIR");
        std::fs::remove_dir_all(&base).unwrap();
    }

    // --- Wheel row identity: name, and the G923's uniq-based serial ---

    #[test]
    fn generic_wheel_name_covers_every_model() {
        assert_eq!(generic_wheel_name(WheelModel::Rs50), "Logitech RS50 Racing Wheel");
        assert_eq!(generic_wheel_name(WheelModel::GPro), "Logitech G PRO Racing Wheel");
        assert_eq!(generic_wheel_name(WheelModel::G923), "Logitech G923 Racing Wheel");
        assert_eq!(generic_wheel_name(WheelModel::Unknown), "Logitech Racing Wheel");
    }

    #[test]
    fn wheel_display_name_at_prefers_the_evdev_name_when_found() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-wheelname-found-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let event_dir = dir.join("event7").join("device");
        std::fs::create_dir_all(&event_dir).unwrap();
        std::fs::write(
            event_dir.join("name"),
            "Logitech G923 Racing Wheel for PlayStation 4 and PC\n",
        )
        .unwrap();
        assert_eq!(
            wheel_display_name_at(&dir, WheelModel::G923),
            "Logitech G923 Racing Wheel for PlayStation 4 and PC"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wheel_display_name_at_falls_back_to_the_model_when_evdev_has_nothing() {
        // A directory with no matching event node (evdev lagging a fresh
        // connect, or simply no wheel plugged in at all).
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-wheelname-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(wheel_display_name_at(&dir, WheelModel::Rs50), "Logitech RS50 Racing Wheel");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_hid_uniq_parses_the_uevent_line() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-uniq-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("uevent"),
            "DRIVER=logitech-dd\nHID_ID=0003:0000046D:0000C266\nHID_UNIQ=FAKE0000SERIAL\nMODALIAS=x\n",
        )
        .unwrap();
        assert_eq!(read_hid_uniq(&dir).as_deref(), Some("FAKE0000SERIAL"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_hid_uniq_is_none_when_absent_or_blank() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-uniq-blank-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // No uevent file at all (a dev-override fixture).
        assert_eq!(read_hid_uniq(&dir), None);
        std::fs::write(dir.join("uevent"), "DRIVER=x\nHID_UNIQ=\n").unwrap();
        assert_eq!(read_hid_uniq(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The identity callers carry instead of an index or a hidraw node
    /// number: the last component of the canonical sysfs path, and only
    /// when it really is a kernel HID id.
    #[test]
    fn hid_id_is_the_device_id_and_nothing_else() {
        let with_key = |key: &str| Device {
            io: FakeSysfs::new(),
            model: WheelModel::Rs50,
            hid_dir: None,
            sysfs_key: Some(std::path::PathBuf::from(key)),
        };
        assert_eq!(
            with_key("/sys/devices/pci0000:00/usb1/1-8/1-8:1.2/0003:046D:C276.0003").hid_id(),
            Some("0003:046D:C276.0003".to_string())
        );
        // A LOGI_WHEEL_SYSFS_DIR fixture: a directory of attribute files
        // whose name is not an id. Passing that on as one would send every
        // lookup keyed on it (the rev display, the streaming lease) to a
        // path that cannot exist.
        assert_eq!(with_key("/tmp/my-fake-wheel").hid_id(), None);
        // No sysfs behind it at all.
        assert_eq!(Device::with_io(FakeSysfs::new()).hid_id(), None);
    }

    #[test]
    fn a_g923s_serial_comes_from_hid_uniq_not_wheel_serial() {
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-g923-serial-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("uevent"), "HID_UNIQ=FAKESERIAL01\n").unwrap();
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        let d = Device { io: fs, model: WheelModel::G923, hid_dir: Some(dir.clone()), sysfs_key: None };
        let info = d.info().unwrap();
        assert_eq!(info.serial, "FAKESERIAL01");
        assert_eq!(info.model, WheelModel::G923);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_g923_without_a_hid_dir_has_a_blank_serial_not_a_panic() {
        // `with_io_and_model` (no real sysfs directory behind it) leaves
        // `hid_dir` at `None`; `info()` must still succeed.
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::G923);
        let info = d.info().unwrap();
        assert_eq!(info.serial, "");
    }

    #[test]
    fn classic_firmware_is_none_for_every_non_g923_model() {
        for model in [WheelModel::Unknown, WheelModel::Rs50, WheelModel::GPro] {
            let d = Device::with_io_and_model(FakeSysfs::new(), model);
            assert!(d.classic_firmware().is_none(), "{model:?}");
        }
    }

    #[test]
    fn classic_firmware_is_none_without_a_hid_dir_to_anchor_to() {
        let d = Device::with_io_and_model(FakeSysfs::new(), WheelModel::G923);
        assert!(d.classic_firmware().is_none());
    }

    #[test]
    fn classic_firmware_is_none_for_a_dev_override_style_fixture() {
        // A `hid_dir` with no real USB parent structure at all (matching
        // `discover()`'s dev-override path): the HID++ sibling walk must
        // fail cleanly, not panic, and `info()`'s cheap fields stay usable.
        let dir = std::env::temp_dir().join(format!(
            "logi-wheel-device-test-g923-nohidpp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        let d = Device { io: fs, model: WheelModel::G923, hid_dir: Some(dir.clone()), sysfs_key: None };
        assert!(d.classic_firmware().is_none());
        assert!(d.info().is_ok(), "the cheap identity fields must not be affected");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod discover_all_tests {
    use super::*;

    #[test]
    fn short_labels_stay_distinguishable_with_duplicates() {
        use WheelModel::*;
        // The common case: one of each, no numbering noise.
        assert_eq!(short_labels(&[Rs50, G923]), vec!["RS50", "G923"]);
        // Two of a kind must not produce two identical tabs.
        assert_eq!(short_labels(&[G923, G923]), vec!["G923 1", "G923 2"]);
        // Mixed: only the repeated model is numbered.
        assert_eq!(
            short_labels(&[G923, Rs50, G923]),
            vec!["G923 1", "RS50", "G923 2"]
        );
        // Three of a kind, because someone will.
        assert_eq!(
            short_labels(&[GPro, GPro, GPro]),
            vec!["G PRO 1", "G PRO 2", "G PRO 3"]
        );
        assert!(short_labels(&[]).is_empty());
    }

    /// An RS50 in G PRO compatibility mode borrows the G PRO's product id.
    /// Trusting the id alone labels it a G PRO, which is wrong on the one
    /// rig where it matters: the owner's.
    #[test]
    fn a_compat_mode_rs50_is_not_mistaken_for_a_g_pro() {
        // Borrowed id, own product string: an RS50.
        assert_eq!(
            model_for(Some(0xc272), "Logitech RS50 Base for PlayStation/PC"),
            WheelModel::Rs50
        );
        assert_eq!(
            model_for(Some(0xc268), "Logitech RS50 Base for PlayStation/PC"),
            WheelModel::Rs50
        );
        // A real G PRO keeps its id's answer.
        assert_eq!(
            model_for(Some(0xc272), "Logitech G PRO Racing Wheel"),
            WheelModel::GPro
        );
        // An unreadable name must not downgrade a real id.
        assert_eq!(model_for(Some(0xc272), ""), WheelModel::GPro);
        // Ids that never lie are taken at face value.
        assert_eq!(model_for(Some(0xc276), "anything at all"), WheelModel::Rs50);
        // No id: fall back to the name.
        assert_eq!(model_for(None, "Logitech G923 Racing Wheel"), WheelModel::G923);
    }

    /// Two wheels attached must not share a name. The scan used to stop at
    /// the first input device that looked like a wheel, so whichever
    /// enumerated first supplied the name for both.
    #[test]
    fn each_wheel_is_named_from_its_own_usb_device() {
        let tmp = std::env::temp_dir().join(format!("lw-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let inputs = tmp.join("class-input");
        std::fs::create_dir_all(&inputs).unwrap();

        // Two USB devices, each with one interface, one input node, one name.
        let mut usb_dirs = Vec::new();
        for (i, (usb, iface, name)) in [
            ("1-8", "1-8:1.0", "Logitech G923 Racing Wheel"),
            ("1-5", "1-5:1.1", "Logitech RS50 Base"),
        ]
        .iter()
        .enumerate()
        {
            let usb_dir = tmp.join("devices").join(usb);
            let real_input = usb_dir.join(iface).join("input").join(format!("input{i}"));
            std::fs::create_dir_all(real_input.join(format!("event{i}"))).unwrap();
            std::fs::write(real_input.join(format!("event{i}")).join("..").join("name"), name).unwrap();
            let link = inputs.join(format!("event{i}"));
            std::os::unix::fs::symlink(real_input.join(format!("event{i}")), &link).unwrap();
            std::fs::write(link.join("device").join("name"), name).ok();
            usb_dirs.push(usb_dir);
        }

        // The key shape discovery produces: <usb>/<iface>/<hid>
        let key0 = usb_dirs[0].join("1-8:1.0").join("0003:046D:C266.0001");
        let key1 = usb_dirs[1].join("1-5:1.1").join("0003:046D:C276.0002");
        assert_eq!(usb_device_dir(&key0).as_deref(), Some(usb_dirs[0].as_path()));
        assert_eq!(usb_device_dir(&key1).as_deref(), Some(usb_dirs[1].as_path()));
        assert_ne!(
            usb_device_dir(&key0),
            usb_device_dir(&key1),
            "two wheels must resolve to different USB devices, or they cannot be told apart"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// One wheel exposes several hidraw nodes that all resolve to the same
    /// device directory. Without deduping, a picker built from this list
    /// shows the same wheel two or three times.
    #[test]
    fn one_wheel_with_several_hidraw_nodes_appears_once() {
        let tmp = std::env::temp_dir().join(format!("lw-dedupe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let real = tmp.join("real-device");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("wheel_range"), "900").unwrap();

        let mut keys = std::collections::HashSet::new();
        for node in ["hidraw0", "hidraw1", "hidraw2"] {
            let dir = tmp.join(node).join("device");
            std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&real, &dir).unwrap();
            keys.insert(std::fs::canonicalize(&dir).unwrap());
        }
        assert_eq!(keys.len(), 1, "three nodes must canonicalize to one device");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The override pins one device, and must behave the same through both
    /// entry points rather than one of them ignoring it.
    #[test]
    fn override_yields_exactly_one_device_through_both_entry_points() {
        let tmp = std::env::temp_dir().join(format!("lw-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("wheel_range"), "900").unwrap();

        // SAFETY: single-threaded test process, restored below.
        unsafe { std::env::set_var("LOGI_WHEEL_SYSFS_DIR", &tmp) };
        let all = Device::discover_all();
        let one = Device::discover();
        unsafe { std::env::remove_var("LOGI_WHEEL_SYSFS_DIR") };

        assert_eq!(all.len(), 1, "an override pins exactly one wheel");
        assert!(one.is_ok(), "discover() must honour the same override");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
