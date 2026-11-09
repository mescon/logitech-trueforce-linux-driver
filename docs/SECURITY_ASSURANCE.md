# Security assurance

This page says what this software is exposed to, what it does about it,
and why the maintainer believes that is enough. It is the assurance case
the project's security work is measured against. The reporting process is
in [../SECURITY.md](../SECURITY.md). The rules for people and credentials
are in [../GOVERNANCE.md](../GOVERNANCE.md).

## What runs where

- **hid-logitech-dd**, a kernel module. It binds the wheel's USB HID
  interfaces (HID is the USB class for input devices). It parses every
  report the wheel sends. It sends HID++ commands (HID++ is Logitech's
  configuration protocol over USB) and the TrueForce stream (Logitech's
  high-rate haptic stream). It exposes settings through sysfs, the
  kernel's settings file tree under `/sys`. It runs with kernel privilege.
- **logi-wheel, logi-wheel-gui**, settings apps. They read and write sysfs
  attributes. They read `/dev/hidraw` (raw access to the USB HID device)
  for diagnostics. They run as the user.
- **logi-tf-sim**, the telemetry daemon (a background service). It listens
  for game telemetry on UDP ports on the loopback interface (network
  traffic that never leaves the machine). It synthesises haptics from that
  telemetry. It writes them to the wheel through hidraw or the kernel's
  sysfs. It runs as the user.
- **logi-ffb**, the DirectInput proxy. DirectInput is the Windows
  game-input interface. The proxy creates a virtual HID device through
  `/dev/uhid`. It receives force-feedback reports from a Wine process
  (Wine runs Windows programs on Linux). It replays them on the real
  wheel's evdev node (evdev is the Linux input event interface). It runs
  as the user.
- **logi-launch** and the Windows helpers it stages into a game's prefix
  (the escape proxy, the telemetry relay). A prefix is a game's private
  Wine environment. They run inside the game's Wine process, with the
  game's own privileges.

## Attack surface and threat model

Who could attack, through what, and what the consequence would be.

| Source | Reaches | Worst plausible outcome | What stands in the way |
|---|---|---|---|
| **A malicious or faulty USB device** presenting Logitech's ids | The kernel module's report parsers and HID++ reply handling | Kernel memory corruption; a crash | Every report is length-checked before a field is read. Feature indexes are resolved from the device's own tables and bounds-checked. Replies are matched to requests by sequence and validated. The driver is built under the kernel's `-Wall`. It is checked by sparse, smatch and CodeQL (static analysers, which find bugs without running the code). Its lifetime-sensitive paths run under a KASAN, lockdep and kmemleak kernel (memory-error, lock-order and memory-leak detectors) in the development VM. Physical access to the USB port is required. |
| **A game, or anything else on the machine, sending UDP telemetry** to the daemon's loopback ports | The telemetry decoders and the relay datagram parser | A daemon crash (loss of haptics) or, at worst, wrong forces | The daemon only binds loopback. Every decoder validates length, magic and version before reading a field. It rejects samples with no usable engine data. It is fuzzed with cargo-fuzz (fed random input to find crashes). Forces are clamped by the wheel's strength setting and the kernel's limits. The wheel's own firmware saturates torque. |
| **A Wine process** writing force-feedback reports to the virtual wheel | The PID report decoder in logi-ffb | A proxy crash (loss of force) or wrong forces | The decoder validates report ids, lengths and block indexes. It is fuzzed. Effects are translated into the kernel's own force-feedback interface, which bounds them. The proxy runs as the user, with no more access than the game itself has. |
| **A game's prefix** where the Windows helpers run | The escape proxy and relay, and through them the wheel's sysfs range | Wrong rotation range; a stalled game | The helpers only read the game's shared memory and write telemetry to loopback. They perform one write on the wheel: the rotation range the SDK announces (the SDK is Logitech's library that a game loads to talk to the wheel). That write is bounds-checked to the range a wheel accepts, and can be switched off. |
| **Another local user** | The wheel's sysfs attributes, which the udev rule (a Linux device-manager rule) makes world-writable | Changed settings; a changed rotation range while someone is driving | Accepted risk, stated here. The attributes are world-writable so that settings apps and games work without root. A machine with a racing wheel attached is assumed to be single-user. No attribute grants any privilege beyond the wheel itself. |
| **A dependency or a build tool** | Everything | Compromised releases | Dependencies are pinned by `Cargo.lock`, updated by Dependabot, and checked by `cargo audit` on every push and weekly. GitHub Actions are pinned to commit hashes. Workflow tokens are read-only by default. Releases are signed and carry SLSA provenance for every asset (a signed record of how a release was built). The signing key's fingerprint is published. |
| **The maintainer's accounts and secrets** | Releases and channels | Compromised releases | Two-factor authentication is required for write access. Secrets live only in GitHub Actions and are rotated on any suspicion. The rules are in GOVERNANCE.md. |

The critical code paths, in order of consequence, are: the kernel module's
report parsing (kernel privilege), the daemon's writes to the wheel
(physical force), and the release pipeline (everyone's machine). Each has
a specific mitigation above, not a general one.

## Secure design principles the project follows

- **Least privilege.** Only the kernel module runs privileged. Only it
  touches the device directly. Everything else runs as the user, through
  interfaces the kernel bounds. Workflow tokens are read-only unless a job
  says otherwise.
- **Validate at the boundary.** Every byte from a device, a socket or a HID
  node passes a parser. The parser checks length and structure before any
  field is used. Those parsers are the fuzz targets.
- **Fail closed on force.** Sometimes the driver cannot tell what a wheel
  is doing (unreadable range, a lost session, a device that stops
  answering). Then it stops sending force instead of guessing, and says so
  in dmesg (the kernel log).
- **One writer per stream.** Two processes writing the TrueForce stream
  corrupt each other's output. So the daemon takes a lease. It steps aside
  when a game's own SDK has the wheel. Most of the 0.4x series was about
  enforcing this.
- **Nothing hidden in the build.** The kernel module and the apps carry
  the same git stamp. The installer refuses to leave an app that does not
  match the module. CI (the automated checks on every change) verifies
  the three committed Windows binaries byte for byte against their source.

## Dependencies and software composition

How dependencies are chosen, obtained and tracked. This is the policy the
Baseline criteria ask for.

- **Selection.** A Rust dependency is added only with a reason in the
  change that adds it. It must be GPL-2.0-compatible. It must be
  maintained. The kernel module has no dependencies beyond the kernel.
- **Obtaining.** Dependencies come from crates.io through cargo, pinned in
  `userspace/logi-wheel/Cargo.lock`. Distribution packages build from that
  lock with `--locked`. GitHub Actions are pinned to full commit hashes.
- **Tracking.** Dependabot opens weekly update pull requests for cargo and
  for Actions. `cargo audit` runs on every push and weekly against the
  RustSec advisory database. It fails the workflow on an advisory.
- **Remediation thresholds.** A vulnerability advisory in a dependency is
  fixed before the next release, by updating or replacing the crate.
  Anything rated high or critical is fixed within 14 days. A license
  problem is fixed before the next release. Some advisories do not affect
  the project (for example maintenance notices about crates that carry no
  vulnerability). Those are recorded twice, with the reason each time. For
  people, in the VEX document at
  [../security/openvex.json](../security/openvex.json) (VEX, a record of
  which advisories do not apply and why). For scanners, in
  `userspace/logi-wheel/osv-scanner.toml`. osv-scanner reads that file
  from the lock file's directory, and so does the OpenSSF Scorecard, which
  runs it. Every scanner entry carries an expiry date. So an accepted
  advisory is reported again when it lapses, and has to be reviewed again.
  Both files are reviewed at each release. No release is cut with an
  unrecorded advisory outstanding.
- **Static analysis findings.** A CodeQL or clippy finding of a security
  weakness blocks the change that introduced it. clippy failures fail CI.
  CodeQL alerts are triaged within 14 days. Each is fixed before the next
  release, or dismissed with a written reason on the alert.

## Software bill of materials

Every release from 0.42.0 onward attaches a CycloneDX SBOM for each Rust
binary (the settings apps, the daemon, the proxy and the Windows relay).
An SBOM is a list of every software component in a release. The release
pipeline generates it from the locked dependency graph. The kernel module
has no third-party components. Its bill of materials is the module source
itself, which the packages ship.

## Verifying a release

Release assets are signed with the project's GnuPG key. Its fingerprint is
`4B5B DD78 0272 3B28 9FA9 34CA CD77 C00A 443B 9E79`. The key is published
with each release as `logitech-trueforce-signing-key.asc`. Signed: the
Arch packages and repository database up to 0.41.0, and every asset from
0.42.0. The README's "Verifying a release" says how to check one. pacman
does it once the key is trusted. For any other asset:

1. Import the key.
2. Check its fingerprint against the one above.
3. Run `gpg --verify <asset>.sig <asset>`.

The identity behind a release is that key. A release signed by any other
key is not from this project, until the README says the key has changed.

From 0.42.0 each release also carries a SLSA build provenance statement
(`release-assets.intoto.jsonl`). The SLSA generic generator produces it.
The release workflow hashes every attached asset. The generator signs the
list keylessly through Sigstore. The statement records the repository, the
tag and the workflow that published them. `slsa-verifier` checks an asset
against it, as the README shows. The GnuPG signature says who signed. The
provenance says what built it and from which commit.

## Support and end of support

The latest release is the supported one and receives security fixes. A
release stops receiving them the moment a newer release exists. There are
no maintenance branches. SECURITY.md carries the same statement.

## What this case does not cover

Physical safety of a direct-drive wheel is a hardware matter. The software
can only bound it, not remove it. The rules for testing with hands on the
rim are in STATUS.md. A compromised kernel or a root-level attacker on the
machine is out of scope, as it is for any driver.
