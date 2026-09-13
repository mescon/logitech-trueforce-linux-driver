// SPDX-License-Identifier: GPL-2.0-only
#![no_main]
use libfuzzer_sys::fuzz_target;
// EA Sports WRC UDP telemetry.
fuzz_target!(|data: &[u8]| {
    let _ = logi_tf_sim::wrc::parse(data);
});
