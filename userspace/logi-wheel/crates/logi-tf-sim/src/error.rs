// SPDX-License-Identifier: GPL-2.0-only
//! Crate error type.

use std::fmt;

/// Errors surfaced by the daemon, the sweep mode, and the stream wrapper.
#[derive(Debug)]
pub enum Error {
    /// No force-feedback session could be held open, so a direct-drive
    /// wheel would move unpredictably. See `ffb_keepalive`.
    Unstabilised,
    /// An OS-level failure, with context (what was being attempted).
    Io(String, std::io::Error),
    /// Another program already holds this wheel's streaming lease, named
    /// as it wrote itself into the lock file. See [`crate::lease`].
    Busy(String),
    /// No supported wheel was found by libtrueforce discovery.
    NoWheel,
    /// The wheel is there but cannot take a synthesised stream, and
    /// will not until its setup changes: the G923 Xbox edition on the
    /// firmware force path, whose stream would silence its force
    /// feedback. The daemon drives the rev display for it instead.
    NoHaptics(String),
    /// A libtrueforce call failed: (function, LOGITF_* return code).
    Stream(String, i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unstabilised => write!(
                f,
                "refusing to run: no force-feedback session could be opened, and \
                 without one a direct-drive wheel can drive itself into its stops. \
                 Check that you can write to the wheel's /dev/input/event* node \
                 (the udev rules grant this); see issue #57"
            ),
            Error::Busy(holder) => write!(
                f,
                "refusing to stream: {holder} is already streaming to this wheel. \
                 The wheel's stream endpoint carries one packet per millisecond, so \
                 two programs on it do not share it, they take turns, and the motor \
                 is square-modulated at 500 Hz (that is the buzz). Stop it and try again"
            ),
            Error::Io(what, e) => write!(f, "{what}: {e}"),
            Error::NoWheel => write!(f, "no supported wheel found"),
            Error::NoHaptics(why) => write!(f, "{why}"),
            Error::Stream(func, rc) => write!(f, "{func} failed (rc {rc})"),
        }
    }
}

impl std::error::Error for Error {}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn the_refusal_says_what_to_do_about_it() {
        let msg = Error::Unstabilised.to_string();
        assert!(msg.contains("refusing to run"), "says it refused: {msg}");
        assert!(msg.contains("force-feedback"), "names the cause: {msg}");
        assert!(msg.contains("/dev/input/event"), "names what to check: {msg}");
        assert!(msg.contains("#57"), "points at the explanation: {msg}");
    }

    /// The refusal a test sweep shows when the daemon (or another sweep)
    /// has the wheel. It has to name the holder: "something else is using
    /// it" sends nobody anywhere.
    #[test]
    fn the_busy_message_names_the_holder() {
        let msg = Error::Busy("logi-tf-sim (pid 4242)".into()).to_string();
        assert!(msg.contains("logi-tf-sim (pid 4242)"), "names who holds it: {msg}");
        assert!(msg.contains("refusing to stream"), "says it refused: {msg}");
        assert!(msg.contains("500 Hz"), "says what sharing would cost: {msg}");
    }
}
