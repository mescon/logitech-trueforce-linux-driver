// SPDX-License-Identifier: GPL-3.0-or-later
//! Keep the window runnable on a distribution older than the build host.
//!
//! glibc versions its symbols, and a binary links against whatever version
//! the build machine's headers offer. Build on current Arch and two of
//! them, `acosf` and `atan2f`, bind to `GLIBC_2.43`, which makes the whole
//! binary refuse to start anywhere older:
//!
//! ```text
//! logi-wheel-gui: /usr/lib/libm.so.6: version `GLIBC_2.43' not found
//! ```
//!
//! That is what SteamOS does with our packages (issue #68). It is a frozen
//! Arch snapshot, so its glibc trails the one our packages are built on,
//! and a Steam Deck cannot run the window at all. Nothing else we ship hits
//! this: the terminal app, the daemon and the FFB proxy stop at 2.39 and
//! run there fine.
//!
//! Neither symbol is ours. They come from the drawing stack, so there is no
//! call site to change. The two versions of each function still exist in
//! current glibc, though, so the binary can simply ask for the old one: a
//! definition here overrides the library's for every reference in the
//! executable, and forwards to `acosf@GLIBC_2.2.5`, which every glibc since
//! 2001 has.
//!
//! Check the result with:
//!
//! ```text
//! objdump -T logi-wheel-gui | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1
//! ```
//!
//! Only for Linux with glibc. A musl build has no symbol versioning and
//! must not do any of this; build.rs applies the `--wrap` flags under the
//! same condition.

core::arch::global_asm!(
    ".symver __logi_compat_acosf, acosf@GLIBC_2.2.5",
    ".symver __logi_compat_atan2f, atan2f@GLIBC_2.2.5",
);

unsafe extern "C" {
    fn __logi_compat_acosf(x: f32) -> f32;
    fn __logi_compat_atan2f(y: f32, x: f32) -> f32;
}

/// Takes every call to `acosf` in this binary, via `--wrap` in build.rs,
/// and forwards it to the old version of the symbol.
///
/// The name matters. An earlier attempt defined `acosf` itself, which is
/// the obvious way to override a library function and is wrong here: an
/// unversioned definition in the executable also satisfies the versioned
/// reference next to it, so the forward called itself and the binary died
/// of a stack overflow on the first draw. Wrapping renames the call sites
/// instead, leaving nothing for the versioned reference to bind to except
/// the library.
#[unsafe(no_mangle)]
pub extern "C" fn __wrap_acosf(x: f32) -> f32 {
    unsafe { __logi_compat_acosf(x) }
}

/// The same for `atan2f`.
#[unsafe(no_mangle)]
pub extern "C" fn __wrap_atan2f(y: f32, x: f32) -> f32 {
    unsafe { __logi_compat_atan2f(y, x) }
}

#[cfg(test)]
mod tests {
    /// The overrides must still compute the function, not merely link.
    ///
    /// A forwarding mistake here would not fail to build or start: the
    /// window would come up with a dial pointing the wrong way, which is
    /// exactly the kind of fault that reaches a user instead of CI. The
    /// reference is the f64 form, whose symbol is old enough that it is
    /// never rebound.
    #[test]
    fn the_forwarded_functions_still_compute() {
        for x in [-1.0f32, -0.5, 0.0, 0.25, 0.75, 1.0] {
            let ours = super::__wrap_acosf(x);
            let reference = (x as f64).acos() as f32;
            assert!(
                (ours - reference).abs() < 1e-5,
                "acosf({x}) gave {ours}, expected about {reference}"
            );
        }
        for (y, x) in [(0.0f32, 1.0f32), (1.0, 0.0), (-1.0, 0.0), (1.0, 1.0), (-2.0, -3.0)] {
            let ours = super::__wrap_atan2f(y, x);
            let reference = (y as f64).atan2(x as f64) as f32;
            assert!(
                (ours - reference).abs() < 1e-5,
                "atan2f({y}, {x}) gave {ours}, expected about {reference}"
            );
        }
    }
}
