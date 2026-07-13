use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Desktop,
    Onboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Driver not loaded or no wheel bound.
    NoWheel,
    /// sysfs read/write failed for an unmapped reason.
    Io(String),
    /// The write needs the wheel in a different mode first.
    WrongMode { needed: Mode },
    /// Attribute absent on this wheel/firmware (-EOPNOTSUPP).
    Unsupported,
    /// -ERANGE or a local range check.
    OutOfRange,
    /// -EINVAL or a local format check.
    Invalid,
    /// Could not parse the current value string.
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoWheel => write!(f, "no wheel found (driver loaded and bound?)"),
            Error::Io(s) => write!(f, "sysfs error: {s}"),
            Error::WrongMode { needed } => write!(f, "needs {needed:?} mode"),
            Error::Unsupported => write!(f, "not supported on this wheel/firmware"),
            Error::OutOfRange => write!(f, "value out of range"),
            Error::Invalid => write!(f, "invalid value"),
            Error::Parse(s) => write!(f, "could not read value: {s}"),
        }
    }
}
impl std::error::Error for Error {}

/// Attributes that a real G Pro exposes only in onboard mode; everything else
/// that returns EPERM does so because it needs desktop mode.
fn onboard_only(attr: &str) -> bool {
    attr == "wheel_brake_force"
}

pub fn map_io_error(e: &std::io::Error, attr: &str) -> Error {
    match e.raw_os_error() {
        Some(1) => Error::WrongMode {
            needed: if onboard_only(attr) { Mode::Onboard } else { Mode::Desktop },
        }, // EPERM
        Some(95) => Error::Unsupported, // EOPNOTSUPP
        Some(34) => Error::OutOfRange,  // ERANGE
        Some(22) => Error::Invalid,     // EINVAL
        _ => Error::Io(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn eperm_on_sensitivity_is_wrong_mode() {
        let e = io::Error::from_raw_os_error(1); // EPERM
        assert!(matches!(map_io_error(&e, "wheel_sensitivity"),
                         Error::WrongMode { needed: Mode::Desktop }));
    }

    #[test]
    fn eperm_on_brake_force_is_wrong_mode_onboard() {
        let e = io::Error::from_raw_os_error(1);
        assert!(matches!(map_io_error(&e, "wheel_brake_force"),
                         Error::WrongMode { needed: Mode::Onboard }));
    }

    #[test]
    fn eopnotsupp_is_unsupported() {
        let e = io::Error::from_raw_os_error(95); // EOPNOTSUPP
        assert!(matches!(map_io_error(&e, "wheel_sensitivity"), Error::Unsupported));
    }

    #[test]
    fn erange_and_einval_map() {
        assert!(matches!(map_io_error(&io::Error::from_raw_os_error(34), "x"),
                         Error::OutOfRange)); // ERANGE
        assert!(matches!(map_io_error(&io::Error::from_raw_os_error(22), "x"),
                         Error::Invalid)); // EINVAL
    }
}
