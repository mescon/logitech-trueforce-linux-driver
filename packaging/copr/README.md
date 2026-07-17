# COPR packaging (Fedora / Nobara akmod)

COPR distributes the driver to regular Fedora and derivatives as an **akmod**:
one build serves every kernel, because `akmods` rebuilds the module on the
user's machine whenever the kernel changes. This is the auto-rebuilding
counterpart to the manual static kmod used on atomic distros
(`packaging/akmods/`), built from the same spec.

The akmod build was verified end-to-end on Fedora 43 (kernel 6.18.7): the
akmod installs, `akmods` builds `hid-logitech-dd.ko` for the running kernel,
and it loads and registers the `logitech-dd` driver.

## How the build works

- `packaging/akmods/logitech-trueforce-kmod.spec` builds akmod-only when
  `kernels` is undefined (it passes `kmodtool --akmod`); no kernel-devel is
  needed at build time.
- The same spec also builds a `logitech-trueforce-tools` subpackage with the
  userspace companions, `logi-ffb` (a DirectInput force-feedback proxy) and
  `logi-dd` (a terminal settings UI), from the `userspace/logi-dd` Rust
  workspace. This pulls `cargo`/`rust` into the build dependencies alongside
  `gcc`/`make`/`kernel-rpm-macros`.
- The userspace binaries are built with `cargo`, which needs build-time
  network access to fetch crate dependencies (nothing is vendored), so the
  COPR project must have build networking enabled.
- `.copr/Makefile` is COPR's "make srpm" entrypoint: it builds the source
  tarball from the git checkout and emits the SRPM. COPR rebuilds that SRPM
  per chroot into `akmod-logitech-trueforce` (plus `logitech-trueforce-tools`).

## Automated publishing

Once the project exists (created once via the steps below), every published
GitHub Release rebuilds and submits the akmod automatically:
`.github/workflows/publish-release.yml` stamps the release version into the
spec and runs `copr-cli build mescon/logitech-trueforce` using the repo secret
`COPR_CONFIG`. The steps below are the one-time project setup / manual fallback.

## Publishing (maintainer, needs a Fedora account + COPR API token)

`copr-cli` reads its token from `~/.config/copr` (get it from
https://copr.fedorainfracloud.org/api/). Then, once per project:

```bash
copr-cli create logitech-trueforce \
  --chroot fedora-41-x86_64 --chroot fedora-42-x86_64 --chroot fedora-rawhide-x86_64 \
  --description "Logitech TrueForce direct-drive wheel driver (RS50, G PRO)"
```

Build from this Git repo using the SRPM method (COPR runs `.copr/Makefile`):

```bash
copr-cli buildscm logitech-trueforce \
  --clone-url https://github.com/mescon/logitech-trueforce-linux-driver.git \
  --commit master --spec packaging/akmods/logitech-trueforce-kmod.spec \
  --method make_srpm
```

Or point the COPR web UI at the repo (Builds -> New Build -> SCM, "make srpm").
Enabling automatic rebuilds on new commits via a GitHub webhook is optional.

## What users run

`akmods` lives in RPM Fusion, so users enable that plus this COPR:

```bash
sudo dnf install -y \
  https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm
sudo dnf copr enable <owner>/logitech-trueforce
sudo dnf install akmod-logitech-trueforce logitech-trueforce-tools
```

The first `akmods` run builds the module for the running kernel (and every
kernel installed afterwards). `logitech-trueforce-tools` installs `logi-ffb`
and `logi-dd` to `/usr/bin`, built from the same repo checkout. See
`docs/GETTING_STARTED.md` for the full flow.
