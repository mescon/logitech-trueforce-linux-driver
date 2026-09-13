# Morning runbook (2026-09-07, written overnight)

Untracked; delete when done. Everything below is committed, pushed, and CI-green
on master (only the slow nix flake build may still be finishing).

## What I did overnight

1. **This machine is on a clean, packaged 0.40.0.** `yay -S
   logitech-trueforce-dkms` installed the 0.40.0 DKMS module (both kernels);
   the loaded module is now the packaged one (version + srcversion match), the
   four apps in /usr/local/bin report 0.40.0, and the eight orphan
   /usr/src/logitech-trueforce-* trees are gone (only 0.40.0 remains).

2. **New security/quality CI, all green:** CodeQL (kernel C), cargo-audit
   (Rust deps, clean), OpenSSF Scorecard, Dependabot. Every GitHub Action is
   pinned to a SHA, read-only workflows declare minimal token permissions, and
   SECURITY.md documents private vulnerability reporting.

3. **README badges (11), every endpoint verified live:** Build, Userspace CI,
   CodeQL, OpenSSF Scorecard, SonarCloud Quality Gate, Latest release, AUR,
   Fedora COPR, openSUSE OBS, Nix flake, License. The COPR and OBS badges show
   real "succeeded" build status.

4. **Fixed a SonarCloud gate failure** my changes surfaced (a read-all token
   scope and an inherited battery sprintf). Quality gate is back to passed.

5. **Verified every distro path delivers 0.40.0** with the exact package names
   the README uses: AUR, Debian .debs, Fedora COPR (akmod-logitech-trueforce +
   logi-wheel-gui), openSUSE OBS, Arch signed repo, NixOS flake. A user on any
   of these can install and it works.

## Two things only you can do (they raise the Scorecard from 5.9)

- **Enable branch protection on `master`** with at least one required review.
  That fixes the Branch-Protection and Code-Review checks (two of the biggest
  remaining zeros) and would push the score toward ~7.5.
- **Enroll at bestpractices.dev** (OpenSSF Best Practices badge) for the
  CII-Best-Practices check. Optional.

I did not touch the publish workflow's token permissions: its archrepo job
uploads and downloads release artifacts on the default token, and restricting
it to chase one Scorecard point could break the release pipeline that just
shipped 0.40.0.

## Still open from earlier (unchanged)

- G923 Xbox #72/#78 (conditions feel), #76/#79 (rev lights) await fenixadam's
  retest on 0.40.0. #74 (AC EVO stutter) is external (Proton hidraw).
- The ACC native path fully works on your RS50 now (steering, force,
  TrueForce, lights, screen); the steer-lock is 2700 = correct DD setup.
