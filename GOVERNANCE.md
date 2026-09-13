# Governance

This document says who decides what in this project, how access is granted
and withdrawn, what happens if the maintainer disappears, and how secrets
are handled. It is short because the project is small; it is written down
so that it is not a matter of memory.

## Roles

- **Maintainer.** One person, [mescon](https://github.com/mescon), holds
  the repository, merges changes, cuts releases, signs packages and holds
  the credentials for the package channels. The maintainer decides what
  goes in, and records the reasoning where the change is made: in the
  commit message, in `CHANGELOG.md`, or in the issue.
- **Contributors.** Anyone who opens an issue, sends a pull request, runs a
  test on hardware the maintainer does not own, or supplies a capture. Most
  of what this driver knows about the wheels came this way, and those
  people are credited in [CREDITS.md](CREDITS.md).
- **Reporters of vulnerabilities.** Handled as described in
  [SECURITY.md](SECURITY.md); credited there and in the changelog unless
  they ask not to be.

There is no steering group, no voting and no company behind the project.
Decisions that affect users are discussed in the issue that raised them
before they are made, and a decision the maintainer takes alone is still
explained in writing.

## Becoming a co-maintainer

The project has a bus factor of one and would rather not. A contributor
who has landed several changes, understands the hardware safety rules in
[docs/STATUS.md](docs/STATUS.md) and the release process, and wants the
responsibility can be made a co-maintainer. Before any account is granted
write or administrative access, the maintainer reviews that account's
contribution history and confirms that two-factor authentication is
enabled on it; access is granted at the least level that the role needs,
and withdrawn when the role ends or after a year without activity.

Every account with write access to this repository MUST use two-factor
authentication.

## Continuity

Everything the project needs to continue lives in this public repository:
the source, the packaging recipes for every channel, the documentation,
and the release process itself (`.github/workflows/publish-release.yml`).
The only things not in the repository are the credentials listed under
Secrets below.

If the maintainer becomes unreachable for more than ninety days, the
project is meant to be forked and carried on. A successor cannot inherit
the signing key or the channel credentials and should not try to: they
generate their own key, publish its fingerprint in their README, and
register the packages under their own accounts. Users verify releases by
the published fingerprint, so a change of key is visible to them and is
explained in the release notes.

## How changes land

The maintainer pushes to `master` directly; every push runs the full CI
(kernel builds on several kernels, sparse and smatch, the Rust tests and
clippy with warnings as errors, CodeQL, cargo-audit, the Nix flake). A
change from anyone else arrives as a pull request and is reviewed by the
maintainer before it is merged. Force pushes to and deletion of `master`
are refused by the repository's branch protection. Changes are kept small
and single-purpose; a release is a tag on `master`, and the release
process is documented in the wiki's Building and Contributing page.

The maintainer's own changes do not get a second human review. That is a
known limitation of a one-person project, not a policy choice, and it is
the first thing a co-maintainer would change.

## Secrets

The project holds these credentials, all of them as GitHub Actions
secrets on this repository and nowhere else:

- the GnuPG signing key for release packages and the Arch repository
  database (its public half is attached to every release and its
  fingerprint is in the README),
- the SSH key that pushes to the AUR,
- the Fedora COPR API token,
- the openSUSE OBS credentials.

Rules: secrets are never committed to the repository or written into
workflow files, logs or issues; only the maintainer can read or change
them; a workflow gets a read-only token by default and asks for exactly
the permission a job needs; every secret is rotated when a person with
access to it leaves the project, when the machine that held a copy is
lost or compromised, or on any suspicion of exposure, and the signing key
is replaced with a new fingerprint published in the README and the
release notes. The repository is scanned for credentials by CodeQL, and
the maintainer checks for them before each release.
