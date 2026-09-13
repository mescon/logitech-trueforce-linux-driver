#![no_main]
use libfuzzer_sys::fuzz_target;
// A PID output report as a Wine process writes it to the virtual wheel.
fuzz_target!(|data: &[u8]| {
    let _ = logi_ffb::pidff::decode(data);
});
