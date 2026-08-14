# Texture-merge validation fixtures

Small artifacts cut from the usbmon captures of the native TF texture-merge
validation session, 2026-08-13/14 (AC EVO under Proton, RS50, branch
native-tf-texture-merge). Each file is raw interface-2 OUT payload bytes,
64 bytes per packet, no headers.

## Headline numbers from that session

- Live merge (`/tmp/live_merge.pcapng`): the kernel merge spliced texture
  into **11,217 of 11,219** real SDK stream packets. Spliced-sample rms
  **411.2** against the capture-fit target of 411 (0.05 percent off);
  spectrum peak exactly **300 Hz** for the fed RPM. The SDK's own
  base-force bytes passed through untouched.
- Merge-off control (`/tmp/merge_off.pcapng`): passthrough byte-identical,
  zero packets modified.

## Files

- `range_push_2700_pass1.bin`, `range_push_90.bin`,
  `range_push_2700_pass2.bin`: the three type-0x0e operating-range push
  frames from the game-launch capture (`/tmp/night_launch2.pcapng`), in
  capture order. The SDK's init pushes 2700.0 once per init pass (seq 0x32
  in both), and the 90-degree push (seq 0x46) is the mid-session clamp the
  range-restore machinery exists for. The float rides at bytes 6-9,
  IEEE-754 little-endian; `test_texmerge.c`'s fixture test decodes all
  three through `hidpp_dd_texmerge_decode_push_deg()` and asserts
  2700/90/2700.
- `spliced_stream_200.bin`: 200 consecutive spliced stream packets
  (12,800 bytes) from the start of the live-merge validation burst in
  `/tmp/live_merge.pcapng`. Every packet: report id 0x01, type 0x01,
  byte10 = 4 (the merge's sample block), byte11 = 0x00 (the SDK's own
  value, preserved), cur = 0x8000 (the headless session's idle base
  force, untouched by the merge). Sequence bytes run continuously
  (121..64 mod 256). This exact slice measures rms 411.2 and a 300 Hz
  fundamental, matching the whole-session numbers above.
