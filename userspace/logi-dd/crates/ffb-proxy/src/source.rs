//! Real-wheel evdev input source.
//!
//! Reads `struct input_event` records from the real wheel's evdev node
//! (steering, pedals, buttons) and folds them into a `descriptor::InputReport`
//! frame, completed whenever an `EV_SYN`/`SYN_REPORT` event arrives.
//!
//! Event records are decoded from raw bytes with explicit `from_le_bytes`
//! offsets rather than by casting a byte buffer to `input_event` in place:
//! the read buffer is a plain `[u8; N]` with no alignment guarantee, and
//! `input_event` (though not packed) still requires proper alignment to
//! read through a reference, so we avoid that entirely.

use std::os::unix::io::{AsFd, AsRawFd, OwnedFd, RawFd};

use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::read;

use crate::descriptor::InputReport;
use crate::{Error, Result};

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

pub const SYN_REPORT: u16 = 0x00;

pub const ABS_X: u16 = 0x00;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;

pub const BTN_TRIGGER: u16 = 0x120;

// Real-wheel axis assignment, hardware-confirmed on the RS50 (the live
// input monitor sessions, and issue #50): throttle/brake/clutch arrive on
// ABS_RX/ABS_RY/ABS_RZ. The earlier ABS_Y/ABS_Z guesses matched nothing
// the wheel emits (ABS_Z is the handbrake accessory), which left pedals
// frozen on the virtual device. Everything else in this module refers to
// these names, not the raw ABS_* codes.
const AXIS_STEERING: u16 = ABS_X;
const AXIS_THROTTLE: u16 = ABS_RX;
const AXIS_BRAKE: u16 = ABS_RY;
const AXIS_CLUTCH: u16 = ABS_RZ;

/// Mirrors the kernel's `struct timeval` as embedded in `struct input_event`
/// on 64-bit Linux (both fields are `long`, i.e. 8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// Mirrors the kernel's `struct input_event` (naturally aligned, not
/// packed): `time`, then `type`, `code`, `value`.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct input_event {
    pub time: Timeval,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

// Byte layout of a decoded input_event on the wire (64-bit kernel ABI):
// tv_sec(8) + tv_usec(8) + type(2) + code(2) + value(4) = 24 bytes.
const TYPE_OFF: usize = 16;
const CODE_OFF: usize = TYPE_OFF + 2;
const VALUE_OFF: usize = CODE_OFF + 2;
const EVENT_SIZE: usize = VALUE_OFF + 4;

fn decode_event(b: &[u8]) -> input_event {
    input_event {
        time: Timeval {
            tv_sec: i64::from_le_bytes(b[0..8].try_into().unwrap()),
            tv_usec: i64::from_le_bytes(b[8..16].try_into().unwrap()),
        },
        type_: u16::from_le_bytes(b[TYPE_OFF..TYPE_OFF + 2].try_into().unwrap()),
        code: u16::from_le_bytes(b[CODE_OFF..CODE_OFF + 2].try_into().unwrap()),
        value: i32::from_le_bytes(b[VALUE_OFF..VALUE_OFF + 4].try_into().unwrap()),
    }
}

/// Fold a single evdev event into `report`. Returns `true` when the event is
/// `EV_SYN`/`SYN_REPORT`, meaning `report` now holds a complete frame.
///
/// Axis values are assigned directly from the raw evdev value, clamped to
/// the report field's `0..=65535` range; this is a placeholder scaling and
/// is expected to be revisited once real hardware ranges are confirmed.
/// Button codes outside the 32-bit `buttons` field (i.e. `code - BTN_TRIGGER
/// >= 32`) are ignored rather than shifted, which would overflow.
pub fn map_event(report: &mut InputReport, ev: &input_event) -> bool {
    match ev.type_ {
        EV_ABS => {
            let value = ev.value.clamp(0, 0xFFFF) as u16;
            match ev.code {
                AXIS_STEERING => report.steering = value,
                AXIS_THROTTLE => report.throttle = value,
                AXIS_BRAKE => report.brake = value,
                AXIS_CLUTCH => report.clutch = value,
                _ => {}
            }
        }
        EV_KEY if ev.code >= BTN_TRIGGER => {
            let bit = ev.code - BTN_TRIGGER;
            if bit < 32 {
                if ev.value != 0 {
                    report.buttons |= 1 << bit;
                } else {
                    report.buttons &= !(1 << bit);
                }
            }
        }
        _ => {}
    }
    ev.type_ == EV_SYN && ev.code == SYN_REPORT
}

/// The real wheel's evdev input node, opened non-blocking so it can be
/// polled alongside the uhid device and PID socket.
pub struct Source {
    fd: OwnedFd,
}

impl Source {
    /// Open the evdev node at `evdev_path` read-only and non-blocking.
    pub fn open(evdev_path: &str) -> Result<Source> {
        let fd = open(evdev_path, OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC, Mode::empty())
            .map_err(|e| Error::Io(format!("open {evdev_path}"), std::io::Error::from(e)))?;
        Ok(Source { fd })
    }

    /// The raw file descriptor, for callers that want to poll it alongside
    /// other sources (the uhid device, the PID command socket).
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_fd().as_raw_fd()
    }

    /// Read whatever events are currently available (non-blocking) and fold
    /// each into `report` via [`map_event`]. Returns `true` if a complete
    /// frame (an `EV_SYN`/`SYN_REPORT`) was seen among them.
    ///
    /// A read that would block (no data queued) is not an error: it means no
    /// complete frame is available yet, so this returns `Ok(false)`.
    ///
    /// Any trailing bytes shorter than one full `input_event` (a short read
    /// landing mid-record) are dropped; the kernel writes evdev events as
    /// whole records so this should not happen in practice.
    pub fn read_into(&mut self, report: &mut InputReport) -> Result<bool> {
        let mut buf = [0u8; EVENT_SIZE * 64];
        let n = match read(&self.fd, &mut buf) {
            Ok(n) => n,
            Err(Errno::EAGAIN) => return Ok(false),
            Err(e) => return Err(Error::Io("read evdev".into(), std::io::Error::from(e))),
        };

        let mut complete = false;
        for chunk in buf[..n].chunks_exact(EVENT_SIZE) {
            if map_event(report, &decode_event(chunk)) {
                complete = true;
            }
        }
        Ok(complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::InputReport;

    fn ev(type_: u16, code: u16, value: i32) -> input_event {
        input_event { time: Timeval { tv_sec: 0, tv_usec: 0 }, type_, code, value }
    }

    #[test]
    fn abs_x_maps_to_steering_and_syn_completes_frame() {
        let mut r = InputReport::default();
        assert!(!map_event(&mut r, &ev(EV_ABS, ABS_X, 0x4000)));
        // Pin the RAW pedal codes to the RS50's hardware truth (issue #50:
        // the old ABS_Y/ABS_Z guesses left pedals frozen). 0x03/0x04/0x05
        // are what the wheel actually emits for throttle/brake/clutch.
        assert!(!map_event(&mut r, &ev(EV_ABS, 0x03, 111)));
        assert_eq!(r.throttle, 111, "throttle must map from raw ABS_RX (0x03)");
        assert!(!map_event(&mut r, &ev(EV_ABS, 0x04, 222)));
        assert_eq!(r.brake, 222, "brake must map from raw ABS_RY (0x04)");
        assert!(!map_event(&mut r, &ev(EV_ABS, 0x05, 333)));
        assert_eq!(r.clutch, 333, "clutch must map from raw ABS_RZ (0x05)");
        assert_eq!(r.steering, 0x4000);
        assert!(map_event(&mut r, &ev(EV_SYN, SYN_REPORT, 0)));
    }

    #[test]
    fn button_sets_bit() {
        let mut r = InputReport::default();
        map_event(&mut r, &ev(EV_KEY, BTN_TRIGGER, 1));
        assert_eq!(r.buttons & 1, 1);
        map_event(&mut r, &ev(EV_KEY, BTN_TRIGGER, 0));
        assert_eq!(r.buttons & 1, 0);
    }
}
