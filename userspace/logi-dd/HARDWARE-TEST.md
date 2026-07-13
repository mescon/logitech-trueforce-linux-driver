# logi-dd Phase 1 - manual hardware pass (RS50 on shu)

Preconditions: `hid-logitech-dd` loaded, wheel bound (a `wheel_range` sysfs
exists), run as the normal user.

Build and run:

    cd userspace/logi-dd && cargo build --release
    ./target/release/logi-dd-tui

Checklist (one change per category, confirm it takes effect on the wheel):

- [ ] Header shows the wheel serial, firmware, and current mode.
- [ ] FFB: change `FFB strength` and confirm the wheel gets stronger/weaker.
- [ ] Rotation: set `Rotation range` to 540, turn lock-to-lock, confirm ~540 deg.
- [ ] Sensitivity: in desktop mode, change `Sensitivity`; in onboard mode the
      edit prompts "needs desktop mode" and `d` switches then the write applies.
- [ ] TrueForce: toggle `Texture routing` tf/kf and feel a rumble effect move
      between the rim buzz and the steering.
- [ ] LEDs (RS50): change `LED colors` / `LED brightness`, press `Apply LEDs`,
      confirm the strip updates.
- [ ] Pedals: change a pedal `curve`; `Brake force` edit prompts onboard mode.
- [ ] Profiles: switch `Mode` desktop<->onboard; `Profile names` shows the slots.
- [ ] Calibration: `Calibrate centre here` re-centres at the current position.
- [ ] Info: serial and firmware match `cat wheel_serial` / `wheel_firmware`.
- [ ] Unsupported attrs on this wheel show greyed "(not on this wheel)".
- [ ] `q` exits cleanly and leaves the terminal usable; LEDs/settings persist.
