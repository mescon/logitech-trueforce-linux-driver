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
- If you cannot use that, email the maintainer at
  `5875228+mescon@users.noreply.github.com` with "SECURITY" in the subject.

Please include the affected component (kernel module, a userspace tool, or a
packaging script), the kernel and distribution, and the steps to reproduce.
This is a driver that runs in the kernel and userspace helpers that touch
device nodes, so a clear reproduction helps triage quickly. You can expect an
acknowledgement within a few days, and a fix or a plan once the report is
confirmed.
