# OBS packaging (openSUSE + Fedora via the Open Build Service)

The [Open Build Service](https://build.opensuse.org) builds and hosts repos
from one source. This package gives the driver an **openSUSE Tumbleweed/Leap**
channel (which no other packaging here covers) and a second Fedora channel,
both as a DKMS RPM built from `logitech-trueforce-dkms.spec`.

Verified in an openSUSE Tumbleweed container (kernel 7.1.2-1-default): the RPM
builds and DKMS compiles `hid-logitech-dd.ko` against the openSUSE kernel. The
spec is distro-conditional (`kernel-default-devel` on SUSE, `kernel-devel`
elsewhere), so the same package builds on Fedora targets too.

## Files

- `logitech-trueforce-dkms.spec` - the DKMS RPM (noarch source package; DKMS
  compiles on the user's machine and rebuilds on kernel upgrades).
- `_service` - pulls the tagged source tarball from GitHub. Bump `revision`
  and `version` together on each release.

The userspace binaries (`logi-ffb`, `logi-dd-tui`) are built with `cargo`,
which needs build-time network access to fetch crate dependencies (nothing
is vendored), so the OBS project must have build networking enabled.

## Automated publishing

Once the package exists (created once via the steps below), every published
GitHub Release updates it automatically: `.github/workflows/publish-release.yml`
stamps the release version into the spec + `_service`, regenerates the source
tarball, and `osc commit`s to `home:mescon/logitech-trueforce-dkms` using the
repo secret `OSC_CONFIG`. The steps below are the one-time setup / manual
fallback.

## Publishing (maintainer, needs an openSUSE account + `osc` configured)

`osc` reads credentials from `~/.config/osc/oscrc` (run `osc` once to create
it). Then:

```bash
osc checkout home:<user>            # your OBS home project
cd home:<user>
osc mkpac logitech-trueforce-dkms
cp /path/to/repo/packaging/obs/{_service,logitech-trueforce-dkms.spec} \
   logitech-trueforce-dkms/
cd logitech-trueforce-dkms
osc service manualrun               # fetch + compress the tarball locally
osc add _service *.spec *.tar.gz
osc commit -m "logitech-trueforce-dkms 0.12.1"
```

In the OBS web UI enable the repositories you want to publish
(openSUSE_Tumbleweed, openSUSE_Leap_15.x, Fedora_41, Fedora_Rawhide, ...); OBS
builds and hosts a `.repo` per target. Users then:

```bash
# openSUSE Tumbleweed example
sudo zypper addrepo https://download.opensuse.org/repositories/home:<user>/openSUSE_Tumbleweed/home:<user>.repo
sudo zypper refresh
sudo zypper install logitech-trueforce-dkms
```

## Extending to Debian/Ubuntu

OBS can also build the Debian/Ubuntu `.deb` in the same project from the
verified packaging in `packaging/debian/` (via OBS `debtransform`). That is not
wired up here because this repo already ships a dedicated, verified Debian
package; add it only if you want a single OBS project to serve every distro.
