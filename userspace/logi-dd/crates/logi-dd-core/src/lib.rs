//! Settings library for the hid-logitech-dd direct-drive wheels.

pub mod error;
pub use error::{Error, Mode};
pub mod sysfs;
pub mod value;
pub use value::{Color, Value};
pub mod kind;
pub use kind::Kind;
pub mod setting;
pub use setting::{Access, Category, ModeReq, SettingSpec};
pub mod registry;
pub use registry::REGISTRY;

#[cfg(test)]
mod smoke {
    #[test]
    fn builds() {
        assert_eq!(2 + 2, 4);
    }
}
