use crate::error::{map_io_error, Error, Mode};
use crate::kind::Kind;
use crate::registry::REGISTRY;
use crate::setting::{Access, ModeReq, SettingSpec};
use crate::sysfs::{RealSysfs, SysfsIo};
use crate::value::Value;

pub struct DeviceInfo {
    pub serial: String,
    pub firmware: String,
    pub mode: Mode,
}

pub struct Device<S: SysfsIo> {
    io: S,
}

impl Device<RealSysfs> {
    /// Find the wheel by the sysfs attribute only this driver creates.
    pub fn discover() -> Result<Device<RealSysfs>, Error> {
        let mut entries = std::fs::read_dir("/sys/class/hidraw")
            .map_err(|_| Error::NoWheel)?;
        while let Some(Ok(e)) = entries.next() {
            let dir = e.path().join("device");
            if dir.join("wheel_range").exists() {
                return Ok(Device { io: RealSysfs::new(dir) });
            }
        }
        Err(Error::NoWheel)
    }
}

impl<S: SysfsIo> Device<S> {
    pub fn with_io(io: S) -> Device<S> {
        Device { io }
    }

    pub fn spec(attr: &str) -> Option<&'static SettingSpec> {
        REGISTRY.iter().find(|s| s.attr == attr)
    }

    pub fn available(&self, attr: &str) -> bool {
        self.io.exists(attr)
    }

    pub fn current_mode(&self) -> Result<Mode, Error> {
        match self.io.read("wheel_mode").map_err(|e| map_io_error(&e, "wheel_mode"))?.trim() {
            "onboard" => Ok(Mode::Onboard),
            _ => Ok(Mode::Desktop),
        }
    }

    pub fn info(&self) -> Result<DeviceInfo, Error> {
        let read = |a: &str| {
            self.io.read(a).map(|s| s.trim().to_string()).unwrap_or_default()
        };
        Ok(DeviceInfo {
            serial: read("wheel_serial"),
            // The driver returns "base: ...\nmotor: ..."; keep it on one line.
            firmware: read("wheel_firmware").replace('\n', " / "),
            mode: self.current_mode()?,
        })
    }

    pub fn read(&self, attr: &str) -> Result<Value, Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
        // Action attributes are write-only triggers; reading the sysfs file
        // returns EACCES. Report the trigger value instead of a permission error.
        if spec.access == Access::Action {
            return Ok(Value::Trigger);
        }
        let raw = self.io.read(attr).map_err(|e| map_io_error(&e, attr))?;
        // wheel_mode / wheel_texture_route report words; map to the enum index.
        if let Kind::Enum(variants) = spec.kind {
            let t = raw.trim();
            if let Some(i) = variants.iter().position(|v| *v == t) {
                return Ok(Value::Enum(i as u8));
            }
        }
        spec.kind.parse(&raw)
    }

    pub fn write(&self, attr: &str, v: &Value) -> Result<(), Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
        if spec.access == Access::ReadOnly {
            return Err(Error::Invalid);
        }
        spec.kind.validate(v)?;
        // Mode gating: reject up front with a WrongMode the UI can act on.
        match spec.mode_req {
            ModeReq::DesktopOnly if self.current_mode()? != Mode::Desktop => {
                return Err(Error::WrongMode { needed: Mode::Desktop });
            }
            ModeReq::OnboardOnly if self.current_mode()? != Mode::Onboard => {
                return Err(Error::WrongMode { needed: Mode::Onboard });
            }
            _ => {}
        }
        let raw = self.raw_for_write(spec, v)?;
        self.io.write(attr, &raw).map_err(|e| map_io_error(&e, attr))
    }

    /// wheel_mode/texture_route take words; write the variant string, not index.
    fn raw_for_write(&self, spec: &SettingSpec, v: &Value) -> Result<String, Error> {
        if let (Kind::Enum(variants), Value::Enum(i)) = (spec.kind, v) {
            if spec.attr == "wheel_mode" || spec.attr == "wheel_texture_route" {
                return variants
                    .get(*i as usize)
                    .map(|s| s.to_string())
                    .ok_or(Error::OutOfRange);
            }
        }
        spec.kind.format(v)
    }

    pub fn ensure_desktop_mode(&self) -> Result<(), Error> {
        if self.current_mode()? == Mode::Desktop {
            return Ok(());
        }
        self.io.write("wheel_mode", "desktop").map_err(|e| map_io_error(&e, "wheel_mode"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::FakeSysfs;
    use crate::value::Value;

    fn dev() -> Device<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "2538WDQ0M9X8");
        fs.set("wheel_sensitivity", "50");
        fs.set("wheel_texture_route", "tf");
        Device::with_io(fs)
    }

    #[test]
    fn reads_typed_value() {
        assert_eq!(dev().read("wheel_range").unwrap(), Value::Int(900));
    }

    #[test]
    fn texture_route_word_parses_to_enum() {
        // driver reports "tf"; registry models it as Enum index 1
        assert_eq!(dev().read("wheel_texture_route").unwrap(), Value::Enum(1));
    }

    #[test]
    fn action_attrs_read_as_trigger_not_permission_error() {
        // wheel_led_apply / wheel_calibrate_here are write-only (0220); reading
        // the file gives EACCES. read() must report the trigger, not the error.
        let fs = FakeSysfs::new();
        fs.set_errno("wheel_led_apply", 13); // EACCES if it tried to read
        let d = Device::with_io(fs);
        assert_eq!(d.read("wheel_led_apply").unwrap(), Value::Trigger);
        // even with the file entirely absent
        assert_eq!(d.read("wheel_calibrate_here").unwrap(), Value::Trigger);
    }

    #[test]
    fn firmware_info_is_single_line() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "X");
        fs.set("wheel_firmware", "base: U1 65.04.B0039\nmotor: SC 02.01.B0042\n");
        let info = Device::with_io(fs).info().unwrap();
        assert!(!info.firmware.contains('\n'), "firmware: {:?}", info.firmware);
        assert_eq!(info.firmware, "base: U1 65.04.B0039 / motor: SC 02.01.B0042");
    }

    #[test]
    fn writes_valid_value() {
        let d = dev();
        d.write("wheel_range", &Value::Int(540)).unwrap();
        assert_eq!(d.read("wheel_range").unwrap(), Value::Int(540));
    }

    #[test]
    fn write_out_of_range_rejected_before_io() {
        let d = dev();
        assert!(matches!(d.write("wheel_range", &Value::Int(45)), Err(Error::OutOfRange)));
    }

    #[test]
    fn desktop_only_write_in_onboard_returns_wrong_mode() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_sensitivity", "50");
        let d = Device::with_io(fs);
        assert!(matches!(d.write("wheel_sensitivity", &Value::Percent(10)),
                         Err(Error::WrongMode { needed: Mode::Desktop })));
    }

    #[test]
    fn ensure_desktop_switches_mode() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        let d = Device::with_io(fs);
        d.ensure_desktop_mode().unwrap();
        assert_eq!(d.current_mode().unwrap(), Mode::Desktop);
    }

    #[test]
    fn available_reflects_presence() {
        let d = dev();
        assert!(d.available("wheel_range"));
        assert!(!d.available("wheel_brake_force"));
    }

    #[test]
    fn info_reads_identity() {
        let i = dev().info().unwrap();
        assert_eq!(i.serial, "2538WDQ0M9X8");
        assert_eq!(i.mode, Mode::Desktop);
    }
}
