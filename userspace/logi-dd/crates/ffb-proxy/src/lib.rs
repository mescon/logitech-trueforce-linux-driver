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
