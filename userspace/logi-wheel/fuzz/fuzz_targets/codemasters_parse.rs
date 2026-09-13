#![no_main]
use libfuzzer_sys::fuzz_target;
// Codemasters-format UDP telemetry (DiRT, GRID, EA WRC in that mode).
fuzz_target!(|data: &[u8]| {
    let _ = logi_tf_sim::codemasters::parse(data);
});
