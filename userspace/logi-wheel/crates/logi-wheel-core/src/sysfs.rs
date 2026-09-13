// SPDX-License-Identifier: GPL-2.0-only
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

pub trait SysfsIo {
    fn read(&self, attr: &str) -> io::Result<String>;
    fn write(&self, attr: &str, val: &str) -> io::Result<()>;
    fn exists(&self, attr: &str) -> bool;
}

pub struct RealSysfs {
    dir: PathBuf,
}

impl RealSysfs {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl SysfsIo for RealSysfs {
    fn read(&self, attr: &str) -> io::Result<String> {
        std::fs::read_to_string(self.dir.join(attr))
    }
    fn write(&self, attr: &str, val: &str) -> io::Result<()> {
        std::fs::write(self.dir.join(attr), val.as_bytes())
    }
    fn exists(&self, attr: &str) -> bool {
        self.dir.join(attr).exists()
    }
}

/// In-memory sysfs for tests. Not thread-safe (single-threaded test use).
pub struct FakeSysfs {
    vals: RefCell<HashMap<String, String>>,
    errno: RefCell<HashMap<String, i32>>,
    read_errno: RefCell<HashMap<String, i32>>,
    log: RefCell<Vec<(String, String)>>,
}

impl FakeSysfs {
    pub fn new() -> Self {
        Self {
            vals: RefCell::new(HashMap::new()),
            errno: RefCell::new(HashMap::new()),
            read_errno: RefCell::new(HashMap::new()),
            log: RefCell::new(Vec::new()),
        }
    }
    pub fn set(&self, attr: &str, val: &str) {
        self.vals.borrow_mut().insert(attr.to_string(), val.to_string());
    }
    pub fn set_absent(&self, attr: &str) {
        self.vals.borrow_mut().remove(attr);
    }
    /// Make a `write` of `attr` fail with this errno (e.g. EINVAL/EPERM on a
    /// rejected value). Never affects `read` or `exists`.
    pub fn set_errno(&self, attr: &str, errno: i32) {
        self.errno.borrow_mut().insert(attr.to_string(), errno);
    }
    /// Make a `read` of `attr` fail with this errno, while `exists` still
    /// reports the attr present: models a sysfs file that is really there
    /// (`ls` shows it) but whose read the wheel/firmware rejects, e.g. an
    /// RS50's pedal-curve/sensitivity/deadzone attrs, which exist as files
    /// on that sub-device but answer EOPNOTSUPP because the pedal MCU has
    /// no such feature. Distinct from `set_errno` (write-only) so the two
    /// never interfere with each other's tests.
    pub fn set_read_errno(&self, attr: &str, errno: i32) {
        self.read_errno.borrow_mut().insert(attr.to_string(), errno);
    }
    /// Every successful `write` so far, oldest first, as (attr, value)
    /// pairs. `set`/failed writes are not recorded, so a test can assert
    /// the exact write sequence a code path produced.
    pub fn writes(&self) -> Vec<(String, String)> {
        self.log.borrow().clone()
    }
}

impl Default for FakeSysfs {
    fn default() -> Self {
        Self::new()
    }
}

/// Forward through `Rc`, so a test can keep a second handle to the
/// `FakeSysfs` a `Device` owns and mutate attributes "behind the device's
/// back" (what an external actor, e.g. the wheel's physical profile
/// button, looks like to a frontend's drift detection).
impl<T: SysfsIo> SysfsIo for std::rc::Rc<T> {
    fn read(&self, attr: &str) -> io::Result<String> {
        (**self).read(attr)
    }
    fn write(&self, attr: &str, val: &str) -> io::Result<()> {
        (**self).write(attr, val)
    }
    fn exists(&self, attr: &str) -> bool {
        (**self).exists(attr)
    }
}

impl SysfsIo for FakeSysfs {
    fn read(&self, attr: &str) -> io::Result<String> {
        if let Some(e) = self.read_errno.borrow().get(attr) {
            return Err(io::Error::from_raw_os_error(*e));
        }
        self.vals
            .borrow()
            .get(attr)
            .cloned()
            .map(|s| format!("{s}\n"))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn write(&self, attr: &str, val: &str) -> io::Result<()> {
        if let Some(e) = self.errno.borrow().get(attr) {
            return Err(io::Error::from_raw_os_error(*e));
        }
        self.vals.borrow_mut().insert(attr.to_string(), val.trim().to_string());
        self.log.borrow_mut().push((attr.to_string(), val.trim().to_string()));
        Ok(())
    }
    fn exists(&self, attr: &str) -> bool {
        self.vals.borrow().contains_key(attr) || self.read_errno.borrow().contains_key(attr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_roundtrip_and_absent() {
        let fs = FakeSysfs::new();
        fs.set("wheel_range", "900");
        assert_eq!(fs.read("wheel_range").unwrap().trim(), "900");
        assert!(fs.exists("wheel_range"));
        assert!(!fs.exists("wheel_missing"));
        fs.write("wheel_range", "540").unwrap();
        assert_eq!(fs.read("wheel_range").unwrap().trim(), "540");
    }

    #[test]
    fn fake_read_errno_makes_the_attr_exist_but_fail_to_read() {
        // Models an RS50's pedal-curve/sensitivity attrs: the sysfs file is
        // there (`exists` is true, same as `ls` would show), but every read
        // fails - EOPNOTSUPP here, standing in for the pedal MCU having no
        // such feature.
        let fs = FakeSysfs::new();
        fs.set_read_errno("wheel_throttle_sensitivity", 95);
        assert!(fs.exists("wheel_throttle_sensitivity"));
        let err = fs.read("wheel_throttle_sensitivity").unwrap_err();
        assert_eq!(err.raw_os_error(), Some(95));
        // A write-errno on a different attr never leaks into its own read:
        // set_errno only ever affects write.
        fs.set("wheel_range", "900");
        fs.set_errno("wheel_range", 1);
        assert_eq!(fs.read("wheel_range").unwrap().trim(), "900");
    }

    #[test]
    fn fake_injected_errno_on_write() {
        let fs = FakeSysfs::new();
        fs.set("wheel_sensitivity", "50");
        fs.set_errno("wheel_sensitivity", 1); // EPERM on write
        let err = fs.write("wheel_sensitivity", "10").unwrap_err();
        assert_eq!(err.raw_os_error(), Some(1));
    }
}
