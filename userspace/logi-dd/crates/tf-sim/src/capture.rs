// SPDX-License-Identifier: GPL-2.0-only
//! `capture`: record a game's raw UDP telemetry to a file so its wire
//! format can be figured out and, eventually, parsed like the other
//! games in this crate.
//!
//! Binds the given port, appends every received datagram to a small
//! self-describing binary log (a [`encode_header`] once, then one
//! [`encode_record`] per datagram), and prints a live packet/byte/size
//! count so the operator can tell traffic is flowing. On SIGINT/SIGTERM
//! it finalizes the file and prints a summary plus the next step
//! (open an issue with the file attached).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// File magic identifying a capture log.
const MAGIC: &[u8; 9] = b"LOGITFCAP";
/// On-disk format version, bumped if the header or record layout changes.
const FORMAT_VERSION: u8 = 1;
/// How often the live packet/byte/size line is printed.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// Poll timeout; bounds how quickly Ctrl-C and the report line land.
const POLL_TIMEOUT_MS: i32 = 200;
/// recv(2) buffer, comfortably larger than any known telemetry packet.
const RECV_BUF: usize = 8192;

/// Build the capture file header: magic, format version, listen port, and
/// a length-prefixed label. Pure, so it is golden-byte tested without a
/// socket or filesystem.
pub fn encode_header(port: u16, label: &str) -> Vec<u8> {
    let label = label.as_bytes();
    let mut out = Vec::with_capacity(MAGIC.len() + 1 + 2 + 2 + label.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&port.to_le_bytes());
    out.extend_from_slice(&(label.len() as u16).to_le_bytes());
    out.extend_from_slice(label);
    out
}

/// Build one record: a monotonic microsecond timestamp, the payload
/// length, then the raw payload bytes. Pure for the same reason as
/// [`encode_header`].
pub fn encode_record(timestamp_us: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 4 + payload.len());
    out.extend_from_slice(&timestamp_us.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Block until `sock` is readable or the timeout expires.
fn poll_readable(sock: &UdpSocket) {
    let mut fd = libc::pollfd { fd: sock.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: fd is a single valid, initialized pollfd. EINTR and other
    // failures just fall through to the (nonblocking) read.
    unsafe { libc::poll(&mut fd, 1, POLL_TIMEOUT_MS) };
}

/// Render the distinct payload sizes seen so far as `[a, b, c]`.
fn sizes_list(sizes: &BTreeSet<usize>) -> String {
    let parts: Vec<String> = sizes.iter().map(|s| s.to_string()).collect();
    format!("[{}]", parts.join(", "))
}

/// Run the capture until `stop` is set (SIGINT/SIGTERM). Binding the port
/// or creating the output file is fatal; everything after that degrades
/// to a best-effort finalize so a late failure still leaves a usable file.
pub fn run(port: u16, out: &str, label: &str, stop: &AtomicBool) -> Result<()> {
    let sock = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| Error::Io(format!("bind UDP port {port}"), e))?;
    sock.set_nonblocking(true).map_err(|e| Error::Io(format!("set_nonblocking on port {port}"), e))?;

    let file = File::create(out).map_err(|e| Error::Io(format!("create {out}"), e))?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&encode_header(port, label)).map_err(|e| Error::Io(format!("write header to {out}"), e))?;

    eprintln!("logi-tf-sim: capture: listening on udp/{port}, writing to {out}");
    if !label.is_empty() {
        eprintln!("logi-tf-sim: capture: label '{label}'");
    }
    eprintln!("logi-tf-sim: capture: drive for about 30s (idle, a few rev sweeps, gear shifts), then press Ctrl-C");

    let start = Instant::now();
    let mut last_report = start;
    let mut packets: u64 = 0;
    let mut bytes: u64 = 0;
    let mut sizes: BTreeSet<usize> = BTreeSet::new();
    let mut buf = [0u8; RECV_BUF];

    while !stop.load(Ordering::SeqCst) {
        poll_readable(&sock);
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, _peer)) => {
                    let ts_us = start.elapsed().as_micros() as u64;
                    writer
                        .write_all(&encode_record(ts_us, &buf[..n]))
                        .map_err(|e| Error::Io(format!("write record to {out}"), e))?;
                    packets += 1;
                    bytes += n as u64;
                    sizes.insert(n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(format!("recv on udp/{port}"), e)),
            }
        }

        let now = Instant::now();
        if now.duration_since(last_report) >= REPORT_INTERVAL {
            last_report = now;
            eprintln!("logi-tf-sim: capture: {packets} packets, {bytes} bytes, sizes seen: {}", sizes_list(&sizes));
        }
    }

    writer.flush().map_err(|e| Error::Io(format!("flush {out}"), e))?;

    eprintln!("logi-tf-sim: capture: stopped");
    eprintln!(
        "logi-tf-sim: capture: {packets} packets, {bytes} bytes, {:.1}s, sizes seen: {}",
        start.elapsed().as_secs_f64(),
        sizes_list(&sizes)
    );
    eprintln!(
        "Recording saved to {out}. To add support for your game, open an issue at the \
         project's GitHub with this file attached and note the game name and what you \
         did (idle, revved, shifted gears)."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_has_the_expected_fields() {
        let bytes = encode_header(20777, "test");
        assert_eq!(&bytes[0..9], MAGIC);
        assert_eq!(bytes[9], FORMAT_VERSION);
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 20777);
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 4);
        assert_eq!(&bytes[14..], b"test");
        assert_eq!(bytes.len(), 14 + 4);
    }

    #[test]
    fn header_golden_bytes() {
        let bytes = encode_header(20999, "AC");
        let mut expected = b"LOGITFCAP".to_vec();
        expected.push(1); // format version
        expected.extend_from_slice(&20999u16.to_le_bytes());
        expected.extend_from_slice(&2u16.to_le_bytes());
        expected.extend_from_slice(b"AC");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn header_with_empty_label_has_no_trailing_bytes() {
        let bytes = encode_header(4444, "");
        assert_eq!(bytes.len(), MAGIC.len() + 1 + 2 + 2);
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 0);
    }

    #[test]
    fn record_round_trips_its_fields() {
        let payload = [1u8, 2, 3, 4, 5];
        let bytes = encode_record(123_456_789, &payload);
        assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 123_456_789);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), payload.len() as u32);
        assert_eq!(&bytes[12..], &payload);
        assert_eq!(bytes.len(), 12 + payload.len());
    }

    #[test]
    fn record_with_empty_payload_is_just_the_prefix() {
        let bytes = encode_record(0, &[]);
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[0..8], &0u64.to_le_bytes());
        assert_eq!(&bytes[8..12], &0u32.to_le_bytes());
    }

    #[test]
    fn sizes_list_renders_sorted_and_deduplicated() {
        let mut set = BTreeSet::new();
        for n in [66, 12, 66, 512] {
            set.insert(n);
        }
        assert_eq!(sizes_list(&set), "[12, 66, 512]");
        assert_eq!(sizes_list(&BTreeSet::new()), "[]");
    }
}
