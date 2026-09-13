# Security Policy

## Supported versions

Security fixes land on `master` and ship in the next tagged release. The
latest release is the supported one; there are no separate maintenance
branches for older versions.

## Reporting a vulnerability

Please report suspected vulnerabilities privately, not in a public issue.

- Preferred: open a private report through GitHub's **Security** tab on this
  repository (**Report a vulnerability**), which opens a private advisory
  only the maintainer can see.
- If you cannot use that, open an ordinary issue that says only that you
  have a security report and how the maintainer can reach you privately;
  do not put the details in the issue.

Please include the affected component (kernel module, a userspace tool, or a
packaging script), the kernel and distribution, and the steps to reproduce.
This is a driver that runs in the kernel and userspace helpers that touch
device nodes, so a clear reproduction helps triage quickly.

## What happens next

Coordinated disclosure, on this timetable:

- **Acknowledgement within 7 days** of the report.
- **Assessment within 14 days**: whether it is a vulnerability, which
  versions it affects, and how severe it is. You are told the outcome
  either way.
- **A fix on master and a release within 60 days** of confirmation, sooner
  for anything that lets another process drive the wheel or corrupt kernel
  memory. If that cannot be met, you are told why and when.
- **Publication** with the release: a GitHub security advisory on this
  repository (with a CVE requested through it when one applies), the entry
  in `CHANGELOG.md` naming the fix and the advisory, and the release notes.
  Details stay private until the fix is out, unless the report is already
  public.
- **Credit** to the reporter in the advisory, the changelog and
  [CREDITS.md](CREDITS.md), unless they ask not to be named.

Published vulnerability data lives in the repository's Security tab
(advisories) and in `CHANGELOG.md`; dependency advisories that do not
affect the project are recorded with their reasons in
[security/openvex.json](security/openvex.json).

## Supported versions, precisely

Only the latest release is supported. A release stops receiving security
updates the moment a newer release is published; there are no maintenance
branches and no backports. Distribution packages follow the same rule:
the packaged version is the latest release, and an older packaged version
is unsupported once its successor is in the channel.

The security design this policy sits on, including the threat model, is in
[docs/SECURITY_ASSURANCE.md](docs/SECURITY_ASSURANCE.md).
