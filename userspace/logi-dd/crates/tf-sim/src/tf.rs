// SPDX-License-Identifier: GPL-2.0-only
//! Safe wrapper over the libtrueforce C library (statically linked).
//!
//! Binds the minimal session surface from `include/trueforce.h`:
//! `dllOpen` (discovery scan) -> `logiTrueForceAvailable` ->
//! `logiTrueForceSetTorqueTFfloat` (which lazily sends the 68-packet
//! init sequence and starts the 250 Hz stream thread on first use) ->
//! `logiTrueForceClearTF` + `logiWheelClose` + `dllClose` on drop.
//!
//! Discovery (which hidraw node, which wheels) is entirely the
//! library's: it walks /sys/class/hidraw for the supported wheel PIDs
//! and picks interface 2, exactly as its README documents.
//!
//! Note: `dllOpen`/`dllClose` manage library-global state, so hold at
//! most one [`TfStream`] per process at a time (the daemon does).

use libc::c_int;

use crate::error::{Error, Result};

const LOGITF_OK: c_int = 0;

#[allow(non_snake_case)]
mod ffi {
    use libc::c_int;

    extern "C" {
        pub fn dllOpen() -> c_int;
        pub fn dllClose() -> c_int;
        pub fn logiTrueForceAvailable(index: c_int) -> bool;
        pub fn logiTrueForceSetTorqueTFfloat(index: c_int, samples: *const f32, count: c_int) -> c_int;
        pub fn logiTrueForceClearTF(index: c_int) -> c_int;
        pub fn logiWheelClose(index: c_int) -> c_int;
    }
}

/// An open TrueForce sample stream to controller `index`.
///
/// Created by [`TfStream::open`]; dropping it clears any queued samples,
/// closes the wheel session, and releases the library.
#[derive(Debug)]
pub struct TfStream {
    index: c_int,
}

impl TfStream {
    /// Run discovery and validate that a supported wheel answers at
    /// `index`. Fails with [`Error::NoWheel`] when none is present, which
    /// callers treat as retryable. The wheel's init sequence is not sent
    /// here; the library performs it on the first [`push`](Self::push)
    /// (that first call blocks for roughly half a second).
    pub fn open(index: i32) -> Result<Self> {
        // SAFETY: no arguments; the library serializes its own state.
        let rc = unsafe { ffi::dllOpen() };
        if rc != LOGITF_OK {
            return Err(Error::Stream("dllOpen".into(), rc));
        }
        // SAFETY: plain index probe against the library's device table.
        if !unsafe { ffi::logiTrueForceAvailable(index) } {
            // SAFETY: balances the dllOpen above; nothing is in use yet.
            unsafe { ffi::dllClose() };
            return Err(Error::NoWheel);
        }
        Ok(TfStream { index })
    }

    /// Queue `samples` (each -1.0..1.0, at 1 kHz) for the wheel.
    ///
    /// Mirrors the Windows SDK semantics: blocks when the library's
    /// 4096-sample ring is full, so a real-time producer self-paces.
    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        // SAFETY: valid pointer + length pair for the slice; the library
        // copies the samples into its ring before returning.
        let rc = unsafe {
            ffi::logiTrueForceSetTorqueTFfloat(self.index, samples.as_ptr(), samples.len() as c_int)
        };
        if rc != LOGITF_OK {
            return Err(Error::Stream("logiTrueForceSetTorqueTFfloat".into(), rc));
        }
        Ok(())
    }
}

impl Drop for TfStream {
    fn drop(&mut self) {
        // SAFETY: index was validated in open(); ClearTF drops queued
        // samples and re-centres the stream window, logiWheelClose stops
        // the stream thread and closes the hidraw session, dllClose
        // releases the module. All are idempotent in the library.
        unsafe {
            ffi::logiTrueForceClearTF(self.index);
            ffi::logiWheelClose(self.index);
            ffi::dllClose();
        }
    }
}
