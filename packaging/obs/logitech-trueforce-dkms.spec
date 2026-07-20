# DKMS RPM for the Open Build Service (openSUSE Tumbleweed/Leap and Fedora).
# Main package is noarch: it ships only the module source + udev rules;
# DKMS compiles on the user's machine and rebuilds on kernel updates. The
# same source, dkms.conf, and udev rules as every other channel; the module
# builds as hid-logitech-dd. The userspace companions are ordinary compiled
# binaries, shipped as layered subpackages: driver <- logi-dd (TUI,
# logi-ffb, logi-tf-sim, shim installer; the complete headless install)
# <- logi-dd-gui (graphical settings app + desktop entry).
%global module   logitech-trueforce
%global modver   0.12.1

Name:           logitech-trueforce-dkms
Version:        0.15.0
Release:        1
Summary:        DKMS kernel driver for Logitech TrueForce direct-drive wheels (RS50, G PRO)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        logitech-trueforce-linux-driver-%{version}.tar.gz
# Vendored crate dependencies (produced by `cargo vendor` in the publish
# workflow): OBS builders have no network access, so the Rust workspace
# builds --offline against this instead of index.crates.io.
Source1:        logi-dd-vendor-%{version}.tar.zst
# Not noarch: the logi-dd/logi-dd-gui subpackages ship compiled Rust
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
# logi-dd-gui's yeslogic-fontconfig-sys dependency links fontconfig/freetype
# at build time (build.rs calls pkg_config::find_library, no dlopen), so the
# devel package and pkg-config must be present or `cargo build` panics and
# aborts the whole %build. pkgconfig(fontconfig) pulls both on openSUSE and
# Fedora, no %if split needed.
BuildRequires:  pkgconfig(fontconfig)
Requires:       dkms >= 2.1.0.0
# The pre-split package pulled the userspace tools in hard; recommending
# logi-dd keeps "install the driver, get the ecosystem" while still
# allowing a lean module-only install.
Recommends:     logi-dd
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

# Layered userspace subpackages: driver <- logi-dd (the complete headless
# install) <- logi-dd-gui. logi-ffb/logi-dd/logi-tf-sim are GPL-2.0-only
# (the main package's License); logi-dd-gui is GPL-3.0-or-later.
%package -n logi-dd
Summary:        Terminal tools for the Logitech direct-drive wheel driver
License:        GPL-2.0-only
Requires:       logitech-trueforce-dkms
# The shim installer edits the wine prefix registry with python3.
Recommends:     python3

%description -n logi-dd
The complete headless toolset for the Logitech direct-drive wheel driver:
logi-dd, a terminal settings UI, logi-ffb, a DirectInput force-feedback
proxy, logi-tf-sim, a simulated-TrueForce daemon driven by game telemetry,
and logitech-trueforce-install-shim, the TrueForce SDK shim installer for
Proton prefixes.

%package -n logi-dd-gui
Summary:        Graphical settings app for the Logitech direct-drive wheel driver
License:        GPL-3.0-or-later
Requires:       logi-dd
# Owns the hicolor icon directories the GUI's launcher icon lands in.
Requires:       hicolor-icon-theme
# logi-dd-gui (Slint GUI) runtime stack: windowing (Wayland/X11), input
# (xkbcommon), and GL/EGL rendering. Derived from `ldd`/`strings` on the
# built binary; Slint dlopen's the wayland/X11/GL bits at runtime rather
# than linking them, so ldd alone would miss them. Both openSUSE
# Tumbleweed and Fedora track current Rust, so logi-dd-gui's MSRV (1.92,
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
Requires:       wayland
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

%description -n logi-dd-gui
logi-dd-gui, a graphical settings app (GPL-3.0-or-later, with a desktop
menu entry) for the Logitech direct-drive wheel driver: wheel settings,
LIGHTSYNC, response curves, game-helper setup pages, and a test section.

%prep
%autosetup -n logitech-trueforce-linux-driver-%{version}
# Unpack the vendored crates into the Rust workspace and point cargo at
# them, so %%build resolves every dependency offline.
tar -xf %{SOURCE1} -C userspace/logi-dd
mkdir -p userspace/logi-dd/.cargo
cat > userspace/logi-dd/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
# Nothing to compile here for the DKMS package: DKMS builds the module on
# the target machine. The userspace companions do build here, including
# logi-dd-gui (the Slint GUI): both openSUSE and Fedora ship a rustc new
# enough for its MSRV, so unlike packaging/debian/rules no version guard
# is needed.
# OBS builders have no network access, so every crate dependency comes
# from the vendor tarball unpacked in %%prep; --offline --locked makes any
# accidental network resolution or lockfile drift a hard build error.
# cargo discovers .cargo/config.toml (which redirects crates.io to the
# vendor directory) by walking up from the CWD, not from --manifest-path,
# so build from inside the workspace.
cd userspace/logi-dd
cargo build --release --offline --locked

%install
# Module source DKMS compiles, under /usr/src (the .c keeps its historical
# name; Kbuild emits hid-logitech-dd.ko).
install -d %{buildroot}%{_usrsrc}/%{module}-%{modver}
install -m 0644 mainline/hid-logitech-hidpp.c mainline/hid-ids.h \
    mainline/hidpp_dd_tf_init.h mainline/Kbuild mainline/Makefile \
    %{buildroot}%{_usrsrc}/%{module}-%{modver}/
sed 's/@PKGVER@/%{modver}/' packaging/aur/logitech-trueforce-dkms/dkms.conf \
    > %{buildroot}%{_usrsrc}/%{module}-%{modver}/dkms.conf
echo "v%{modver}" > %{buildroot}%{_usrsrc}/%{module}-%{modver}/.git_hash
# udev rules: hand the wheel's sysfs + hidraw nodes, and /dev/uhid for the
# logi-ffb virtual-device proxy, to the input group. Both ship with the
# driver package.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
install -D -m 0644 udev/71-logi-ffb-uhid.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules
# Headless toolset (the logi-dd subpackage).
install -D -m 0755 userspace/logi-dd/target/release/logi-dd \
    %{buildroot}%{_bindir}/logi-dd
install -D -m 0755 userspace/logi-dd/target/release/logi-ffb \
    %{buildroot}%{_bindir}/logi-ffb
install -D -m 0755 userspace/logi-dd/target/release/logi-tf-sim \
    %{buildroot}%{_bindir}/logi-tf-sim
# TrueForce-in-Proton shim installer (no-op without the proprietary SDK DLLs).
install -D -m 0755 tools/install-tf-shim.sh \
    %{buildroot}%{_bindir}/logitech-trueforce-install-shim
# The GUI + its desktop integration (the logi-dd-gui subpackage).
install -D -m 0755 userspace/logi-dd/target/release/logi-dd-gui \
    %{buildroot}%{_bindir}/logi-dd-gui
install -D -m 0644 desktop/logi-dd-gui.desktop \
    %{buildroot}%{_datadir}/applications/logi-dd-gui.desktop
install -D -m 0644 desktop/logi-dd-gui.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/logi-dd-gui.svg

%files
%license COPYING
%doc README.md docs/GETTING_STARTED.md
%{_usrsrc}/%{module}-%{modver}/
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules

%files -n logi-dd
%{_bindir}/logi-dd
%{_bindir}/logi-ffb
%{_bindir}/logi-tf-sim
%{_bindir}/logitech-trueforce-install-shim

%files -n logi-dd-gui
%{_bindir}/logi-dd-gui
%{_datadir}/applications/logi-dd-gui.desktop
%{_datadir}/icons/hicolor/scalable/apps/logi-dd-gui.svg

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
