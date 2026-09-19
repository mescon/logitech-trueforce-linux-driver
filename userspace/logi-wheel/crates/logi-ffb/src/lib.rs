// SPDX-License-Identifier: GPL-2.0-only
//! Userspace DirectInput force-feedback proxy for the Logitech direct-drive wheels.

pub mod descriptor;
pub mod uhid;
pub mod pidff;
pub mod source;
pub mod sink;
pub mod steering;
pub mod proxy;
pub mod cli;

use std::fmt;

/// Say something to the person running this: on stderr, and also appended
/// to the file `LOGI_LAUNCH_LOG` names when that is set. The launcher sets
/// it to its own log so that a proxy that refuses to start explains itself
/// next to the plan that started it. Under Steam, stderr goes to a console
/// nobody reads, and a game that "did not start" came with no reason
/// attached (#105). Never fails: a log that cannot be written is not a
/// reason to lose the message on stderr.
pub fn note(msg: &str) {
    eprintln!("{msg}");
    if let Some(path) = std::env::var_os("LOGI_LAUNCH_LOG") {
        note_to(std::path::Path::new(&path), msg);
    }
}

/// The file half of [`note`], separated so it can be tested without
/// touching the process environment.
pub fn note_to(path: &std::path::Path, msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

#[cfg(test)]
mod note_tests {
    #[test]
    fn note_to_appends_one_line_per_call() {
        let dir = std::env::temp_dir().join(format!("logi-ffb-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("launch.log");
        super::note_to(&log, "logi-ffb: first");
        super::note_to(&log, "logi-ffb: second");
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text, "logi-ffb: first\nlogi-ffb: second\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_to_an_unwritable_path_is_silent() {
        super::note_to(std::path::Path::new("/proc/no-such-dir/launch.log"), "dropped");
    }
}

#[derive(Debug)]
pub enum Error {
    /// The real wheel's evdev FF node could not be found.
    WheelNotFound,
    /// A syscall or I/O operation failed, with context.
    Io(String, std::io::Error),
    /// A PID or HID payload was malformed.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WheelNotFound => write!(f, "no Logitech direct-drive wheel with an FF interface was found"),
            Error::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
