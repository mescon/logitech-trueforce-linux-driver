# DKMS RPM for the Open Build Service (openSUSE Tumbleweed/Leap and Fedora).
# Main package is noarch: it ships only the module source + udev rules;
# DKMS compiles on the user's machine and rebuilds on kernel updates. The
# same source, dkms.conf, and udev rules as every other channel; the module
# builds as hid-logitech-dd. The userspace companions are ordinary compiled
# binaries, shipped as layered subpackages: driver <- logi-wheel (TUI,
# logi-ffb, logi-tf-sim, shim installer; the complete headless install)
# <- logi-wheel-gui (graphical settings app + desktop entry).
%global module   logitech-trueforce
%global modver   0.12.1

Name:           logitech-trueforce-dkms
Version:        0.15.0
Release:        1
Summary:        DKMS kernel driver for Logitech racing wheels (RS50, G PRO, G923)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        logitech-trueforce-linux-driver-%{version}.tar.gz
# Vendored crate dependencies (produced by `cargo vendor` in the publish
# workflow): OBS builders have no network access, so the Rust workspace
# builds --offline against this instead of index.crates.io.
Source1:        logi-wheel-vendor-%{version}.tar.zst
# Not noarch: the logi-wheel/logi-wheel-gui subpackages ship compiled Rust
# binaries (rpmlint aborts on binaries in noarch packages); the dkms
# sources riding an arch package is the conventional trade-off.
BuildRequires:  cargo, rust
# owns the hicolor icon directories during the post-build filelist check
BuildRequires:  hicolor-icon-theme
# Extracts the zstd-compressed vendor tarball (Source1) in %%prep.
BuildRequires:  zstd
# logi-tf-sim's build.rs compiles the in-repo libtrueforce.a via make+gcc
# and links it statically (no runtime dependency).
BuildRequires:  gcc, make
# logi-wheel-gui's yeslogic-fontconfig-sys dependency links fontconfig/freetype
# at build time (build.rs calls pkg_config::find_library, no dlopen), so the
# devel package and pkg-config must be present or `cargo build` panics and
# aborts the whole %build. pkgconfig(fontconfig) pulls both on openSUSE and
# Fedora, no %if split needed.
BuildRequires:  pkgconfig(fontconfig)
Requires:       dkms >= 2.1.0.0
# The pre-split package pulled the userspace tools in hard; recommending
# logi-wheel keeps "install the driver, get the ecosystem" while still
# allowing a lean module-only install.
Recommends:     logi-wheel
# Switches an Xbox edition (G923 c26d, RS50 c275) into PC mode (c26e, c276) on plug-in;
# the udev rule that runs it is a no-op without the binary present.
Recommends:     usb_modeswitch
Requires(post): dkms
Requires(preun): dkms
# The user needs kernel headers + a compiler for DKMS to build against.
%if 0%{?suse_version}
Recommends:     kernel-default-devel
Recommends:     gcc make
%else
Recommends:     kernel-devel
Recommends:     gcc make
%endif

%description
Force feedback, TrueForce texture routing, and G HUB-equivalent settings
exposed through sysfs for the Logitech RS50 and G PRO direct-drive racing
wheels. DKMS builds and installs the module (hid-logitech-dd) for the running
kernel and rebuilds it on kernel upgrades.

The module is scoped to the direct-drive wheel USB IDs (c276 RS50 native, c272
G PRO Xbox/PC and RS50 compat, c268 G PRO PS/PC) and coexists with the in-tree
hid-logitech-hidpp driver, which continues to serve every other Logitech
device, so no blacklist is needed.

TrueForce in Proton sims additionally needs Logitech's proprietary signed SDK
DLLs, which are not shipped by this package; see the bundled Getting Started
guide.

# Layered userspace subpackages: driver <- logi-wheel (the complete headless
# install) <- logi-wheel-gui. logi-ffb/logi-wheel/logi-tf-sim are GPL-2.0-only
# (the main package's License); logi-wheel-gui is GPL-3.0-or-later.
%package -n logi-wheel
Summary:        Terminal tools for the Logitech racing wheel driver
License:        GPL-2.0-only
Requires:       logitech-trueforce-dkms
# The shim installer edits the wine prefix registry with python3.
Recommends:     python3
# Renamed from logi-dd (0.20.0): "dd" meant direct-drive, but the app
# configures every supported wheel, including the gear-driven G923. These
# move an existing logi-dd install onto this package automatically.
Provides:       logi-dd = %{version}-%{release}
Obsoletes:      logi-dd < %{version}-%{release}

%description -n logi-wheel
The complete headless toolset for the Logitech direct-drive wheel driver:
logi-wheel, a terminal settings UI, logi-ffb, a DirectInput force-feedback
proxy, logi-tf-sim, a simulated-TrueForce daemon driven by game telemetry,
and logi-shim, the TrueForce SDK shim installer for
Proton prefixes.

%package -n logi-wheel-gui
Summary:        Graphical settings app for the Logitech racing wheel driver
License:        GPL-3.0-or-later
Requires:       logi-wheel
# Owns the hicolor icon directories the GUI's launcher icon lands in.
Requires:       hicolor-icon-theme
# Renamed from logi-dd-gui (0.20.0); see logi-wheel's subpackage for why.
Provides:       logi-dd-gui = %{version}-%{release}
Obsoletes:      logi-dd-gui < %{version}-%{release}
# logi-wheel-gui (Slint GUI) runtime stack: windowing (Wayland/X11), input
# (xkbcommon), and GL/EGL rendering. Derived from `ldd`/`strings` on the
# built binary; Slint dlopen's the wayland/X11/GL bits at runtime rather
# than linking them, so ldd alone would miss them. Both openSUSE
# Tumbleweed and Fedora track current Rust, so logi-wheel-gui's MSRV (1.92,
# from Slint 1.17.1) always builds here; no version guard needed (contrast
# packaging/debian/rules).
%if 0%{?suse_version}
Requires:       libwayland-client0
Requires:       libxkbcommon0
Requires:       libxkbcommon-x11-0
Requires:       libX11-6
Requires:       libX11-xcb1
Requires:       libxcb1
Requires:       libXcursor1
Requires:       libXi6
Requires:       libXrender1
Requires:       Mesa-libEGL1
Requires:       Mesa-libGL1
Requires:       libfontconfig1
Requires:       libfreetype6
%else
# Fedora has no binary package called "wayland" - that is the SOURCE
# package name. The runtime libraries ship as libwayland-client /
# libwayland-cursor / libwayland-egl, so "Requires: wayland" made the
# package uninstallable with "nothing provides wayland" (issue #27).
# packaging/debian/control had this right all along.
Requires:       libwayland-client
Requires:       libwayland-cursor
Requires:       libwayland-egl
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libX11
Requires:       libX11-xcb
Requires:       libxcb
Requires:       libXcursor
Requires:       libXi
Requires:       libXrender
Requires:       mesa-libEGL
Requires:       mesa-libGL
Requires:       fontconfig
Requires:       freetype
%endif

%description -n logi-wheel-gui
logi-wheel-gui, a graphical settings app (GPL-3.0-or-later, with a desktop
menu entry) for the Logitech direct-drive wheel driver: wheel settings,
LIGHTSYNC, response curves, game-helper setup pages, and a test section.

%prep
%autosetup -n logitech-trueforce-linux-driver-%{version}
# Unpack the vendored crates into the Rust workspace and point cargo at
# them, so %%build resolves every dependency offline.
tar -xf %{SOURCE1} -C userspace/logi-wheel
mkdir -p userspace/logi-wheel/.cargo
cat > userspace/logi-wheel/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
# Nothing to compile here for the DKMS package: DKMS builds the module on
# the target machine. The userspace companions do build here, including
# logi-wheel-gui (the Slint GUI): both openSUSE and Fedora ship a rustc new
# enough for its MSRV, so unlike packaging/debian/rules no version guard
# is needed.
# OBS builders have no network access, so every crate dependency comes
# from the vendor tarball unpacked in %%prep; --offline --locked makes any
# accidental network resolution or lockfile drift a hard build error.
# cargo discovers .cargo/config.toml (which redirects crates.io to the
# vendor directory) by walking up from the CWD, not from --manifest-path,
# so build from inside the workspace.
cd userspace/logi-wheel
cargo build --release --offline --locked
cd ../..
# logi-rpm-bridge: the small C bridge that feeds relayed game RPM to the
# driver's kernel texture merge; logi-launch starts and stops it around a
# game session.
gcc %{optflags} -o tools/logi-rpm-bridge tools/logi-rpm-bridge.c

%install
# Module source DKMS compiles, under /usr/src (the .c keeps its historical
# name; Kbuild emits hid-logitech-dd.ko).
install -d %{buildroot}%{_usrsrc}/%{module}-%{modver}
# dd-lg4ff.c/.h carry the ported classic force-feedback engine for the
# G923 (c266/c267); the Kbuild links it into the same hid-logitech-dd.ko.
install -m 0644 mainline/hid-logitech-hidpp.c mainline/dd-lg4ff.c \
    mainline/dd-lg4ff.h mainline/hid-ids.h \
    mainline/hidpp_dd_texture_merge.h \
    mainline/hidpp_dd_tf_init.h mainline/Kbuild mainline/Makefile \
    %{buildroot}%{_usrsrc}/%{module}-%{modver}/
sed 's/@PKGVER@/%{modver}/' packaging/aur/logitech-trueforce-dkms/dkms.conf \
    > %{buildroot}%{_usrsrc}/%{module}-%{modver}/dkms.conf
echo "v%{modver}" > %{buildroot}%{_usrsrc}/%{module}-%{modver}/.git_hash
# udev rules: hand the wheel's sysfs + hidraw nodes, and /dev/uhid for the
# logi-ffb virtual-device proxy, to the input group. All three ship with
# the driver package.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
install -D -m 0644 udev/71-logi-ffb-uhid.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules
# G923 (c266/c267/c26e) driver pre-emption: PID-scoped rebind rule plus a
# softdep/blacklist hint (see the file for why the fork blacklist is safe).
install -D -m 0644 udev/72-logitech-g923-rebind.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/72-logitech-g923-rebind.rules
# Xbox editions (G923 c26d, RS50 c275) boot-mode switch: needs usb_modeswitch
# (Recommends above), a no-op without it.
install -D -m 0644 udev/73-logitech-xbox-modeswitch.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/73-logitech-xbox-modeswitch.rules
install -D -m 0644 packaging/modprobe.d/hid-logitech-dd.conf \
    %{buildroot}%{_sysconfdir}/modprobe.d/hid-logitech-dd.conf

# Headless toolset (the logi-wheel subpackage).
install -D -m 0755 userspace/logi-wheel/target/release/logi-wheel \
    %{buildroot}%{_bindir}/logi-wheel
install -D -m 0755 userspace/logi-wheel/target/release/logi-ffb \
    %{buildroot}%{_bindir}/logi-ffb
install -D -m 0755 userspace/logi-wheel/target/release/logi-tf-sim \
    %{buildroot}%{_bindir}/logi-tf-sim
# Transitional symlink: scripts and habits built around the old logi-dd
# binary name keep working.
ln -s logi-wheel %{buildroot}%{_bindir}/logi-dd
# TrueForce-in-Proton shim installer (no-op without the proprietary SDK DLLs).
install -D -m 0755 tools/install-tf-shim.sh \
    %{buildroot}%{_bindir}/logi-shim
# The rotation proxy that installer stages with --range-proxy. Prebuilt: it
# is a Windows DLL and its users run Linux without a cross-compiler.
install -D -m 0644 tools/tf-range-proxy.dll \
    %{buildroot}%{_datadir}/logitech-trueforce/tf-range-proxy.dll
# The dinput8 escape proxy logi-launch stages into an SDK game's own
# directory: it answers the SDK's range getters and relays the game's RPM
# telemetry for the kernel texture merge. Prebuilt, same reason.
install -D -m 0644 tools/dinput8-escape.dll \
    %{buildroot}%{_datadir}/logitech-trueforce/dinput8-escape.dll
# The RPM feed for the kernel texture merge; logi-launch starts and stops
# it around a game session.
install -D -m 0755 tools/logi-rpm-bridge \
    %{buildroot}%{_bindir}/logi-rpm-bridge
install -D -m 0644 userspace/logi-wheel/target/release/liblogi_tf_scs.so \
    %{buildroot}%{_datadir}/logitech-trueforce/liblogi_tf_scs.so
# A Windows executable: it runs inside the game's Proton prefix.
# Prebuilt because no distro builder ships a Rust Windows target.
install -D -m 0644 tools/logi-tf-relay.exe \
    %{buildroot}%{_datadir}/logitech-trueforce/logi-tf-relay.exe
# The recorded TrueForce init burst logi-launch replays when LOGI_TF_REARM
# is set. Without it that recovery path silently cannot work here alone.
install -D -m 0644 tools/tf-init.bin \
    %{buildroot}%{_datadir}/logitech-trueforce/tf-init.bin
# G923 Xbox mode-switch helper, dispatched by udev rule 73.
install -D -m 0755 tools/xbox-modeswitch.sh \
    %{buildroot}%{_bindir}/logi-wheel-modeswitch
# Rebinds a wheel that another driver claimed, which the settings apps'
# diagnostics offer as a fix. Kept as a script rather than a command in the
# app because a wheel presents several HID interfaces and all of them have
# to be moved.
install -D -m 0755 tools/rebind-wheel.sh \
    %{buildroot}%{_bindir}/logi-rebind-wheel
# Steam launch-options wrapper: starts an in-prefix Windows helper
# (logi-tf-relay, or a telemetry bridge) after the game is up. Useless
# unless it is on PATH, because the whole point is that a user types
# `logi-launch %command%` and nothing else.
install -D -m 0755 tools/logi-launch.sh \
    %{buildroot}%{_bindir}/logi-launch
# Transitional symlink for the pre-v0.22.0 name.
ln -s logi-shim %{buildroot}%{_bindir}/logitech-trueforce-install-shim
# The GUI + its desktop integration (the logi-wheel-gui subpackage).
install -D -m 0755 userspace/logi-wheel/target/release/logi-wheel-gui \
    %{buildroot}%{_bindir}/logi-wheel-gui
install -D -m 0644 desktop/logi-wheel-gui.desktop \
    %{buildroot}%{_datadir}/applications/logi-wheel-gui.desktop
install -D -m 0644 desktop/logi-wheel-gui.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/logi-wheel-gui.svg
# Transitional symlink: scripts and habits built around the old
# logi-dd-gui binary name keep working.
ln -s logi-wheel-gui %{buildroot}%{_bindir}/logi-dd-gui

%files
%license COPYING
%doc README.md
%{_usrsrc}/%{module}-%{modver}/
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules
%{_prefix}/lib/udev/rules.d/72-logitech-g923-rebind.rules
%{_prefix}/lib/udev/rules.d/73-logitech-xbox-modeswitch.rules
%config(noreplace) %{_sysconfdir}/modprobe.d/hid-logitech-dd.conf

%files -n logi-wheel
%{_bindir}/logi-wheel
%{_bindir}/logi-dd
%{_bindir}/logi-ffb
%{_bindir}/logi-tf-sim
%{_bindir}/logi-shim
%dir %{_datadir}/logitech-trueforce
%{_datadir}/logitech-trueforce/tf-range-proxy.dll
# These two lines had drifted into %%install as bare paths (a latent shell
# error there and unpackaged files here); they belong in this list.
%{_datadir}/logitech-trueforce/liblogi_tf_scs.so
%{_datadir}/logitech-trueforce/logi-tf-relay.exe
%{_datadir}/logitech-trueforce/tf-init.bin
%{_datadir}/logitech-trueforce/dinput8-escape.dll
%{_bindir}/logi-rpm-bridge
%{_bindir}/logi-wheel-modeswitch
%{_bindir}/logi-rebind-wheel
%{_bindir}/logi-launch
%{_bindir}/logitech-trueforce-install-shim

%files -n logi-wheel-gui
%{_bindir}/logi-wheel-gui
%{_bindir}/logi-dd-gui
%{_datadir}/applications/logi-wheel-gui.desktop
%{_datadir}/icons/hicolor/scalable/apps/logi-wheel-gui.svg

%post
dkms add -m %{module} -v %{modver} --rpm_safe_upgrade >/dev/null 2>&1 || true
# Build + install for the running kernel if its headers are present; never
# fail the package install if they are not (the user can build later).
if dkms build -m %{module} -v %{modver} >/dev/null 2>&1; then
    dkms install -m %{module} -v %{modver} --force >/dev/null 2>&1 || true
fi

%preun
dkms remove -m %{module} -v %{modver} --all --rpm_safe_upgrade >/dev/null 2>&1 || true

%changelog
* Sun Jul 26 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.20.0-1
- Renamed the userspace subpackages: logi-dd -> logi-wheel, logi-dd-gui ->
  logi-wheel-gui ("dd" meant direct-drive, but the app now also covers the
  gear-driven G923). Provides/Obsoletes on the old names move existing
  installs over automatically.

* Mon Jul 20 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.16.1-1
- Build the Rust workspace offline against vendored crate dependencies
  (new Source1 tarball produced by the publish workflow): OBS builders
  have no network access, so the previous cargo build failed to resolve
  index.crates.io and the repository kept serving stale binaries.

* Sat Jul 18 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.15.0-1
- Ship the userspace ecosystem as layered subpackages: logi-dd (settings
  TUI, logi-ffb DirectInput force-feedback proxy, logi-tf-sim
  simulated-TrueForce daemon, and the TrueForce SDK shim installer;
  requires the driver package, which now carries both udev rules) and
  logi-dd-gui (graphical settings app, GPL-3.0-or-later, with desktop
  entry, icon, and the GUI's windowing/rendering runtime dependencies;
  requires logi-dd). Built from the userspace/logi-dd Rust workspace.
