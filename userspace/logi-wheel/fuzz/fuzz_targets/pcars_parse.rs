#![no_main]
use libfuzzer_sys::fuzz_target;
// Project CARS 2 / Automobilista 2 UDP telemetry.
fuzz_target!(|data: &[u8]| {
    let _ = logi_tf_sim::pcars::parse(data);
});
