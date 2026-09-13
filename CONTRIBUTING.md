# Contributing

Everything useful this project knows about the wheels came from people who
were not its maintainer. This page says how to take part, what a change
needs before it can land, and what the project will do with what you send.

## Ways to contribute

- **Report a problem.** Open an issue. Run `tools/setup.sh report` first and
  paste its block: it prints the kernel, distribution, driver and app
  versions, the wheel's identity and the last launcher log in one go, and
  those are the questions you would otherwise be asked. For force-feedback
  or TrueForce problems, a USB capture from `tools/linux_game_capture.sh`
  (or `tools/windows-usb-capture.bat` on Windows) usually settles the
  question in one round; the capture script's own text says what it
  records.
- **Test on hardware the maintainer does not own.** Most wheel variants and
  most games are verified only through reports. Say what you ran, on which
  build (`logi-tf-sim --version` prints the build stamp), and what you
  felt; "works" and "does not work" are both useful when they name the
  build.
- **Send code.** Open a pull request against `master`. Small, single-purpose
  changes land fast; a large change is better discussed in an issue first.
- **Improve the documentation**, in the repository or the wiki.

Everyone who helps is credited in [CREDITS.md](CREDITS.md).

## What a code change needs

- **Sign your commits off.** Every commit carries a `Signed-off-by:` line
  (`git commit -s`), which is your statement under the
  [Developer Certificate of Origin 1.1](https://developercertificate.org/)
  that you are entitled to submit the work under this project's license.
  There is no contributor license agreement.
- **Tests come with the change.** A change that adds functionality carries
  the tests for it; a bug fix carries a test that would have caught the
  bug. The Rust workspace tests run with `cargo test --workspace` in
  `userspace/logi-wheel`; the driver's arithmetic has header-only C
  harnesses under `tests/` that build with a plain C compiler. CI runs all
  of it on every push and every pull request, and a pull request must be
  green to merge.
- **Coding standards.** The kernel driver follows the Linux kernel coding
  style; CI runs `checkpatch.pl` over it. The Rust workspace builds with
  `cargo clippy --workspace --all-targets -- -D warnings`, and a warning is
  a failure. There is no `rustfmt` step by choice: the workspace is
  hand-formatted and a reformat would rewrite files for no gain. Shell
  scripts are POSIX where they can be and `bash` where they must; each
  one says which at the top.
- **Write down why.** Commit messages explain the reasoning, not just the
  diff, and `CHANGELOG.md` gets an entry under `Unreleased` for anything a
  user would notice. Plain language; the changelog is read by people who
  do not know the code.
- **No new binaries.** The three Windows binaries under `tools/` are built
  by their scripts from committed source and byte-verified in CI; nothing
  else compiled belongs in the tree.
- **Dependencies.** A new Rust dependency needs a reason in the pull
  request; it must be GPL-2.0-compatible and maintained. Dependabot and
  `cargo audit` watch what is already there; see
  [docs/SECURITY_ASSURANCE.md](docs/SECURITY_ASSURANCE.md) for the policy.
- **Hardware safety.** A direct-drive wheel produces up to 8 Nm. Anything
  that changes force output is tested with hands clear of the rim first,
  and the consent rules in [docs/STATUS.md](docs/STATUS.md) apply to any
  test that needs a person at the wheel.

## Review and merging

The maintainer reviews every pull request, usually within a few days, and
merges it once CI is green and the points above are met. Disagreements are
settled in the pull request, in writing. The maintainer's own changes go
through the same CI; see [GOVERNANCE.md](GOVERNANCE.md) for the rest of
how the project is run and [SECURITY.md](SECURITY.md) for anything that
should not be public.

## Code of conduct

Participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).
