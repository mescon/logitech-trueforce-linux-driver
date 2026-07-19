// SPDX-License-Identifier: GPL-2.0-only
//! Crate error type.

use std::fmt;

/// Errors surfaced by the daemon, the sweep mode, and the stream wrapper.
#[derive(Debug)]
pub enum Error {
    /// An OS-level failure, with context (what was being attempted).
    Io(String, std::io::Error),
    /// No supported wheel was found by libtrueforce discovery.
    NoWheel,
    /// A libtrueforce call failed: (function, LOGITF_* return code).
    Stream(String, i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(what, e) => write!(f, "{what}: {e}"),
            Error::NoWheel => write!(f, "no supported wheel found"),
            Error::Stream(func, rc) => write!(f, "{func} failed (rc {rc})"),
        }
    }
}

impl std::error::Error for Error {}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
