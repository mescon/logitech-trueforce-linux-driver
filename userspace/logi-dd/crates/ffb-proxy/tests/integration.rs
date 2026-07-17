//! Runs only on a host with /dev/uhid writable (root). Enable with:
//! `cargo test -p ffb-proxy -- --ignored`

#[test]
#[ignore]
fn create_and_destroy_virtual_device() {
    let mut dev = ffb_proxy::uhid::Device::create().expect("create uhid device");
    // Pump events until START, then send one input report and destroy.
    let mut saw_start = false;
    for _ in 0..20 {
        if dev.read_event().expect("read event") == ffb_proxy::uhid::Event::Start {
            saw_start = true;
            break;
        }
    }
    assert!(saw_start, "expected UHID_START");
    let rep = ffb_proxy::descriptor::InputReport::default().to_bytes();
    dev.send_input(&rep).expect("send input");
    drop(dev); // sends UHID_DESTROY
}
