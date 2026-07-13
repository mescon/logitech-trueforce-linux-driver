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
}

impl FakeSysfs {
    pub fn new() -> Self {
        Self {
            vals: RefCell::new(HashMap::new()),
            errno: RefCell::new(HashMap::new()),
        }
    }
    pub fn set(&self, attr: &str, val: &str) {
        self.vals.borrow_mut().insert(attr.to_string(), val.to_string());
    }
    pub fn set_absent(&self, attr: &str) {
        self.vals.borrow_mut().remove(attr);
    }
    pub fn set_errno(&self, attr: &str, errno: i32) {
        self.errno.borrow_mut().insert(attr.to_string(), errno);
    }
}

impl Default for FakeSysfs {
    fn default() -> Self {
        Self::new()
    }
}

impl SysfsIo for FakeSysfs {
    fn read(&self, attr: &str) -> io::Result<String> {
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
        Ok(())
    }
    fn exists(&self, attr: &str) -> bool {
        self.vals.borrow().contains_key(attr)
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
    fn fake_injected_errno_on_write() {
        let fs = FakeSysfs::new();
        fs.set("wheel_sensitivity", "50");
        fs.set_errno("wheel_sensitivity", 1); // EPERM on write
        let err = fs.write("wheel_sensitivity", "10").unwrap_err();
        assert_eq!(err.raw_os_error(), Some(1));
    }
}
