//! Dev-only harness: discover the real wheel and write one attribute through
//! the same `Device` path the app uses. Not shipped in any binary.
//!
//!   cargo run --example apply -p logi-dd-core -- <attr> <value>
//!
//! e.g. cargo run --example apply -p logi-dd-core -- \
//!          wheel_throttle_curve "0:0 20000:5000 45000:5000 65535:65535"

use logi_dd_core::{Device, Value, REGISTRY};

fn main() {
    let mut args = std::env::args().skip(1);
    let attr = args.next().expect("usage: apply <attr> <value>");
    let raw = args.next().expect("usage: apply <attr> <value>");

    let spec = REGISTRY
        .iter()
        .find(|s| s.attr == attr)
        .unwrap_or_else(|| panic!("unknown attr {attr}"));

    let dev = Device::discover().expect("no wheel found");
    let v: Value = spec.kind.parse(&raw).expect("bad value for this attr");
    dev.write(&attr, &v).expect("write failed");
    println!("wrote {attr} = {v:?}");
}
