// SPDX-License-Identifier: GPL-2.0-only
//! Build glue for the libtrueforce FFI.
//!
//! Links the in-repo `userspace/libtrueforce` static archive. If the
//! archive is absent (fresh checkout, CI), it is built via the library's
//! own Makefile so there is exactly one authoritative build recipe.
//! Static linking is preferred: the shipped `logi-tf-sim` binary then has
//! no runtime dependency on `libtrueforce.so`.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let lib_dir = manifest
        .join("../../../libtrueforce")
        .canonicalize()
        .expect("userspace/libtrueforce not found relative to the tf-sim crate");
    let archive = lib_dir.join("libtrueforce.a");

    if !archive.exists() {
        let status = Command::new("make")
            .arg("-C")
            .arg(&lib_dir)
            .arg("libtrueforce.a")
            .status()
            .expect("failed to run make for libtrueforce");
        assert!(status.success(), "make -C {} libtrueforce.a failed", lib_dir.display());
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=trueforce");
    // libtrueforce uses pthreads (stream thread, mutexes). glibc >= 2.34
    // folds pthread into libc, but older toolchains still need the flag.
    println!("cargo:rustc-link-lib=pthread");

    println!("cargo:rerun-if-changed={}", lib_dir.join("include/trueforce.h").display());
    println!("cargo:rerun-if-changed={}", lib_dir.join("Makefile").display());
    for src in ["discovery.c", "exports.c", "kf.c", "session.c", "status.c", "stream.c", "sysfs.c", "internal.h", "tf_init_data.h"] {
        println!("cargo:rerun-if-changed={}", lib_dir.join("src").join(src).display());
    }
}
