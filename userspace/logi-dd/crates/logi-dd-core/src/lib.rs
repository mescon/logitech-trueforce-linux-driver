//! Settings library for the hid-logitech-dd direct-drive wheels.

pub mod error;
pub use error::{Error, Mode};
pub mod sysfs;
pub mod value;
pub use value::{Color, Value};

#[cfg(test)]
mod smoke {
    #[test]
    fn builds() {
        assert_eq!(2 + 2, 4);
    }
}
