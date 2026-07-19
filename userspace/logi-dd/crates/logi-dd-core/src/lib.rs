//! Settings library for the hid-logitech-dd direct-drive wheels.

pub mod error;
pub use error::{Error, Mode};
pub mod sysfs;
pub mod value;
pub use value::{Color, Value};
pub mod kind;
pub use kind::Kind;
pub mod curve;
pub mod evtest;
pub mod lightsync;
pub mod setting;
pub use setting::{Access, Category, ModeReq, SettingSpec};
pub mod registry;
pub use registry::REGISTRY;
pub mod helpers;
pub mod profiles;
pub mod shaping;
pub mod steam;
pub mod tfsim;
pub mod device;
pub use device::{Device, DeviceInfo};

/// Project home, shown in the Info view of both front-ends so users know
/// where to find the documentation and source.
pub const PROJECT_URL: &str = "https://github.com/mescon/logitech-trueforce-linux-driver";

#[cfg(test)]
mod smoke {
    #[test]
    fn builds() {
        assert_eq!(2 + 2, 4);
    }
}
