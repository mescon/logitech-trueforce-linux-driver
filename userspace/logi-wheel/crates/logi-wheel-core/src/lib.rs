//! Settings library for the hid-logitech-dd direct-drive wheels, plus the
//! G923's classic lg4ff-style FFB engine (see [`WheelModel`], [`REGISTRY`]
//! vs [`CLASSIC_REGISTRY`]).

pub mod error;
pub use error::{Error, Mode};
pub mod sysfs;
pub mod value;
pub use value::{Color, Value};
pub mod kind;
pub use kind::Kind;
pub mod clipboard;
pub mod curve;
pub mod driver;
pub mod evtest;
pub mod fftest;
pub mod hidpp;
pub mod lightsync;
pub mod setting;
pub use setting::{Access, Category, ModeReq, SettingSpec};
pub mod registry;
pub use registry::{CLASSIC_REGISTRY, REGISTRY};
pub mod helpers;
pub mod launch;
pub mod launchers;
pub mod onboard;
pub mod profiles;
pub mod shaping;
pub mod steam;
pub mod tfsim;
pub mod device;
pub mod diagnose;
pub mod diagnostics;
pub use device::{Device, DeviceInfo, WheelModel};
pub mod games;

pub mod relay;
pub mod tfstream;
pub mod telemetry;
pub mod telemetry_helpers;

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
