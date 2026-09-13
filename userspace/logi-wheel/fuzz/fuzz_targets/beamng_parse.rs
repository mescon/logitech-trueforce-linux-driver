// SPDX-License-Identifier: GPL-2.0-only
#![no_main]
use libfuzzer_sys::fuzz_target;
// BeamNG OutGauge UDP telemetry; the decoder keeps state across packets,
// so each input is fed as a run of packets split at every 96 bytes.
fuzz_target!(|data: &[u8]| {
    let mut d = logi_tf_sim::beamng::Decoder::new();
    for chunk in data.chunks(96) {
        let _ = d.parse(chunk);
    }
    let _ = d.parse(data);
});
