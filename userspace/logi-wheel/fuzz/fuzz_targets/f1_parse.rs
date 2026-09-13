// SPDX-License-Identifier: GPL-2.0-only
#![no_main]
use libfuzzer_sys::fuzz_target;
// EA Sports F1 UDP telemetry; the decoder keeps state across packets.
fuzz_target!(|data: &[u8]| {
    let mut d = logi_tf_sim::f1::Decoder::new();
    for chunk in data.chunks(1349) {
        let _ = d.parse(chunk);
    }
    let _ = d.parse(data);
});
