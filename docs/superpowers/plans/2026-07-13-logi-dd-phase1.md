# logi-dd Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust workspace `logi-dd` with a settings-library crate (`logi-dd-core`) and a terminal UI (`logi-dd-tui`) that view and change every setting the `hid-logitech-dd` driver exposes for the wheel.

**Architecture:** `logi-dd-core` models each driver `wheel_*` sysfs attribute as a data-driven `SettingSpec` (type, range, category, mode requirement) and does all sysfs I/O through a `SysfsIo` trait so it is testable with a fake in-memory sysfs. `logi-dd-tui` is a thin ratatui frontend rendering the registry generically. No daemon; the core is written so a later daemon can wrap it unchanged.

**Tech Stack:** Rust (edition 2021), ratatui + crossterm for the TUI. No non-Rust runtime dependencies.

## Global Constraints

- Rust edition **2021**; MSRV **1.74** (ratatui floor). One line in each crate's `Cargo.toml`.
- Dependencies limited to: `ratatui`, `crossterm` (tui crate only). Core crate has **zero** external deps.
- Workspace lives at `userspace/logi-dd/`. Binary name: `logi-dd-tui`.
- Runs as the normal user; never requires root (sysfs is group-writable via the udev rule).
- Exact sysfs encodings are authoritative in `docs/SYSFS_API.md`; the values in this plan are copied from it.
- No AI/Claude mentions in code, comments, or commits. No em-dashes in any text.
- Commit trailers: none.

---

## File Structure

```
userspace/logi-dd/
  Cargo.toml                       # [workspace]
  crates/
    logi-dd-core/
      Cargo.toml
      src/
        lib.rs                     # module wiring + re-exports
        error.rs                   # Error, errno -> Error mapping
        sysfs.rs                   # SysfsIo trait, RealSysfs, FakeSysfs
        value.rs                   # Value, Color
        kind.rs                    # Kind: parse/format/validate
        setting.rs                 # SettingId, Category, Access, ModeReq, SettingSpec
        registry.rs                # REGISTRY: &[SettingSpec]
        device.rs                  # Device: discover/info/read/write/mode
    logi-dd-tui/
      Cargo.toml
      src/
        main.rs                    # terminal setup + run loop
        app.rs                     # App state (rows, focus, status, edit)
        edit.rs                    # kind-aware edit state machine
        ui.rs                      # ratatui rendering
```

Responsibilities: `kind.rs` owns all value encoding/validation (the correctness core); `registry.rs` owns the list of attributes; `device.rs` owns discovery + I/O + mode-gating; the TUI files split state (`app`), editing (`edit`), and drawing (`ui`).

---

### Task 1: Workspace + core crate skeleton

**Files:**
- Create: `userspace/logi-dd/Cargo.toml`
- Create: `userspace/logi-dd/crates/logi-dd-core/Cargo.toml`
- Create: `userspace/logi-dd/crates/logi-dd-core/src/lib.rs`

**Interfaces:**
- Produces: a buildable, testable `logi-dd-core` crate.

- [ ] **Step 1: Create the workspace manifest**

`userspace/logi-dd/Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/logi-dd-core", "crates/logi-dd-tui"]

[workspace.package]
edition = "2021"
rust-version = "1.74"
version = "0.1.0"
license = "GPL-2.0"
```

- [ ] **Step 2: Create the core crate manifest**

`userspace/logi-dd/crates/logi-dd-core/Cargo.toml`:
```toml
[package]
name = "logi-dd-core"
edition.workspace = true
rust-version.workspace = true
version.workspace = true
license.workspace = true
```

- [ ] **Step 3: Create lib.rs with a placeholder test**

`userspace/logi-dd/crates/logi-dd-core/src/lib.rs`:
```rust
//! Settings library for the hid-logitech-dd direct-drive wheels.

#[cfg(test)]
mod smoke {
    #[test]
    fn builds() {
        assert_eq!(2 + 2, 4);
    }
}
```

Note: `logi-dd-tui` is a workspace member but does not exist yet; create a stub so the workspace resolves.

- [ ] **Step 4: Create a tui stub so the workspace builds**

`userspace/logi-dd/crates/logi-dd-tui/Cargo.toml`:
```toml
[package]
name = "logi-dd-tui"
edition.workspace = true
rust-version.workspace = true
version.workspace = true
license.workspace = true

[[bin]]
name = "logi-dd-tui"
path = "src/main.rs"

[dependencies]
logi-dd-core = { path = "../logi-dd-core" }
```

`userspace/logi-dd/crates/logi-dd-tui/src/main.rs`:
```rust
fn main() {}
```

- [ ] **Step 5: Verify it builds and tests**

Run: `cd userspace/logi-dd && cargo test`
Expected: PASS (1 test `smoke::builds`).

- [ ] **Step 6: Commit**

```bash
git add userspace/logi-dd
git commit -m "logi-dd: scaffold Rust workspace (core + tui crates)"
```

---

### Task 2: Error type and errno mapping

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/error.rs`
- Modify: `userspace/logi-dd/crates/logi-dd-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum Mode { Desktop, Onboard }`
  - `enum Error { NoWheel, Io(String), WrongMode { needed: Mode }, Unsupported, OutOfRange, Invalid, Parse(String) }`
  - `fn map_io_error(e: &std::io::Error, attr: &str) -> Error`

- [ ] **Step 1: Write the failing test**

Append to `error.rs` (create the file with this test first):
```rust
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
```

- [ ] **Step 2: Write the implementation above the tests**

Prepend to `error.rs`:
```rust
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
```

- [ ] **Step 3: Wire the module**

In `lib.rs` add near the top:
```rust
pub mod error;
pub use error::{Error, Mode};
```

- [ ] **Step 4: Run tests**

Run: `cd userspace/logi-dd && cargo test -p logi-dd-core error`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: Error type and sysfs errno mapping"
```

---

### Task 3: SysfsIo trait with real and fake backends

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/sysfs.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `trait SysfsIo { fn read(&self, attr: &str) -> io::Result<String>; fn write(&self, attr: &str, val: &str) -> io::Result<()>; fn exists(&self, attr: &str) -> bool; }`
  - `struct RealSysfs { dir: PathBuf }` with `RealSysfs::new(dir: PathBuf)`
  - `struct FakeSysfs` with `FakeSysfs::new()`, `.set(attr, val)`, `.set_absent(attr)`, `.set_errno(attr, errno)` for tests

- [ ] **Step 1: Write the failing test**

`sysfs.rs`:
```rust
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
```

- [ ] **Step 2: Implement the trait and backends**

Prepend to `sysfs.rs`:
```rust
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
```

- [ ] **Step 3: Wire the module**

In `lib.rs`:
```rust
pub mod sysfs;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p logi-dd-core sysfs`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: SysfsIo trait with real and fake backends"
```

---

### Task 4: Value and Color types

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/value.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `struct Color { r: u8, g: u8, b: u8 }` with `Color::to_hex() -> String` and `Color::from_hex(&str) -> Result<Color, Error>`
  - `enum Value { Percent(u8), Int(i32), Enum(u8), Bool(bool), Text(String), Rgb(Vec<Color>), Curve(Vec<(u16, u16)>), Trigger }`

- [ ] **Step 1: Write the failing test**

`value.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_hex_roundtrip() {
        let c = Color::from_hex("ff8000").unwrap();
        assert_eq!(c, Color { r: 0xff, g: 0x80, b: 0x00 });
        assert_eq!(c.to_hex(), "ff8000");
    }

    #[test]
    fn color_bad_hex_errors() {
        assert!(Color::from_hex("zz0000").is_err());
        assert!(Color::from_hex("fff").is_err());
    }
}
```

- [ ] **Step 2: Implement**

Prepend to `value.rs`:
```rust
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn to_hex(&self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
    pub fn from_hex(s: &str) -> Result<Color, Error> {
        let s = s.trim();
        if s.len() != 6 {
            return Err(Error::Invalid);
        }
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| Error::Invalid);
        Ok(Color { r: byte(0)?, g: byte(2)?, b: byte(4)? })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Percent(u8),
    Int(i32),
    Enum(u8),
    Bool(bool),
    Text(String),
    Rgb(Vec<Color>),
    Curve(Vec<(u16, u16)>),
    Trigger,
}
```

- [ ] **Step 3: Wire the module**

In `lib.rs`:
```rust
pub mod value;
pub use value::{Color, Value};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p logi-dd-core value`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: Value and Color types"
```

---

### Task 5: Kind - parse, format, validate

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/kind.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `enum Kind { Percent, IntRange { min: i32, max: i32, step: i32, unit: &'static str }, Enum(&'static [&'static str]), Toggle { off: &'static str, on: &'static str }, TextField { max_len: usize }, RgbStrip { leds: usize }, Curve, Action }`
  - `impl Kind { fn parse(&self, raw: &str) -> Result<Value, Error>; fn format(&self, v: &Value) -> Result<String, Error>; fn validate(&self, v: &Value) -> Result<(), Error>; }`

Encodings copied from `docs/SYSFS_API.md`: percent = decimal `0`-`100`; IntRange = decimal within [min,max]; Enum/Toggle = decimal index; texture_route uses `tf`/`kf` so it is modeled as `Enum(&["kf","tf"])` where index 0=`kf`,1=`tf` (see registry); RgbStrip = 10 space-separated `RRGGBB`; Curve = `reset` or space-separated `in:out` pairs (0-65535).

- [ ] **Step 1: Write the failing tests (percent, int, enum, toggle)**

`kind.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Color;

    #[test]
    fn percent_roundtrip_and_bounds() {
        let k = Kind::Percent;
        assert_eq!(k.parse("50\n").unwrap(), Value::Percent(50));
        assert_eq!(k.format(&Value::Percent(50)).unwrap(), "50");
        assert!(k.validate(&Value::Percent(100)).is_ok());
        assert!(matches!(k.parse("250"), Err(Error::OutOfRange)));
    }

    #[test]
    fn intrange_range() {
        let k = Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" };
        assert_eq!(k.parse("900").unwrap(), Value::Int(900));
        assert_eq!(k.format(&Value::Int(900)).unwrap(), "900");
        assert!(matches!(k.parse("45"), Err(Error::OutOfRange)));
        assert!(matches!(k.validate(&Value::Int(2701)), Err(Error::OutOfRange)));
    }

    #[test]
    fn enum_index() {
        let k = Kind::Enum(&["kf", "tf"]);
        assert_eq!(k.parse("1").unwrap(), Value::Enum(1));
        assert_eq!(k.format(&Value::Enum(1)).unwrap(), "1");
        assert!(matches!(k.parse("2"), Err(Error::OutOfRange)));
    }

    #[test]
    fn toggle() {
        let k = Kind::Toggle { off: "off", on: "on" };
        assert_eq!(k.parse("1").unwrap(), Value::Bool(true));
        assert_eq!(k.format(&Value::Bool(false)).unwrap(), "0");
    }

    #[test]
    fn rgb_strip_ten_colors() {
        let k = Kind::RgbStrip { leds: 10 };
        let raw = "ff0000 00ff00 0000ff ffffff 000000 111111 222222 333333 444444 555555";
        let v = k.parse(raw).unwrap();
        if let Value::Rgb(cs) = &v {
            assert_eq!(cs.len(), 10);
            assert_eq!(cs[0], Color { r: 255, g: 0, b: 0 });
        } else {
            panic!("not rgb");
        }
        assert_eq!(k.format(&v).unwrap(), raw);
        assert!(matches!(k.parse("ff0000"), Err(Error::Invalid))); // wrong count
    }

    #[test]
    fn curve_reset_and_pairs() {
        let k = Kind::Curve;
        assert_eq!(k.parse("reset").unwrap(), Value::Curve(vec![]));
        assert_eq!(k.format(&Value::Curve(vec![])).unwrap(), "reset");
        let v = k.parse("0:0 32768:16384 65535:65535").unwrap();
        assert_eq!(v, Value::Curve(vec![(0, 0), (32768, 16384), (65535, 65535)]));
        assert_eq!(k.format(&v).unwrap(), "0:0 32768:16384 65535:65535");
    }
}
```

- [ ] **Step 2: Implement Kind**

Prepend to `kind.rs`:
```rust
use crate::error::Error;
use crate::value::{Color, Value};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Percent,
    IntRange { min: i32, max: i32, step: i32, unit: &'static str },
    Enum(&'static [&'static str]),
    Toggle { off: &'static str, on: &'static str },
    TextField { max_len: usize },
    RgbStrip { leds: usize },
    Curve,
    Action,
}

impl Kind {
    pub fn parse(&self, raw: &str) -> Result<Value, Error> {
        let raw = raw.trim();
        match self {
            Kind::Percent => {
                let n: i32 = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if !(0..=100).contains(&n) {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Percent(n as u8))
            }
            Kind::IntRange { min, max, .. } => {
                let n: i32 = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if n < *min || n > *max {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Int(n))
            }
            Kind::Enum(variants) => {
                let n: usize = raw.parse().map_err(|_| Error::Parse(raw.into()))?;
                if n >= variants.len() {
                    return Err(Error::OutOfRange);
                }
                Ok(Value::Enum(n as u8))
            }
            Kind::Toggle { .. } => match raw {
                "0" => Ok(Value::Bool(false)),
                "1" => Ok(Value::Bool(true)),
                _ => Err(Error::Parse(raw.into())),
            },
            Kind::TextField { .. } => Ok(Value::Text(raw.to_string())),
            Kind::RgbStrip { leds } => {
                let cs: Result<Vec<Color>, Error> =
                    raw.split_whitespace().map(Color::from_hex).collect();
                let cs = cs?;
                if cs.len() != *leds {
                    return Err(Error::Invalid);
                }
                Ok(Value::Rgb(cs))
            }
            Kind::Curve => {
                if raw == "reset" || raw.is_empty() || raw.contains("built-in") {
                    return Ok(Value::Curve(vec![]));
                }
                let mut pts = Vec::new();
                for tok in raw.split_whitespace() {
                    let (a, b) = tok.split_once(':').ok_or(Error::Parse(tok.into()))?;
                    let inp: u16 = a.parse().map_err(|_| Error::Parse(tok.into()))?;
                    let out: u16 = b.parse().map_err(|_| Error::Parse(tok.into()))?;
                    pts.push((inp, out));
                }
                Ok(Value::Curve(pts))
            }
            Kind::Action => Ok(Value::Trigger),
        }
    }

    pub fn format(&self, v: &Value) -> Result<String, Error> {
        Ok(match (self, v) {
            (Kind::Percent, Value::Percent(n)) => n.to_string(),
            (Kind::IntRange { .. }, Value::Int(n)) => n.to_string(),
            (Kind::Enum(_), Value::Enum(n)) => n.to_string(),
            (Kind::Toggle { .. }, Value::Bool(b)) => (if *b { "1" } else { "0" }).into(),
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => {
                cs.iter().map(Color::to_hex).collect::<Vec<_>>().join(" ")
            }
            (Kind::Curve, Value::Curve(pts)) => {
                if pts.is_empty() {
                    "reset".into()
                } else {
                    pts.iter().map(|(a, b)| format!("{a}:{b}")).collect::<Vec<_>>().join(" ")
                }
            }
            (Kind::Action, Value::Trigger) => "1".into(),
            _ => return Err(Error::Invalid),
        })
    }

    pub fn validate(&self, v: &Value) -> Result<(), Error> {
        // parse(format(v)) proves the value satisfies this kind's constraints.
        let s = self.format(v)?;
        match self {
            Kind::Action => Ok(()),
            _ => self.parse(&s).map(|_| ()),
        }
    }

    /// Human-readable rendering of a value for display.
    pub fn display(&self, v: &Value) -> String {
        match (self, v) {
            (Kind::Percent, Value::Percent(n)) => format!("{n}%"),
            (Kind::IntRange { unit, .. }, Value::Int(n)) => format!("{n} {unit}"),
            (Kind::Enum(variants), Value::Enum(n)) => variants
                .get(*n as usize)
                .map(|s| s.to_string())
                .unwrap_or_else(|| n.to_string()),
            (Kind::Toggle { off, on }, Value::Bool(b)) => {
                (if *b { *on } else { *off }).to_string()
            }
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            (Kind::RgbStrip { .. }, Value::Rgb(cs)) => format!("{} LEDs", cs.len()),
            (Kind::Curve, Value::Curve(p)) if p.is_empty() => "built-in".into(),
            (Kind::Curve, Value::Curve(p)) => format!("{} points", p.len()),
            (Kind::Action, _) => "[trigger]".into(),
            _ => "?".into(),
        }
    }
}
```

- [ ] **Step 3: Wire the module**

In `lib.rs`:
```rust
pub mod kind;
pub use kind::Kind;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p logi-dd-core kind`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: Kind parse/format/validate for every value type"
```

---

### Task 6: Setting spec, categories, and the registry

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/setting.rs`
- Create: `userspace/logi-dd/crates/logi-dd-core/src/registry.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:
  - `enum Category { Ffb, Rotation, Sensitivity, TrueForce, Pedals, Leds, Profiles, Calibration, Info }` with `Category::ALL: &[Category]` and `Category::label(&self) -> &'static str`
  - `enum Access { ReadWrite, ReadOnly, Action }`
  - `enum ModeReq { Any, DesktopOnly, OnboardOnly }`
  - `struct SettingSpec { pub attr: &'static str, pub label: &'static str, pub help: &'static str, pub category: Category, pub kind: Kind, pub access: Access, pub mode_req: ModeReq }`
  - `const REGISTRY: &[SettingSpec]`

- [ ] **Step 1: Write the failing test**

`registry.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting::{Access, Category};

    #[test]
    fn registry_has_no_duplicate_attrs() {
        let mut seen = std::collections::HashSet::new();
        for s in REGISTRY {
            assert!(seen.insert(s.attr), "duplicate attr {}", s.attr);
        }
    }

    #[test]
    fn every_kind_roundtrips_a_sample() {
        // Each spec's kind must be able to format+parse a known-good sample
        // drawn from its own current default, proving the registry is coherent.
        for s in REGISTRY {
            if matches!(s.access, Access::Action) {
                continue;
            }
            // pick a trivially valid raw for this kind and round-trip it
            let raw = super::sample_raw(s);
            let v = s.kind.parse(&raw).unwrap_or_else(|e| panic!("{}: {e}", s.attr));
            let back = s.kind.format(&v).unwrap();
            assert!(!back.is_empty() || matches!(s.kind, crate::Kind::Curve),
                    "{}: empty format", s.attr);
        }
    }

    #[test]
    fn known_attrs_present() {
        for a in ["wheel_strength", "wheel_range", "wheel_sensitivity",
                  "wheel_mode", "wheel_led_colors", "wheel_serial"] {
            assert!(REGISTRY.iter().any(|s| s.attr == a), "missing {a}");
        }
    }

    #[test]
    fn brake_force_is_onboard_only() {
        let s = REGISTRY.iter().find(|s| s.attr == "wheel_brake_force").unwrap();
        assert!(matches!(s.mode_req, super::super::setting::ModeReq::OnboardOnly));
        let _ = Category::Pedals;
    }
}
```

- [ ] **Step 2: Implement setting.rs**

`setting.rs`:
```rust
use crate::kind::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Ffb,
    Rotation,
    Sensitivity,
    TrueForce,
    Pedals,
    Leds,
    Profiles,
    Calibration,
    Info,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::Ffb,
        Category::Rotation,
        Category::Sensitivity,
        Category::TrueForce,
        Category::Pedals,
        Category::Leds,
        Category::Profiles,
        Category::Calibration,
        Category::Info,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            Category::Ffb => "Force feedback",
            Category::Rotation => "Rotation",
            Category::Sensitivity => "Sensitivity",
            Category::TrueForce => "TrueForce",
            Category::Pedals => "Pedals",
            Category::Leds => "LEDs",
            Category::Profiles => "Profiles / mode",
            Category::Calibration => "Calibration",
            Category::Info => "Info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadWrite,
    ReadOnly,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeReq {
    Any,
    DesktopOnly,
    OnboardOnly,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub attr: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub access: Access,
    pub mode_req: ModeReq,
}
```

- [ ] **Step 3: Implement registry.rs**

Prepend to `registry.rs` (values/ranges copied from `docs/SYSFS_API.md`):
```rust
use crate::kind::Kind;
use crate::setting::{Access, Category, ModeReq, SettingSpec};

use Access::*;
use Category::*;
use ModeReq::*;

const PCT: Kind = Kind::Percent;

pub const REGISTRY: &[SettingSpec] = &[
    // --- Force feedback ---
    SettingSpec { attr: "wheel_strength", label: "FFB strength", help: "Overall force output (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_damping", label: "Damping", help: "Firmware turn resistance (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter", label: "FFB filter", help: "Smoothing level (1=min .. 15=max).", category: Ffb, kind: Kind::IntRange { min: 1, max: 15, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_filter_auto", label: "Auto FFB filter", help: "Let the wheel adjust the filter automatically.", category: Ffb, kind: Kind::Toggle { off: "manual", on: "auto" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_spring_damping", label: "Spring damping", help: "Anti-oscillation damping on the emulated spring (0-100%).", category: Ffb, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_ffb_constant_sign", label: "Invert constant force", help: "Flip the sign of constant forces (Wine/native fix).", category: Ffb, kind: Kind::Toggle { off: "normal", on: "inverted" }, access: ReadWrite, mode_req: Any },
    // --- Rotation ---
    SettingSpec { attr: "wheel_range", label: "Rotation range", help: "Steering rotation (90-2700 deg).", category: Rotation, kind: Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_range_restore", label: "Auto range restore", help: "Auto-recover from a launch-time 90-degree reset.", category: Rotation, kind: Kind::Toggle { off: "off", on: "on" }, access: ReadWrite, mode_req: Any },
    // --- Sensitivity ---
    SettingSpec { attr: "wheel_sensitivity", label: "Sensitivity", help: "Steering response (0-100%, 50=built-in). Desktop mode only.", category: Sensitivity, kind: PCT, access: ReadWrite, mode_req: DesktopOnly },
    SettingSpec { attr: "wheel_response_curve", label: "Response curve", help: "Full steering response curve. 'reset' for built-in.", category: Sensitivity, kind: Kind::Curve, access: ReadWrite, mode_req: DesktopOnly },
    // --- TrueForce ---
    SettingSpec { attr: "wheel_trueforce", label: "TrueForce intensity", help: "Audio-haptic texture intensity (0-100%).", category: TrueForce, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_texture_route", label: "Texture routing", help: "Route rumble/texture to TrueForce (tf) or steering (kf).", category: TrueForce, kind: Kind::Enum(&["kf", "tf"]), access: ReadWrite, mode_req: Any },
    // --- Pedals ---
    SettingSpec { attr: "wheel_brake_force", label: "Brake force", help: "Load-cell brake threshold (0-100%). Onboard mode only.", category: Pedals, kind: PCT, access: ReadWrite, mode_req: OnboardOnly },
    SettingSpec { attr: "wheel_combined_pedals", label: "Combined pedals", help: "Throttle and brake on one axis.", category: Pedals, kind: Kind::Toggle { off: "separate", on: "combined" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_throttle_curve", label: "Throttle curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_brake_curve", label: "Brake curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_clutch_curve", label: "Clutch curve", help: "0=linear, 1=low-sensitivity, 2=high-sensitivity.", category: Pedals, kind: Kind::Enum(&["linear", "low", "high"]), access: ReadWrite, mode_req: Any },
    // --- LEDs (RS50 LIGHTSYNC) ---
    SettingSpec { attr: "wheel_led_brightness", label: "LED brightness", help: "Global LED brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_effect", label: "LED effect", help: "Animation mode (1-9).", category: Leds, kind: Kind::IntRange { min: 1, max: 9, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_direction", label: "LED direction", help: "Animation direction.", category: Leds, kind: Kind::Enum(&["L to R", "R to L", "inside-out", "outside-in"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_colors", label: "LED colors", help: "10 strip colors, LED1 leftmost.", category: Leds, kind: Kind::RgbStrip { leds: 10 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot", label: "LED slot", help: "Active custom slot (0-4).", category: Leds, kind: Kind::IntRange { min: 0, max: 4, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_name", label: "LED slot name", help: "Name of the selected slot (max 8 chars).", category: Leds, kind: Kind::TextField { max_len: 8 }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_slot_brightness", label: "LED slot brightness", help: "Per-slot brightness (0-100%).", category: Leds, kind: PCT, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_led_apply", label: "Apply LEDs", help: "Commit the current slot config to the wheel.", category: Leds, kind: Kind::Action, access: Action, mode_req: Any },
    // --- LEDs (real G Pro rev strip) ---
    SettingSpec { attr: "wheel_rev_level", label: "Rev lights", help: "Number of rev LEDs lit (0-10).", category: Leds, kind: Kind::IntRange { min: 0, max: 10, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    // --- Profiles / mode ---
    SettingSpec { attr: "wheel_mode", label: "Mode", help: "desktop (host-controlled) or onboard (wheel-stored).", category: Profiles, kind: Kind::Enum(&["desktop", "onboard"]), access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile", label: "Profile", help: "Active profile (0=desktop, 1-5 onboard).", category: Profiles, kind: Kind::IntRange { min: 0, max: 5, step: 1, unit: "" }, access: ReadWrite, mode_req: Any },
    SettingSpec { attr: "wheel_profile_names", label: "Profile names", help: "The 5 onboard slot names.", category: Profiles, kind: Kind::TextField { max_len: 64 }, access: ReadOnly, mode_req: Any },
    // --- Calibration ---
    SettingSpec { attr: "wheel_calibrate_here", label: "Calibrate centre here", help: "Adopt the current physical position as centre.", category: Calibration, kind: Kind::Action, access: Action, mode_req: Any },
    // --- Info ---
    SettingSpec { attr: "wheel_serial", label: "Serial", help: "Device serial number.", category: Info, kind: Kind::TextField { max_len: 32 }, access: ReadOnly, mode_req: Any },
    SettingSpec { attr: "wheel_firmware", label: "Firmware", help: "Base and motor firmware versions.", category: Info, kind: Kind::TextField { max_len: 128 }, access: ReadOnly, mode_req: Any },
];

/// A trivially-valid raw string for each kind, used by the registry coherence
/// test to prove every spec can round-trip.
#[cfg(test)]
pub(crate) fn sample_raw(s: &SettingSpec) -> String {
    match s.kind {
        Kind::Percent => "50".into(),
        Kind::IntRange { min, .. } => min.to_string(),
        Kind::Enum(_) => "0".into(),
        Kind::Toggle { .. } => "0".into(),
        Kind::TextField { .. } => "RACE".into(),
        Kind::RgbStrip { leds } => vec!["000000"; leds].join(" "),
        Kind::Curve => "reset".into(),
        Kind::Action => "1".into(),
    }
}
```

Note: `wheel_mode` is modeled as an `Enum` for display, but the driver accepts the words `desktop`/`onboard`. Task 7's `Device::write` special-cases `wheel_mode` to write the word, not the index (documented there).

- [ ] **Step 4: Wire the modules**

In `lib.rs`:
```rust
pub mod setting;
pub mod registry;
pub use setting::{Access, Category, ModeReq, SettingSpec};
pub use registry::REGISTRY;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p logi-dd-core registry`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: SettingSpec model and the wheel_* registry"
```

---

### Task 7: Device - discovery, read, write, mode-gating

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-core/src/device.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `SysfsIo`, `REGISTRY`, `Kind`, `Value`, `Error`, `Mode`, `SettingSpec`, `Access`, `ModeReq`.
- Produces:
  - `struct DeviceInfo { pub serial: String, pub firmware: String, pub mode: Mode }`
  - `struct Device<S: SysfsIo> { io: S }` with:
    - `Device::<RealSysfs>::discover() -> Result<Device<RealSysfs>, Error>`
    - `Device::with_io(io: S) -> Device<S>`
    - `fn info(&self) -> Result<DeviceInfo, Error>`
    - `fn spec(attr: &str) -> Option<&'static SettingSpec>`
    - `fn available(&self, attr: &str) -> bool`
    - `fn read(&self, attr: &str) -> Result<Value, Error>`
    - `fn current_mode(&self) -> Result<Mode, Error>`
    - `fn write(&self, attr: &str, v: &Value) -> Result<(), Error>`
    - `fn ensure_desktop_mode(&self) -> Result<(), Error>`

- [ ] **Step 1: Write the failing tests**

`device.rs`:
```rust
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
```

- [ ] **Step 2: Implement Device**

Prepend to `device.rs`:
```rust
use crate::error::{map_io_error, Error, Mode};
use crate::kind::Kind;
use crate::registry::REGISTRY;
use crate::setting::{Access, ModeReq, SettingSpec};
use crate::sysfs::{RealSysfs, SysfsIo};
use crate::value::Value;
use std::path::PathBuf;

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
            firmware: read("wheel_firmware"),
            mode: self.current_mode()?,
        })
    }

    pub fn read(&self, attr: &str) -> Result<Value, Error> {
        let spec = Self::spec(attr).ok_or(Error::Invalid)?;
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
```

- [ ] **Step 3: Wire the module**

In `lib.rs`:
```rust
pub mod device;
pub use device::{Device, DeviceInfo};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p logi-dd-core device`
Expected: PASS (8 tests).

- [ ] **Step 5: Full core test + clippy**

Run: `cargo test -p logi-dd-core && cargo clippy -p logi-dd-core -- -D warnings`
Expected: all pass, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-core
git commit -m "logi-dd-core: Device discovery, typed read/write, mode-gating"
```

---

### Task 8: TUI app state

**Files:**
- Modify: `userspace/logi-dd/crates/logi-dd-tui/Cargo.toml`
- Create: `userspace/logi-dd/crates/logi-dd-tui/src/app.rs`
- Modify: `userspace/logi-dd/crates/logi-dd-tui/src/main.rs`

**Interfaces:**
- Consumes: `logi_dd_core::{Device, SysfsIo, REGISTRY, Category, Access, Value, Error}`.
- Produces:
  - `struct Row { pub attr: &'static str, pub label: &'static str, pub value: Result<Value, Error>, pub available: bool }`
  - `struct App<S: SysfsIo> { pub device: Device<S>, pub cat_idx: usize, pub row_idx: usize, pub rows: Vec<Row>, pub status: String, pub edit: Option<edit::EditState> }`
  - `impl App { fn new(device) -> Self; fn category(&self) -> Category; fn reload(&mut self); fn move_cat(&mut self, d: i32); fn move_row(&mut self, d: i32); fn selected(&self) -> Option<&Row>; }`

- [ ] **Step 1: Add ratatui deps**

Modify `logi-dd-tui/Cargo.toml` to add:
```toml
ratatui = "0.28"
crossterm = "0.28"
```

- [ ] **Step 2: Write the failing test**

`app.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::sysfs::FakeSysfs;

    fn app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_strength", "62");
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.reload();
        a
    }

    #[test]
    fn rows_follow_selected_category() {
        let mut a = app();
        // first category is Ffb; wheel_strength should be a row
        assert!(a.rows.iter().any(|r| r.attr == "wheel_strength"));
        // absent attrs are marked unavailable, not dropped
        let s = a.rows.iter().find(|r| r.attr == "wheel_ffb_filter").unwrap();
        assert!(!s.available);
    }

    #[test]
    fn move_row_clamps() {
        let mut a = app();
        a.row_idx = 0;
        a.move_row(-1);
        assert_eq!(a.row_idx, 0);
    }
}
```

- [ ] **Step 3: Implement app.rs**

Prepend to `app.rs`:
```rust
use crate::edit;
use logi_dd_core::setting::Access;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Error, Value, REGISTRY};

pub struct Row {
    pub attr: &'static str,
    pub label: &'static str,
    pub value: Result<Value, Error>,
    pub available: bool,
    pub access: Access,
}

pub struct App<S: SysfsIo> {
    pub device: Device<S>,
    pub cat_idx: usize,
    pub row_idx: usize,
    pub rows: Vec<Row>,
    pub status: String,
    pub edit: Option<edit::EditState>,
    pub quit: bool,
}

impl<S: SysfsIo> App<S> {
    pub fn new(device: Device<S>) -> Self {
        let mut a = App {
            device,
            cat_idx: 0,
            row_idx: 0,
            rows: Vec::new(),
            status: String::new(),
            edit: None,
            quit: false,
        };
        a.reload();
        a
    }

    pub fn category(&self) -> Category {
        Category::ALL[self.cat_idx]
    }

    pub fn reload(&mut self) {
        let cat = self.category();
        self.rows = REGISTRY
            .iter()
            .filter(|s| s.category == cat)
            .map(|s| Row {
                attr: s.attr,
                label: s.label,
                available: self.device.available(s.attr),
                access: s.access,
                value: self.device.read(s.attr),
            })
            .collect();
        if self.row_idx >= self.rows.len() {
            self.row_idx = self.rows.len().saturating_sub(1);
        }
    }

    pub fn move_cat(&mut self, d: i32) {
        let n = Category::ALL.len() as i32;
        self.cat_idx = ((self.cat_idx as i32 + d).rem_euclid(n)) as usize;
        self.row_idx = 0;
        self.reload();
    }

    pub fn move_row(&mut self, d: i32) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i32;
        self.row_idx = ((self.row_idx as i32 + d).clamp(0, n - 1)) as usize;
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.row_idx)
    }
}
```

- [ ] **Step 4: Stub edit module so it compiles**

Create `userspace/logi-dd/crates/logi-dd-tui/src/edit.rs`:
```rust
/// Placeholder; filled in Task 9.
pub struct EditState;
```

Update `main.rs`:
```rust
mod app;
mod edit;

fn main() {}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p logi-dd-tui app`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-tui
git commit -m "logi-dd-tui: app state (categories, rows, navigation)"
```

---

### Task 9: TUI edit state machine

**Files:**
- Modify: `userspace/logi-dd/crates/logi-dd-tui/src/edit.rs`

**Interfaces:**
- Consumes: `logi_dd_core::{Kind, Value}`.
- Produces:
  - `struct EditState { pub attr: &'static str, pub kind: Kind, pub draft: Value, pub buffer: String }`
  - `impl EditState { fn start(attr, kind, current: &Value) -> EditState; fn bump(&mut self, d: i32); fn push_char(&mut self, c: char); fn backspace(&mut self); fn commit_value(&self) -> Result<Value, logi_dd_core::Error>; }`

- [ ] **Step 1: Write the failing tests**

Replace `edit.rs` test section:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::{Kind, Value};

    #[test]
    fn percent_bump_clamps() {
        let mut e = EditState::start("wheel_strength", Kind::Percent, &Value::Percent(99));
        e.bump(5);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(100));
        e.bump(-200);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(0));
    }

    #[test]
    fn intrange_bump_respects_step_and_bounds() {
        let k = Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" };
        let mut e = EditState::start("wheel_range", k, &Value::Int(900));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Int(910));
    }

    #[test]
    fn enum_bump_wraps() {
        let k = Kind::Enum(&["kf", "tf"]);
        let mut e = EditState::start("wheel_texture_route", k, &Value::Enum(1));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Enum(0));
    }

    #[test]
    fn text_edit_buffer() {
        let mut e = EditState::start("wheel_led_slot_name", Kind::TextField { max_len: 8 }, &Value::Text("RACE".into()));
        e.push_char('R');
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACER".into()));
        e.backspace();
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACE".into()));
    }
}
```

- [ ] **Step 2: Implement edit.rs**

Replace the non-test part of `edit.rs`:
```rust
use logi_dd_core::{Error, Kind, Value};

pub struct EditState {
    pub attr: &'static str,
    pub kind: Kind,
    pub draft: Value,
    pub buffer: String,
}

impl EditState {
    pub fn start(attr: &'static str, kind: Kind, current: &Value) -> EditState {
        let buffer = match (kind, current) {
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            _ => String::new(),
        };
        EditState { attr, kind, draft: current.clone(), buffer }
    }

    pub fn bump(&mut self, d: i32) {
        self.draft = match (self.kind, &self.draft) {
            (Kind::Percent, Value::Percent(n)) => {
                Value::Percent((*n as i32 + d).clamp(0, 100) as u8)
            }
            (Kind::IntRange { min, max, step, .. }, Value::Int(n)) => {
                Value::Int((*n + d * step).clamp(min, max))
            }
            (Kind::Enum(vs), Value::Enum(n)) => {
                let len = vs.len() as i32;
                Value::Enum((*n as i32 + d).rem_euclid(len) as u8)
            }
            (Kind::Toggle { .. }, Value::Bool(b)) => Value::Bool(!*b),
            (_, v) => v.clone(),
        };
    }

    pub fn push_char(&mut self, c: char) {
        if let Kind::TextField { max_len } = self.kind {
            if self.buffer.chars().count() < max_len {
                self.buffer.push(c);
                self.draft = Value::Text(self.buffer.clone());
            }
        }
    }

    pub fn backspace(&mut self) {
        if matches!(self.kind, Kind::TextField { .. }) {
            self.buffer.pop();
            self.draft = Value::Text(self.buffer.clone());
        }
    }

    pub fn commit_value(&self) -> Result<Value, Error> {
        self.kind.validate(&self.draft)?;
        Ok(self.draft.clone())
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p logi-dd-tui edit`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-tui
git commit -m "logi-dd-tui: kind-aware edit state machine"
```

---

### Task 10: TUI rendering, event loop, and main

**Files:**
- Create: `userspace/logi-dd/crates/logi-dd-tui/src/ui.rs`
- Modify: `userspace/logi-dd/crates/logi-dd-tui/src/main.rs`
- Modify: `userspace/logi-dd/crates/logi-dd-tui/src/app.rs`

**Interfaces:**
- Consumes: `App`, `EditState`, ratatui, crossterm.
- Produces:
  - `fn ui::draw<S: SysfsIo>(f: &mut ratatui::Frame, app: &App<S>)`
  - `impl App { fn on_key(&mut self, key: crossterm::event::KeyCode); fn begin_edit(&mut self); fn commit_edit(&mut self); }`

- [ ] **Step 1: Write the failing test for the key/edit flow**

Append to `app.rs` tests:
```rust
    #[test]
    fn edit_commit_writes_and_reloads() {
        use crossterm::event::KeyCode;
        let mut a = app();
        // navigate to wheel_strength row
        a.cat_idx = 0;
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_strength").unwrap();
        a.on_key(KeyCode::Enter);       // begin edit
        assert!(a.edit.is_some());
        a.on_key(KeyCode::Right);       // bump +1
        a.on_key(KeyCode::Enter);       // commit
        assert!(a.edit.is_none());
        assert_eq!(a.device.read("wheel_strength").unwrap(),
                   logi_dd_core::Value::Percent(63));
    }

    #[test]
    fn wrong_mode_sets_status_prompt() {
        use crossterm::event::KeyCode;
        let fs = logi_dd_core::sysfs::FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_sensitivity", "50");
        let mut a = App::new(logi_dd_core::Device::with_io(fs));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Sensitivity).unwrap();
        a.reload();
        a.row_idx = a.rows.iter().position(|r| r.attr == "wheel_sensitivity").unwrap();
        a.on_key(KeyCode::Enter);
        a.on_key(KeyCode::Left);
        a.on_key(KeyCode::Enter);       // commit -> WrongMode
        assert!(a.status.to_lowercase().contains("desktop"));
    }
```

- [ ] **Step 2: Add the key/edit methods to App**

Append to the `impl<S: SysfsIo> App<S>` block in `app.rs`:
```rust
    pub fn begin_edit(&mut self) {
        let Some(row) = self.selected() else { return };
        let Some(spec) = Device::<S>::spec(row.attr) else { return };
        if spec.access == Access::ReadOnly {
            return;
        }
        if spec.access == Access::Action {
            match self.device.write(row.attr, &Value::Trigger) {
                Ok(()) => self.status = format!("{}: done", row.label),
                Err(e) => self.status = format!("{}: {e}", row.label),
            }
            return;
        }
        let Ok(cur) = &row.value else {
            self.status = "cannot edit (value unreadable)".into();
            return;
        };
        self.edit = Some(edit::EditState::start(spec.attr, spec.kind, cur));
    }

    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let attr = e.attr;
        let label = Device::<S>::spec(attr).map(|s| s.label).unwrap_or(attr);
        match e.commit_value().and_then(|v| self.device.write(attr, &v)) {
            Ok(()) => {
                self.status = format!("{label} set");
            }
            Err(Error::WrongMode { needed: logi_dd_core::Mode::Desktop }) => {
                self.status = "needs desktop mode: press 'd' to switch, then retry".into();
            }
            Err(err) => {
                self.status = format!("{label}: {err}");
            }
        }
        self.reload();
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode::*;
        if let Some(ed) = self.edit.as_mut() {
            match key {
                Enter => self.commit_edit(),
                Esc => self.edit = None,
                Left => ed.bump(-1),
                Right => ed.bump(1),
                Backspace => ed.backspace(),
                Char(c) => ed.push_char(c),
                _ => {}
            }
            return;
        }
        match key {
            Char('q') => self.quit = true,
            Char('r') => self.reload(),
            Char('d') => {
                self.status = match self.device.ensure_desktop_mode() {
                    Ok(()) => "switched to desktop mode".into(),
                    Err(e) => format!("mode switch: {e}"),
                };
                self.reload();
            }
            Up => self.move_row(-1),
            Down => self.move_row(1),
            Left => self.move_cat(-1),
            Right => self.move_cat(1),
            Enter => self.begin_edit(),
            _ => {}
        }
    }
```

Also add `use logi_dd_core::Mode;` is not needed (fully-qualified above); ensure `Value` and `Error` are already imported (they are).

- [ ] **Step 3: Run the flow tests**

Run: `cargo test -p logi-dd-tui app`
Expected: PASS (4 tests total).

- [ ] **Step 4: Implement rendering (ui.rs)**

`ui.rs`:
```rust
use crate::app::App;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn draw<S: SysfsIo>(f: &mut Frame, app: &App<S>) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)])
        .split(f.area());

    // header: device + mode
    let info = app.device.info().ok();
    let header = match &info {
        Some(i) => format!(" logi-dd   serial {}   fw {}   mode: {:?}", i.serial, i.firmware, i.mode),
        None => " logi-dd   (no wheel)".to_string(),
    };
    f.render_widget(Paragraph::new(header).block(Block::default().borders(Borders::ALL)), root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(root[1]);

    // categories
    let cats: Vec<ListItem> = Category::ALL
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = if i == app.cat_idx {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(c.label()).style(style)
        })
        .collect();
    f.render_widget(List::new(cats).block(Block::default().borders(Borders::ALL).title("Category")), body[0]);

    // settings in the selected category
    let rows: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let spec = Device::<S>::spec(row.attr);
            let val = match (&row.value, spec) {
                _ if !row.available => "(not on this wheel)".to_string(),
                (Ok(v), Some(s)) => {
                    if let Some(ed) = &app.edit {
                        if i == app.row_idx {
                            s.kind.display(&ed.draft)
                        } else {
                            s.kind.display(v)
                        }
                    } else {
                        s.kind.display(v)
                    }
                }
                (Err(e), _) => format!("<{e}>"),
                _ => "?".to_string(),
            };
            let mut style = Style::default();
            if !row.available {
                style = style.add_modifier(Modifier::DIM);
            }
            if i == app.row_idx {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(Line::from(format!("{:<24} {}", row.label, val))).style(style)
        })
        .collect();
    f.render_widget(List::new(rows).block(Block::default().borders(Borders::ALL).title("Settings")), body[1]);

    // status / help
    let help = if app.edit.is_some() {
        "editing:  <-/->  adjust   type  text   Enter  commit   Esc  cancel"
    } else {
        "up/down  select    <-/->  category    Enter  edit    d  desktop mode    r  refresh    q  quit"
    };
    let status = format!("{}\n{}", app.status, help);
    f.render_widget(Paragraph::new(status), root[2]);
}
```

- [ ] **Step 5: Implement main.rs (terminal + run loop)**

`main.rs`:
```rust
mod app;
mod edit;
mod ui;

use app::App;
use crossterm::event::{self, Event};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use logi_dd_core::sysfs::RealSysfs;
use logi_dd_core::Device;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = match Device::discover() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("logi-dd: {e}");
            std::process::exit(1);
        }
    };
    run(App::new(device))
}

fn run(mut app: App<RealSysfs>) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let res = loop {
        term.draw(|f| ui::draw(f, &app))?;
        if let Event::Key(k) = event::read()? {
            if k.kind == event::KeyEventKind::Press {
                app.on_key(k.code);
            }
        }
        if app.quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    res
}
```

- [ ] **Step 6: Build and clippy the whole workspace**

Run: `cd userspace/logi-dd && cargo build && cargo test && cargo clippy -- -D warnings`
Expected: builds, all tests pass, no clippy warnings.

- [ ] **Step 7: Commit**

```bash
git add userspace/logi-dd/crates/logi-dd-tui
git commit -m "logi-dd-tui: rendering, key handling, and run loop"
```

---

### Task 11: CI job for the workspace

**Files:**
- Create: `.github/workflows/logi-dd.yml`

**Interfaces:**
- Produces: a CI workflow that builds, tests, and clippy-checks the Rust workspace on push/PR.

- [ ] **Step 1: Write the workflow**

`.github/workflows/logi-dd.yml`:
```yaml
name: logi-dd
on:
  push:
    paths: ["userspace/logi-dd/**", ".github/workflows/logi-dd.yml"]
  pull_request:
    paths: ["userspace/logi-dd/**", ".github/workflows/logi-dd.yml"]
jobs:
  build:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: userspace/logi-dd
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        run: rustup toolchain install stable --profile minimal --component clippy
      - name: Build
        run: cargo build --workspace --verbose
      - name: Test
        run: cargo test --workspace --verbose
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Verify locally (the same commands CI runs)**

Run: `cd userspace/logi-dd && cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/logi-dd.yml
git commit -m "ci: build/test/clippy the logi-dd Rust workspace"
```

---

### Task 12: Manual hardware checklist

**Files:**
- Create: `userspace/logi-dd/HARDWARE-TEST.md`

**Interfaces:**
- Produces: a human checklist for validating on the live RS50 (shu), where CI cannot.

- [ ] **Step 1: Write the checklist**

`userspace/logi-dd/HARDWARE-TEST.md`:
```markdown
# logi-dd Phase 1 - manual hardware pass (RS50 on shu)

Preconditions: `hid-logitech-dd` loaded, wheel bound (a `wheel_range` sysfs
exists), run as the normal user.

Build and run:

    cd userspace/logi-dd && cargo build --release
    ./target/release/logi-dd-tui

Checklist (one change per category, confirm it takes effect on the wheel):

- [ ] Header shows the wheel serial, firmware, and current mode.
- [ ] FFB: change `FFB strength` and confirm the wheel gets stronger/weaker.
- [ ] Rotation: set `Rotation range` to 540, turn lock-to-lock, confirm ~540 deg.
- [ ] Sensitivity: in desktop mode, change `Sensitivity`; in onboard mode the
      edit prompts "needs desktop mode" and `d` switches then the write applies.
- [ ] TrueForce: toggle `Texture routing` tf/kf and feel a rumble effect move
      between the rim buzz and the steering.
- [ ] LEDs (RS50): change `LED colors` / `LED brightness`, press `Apply LEDs`,
      confirm the strip updates.
- [ ] Pedals: change a pedal `curve`; `Brake force` edit prompts onboard mode.
- [ ] Profiles: switch `Mode` desktop<->onboard; `Profile names` shows the slots.
- [ ] Calibration: `Calibrate centre here` re-centres at the current position.
- [ ] Info: serial and firmware match `cat wheel_serial` / `wheel_firmware`.
- [ ] Unsupported attrs on this wheel show greyed "(not on this wheel)".
- [ ] `q` exits cleanly and leaves the terminal usable; LEDs/settings persist.
```

- [ ] **Step 2: Commit**

```bash
git add userspace/logi-dd/HARDWARE-TEST.md
git commit -m "logi-dd: manual hardware test checklist for Phase 1"
```

---

## Self-Review

**Spec coverage:**
- Workspace + two crates (spec 3): Tasks 1, 8. Covered.
- Device discovery (4.1): Task 7. Covered.
- Settings model / Kind / Value (4.2, 4.4): Tasks 4, 5, 6. Covered.
- Mode gating + ensure_desktop (4.3): Task 7 + TUI prompt Task 10. Covered.
- Errors + errno mapping (4.5): Task 2, used in Task 7. Covered.
- TUI layout/interaction (spec 5): Tasks 8, 9, 10. Covered.
- Settings catalog (spec 6): Task 6 registry. Covered (curves as summary+reset; RGB editor is a single-value edit in Phase 1 - note below).
- Deferred compat aliases (spec 6 note): not in registry. Consistent.
- Testing (spec 7): fake-sysfs unit tests throughout; CI Task 11; manual Task 12. Covered.
- Packaging/CI (spec 8): Task 11.

**Known Phase-1 simplification (explicit, not a gap):** `wheel_led_colors` (RgbStrip) and the curve attributes are read and written as whole values; the TUI edits RGB via text entry of the 10-hex string and curves via `reset` only (full per-LED / per-point editors are the Phase 2 GUI). The core fully supports the values; only the TUI editor is minimal. If a richer TUI RGB editor is wanted in Phase 1, add it as a follow-up task; it is intentionally out of the minimal path here.

**Placeholder scan:** no TBD/TODO; every code step has complete code.

**Type consistency:** `Device::with_io`, `Device::spec`, `Device::available`, `Device::read/write`, `Device::ensure_desktop_mode`, `Device::current_mode`, `Device::info` used consistently across Tasks 7-10. `EditState::{start,bump,push_char,backspace,commit_value}` consistent Tasks 9-10. `Kind::{parse,format,validate,display}` consistent Tasks 5-10. `App::{new,reload,move_cat,move_row,selected,on_key,begin_edit,commit_edit}` consistent Tasks 8-10.

One note for the implementer: `Device::spec` and `Device::available` are called as `Device::<S>::spec(...)` from the TUI (associated fn on a generic type); the tests in Task 6 call the free `REGISTRY` scan. Both resolve to the same registry.
