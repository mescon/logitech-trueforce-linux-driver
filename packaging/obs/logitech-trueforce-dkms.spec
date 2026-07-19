# DKMS RPM for the Open Build Service (openSUSE Tumbleweed/Leap and Fedora).
# Main package is noarch: it ships only the module source; DKMS compiles on
# the user's machine and rebuilds on kernel updates. The same source,
# dkms.conf, and udev rule as every other channel; the module builds as
# hid-logitech-dd. The userspace companions (logi-ffb, logi-dd) are
# ordinary compiled binaries, so they ship in an arch-specific -tools
# subpackage instead of joining the noarch main package.
%global module   logitech-trueforce
%global modver   0.12.1

Name:           logitech-trueforce-dkms
Version:        0.15.0
Release:        1
Summary:        DKMS kernel driver for Logitech TrueForce direct-drive wheels (RS50, G PRO)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        logitech-trueforce-linux-driver-%{version}.tar.gz
BuildArch:      noarch
BuildRequires:  cargo, rust
# logi-dd-gui's yeslogic-fontconfig-sys dependency links fontconfig/freetype
# at build time (build.rs calls pkg_config::find_library, no dlopen), so the
# devel package and pkg-config must be present or `cargo build` panics and
# aborts the whole %build. pkgconfig(fontconfig) pulls both on openSUSE and
# Fedora, no %if split needed.
BuildRequires:  pkgconfig(fontconfig)
Requires:       dkms >= 2.1.0.0
Requires:       logitech-trueforce-tools
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

# logi-ffb/logi-dd are GPL-2.0-only (the main package's License); logi-dd-gui
# is GPL-3.0-or-later, so this subpackage carries both.
%package -n logitech-trueforce-tools
Summary:        Userspace tools for the Logitech direct-drive wheel driver
License:        GPL-2.0-only AND GPL-3.0-or-later
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

%description -n logitech-trueforce-tools
logi-ffb, a DirectInput force-feedback proxy, logi-dd, a terminal settings
UI, and logi-dd-gui, a graphical settings app (GPL-3.0-or-later), for the
Logitech direct-drive wheel driver. Includes the udev rule granting the
"input" group access to /dev/uhid, which logi-ffb needs to create its
virtual force-feedback device.

%prep
%autosetup -n logitech-trueforce-linux-driver-%{version}

%build
# Nothing to compile here for the DKMS package: DKMS builds the module on
# the target machine. The userspace companions do build here, including
# logi-dd-gui (the Slint GUI): both openSUSE and Fedora ship a rustc new
# enough for its MSRV, so unlike packaging/debian/rules no version guard
# is needed.
# cargo fetches crate dependencies over the network at build time (nothing
# is vendored), so the OBS project must allow build-time network access.
cargo build --release --manifest-path userspace/logi-dd/Cargo.toml

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
# udev rule: hand the wheel's sysfs + hidraw nodes to the input group.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
# TrueForce-in-Proton shim installer (no-op without the proprietary SDK DLLs).
install -D -m 0755 tools/install-tf-shim.sh \
    %{buildroot}%{_bindir}/logitech-trueforce-install-shim
# Userspace binaries + their udev rule ship in the -tools subpackage.
install -D -m 0755 userspace/logi-dd/target/release/logi-ffb \
    %{buildroot}%{_bindir}/logi-ffb
install -D -m 0755 userspace/logi-dd/target/release/logi-dd \
    %{buildroot}%{_bindir}/logi-dd
install -D -m 0755 userspace/logi-dd/target/release/logi-dd-gui \
    %{buildroot}%{_bindir}/logi-dd-gui
install -D -m 0644 udev/71-logi-ffb-uhid.rules \
    %{buildroot}%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules

%files
%license COPYING
%doc README.md docs/GETTING_STARTED.md
%{_usrsrc}/%{module}-%{modver}/
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
%{_bindir}/logitech-trueforce-install-shim

%files -n logitech-trueforce-tools
%{_bindir}/logi-ffb
%{_bindir}/logi-dd
%{_bindir}/logi-dd-gui
%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules

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
* Sat Jul 18 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.15.0-1
- Add a logitech-trueforce-tools subpackage with logi-ffb (DirectInput
  force-feedback proxy), logi-dd (settings TUI), and logi-dd-gui (graphical
  settings app, GPL-3.0-or-later), built from the userspace/logi-dd Rust
  workspace, plus the udev rule for /dev/uhid access and the GUI's
  windowing/rendering runtime dependencies.
