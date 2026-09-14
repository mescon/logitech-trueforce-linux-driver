# Security assurance

What this software is exposed to, what it does about it, and why the
maintainer believes that is enough. This is the assurance case the
project's security work is measured against; the reporting process is in
[../SECURITY.md](../SECURITY.md), and the rules for people and credentials
are in [../GOVERNANCE.md](../GOVERNANCE.md).

## What runs where

- **hid-logitech-dd**, a kernel module. It binds the wheel's USB HID
  interfaces, parses every report the wheel sends, sends HID++ commands and
  the TrueForce stream, and exposes settings through sysfs. It runs with
  kernel privilege.
- **logi-wheel, logi-wheel-gui**, settings apps. They read and write sysfs
  attributes and read `/dev/hidraw` for diagnostics. They run as the user.
- **logi-tf-sim**, the telemetry daemon. It listens on UDP ports on the
  loopback interface for game telemetry, synthesises haptics, and writes
  them to the wheel through hidraw or the kernel's sysfs. It runs as the
  user.
- **logi-ffb**, the DirectInput proxy. It creates a virtual HID device
  through `/dev/uhid`, receives force-feedback reports from a Wine process,
  and replays them on the real wheel's evdev node. It runs as the user.
- **logi-launch** and the Windows helpers it stages into a game's prefix
  (the escape proxy, the telemetry relay). They run inside the game's Wine
  process, with the game's own privileges.

## Attack surface and threat model

Who could attack, through what, and what the consequence would be.

| Source | Reaches | Worst plausible outcome | What stands in the way |
|---|---|---|---|
| **A malicious or faulty USB device** presenting Logitech's ids | The kernel module's report parsers and HID++ reply handling | Kernel memory corruption; a crash | Every report is length-checked before a field is read; feature indexes are resolved from the device's own tables and bounds-checked; replies are matched to requests by sequence and validated; the driver is built under the kernel's `-Wall`, checked by sparse, smatch and CodeQL, and its lifetime-sensitive paths are exercised under a KASAN, lockdep and kmemleak kernel in the development VM. Physical access to the USB port is required. |
| **A game, or anything else on the machine, sending UDP telemetry** to the daemon's loopback ports | The telemetry decoders and the relay datagram parser | A daemon crash (loss of haptics) or, at worst, wrong forces | The daemon only binds loopback; every decoder validates length, magic and version before reading a field, rejects samples with no usable engine data, and is fuzzed with cargo-fuzz; forces are clamped by the wheel's strength setting and the kernel's limits, and the wheel's own firmware saturates torque. |
| **A Wine process** writing force-feedback reports to the virtual wheel | The PID report decoder in logi-ffb | A proxy crash (loss of force) or wrong forces | The decoder validates report ids, lengths and block indexes and is fuzzed; effects are translated into the kernel's own force-feedback interface, which bounds them; the proxy runs as the user, with no more access than the game itself has. |
| **A game's prefix** where the Windows helpers run | The escape proxy and relay, and through them the wheel's sysfs range | Wrong rotation range; a stalled game | The helpers only read the game's shared memory and write telemetry to loopback; the one write they perform on the wheel (the rotation range the SDK announces) is bounds-checked to the range a wheel accepts, and can be switched off. |
| **Another local user** | The wheel's sysfs attributes, which the udev rule makes world-writable | Changed settings; a changed rotation range while someone is driving | Accepted risk, stated here: the attributes are world-writable so that settings apps and games work without root, and a machine with a racing wheel attached is assumed to be single-user. No attribute grants any privilege beyond the wheel itself. |
| **A dependency or a build tool** | Everything | Compromised releases | Dependencies are pinned by `Cargo.lock`, updated by Dependabot, checked by `cargo audit` on every push and weekly; GitHub Actions are pinned to commit hashes; workflow tokens are read-only by default; releases are signed, carry SLSA provenance for every asset, and the signing key's fingerprint is published. |
| **The maintainer's accounts and secrets** | Releases and channels | Compromised releases | Two-factor authentication is required for write access; secrets live only in GitHub Actions and are rotated on any suspicion; the rules are in GOVERNANCE.md. |

The critical code paths, in order of consequence, are the kernel module's
report parsing (kernel privilege), the daemon's writes to the wheel (physical
force), and the release pipeline (everyone's machine). Each has a specific
mitigation above rather than a general one.

## Secure design principles the project follows

- **Least privilege.** Only the kernel module runs privileged, and only it
  touches the device directly; everything else runs as the user, through
  interfaces the kernel bounds. Workflow tokens are read-only unless a job
  says otherwise.
- **Validate at the boundary.** Every byte from a device, a socket or a HID
  node passes a parser that checks length and structure before any field
  is used, and those parsers are the fuzz targets.
- **Fail closed on force.** When the driver cannot tell what a wheel is
  doing (unreadable range, a lost session, a device that stops answering)
  it stops sending force rather than guessing, and says so in dmesg.
- **One writer per stream.** Two processes writing the TrueForce stream
  corrupt each other's output; the daemon takes a lease, and steps aside
  when a game's own SDK has the wheel. Most of the 0.4x series was about
  enforcing this.
- **Nothing hidden in the build.** The kernel module and the apps carry
  the same git stamp; the installer refuses to leave an app that does not
  match the module; the three committed Windows binaries are byte-verified
  against their source in CI.

## Dependencies and software composition

How dependencies are chosen, obtained and tracked (this is the policy the
Baseline criteria ask for):

- **Selection.** A Rust dependency is added only with a reason in the
  change that adds it, must be GPL-2.0-compatible, and must be maintained.
  The kernel module has no dependencies beyond the kernel.
- **Obtaining.** Dependencies come from crates.io through cargo, pinned in
  `userspace/logi-wheel/Cargo.lock`; distribution packages build from that
  lock with `--locked`. GitHub Actions are pinned to full commit hashes.
- **Tracking.** Dependabot opens weekly update pull requests for cargo and
  for Actions; `cargo audit` runs on every push and weekly against the
  RustSec advisory database and fails the workflow on an advisory.
- **Remediation thresholds.** A vulnerability advisory in a dependency is
  fixed, by updating or replacing the crate, before the next release, and
  within 14 days for anything rated high or critical; a license problem is
  fixed before the next release. Advisories that do not affect the project
  (for example maintenance notices about crates that carry no
  vulnerability) are recorded twice, with the reason each time: in the VEX
  document at [../security/openvex.json](../security/openvex.json) for
  people, and in `userspace/logi-wheel/osv-scanner.toml` for scanners
  (osv-scanner reads that file from the lock file's directory, and so does
  the OpenSSF Scorecard, which runs it). Every scanner entry carries an
  expiry date, so an accepted advisory is reported again when it lapses and
  has to be reviewed again. Both files are reviewed at each release. No
  release is cut with an unrecorded advisory outstanding.
- **Static analysis findings.** A CodeQL or clippy finding of a security
  weakness blocks the change that introduced it: clippy failures fail CI,
  and CodeQL alerts are triaged within 14 days and fixed before the next
  release or dismissed with a written reason on the alert.

## Software bill of materials

Every release from 0.42.0 onward attaches a CycloneDX SBOM for each Rust
binary (the settings apps, the daemon, the proxy and the Windows relay),
generated by the release pipeline from the locked dependency graph. The
kernel module has no third-party components; its bill of materials is the
module source itself, which the packages ship.

## Verifying a release

Release assets are signed with the project's GnuPG key, fingerprint
`4B5B DD78 0272 3B28 9FA9 34CA CD77 C00A 443B 9E79`, published with each
release as `logitech-trueforce-signing-key.asc`: the Arch packages and
repository database up to 0.41.0, every asset from 0.42.0. The README's
"Verifying a release" says how to check one: pacman does it once the key
is trusted; for any other asset, `gpg --verify <asset>.sig <asset>` after
importing the key and checking the fingerprint. The identity behind a
release is that key: a release signed by any other key is not from this
project until the README says the key has changed.

From 0.42.0 each release also carries a SLSA build provenance statement
(`release-assets.intoto.jsonl`) produced by the SLSA generic generator:
the release workflow hashes every attached asset and the generator signs
the list keylessly through Sigstore, recording the repository, the tag and
the workflow that published them. `slsa-verifier` checks an asset against
it, as the README shows. The GnuPG signature says who signed; the
provenance says what built it and from which commit.

## Support and end of support

The latest release is the supported one and receives security fixes; a
release stops receiving them the moment a newer release exists. There are
no maintenance branches. SECURITY.md carries the same statement.

## What this case does not cover

Physical safety of a direct-drive wheel is a hardware matter the software
can only bound, not remove; the rules for testing with hands on the rim are
in STATUS.md. A compromised kernel or a root-level attacker on the machine
is out of scope, as it is for any driver.
