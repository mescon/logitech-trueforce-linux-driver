#![no_main]
use libfuzzer_sys::fuzz_target;
// The in-prefix relay's datagram, as the daemon reads it off udp/20780.
fuzz_target!(|data: &[u8]| {
    let _ = logi_wheel_core::relay::decode(data);
    let _ = logi_wheel_core::relay::parse(data);
});
