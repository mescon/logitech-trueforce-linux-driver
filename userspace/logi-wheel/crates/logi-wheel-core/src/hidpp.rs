//! Minimal HID++ 2.0 short-report query: just enough to read a classic
//! (G923) wheel's main-application firmware string off its vendor HID++
//! interface, the way Solaar/G HUB do. The direct-drive wheels never need
//! this: their kernel driver already runs the same exchange and exposes
//! the result as the `wheel_firmware` sysfs attribute. This module exists
//! purely to fill that identity gap for the classic engine, which has no
//! such attribute (see `device::Device::classic_firmware`).
//!
//! Protocol constants and the report layout mirror
//! `mainline/hid-logitech-hidpp.c`'s own DeviceInformation (feature 0x0003)
//! query (`hidpp_dd_query_device_identity`, `hidpp_root_get_feature`,
//! `hidpp_dd_format_fw_entity`), and were verified byte-for-byte against a
//! live G923 (PID c266) on 2026-07-26: `Root.getFeature(0x0003)` resolved
//! to feature index 2 on that unit (never hardcoded here - a firmware
//! update could renumber it), and its three DeviceInfo entities were
//! bootloader / main application / hardware, in that order (`fwType` byte
//! 1 / 0 / 2) - which is why entities are scanned for `fwType == 0` rather
//! than assumed to sit at a fixed index.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// How long a single HID++ request waits for its reply before giving up.
/// The wheel answers in well under this on a live bus; a node that never
/// answers (unplugged mid-query, wrong interface) times out instead of
/// hanging the caller forever.
const REPORT_TIMEOUT: Duration = Duration::from_millis(500);

/// HID++ 2.0 constants this module needs; see
/// `mainline/hid-logitech-hidpp.c` for the authoritative definitions
/// (`LINUX_KERNEL_SW_ID`, the `HIDPP_DD_HIDPP_FN_*` function bytes, and
/// `0xff` as the device index for a directly-connected wheel base).
const REPORT_ID_SHORT: u8 = 0x10;
const REPORT_ID_LONG: u8 = 0x11;
const DEVICE_INDEX_BASE: u8 = 0xff;
const SW_ID: u8 = 0x0a;
const ROOT_FEATURE_INDEX: u8 = 0x00;
const FN_GET_INFO: u8 = 0x00; // function 0: getInfo / Root.getFeature
const FN_GET: u8 = 0x10; // function 1: getFwInfo
const FEATURE_DEVICE_INFORMATION: u16 = 0x0003;
/// getFwInfo's `fwType` byte for the main application firmware, as
/// opposed to the bootloader or hardware-revision entities the same
/// feature also reports.
const FW_TYPE_MAIN_APP: u8 = 0x00;
/// Entities beyond this are not worth scanning: no HID++ 2.0 device this
/// project has seen reports more than a handful, and a device that somehow
/// claimed an implausible count would otherwise turn one bad reply into a
/// long stall.
const MAX_ENTITIES: u8 = 8;

/// A HID++ transport: one report out, one report back. Abstracts the real
/// hidraw node so the protocol logic below can be exercised against an
/// in-memory mock instead of hardware.
pub trait HidppIo {
    fn write_report(&mut self, report: &[u8]) -> io::Result<()>;
    /// Block for at most `timeout` waiting for the next report;
    /// `ErrorKind::TimedOut` when nothing arrived in time.
    fn read_report(&mut self, timeout: Duration) -> io::Result<Vec<u8>>;
}

/// The real transport: a hidraw character device node opened read/write.
pub struct RealHidppIo {
    file: std::fs::File,
}

impl RealHidppIo {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self { file })
    }
}

impl HidppIo for RealHidppIo {
    fn write_report(&mut self, report: &[u8]) -> io::Result<()> {
        self.file.write_all(report)
    }

    /// hidraw's `read` blocks indefinitely with no portable timeout short
    /// of an ioctl/poll this crate deliberately avoids depending on
    /// (`logi-wheel-core` has no dependencies at all today); run it on a
    /// scratch thread instead and bound how long the caller waits on ITS
    /// result. A reply that never arrives leaks one thread parked on a
    /// read that will return whenever the node next produces a report (or
    /// never, if it is gone) - acceptable for a query made once per
    /// Info-page load/refresh, never per frame.
    fn read_report(&mut self, timeout: Duration) -> io::Result<Vec<u8>> {
        let mut file = self.file.try_clone()?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let result = file.read(&mut buf).map(|n| buf[..n].to_vec());
            let _ = tx.send(result);
        });
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "HID++ report timed out")))
    }
}

/// Build one 7-byte HID++ short report: report id, device index, feature
/// index, function nibble OR'd with the software id, then up to 3 param
/// bytes.
fn short_report(device_index: u8, feature_index: u8, function: u8, params: &[u8]) -> [u8; 7] {
    let mut r = [0u8; 7];
    r[0] = REPORT_ID_SHORT;
    r[1] = device_index;
    r[2] = feature_index;
    r[3] = function | SW_ID;
    for (i, p) in params.iter().take(3).enumerate() {
        r[4 + i] = *p;
    }
    r
}

/// Send one short HID++ request and return its response params (everything
/// past the 4-byte header - 3 bytes for a short reply, 16 for the long
/// reply this wheel always answers with in practice; either way the
/// params start at offset 4). `None` on any I/O failure, a timeout, or a
/// reply that does not echo this request's device/feature/function (a
/// stale or unrelated report caught on the wire).
fn request<T: HidppIo>(
    io: &mut T,
    device_index: u8,
    feature_index: u8,
    function: u8,
    params: &[u8],
) -> Option<Vec<u8>> {
    let report = short_report(device_index, feature_index, function, params);
    io.write_report(&report).ok()?;
    let resp = io.read_report(REPORT_TIMEOUT).ok()?;
    if resp.len() < 4
        || resp[1] != device_index
        || resp[2] != feature_index
        || resp[3] != (function | SW_ID)
    {
        return None;
    }
    Some(resp[4..].to_vec())
}

/// `Root.getFeature(feature_id)`: the feature's index on this device, or
/// `None` when unsupported (the device replies with index 0) or the
/// request otherwise failed. Always resolved fresh, never hardcoded: a
/// firmware revision can renumber a feature's index.
pub fn resolve_feature_index<T: HidppIo>(io: &mut T, feature_id: u16) -> Option<u8> {
    let params = [(feature_id >> 8) as u8, (feature_id & 0xff) as u8];
    let resp = request(io, DEVICE_INDEX_BASE, ROOT_FEATURE_INDEX, FN_GET_INFO, &params)?;
    match resp.first().copied() {
        Some(0) | None => None,
        Some(idx) => Some(idx),
    }
}

/// DeviceInformation's `getInfo` (function 0): the number of firmware
/// entities this device reports.
fn entity_count<T: HidppIo>(io: &mut T, feature_index: u8) -> Option<u8> {
    let resp = request(io, DEVICE_INDEX_BASE, feature_index, FN_GET_INFO, &[])?;
    resp.first().copied()
}

/// One `getFwInfo` entity: `fw_type` (0 = main application firmware), the
/// prefix, and the raw version/build bytes, exactly as
/// `mainline/hid-logitech-hidpp.c`'s `hidpp_dd_format_fw_entity` reads
/// them (see that function's doc comment for the on-wire layout this
/// mirrors).
struct FwEntity {
    fw_type: u8,
    prefix: String,
    major: u8,
    minor: u8,
    build: u16,
}

impl FwEntity {
    /// `resp` is `getFwInfo`'s params (offset 4 of the raw report): byte 0
    /// `fwType`, bytes 1-3 a 3-character ASCII prefix, byte 4 the major
    /// version, byte 5 the minor version, bytes 6-7 the build number
    /// (big-endian). Non-printable prefix bytes are dropped (the kernel's
    /// own `hidpp_dd_format_fw_entity` does the same, for pad NULs), and
    /// the result is trimmed: the live G923 capture's prefix field is
    /// itself space-padded ("U1 ", not NUL-padded), which the kernel's
    /// filter alone lets through and its format string then joins with
    /// ANOTHER space ("U1  38.00.B0038", double space); trimming here
    /// avoids reproducing that cosmetic wart in a user-facing row.
    fn parse(resp: &[u8]) -> Option<Self> {
        if resp.len() < 8 {
            return None;
        }
        let prefix: String = resp[1..4]
            .iter()
            .copied()
            .filter(|&b| (0x20..0x7f).contains(&b))
            .map(|b| b as char)
            .collect::<String>()
            .trim()
            .to_string();
        Some(Self {
            fw_type: resp[0],
            prefix,
            major: resp[4],
            minor: resp[5],
            build: u16::from_be_bytes([resp[6], resp[7]]),
        })
    }

    /// `"{prefix} {major:02x}.{minor:02x}.B{build:04X}"`, matching the
    /// kernel driver's own
    /// `scnprintf(out, len, "%s %02x.%02x.B%02x%02x", name, p[4], p[5],
    /// p[6], p[7])` exactly, so a G923's Firmware row reads in the same
    /// style as an RS50's `wheel_firmware`.
    fn format(&self) -> String {
        format!("{} {:02x}.{:02x}.B{:04X}", self.prefix, self.major, self.minor, self.build)
    }
}

fn fw_entity<T: HidppIo>(io: &mut T, feature_index: u8, entity: u8) -> Option<FwEntity> {
    let resp = request(io, DEVICE_INDEX_BASE, feature_index, FN_GET, &[entity])?;
    FwEntity::parse(&resp)
}

/// `Root.getFeature(0x0003)` then a scan of its entities for the main
/// application firmware, formatted the same way the kernel driver's own
/// DeviceInfo query does. `None` on any failure along the way (feature
/// unsupported, a request timed out, no main-app entity turned up): the
/// caller shows "unavailable" rather than block or panic on it.
pub fn query_main_firmware<T: HidppIo>(io: &mut T) -> Option<String> {
    let feature_index = resolve_feature_index(io, FEATURE_DEVICE_INFORMATION)?;
    let count = entity_count(io, feature_index)?.min(MAX_ENTITIES);
    (0..count)
        .filter_map(|entity| fw_entity(io, feature_index, entity))
        .find(|fw| fw.fw_type == FW_TYPE_MAIN_APP)
        .map(|fw| fw.format())
}

/// Whether `descriptor` (a HID device's raw `report_descriptor` sysfs
/// bytes) belongs to a Logitech HID++ 2.0 vendor interface: both the
/// short (Report ID 0x10) and long (Report ID 0x11) HID++ report
/// collections declared side by side. Verified live on a G923's
/// interface-1 hidraw node 2026-07-26 (usage page 0xFF00, six 8-bit short
/// fields, then nineteen 8-bit long fields). A joystick interface's own
/// descriptor can mention usage page 0xFF00 too (an appended vendor byte
/// tacked onto the same report), but never declares BOTH report IDs, so
/// the pair is what distinguishes the HID++ interface from every other one
/// this wheel exposes.
fn looks_like_hidpp(descriptor: &[u8]) -> bool {
    has_report_id(descriptor, REPORT_ID_SHORT) && has_report_id(descriptor, REPORT_ID_LONG)
}

/// Whether `descriptor` declares a Report ID item (`0x85`) for `id`. A
/// byte-pair scan rather than a full HID item parser: good enough to tell
/// report-ID collections apart, which is all this module needs.
fn has_report_id(descriptor: &[u8], id: u8) -> bool {
    descriptor.windows(2).any(|w| w == [0x85, id])
}

/// The `/dev/hidrawN` path for a HID device directory's `hidraw/hidrawN`
/// child, or `None` if it has none (defensive; not seen in practice).
fn hidraw_node(hid_dir: &Path) -> Option<PathBuf> {
    let entry = std::fs::read_dir(hid_dir.join("hidraw")).ok()?.filter_map(|e| e.ok()).next()?;
    Some(Path::new("/dev").join(entry.file_name()))
}

/// Find the sibling HID device (same USB device, a different interface)
/// whose descriptor is the HID++ vendor interface, and return its hidraw
/// node path. `if0_dir` is the interface-0 HID device directory
/// `Device::discover` already resolved (the one carrying
/// `range`/`gain`/`autocenter`); its grandparent in sysfs is the shared
/// USB device directory (e.g. `.../1-9`), with each interface's own HID
/// device living under `.../1-9/1-9:1.N/<bus:vid:pid.seq>`.
pub fn find_hidpp_sibling(if0_dir: &Path) -> Option<PathBuf> {
    let if0_real = std::fs::canonicalize(if0_dir).ok()?;
    let usb_device_dir = if0_real.parent()?.parent()?;
    let Ok(iface_entries) = std::fs::read_dir(usb_device_dir) else {
        return None;
    };
    for iface_entry in iface_entries.filter_map(|e| e.ok()) {
        let Ok(hid_entries) = std::fs::read_dir(iface_entry.path()) else {
            continue;
        };
        for hid_entry in hid_entries.filter_map(|e| e.ok()) {
            let hid_dir = hid_entry.path();
            let Ok(descriptor) = std::fs::read(hid_dir.join("report_descriptor")) else {
                continue;
            };
            if !looks_like_hidpp(&descriptor) {
                continue;
            }
            if let Some(node) = hidraw_node(&hid_dir) {
                return Some(node);
            }
        }
    }
    None
}

/// The full G923 firmware query: resolve the HID++ sibling node from the
/// classic wheel's interface-0 directory, open it, and run
/// `query_main_firmware`. `None` at any step (no sibling found, the node
/// could not be opened - permissions, unplugged - or the query itself
/// failed/timed out) rather than an error: the Info page shows
/// "unavailable" either way. This is a real, if small, round trip over
/// USB; call it once per Info-page load/refresh, never per frame.
pub fn query_g923_firmware(if0_dir: &Path) -> Option<String> {
    let node = find_hidpp_sibling(if0_dir)?;
    let mut io = RealHidppIo::open(&node).ok()?;
    query_main_firmware(&mut io)
}

/// HID++ feature pages worth asking a wheel about, with what each one is
/// for. Mirrors the `HIDPP_DD_PAGE_*` constants in the kernel driver.
///
/// The list exists because a wheel's capabilities are otherwise discovered
/// one bug report at a time. The G923 Xbox edition takes the G920 code path,
/// which never runs the direct-drive feature discovery, so nothing in this
/// project has ever asked that wheel what it supports. See issue #27.
pub const KNOWN_FEATURES: &[(u16, &str)] = &[
    (0x8040, "Brightness control"),
    (0x807A, "RPM indicator (the rev display)"),
    (0x807B, "RPM LED pattern"),
    (0x80A4, "Axis response curve"),
    (0x80D0, "Combined pedals (also broadcasts profile changes)"),
    (0x8123, "Force feedback (G920 family)"),
    (0x8133, "Global damping"),
    (0x8134, "Brake force threshold"),
    (0x8136, "Torque limit"),
    (0x8137, "Configuration profiles"),
    (0x8138, "Operating range"),
    (0x8139, "TrueForce"),
    (0x8140, "FFB filter"),
    (0x1BC0, "Sync/prepare"),
];

/// Ask a wheel which of [`KNOWN_FEATURES`] it implements.
///
/// Returns one row per feature: its id, its description, and the index the
/// device assigned it, or `None` when the device does not implement it.
/// `if0_dir` is the interface-0 HID device directory, the same input
/// [`query_g923_firmware`] takes.
///
/// This is the only way to find out what a wheel supports without owning
/// one. Every query is a `Root.getFeature` read, the same transaction the
/// firmware query already performs on this wheel family, so it changes
/// nothing on the device.
pub fn probe_features(if0_dir: &Path) -> Option<Vec<(u16, &'static str, Option<u8>)>> {
    let node = find_hidpp_sibling(if0_dir)?;
    let mut io = RealHidppIo::open(&node).ok()?;
    Some(
        KNOWN_FEATURES
            .iter()
            .map(|(id, what)| (*id, *what, resolve_feature_index(&mut io, *id)))
            .collect(),
    )
}

// ---------------------------------------------------------------------
// Rev-light probing
//
// Which command lights a wheel's rev strip is not something the feature map
// answers. The PlayStation G923 implements 0x807A and yet its strip is
// driven by the classic lg4ff command instead, so at least two dialects
// exist on wheels that both "have LIGHTSYNC". The only reliable way to find
// out which one a given wheel obeys is to send each and watch the rim.
//
// See issue #27: the Xbox G923 takes the G920 code path, which never runs
// feature discovery, so nothing here had ever asked that wheel anything.
// ---------------------------------------------------------------------

/// G HUB's software id, kept verbatim from the captures the driver's
/// rev-light code was written from.
const REV_SW_ID: u8 = 0x0d;

/// One long 0x807A send in the level-based dialect.
///
/// Takes `function` as a plain function NUMBER and shifts it, exactly like
/// [`rev_short`], because those two are the pair this dialect is written in
/// and a helper that silently disagreed with its sibling produced a
/// malformed command: fn6 was sent as `6 | 0x0d` = `0x0f` instead of
/// `(6 << 4) | 0x0d` = `0x6d`, so the level command never reached the wheel
/// and `--led-probe` reported "sent" while the strip stayed dark. The
/// kernel's own constants are pre-shifted for the same reason
/// (`HIDPP_DD_LIGHTSYNC_FN_SET_CONFIG` is `0x60`).
fn rev_long(device_index: u8, feature_index: u8, function: u8, params: &[u8]) -> [u8; 20] {
    let mut r = [0u8; 20];
    r[0] = REPORT_ID_LONG;
    r[1] = device_index;
    r[2] = feature_index;
    r[3] = (function << 4) | REV_SW_ID;
    for (i, p) in params.iter().take(16).enumerate() {
        r[4 + i] = *p;
    }
    r
}

/// One short 0x807A send in the level-based dialect: `fn` in the high
/// nibble, G HUB's software id in the low one.
fn rev_short<T: HidppIo>(io: &mut T, idx: u8, function: u8, p0: u8) -> io::Result<()> {
    let mut r = [0u8; 7];
    r[0] = REPORT_ID_SHORT;
    r[1] = 0xff;
    r[2] = idx;
    r[3] = (function << 4) | REV_SW_ID;
    r[4] = p0;
    io.write_report(&r)
}

/// Drive the rev strip through the **level-based 0x807A dialect**, the one
/// a real G PRO rim speaks.
///
/// The arm burst (fn0, fn1, fn2, fn0, a few ms apart) is what makes the
/// wheel accept levels at all; the level itself is a short fn2 followed by
/// a long fn6 carrying `00 01 00 0a 00 LL`. Both sequences are taken from
/// the kernel driver's `hidpp_dd_rev_send_level`, which was written from
/// G HUB captures.
pub fn rev_level_via_lightsync<T: HidppIo>(io: &mut T, idx: u8, level: u8) -> io::Result<()> {
    for (function, p0) in [(0u8, 0u8), (1, 0), (2, 0), (0, 0)] {
        rev_short(io, idx, function, p0)?;
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    rev_short(io, idx, 2, 0)?;
    let long = rev_long(0xff, idx, 6, &[0x00, 0x01, 0x00, 0x0a, 0x00, level.min(10)]);
    io.write_report(&long)
}

/// Drive the rev strip through the **classic lg4ff command**, the one the
/// PlayStation G923 obeys.
///
/// `mask` is a bitmask of the five LED pairs, so `0x1f` is all of them.
/// This is not a HID++ transaction at all: it is a plain 7-byte output
/// report, and it goes to the wheel's joystick interface rather than its
/// HID++ one.
pub fn rev_mask_via_lg4ff<T: HidppIo>(io: &mut T, mask: u8) -> io::Result<()> {
    io.write_report(&[0xf8, 0x12, mask & 0x1f, 0x00, 0x00, 0x00, 0x00])
}

/// Open the wheel's joystick-interface hidraw node, where the classic
/// lg4ff command is sent. That is the interface carrying the gamepad
/// descriptor, not the HID++ one [`find_hidpp_sibling`] returns.
pub fn open_joystick_node(if0_dir: &Path) -> Option<RealHidppIo> {
    RealHidppIo::open(&joystick_node(if0_dir)?).ok()
}

/// The path [`open_joystick_node`] would open, so a diagnostic can say
/// where it wrote rather than only that it wrote.
///
/// Worth reporting because a write to the wrong interface does not
/// necessarily fail: on a G923, the classic LED command sent to the
/// `0xFFFD` vendor node is accepted by the kernel and does nothing, while
/// the same write to the `0xFF00` HID++ node fails with `EPIPE`. A test
/// that only prints "sent" therefore cannot distinguish "the wheel ignored
/// this dialect" from "this never reached the wheel's joystick interface",
/// and that difference has cost a remote tester months.
pub fn joystick_node(if0_dir: &Path) -> Option<PathBuf> {
    hidraw_node(if0_dir)
}

/// The first bytes of a HID device's report descriptor, as a short label.
/// `05 01 09 04` is Generic Desktop / Joystick, which is the interface the
/// classic LED command has to land on.
pub fn descriptor_kind(hid_dir: &Path) -> String {
    let Ok(bytes) = std::fs::read(hid_dir.join("report_descriptor")) else {
        return "unreadable".to_string();
    };
    match bytes.get(..4) {
        Some([0x05, 0x01, 0x09, 0x04]) => "Joystick".to_string(),
        Some([0x05, 0x01, 0x09, 0x05]) => "Gamepad".to_string(),
        Some([0x06, 0x00, 0xff, ..]) => "vendor 0xFF00 (HID++)".to_string(),
        Some([0x06, 0xfd, 0xff, ..]) => "vendor 0xFFFD (TrueForce)".to_string(),
        Some(b) => format!("unrecognised ({:02x} {:02x} {:02x} {:02x})", b[0], b[1], b[2], b[3]),
        None => "empty".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// An in-memory `HidppIo` a test can pre-load with canned responses (or
    /// leave a request unanswered, to exercise the timeout path) and
    /// inspect every request the code under test sent.
    #[derive(Default)]
    struct MockIo {
        responses: VecDeque<Option<Vec<u8>>>,
        sent: Vec<Vec<u8>>,
    }

    impl MockIo {
        fn push(&mut self, resp: Vec<u8>) {
            self.responses.push_back(Some(resp));
        }
        fn push_timeout(&mut self) {
            self.responses.push_back(None);
        }
    }

    impl HidppIo for MockIo {
        fn write_report(&mut self, report: &[u8]) -> io::Result<()> {
            self.sent.push(report.to_vec());
            Ok(())
        }
        fn read_report(&mut self, _timeout: Duration) -> io::Result<Vec<u8>> {
            match self.responses.pop_front() {
                Some(Some(resp)) => Ok(resp),
                Some(None) | None => Err(io::Error::new(io::ErrorKind::TimedOut, "mock timeout")),
            }
        }
    }

    /// A 20-byte long-report reply: header (id, device index, feature
    /// index, funcindex_clientid) then `params` zero-padded to 16 bytes,
    /// matching what the real wheel sends back regardless of request
    /// report type (verified live: it always answers 0x11 long).
    fn long_reply(device_index: u8, feature_index: u8, funcindex_clientid: u8, params: &[u8]) -> Vec<u8> {
        let mut r = vec![REPORT_ID_LONG, device_index, feature_index, funcindex_clientid];
        r.extend_from_slice(params);
        r.resize(20, 0);
        r
    }

    #[test]
    fn the_rev_dialect_shifts_its_function_into_the_high_nibble() {
        // The kernel sends fn6 as HIDPP_DD_LIGHTSYNC_FN_SET_CONFIG (0x60)
        // or'd with the software id: byte 3 is 0x6d, not 0x0f. It was 0x0f
        // for a while, because this borrowed a helper whose `function`
        // argument was already shifted, and the level command silently
        // never reached the wheel: --led-probe reported "sent" and the
        // strip stayed dark, which is the exact false negative the probe
        // exists to rule out (issue #27).
        let r = rev_long(0xff, 0x11, 6, &[0x00, 0x01, 0x00, 0x0a, 0x00, 5]);
        assert_eq!(r[0], REPORT_ID_LONG);
        assert_eq!(r[1], 0xff);
        assert_eq!(r[2], 0x11);
        assert_eq!(r[3], 0x6d, "fn6 in the high nibble, sw id 0x0d in the low");
        assert_eq!(&r[4..10], &[0x00, 0x01, 0x00, 0x0a, 0x00, 5]);
        // And the short helper it must agree with.
        let mut io = MockIo::default();
        rev_short(&mut io, 0x11, 2, 0).unwrap();
        assert_eq!(io.sent[0][3], 0x2d, "fn2 encodes the same way");
    }

    #[test]
    fn short_report_layout_matches_the_kernels_wire_format() {
        let r = short_report(0xff, 0x02, FN_GET, &[0x00]);
        assert_eq!(r, [0x10, 0xff, 0x02, 0x1a, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn resolve_feature_index_reads_the_returned_index() {
        let mut io = MockIo::default();
        // Root.getFeature(0x0003) -> feature index 2, matching the live
        // G923 capture (params[0] = 2).
        io.push(long_reply(0xff, 0x00, FN_GET_INFO | SW_ID, &[0x02, 0x00, 0x02]));
        let idx = resolve_feature_index(&mut io, 0x0003).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(io.sent[0], [0x10, 0xff, 0x00, FN_GET_INFO | SW_ID, 0x00, 0x03, 0x00]);
    }

    #[test]
    fn resolve_feature_index_is_none_when_unsupported() {
        let mut io = MockIo::default();
        io.push(long_reply(0xff, 0x00, FN_GET_INFO | SW_ID, &[0x00]));
        assert!(resolve_feature_index(&mut io, 0x0003).is_none());
    }

    #[test]
    fn resolve_feature_index_is_none_on_timeout() {
        let mut io = MockIo::default();
        io.push_timeout();
        assert!(resolve_feature_index(&mut io, 0x0003).is_none());
    }

    #[test]
    fn resolve_feature_index_is_none_on_a_mismatched_reply() {
        // A reply that does not echo the request's feature index (0x00,
        // the root) must not be trusted as an answer to this request.
        let mut io = MockIo::default();
        io.push(long_reply(0xff, 0x05, FN_GET_INFO | SW_ID, &[0x02]));
        assert!(resolve_feature_index(&mut io, 0x0003).is_none());
    }

    #[test]
    fn fw_entity_parses_the_live_captured_layout() {
        // Entity 1 off the real wheel: main app, prefix "U1 ", major
        // 0x38, minor 0x00, build 0x0034.
        let params = [0x00, 0x55, 0x31, 0x20, 0x38, 0x00, 0x00, 0x34];
        let fw = FwEntity::parse(&params).unwrap();
        assert_eq!(fw.fw_type, 0);
        assert_eq!(fw.prefix, "U1");
        assert_eq!(fw.format(), "U1 38.00.B0034");
    }

    #[test]
    fn fw_entity_drops_non_printable_prefix_padding() {
        // A short name padded with NULs (0x00 is outside 0x20..0x7f).
        let params = [0x01, 0x42, 0x4f, 0x00, 0x01, 0x08, 0x00, 0x00];
        let fw = FwEntity::parse(&params).unwrap();
        assert_eq!(fw.prefix, "BO");
    }

    #[test]
    fn fw_entity_is_none_for_a_short_response() {
        assert!(FwEntity::parse(&[0x00, 0x55, 0x31]).is_none());
    }

    #[test]
    fn query_main_firmware_scans_past_the_bootloader_entity() {
        // Reproduces the live capture shape: entity 0 is the bootloader
        // (fw_type 1), entity 1 is the main app (fw_type 0) - the query
        // must not stop at entity 0 and must not assume entity 0 is main.
        let mut io = MockIo::default();
        let feature_index = 0x02;
        io.push(long_reply(0xff, 0x00, FN_GET_INFO | SW_ID, &[feature_index])); // getFeature
        io.push(long_reply(0xff, feature_index, FN_GET_INFO | SW_ID, &[3])); // 3 entities
        io.push(long_reply(
            0xff,
            feature_index,
            FN_GET | SW_ID,
            &[0x01, 0x42, 0x4f, 0x54, 0x94, 0x00, 0x00, 0x34],
        )); // entity 0: bootloader
        io.push(long_reply(
            0xff,
            feature_index,
            FN_GET | SW_ID,
            &[0x00, 0x55, 0x31, 0x20, 0x38, 0x00, 0x00, 0x34],
        )); // entity 1: main app
        let fw = query_main_firmware(&mut io).unwrap();
        assert_eq!(fw, "U1 38.00.B0034");
    }

    #[test]
    fn query_main_firmware_is_none_when_the_feature_is_unsupported() {
        let mut io = MockIo::default();
        io.push(long_reply(0xff, 0x00, FN_GET_INFO | SW_ID, &[0x00]));
        assert!(query_main_firmware(&mut io).is_none());
    }

    #[test]
    fn query_main_firmware_is_none_when_no_entity_is_the_main_app() {
        let mut io = MockIo::default();
        let feature_index = 0x02;
        io.push(long_reply(0xff, 0x00, FN_GET_INFO | SW_ID, &[feature_index]));
        io.push(long_reply(0xff, feature_index, FN_GET_INFO | SW_ID, &[1]));
        io.push(long_reply(
            0xff,
            feature_index,
            FN_GET | SW_ID,
            &[0x01, 0x42, 0x4f, 0x54, 0x94, 0x00, 0x00, 0x34],
        )); // only entity: bootloader
        assert!(query_main_firmware(&mut io).is_none());
    }

    // --- report-descriptor sniffing + sibling discovery ---

    #[test]
    fn looks_like_hidpp_requires_both_report_ids() {
        // The real G923 interface-1 descriptor bytes (short 0x10 + long
        // 0x11 collections), captured 2026-07-26.
        let hidpp_descriptor: &[u8] = &[
            0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x10, 0x75, 0x08, 0x95, 0x06, 0x15,
            0x00, 0x26, 0xff, 0x00, 0x09, 0x01, 0x81, 0x00, 0x09, 0x01, 0x91, 0x00, 0xc0, 0x06,
            0x00, 0xff, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x11, 0x75, 0x08, 0x95, 0x13, 0x15, 0x00,
            0x26, 0xff, 0x00, 0x09, 0x02, 0x81, 0x00, 0x09, 0x02, 0x91, 0x00, 0xc0,
        ];
        assert!(looks_like_hidpp(hidpp_descriptor));

        // The joystick interface's descriptor mentions usage page 0xFF00
        // too (an appended vendor byte), but declares no report IDs at
        // all: must not be mistaken for the HID++ interface.
        let joystick_descriptor: &[u8] =
            &[0x06, 0x00, 0xff, 0x09, 0x00, 0x09, 0x01, 0x95, 0x02, 0x81, 0x02];
        assert!(!looks_like_hidpp(joystick_descriptor));

        // A third interface with only the short report ID present.
        let short_only: &[u8] = &[0x06, 0x00, 0xff, 0x85, 0x10, 0x95, 0x06];
        assert!(!looks_like_hidpp(short_only));
    }

    /// Build a fake sysfs tree mirroring the real layout `find_hidpp_sibling`
    /// walks: `<base>/usb/1-9/1-9:1.N/<hid_dir>[/hidraw/hidrawN]`, and
    /// return `(if0_dir, base)` so the test can point `find_hidpp_sibling`
    /// at `if0_dir` and clean up `base` afterwards.
    fn fake_usb_tree(base: &Path, interfaces: &[(&str, &[u8], Option<&str>)]) -> PathBuf {
        let usb_dir = base.join("1-9");
        let mut if0_dir = None;
        for (n, (hid_name, descriptor, hidraw_name)) in interfaces.iter().enumerate() {
            let iface_dir = usb_dir.join(format!("1-9:1.{n}"));
            let hid_dir = iface_dir.join(hid_name);
            std::fs::create_dir_all(&hid_dir).unwrap();
            std::fs::write(hid_dir.join("report_descriptor"), descriptor).unwrap();
            if let Some(node_name) = hidraw_name {
                std::fs::create_dir_all(hid_dir.join("hidraw").join(node_name)).unwrap();
            }
            if n == 0 {
                if0_dir = Some(hid_dir);
            }
        }
        if0_dir.unwrap()
    }

    #[test]
    fn find_hidpp_sibling_locates_the_matching_interface() {
        let base = std::env::temp_dir()
            .join(format!("logi-wheel-hidpp-test-{}-{}", std::process::id(), line!()));
        let joystick_descriptor: &[u8] = &[0x06, 0x00, 0xff, 0x09, 0x00];
        let hidpp_descriptor: &[u8] =
            &[0x06, 0x00, 0xff, 0x85, 0x10, 0x00, 0x06, 0x00, 0xff, 0x85, 0x11, 0x00];
        let if0_dir = fake_usb_tree(
            &base,
            &[
                ("0003:046D:C266.0004", joystick_descriptor, Some("hidraw3")),
                ("0003:046D:C266.0005", hidpp_descriptor, Some("hidraw4")),
            ],
        );
        let sibling = find_hidpp_sibling(&if0_dir).expect("sibling found");
        assert_eq!(sibling, PathBuf::from("/dev/hidraw4"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn find_hidpp_sibling_is_none_without_a_matching_interface() {
        let base = std::env::temp_dir()
            .join(format!("logi-wheel-hidpp-test-{}-{}", std::process::id(), line!()));
        let joystick_descriptor: &[u8] = &[0x06, 0x00, 0xff, 0x09, 0x00];
        let if0_dir = fake_usb_tree(
            &base,
            &[("0003:046D:C266.0004", joystick_descriptor, Some("hidraw3"))],
        );
        assert!(find_hidpp_sibling(&if0_dir).is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn query_g923_firmware_is_none_for_a_dev_override_directory() {
        // A `LOGI_WHEEL_SYSFS_DIR` fixture is a plain temp dir with no USB
        // parent structure at all; the sibling walk must fail cleanly
        // rather than panic on the missing `parent()`s.
        let dir = std::env::temp_dir()
            .join(format!("logi-wheel-hidpp-test-plain-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(query_g923_firmware(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
