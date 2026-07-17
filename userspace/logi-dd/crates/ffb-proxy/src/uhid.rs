//! Wrapper around `/dev/uhid`, the kernel's userspace-HID device interface.
//!
//! Unlike most kernel character devices, `/dev/uhid` is not ioctl-driven: a
//! process creates and drives a virtual HID device by reading and writing
//! fixed-size `struct uhid_event` records. This module hand-rolls the wire
//! layout from `linux/uhid.h` (event type numbers, `uhid_create2_req`,
//! `uhid_input2_req`, `uhid_output_req`) because there is no safe-Rust crate
//! wrapping it and the layouts are small and stable.
//!
//! We deliberately avoid taking references into `#[repr(C, packed)]` structs
//! (which is undefined behavior for misaligned fields); everything here is
//! encoded/decoded through explicit byte-offset slicing and
//! `to_le_bytes`/`from_le_bytes`, matching the kernel's native-endian little
//! endian layout on the target architecture.

use std::os::unix::io::{AsFd, OwnedFd, RawFd};

use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::{read, write};

use crate::{descriptor, Error, Result};

const UHID_PATH: &str = "/dev/uhid";

pub const UHID_DESTROY: u32 = 1;
pub const UHID_START: u32 = 2;
pub const UHID_OPEN: u32 = 4;
pub const UHID_CLOSE: u32 = 5;
pub const UHID_OUTPUT: u32 = 6;
pub const UHID_CREATE2: u32 = 11;
pub const UHID_INPUT2: u32 = 12;

pub const BUS_USB: u16 = 0x03;

const NAME_LEN: usize = 128;
const PHYS_LEN: usize = 64;
const UNIQ_LEN: usize = 64;
const RD_DATA_LEN: usize = 4096;
const INPUT2_DATA_LEN: usize = 4096;
const OUTPUT_DATA_LEN: usize = 4096;

/// Mirrors `struct uhid_create2_req` from `linux/uhid.h` (packed, native
/// endian). Not read/written directly; used only for its `size_of` so the
/// event buffer is sized to hold the largest union member.
#[repr(C, packed)]
pub struct Create2Req {
    pub name: [u8; NAME_LEN],
    pub phys: [u8; PHYS_LEN],
    pub uniq: [u8; UNIQ_LEN],
    pub rd_size: u16,
    pub bus: u16,
    pub vendor: u32,
    pub product: u32,
    pub version: u32,
    pub country: u32,
    pub rd_data: [u8; RD_DATA_LEN],
}

/// Mirrors `struct uhid_input2_req`.
#[repr(C, packed)]
pub struct Input2Req {
    pub size: u16,
    pub data: [u8; INPUT2_DATA_LEN],
}

/// Mirrors `struct uhid_output_req`.
#[repr(C, packed)]
pub struct OutputReq {
    pub data: [u8; OUTPUT_DATA_LEN],
    pub size: u16,
    pub rtype: u8,
}

/// Byte offsets of each field within the union, i.e. relative to the byte
/// right after the leading `u32 type` field of `struct uhid_event`.
mod create2_off {
    use super::*;
    pub const NAME: usize = 0;
    pub const PHYS: usize = NAME + NAME_LEN;
    pub const UNIQ: usize = PHYS + PHYS_LEN;
    pub const RD_SIZE: usize = UNIQ + UNIQ_LEN;
    pub const BUS: usize = RD_SIZE + 2;
    pub const VENDOR: usize = BUS + 2;
    pub const PRODUCT: usize = VENDOR + 4;
    pub const VERSION: usize = PRODUCT + 4;
    pub const COUNTRY: usize = VERSION + 4;
    pub const RD_DATA: usize = COUNTRY + 4;
}

mod input2_off {
    pub const SIZE: usize = 0;
    pub const DATA: usize = 2;
}

mod output_off {
    use super::OUTPUT_DATA_LEN;
    pub const DATA: usize = 0;
    pub const SIZE: usize = DATA + OUTPUT_DATA_LEN;
    // `rtype` (offset SIZE + 2) follows `size`; the report type byte is not
    // currently consumed, only the raw report bytes are.
}

/// Size of the fixed record read from / written to `/dev/uhid`: the leading
/// `u32 type` field plus the largest union member (`uhid_create2_req`).
pub const EVENT_SIZE: usize = 4 + core::mem::size_of::<Create2Req>();

/// Events read back from the kernel via `/dev/uhid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `UHID_START`: a driver has bound to the virtual device.
    Start,
    /// `UHID_OPEN`: the device has its first user (evdev node opened).
    Open,
    /// `UHID_CLOSE`: the device lost its last user.
    Close,
    /// `UHID_OUTPUT`: an output report (FFB command) from the driver, carrying
    /// the raw report bytes (report id first, if any).
    Output(Vec<u8>),
    /// Any other event type we do not act on, carrying the raw type number.
    Other(u32),
}

/// Build a `UHID_CREATE2` event buffer.
///
/// `name` is truncated to fit `NAME_LEN - 1` bytes and NUL-terminated (the
/// kernel expects a NUL-terminated C string, but does not require the field
/// to be fully used). `rd_data` is copied into the fixed 4096-byte report
/// descriptor slot; the actual size is recorded in `rd_size`.
pub fn encode_create2(name: &str, bus: u16, vendor: u32, product: u32, rd_data: &[u8]) -> Vec<u8> {
    assert!(rd_data.len() <= RD_DATA_LEN, "report descriptor too large for uhid_create2_req");

    let mut buf = vec![0u8; EVENT_SIZE];
    buf[0..4].copy_from_slice(&UHID_CREATE2.to_le_bytes());

    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(NAME_LEN - 1);
    let name_off = 4 + create2_off::NAME;
    buf[name_off..name_off + copy_len].copy_from_slice(&name_bytes[..copy_len]);
    // Remaining name bytes (including the terminator) stay zero-initialized.

    let rd_size_off = 4 + create2_off::RD_SIZE;
    buf[rd_size_off..rd_size_off + 2].copy_from_slice(&(rd_data.len() as u16).to_le_bytes());

    let bus_off = 4 + create2_off::BUS;
    buf[bus_off..bus_off + 2].copy_from_slice(&bus.to_le_bytes());

    let vendor_off = 4 + create2_off::VENDOR;
    buf[vendor_off..vendor_off + 4].copy_from_slice(&vendor.to_le_bytes());

    let product_off = 4 + create2_off::PRODUCT;
    buf[product_off..product_off + 4].copy_from_slice(&product.to_le_bytes());

    // version and country are left at 0; the kernel does not require them.

    let rd_data_off = 4 + create2_off::RD_DATA;
    buf[rd_data_off..rd_data_off + rd_data.len()].copy_from_slice(rd_data);

    buf
}

/// Build a `UHID_INPUT2` event buffer carrying `report` as the input data.
pub fn encode_input2(report: &[u8]) -> Vec<u8> {
    assert!(report.len() <= INPUT2_DATA_LEN, "input report too large for uhid_input2_req");

    let mut buf = vec![0u8; EVENT_SIZE];
    buf[0..4].copy_from_slice(&UHID_INPUT2.to_le_bytes());

    let size_off = 4 + input2_off::SIZE;
    buf[size_off..size_off + 2].copy_from_slice(&(report.len() as u16).to_le_bytes());

    let data_off = 4 + input2_off::DATA;
    buf[data_off..data_off + report.len()].copy_from_slice(report);

    buf
}

/// Build a bare event buffer with only the type field set (used for
/// `UHID_DESTROY`, which carries no union payload).
fn encode_bare(event_type: u32) -> Vec<u8> {
    let mut buf = vec![0u8; EVENT_SIZE];
    buf[0..4].copy_from_slice(&event_type.to_le_bytes());
    buf
}

/// Parse a raw `EVENT_SIZE` buffer read from `/dev/uhid` into an [`Event`].
///
/// Returns `Err` only if `buf` is shorter than a valid event buffer; unknown
/// event types are not an error, they map to `Event::Other`.
pub fn parse_event(buf: &[u8]) -> Result<Event> {
    if buf.len() < 4 {
        return Err(Error::Protocol("uhid event buffer shorter than the type field".into()));
    }
    let event_type = u32::from_le_bytes(buf[0..4].try_into().unwrap());

    Ok(match event_type {
        UHID_START => Event::Start,
        UHID_OPEN => Event::Open,
        UHID_CLOSE => Event::Close,
        UHID_OUTPUT => {
            let size_off = 4 + output_off::SIZE;
            if buf.len() < size_off + 2 {
                return Err(Error::Protocol("uhid output event truncated before size field".into()));
            }
            let size = u16::from_le_bytes(buf[size_off..size_off + 2].try_into().unwrap()) as usize;

            let data_off = 4 + output_off::DATA;
            if buf.len() < data_off + size {
                return Err(Error::Protocol("uhid output event truncated before report data".into()));
            }
            Event::Output(buf[data_off..data_off + size].to_vec())
        }
        other => Event::Other(other),
    })
}

/// A virtual uhid device: the joystick + FFB HID interface presented to the
/// kernel, backed by `/dev/uhid`.
pub struct Device {
    fd: OwnedFd,
}

impl Device {
    /// Open `/dev/uhid` and register a `UHID_CREATE2` device using the
    /// wheel's identity and report descriptor from [`descriptor`].
    pub fn create() -> Result<Device> {
        let fd = open(UHID_PATH, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
            .map_err(|e| Error::Io(format!("open {UHID_PATH}"), std::io::Error::from(e)))?;

        let event = encode_create2(
            descriptor::NAME,
            BUS_USB,
            descriptor::VENDOR as u32,
            descriptor::PRODUCT as u32,
            descriptor::report_descriptor(),
        );

        write(&fd, &event).map_err(|e| Error::Io("write UHID_CREATE2".into(), std::io::Error::from(e)))?;

        Ok(Device { fd })
    }

    /// Block until the next event is available on `/dev/uhid` and parse it.
    pub fn read_event(&mut self) -> Result<Event> {
        let mut buf = vec![0u8; EVENT_SIZE];
        let n = read(&self.fd, &mut buf).map_err(|e| Error::Io("read uhid event".into(), std::io::Error::from(e)))?;
        buf.truncate(n);
        parse_event(&buf)
    }

    /// Send an input report (e.g. steering/pedal/button state) up through
    /// the virtual device via `UHID_INPUT2`.
    pub fn send_input(&mut self, report: &[u8]) -> Result<()> {
        let event = encode_input2(report);
        write(&self.fd, &event).map_err(|e| Error::Io("write UHID_INPUT2".into(), std::io::Error::from(e)))?;
        Ok(())
    }

    /// The raw file descriptor, for callers that want to poll it alongside
    /// other sources (e.g. the real wheel's evdev FF node).
    pub fn raw_fd(&self) -> RawFd {
        use std::os::fd::AsRawFd;
        self.fd.as_fd().as_raw_fd()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best-effort: if the kernel already tore the device down (or the fd
        // is otherwise gone), there is nothing useful to do with the error.
        let _ = write(&self.fd, &encode_bare(UHID_DESTROY));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create2_event_encodes_name_sizes_and_ids() {
        let bytes = encode_create2("wheel", 0x03, 0x046d, 0xc276, &[0xAA, 0xBB]);
        // type field first, little-endian u32 == UHID_CREATE2
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), UHID_CREATE2);
        // name starts right after the type field
        assert_eq!(&bytes[4..9], b"wheel");
        // rd_size and rd_data present
        assert!(bytes.len() >= 4 + core::mem::size_of::<Create2Req>());
    }

    #[test]
    fn output_event_parses_report_bytes() {
        // craft a raw event buffer with type=UHID_OUTPUT and a 4-byte report
        let mut buf = vec![0u8; EVENT_SIZE];
        buf[0..4].copy_from_slice(&UHID_OUTPUT.to_le_bytes());
        let data_off = 4; // union starts after type; uhid_output_req.data is first field
        buf[data_off..data_off + 4].copy_from_slice(&[0x51, 0x01, 0x02, 0x03]);
        // size field sits after 4096 data bytes
        let size_off = data_off + 4096;
        buf[size_off..size_off + 2].copy_from_slice(&4u16.to_le_bytes());
        let ev = parse_event(&buf).unwrap();
        assert!(matches!(ev, Event::Output(ref r) if r == &[0x51, 0x01, 0x02, 0x03]));
    }
}
