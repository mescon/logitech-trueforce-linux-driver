# logitech-trueforce kmod/akmod spec for Fedora atomic distros
# (Bazzite, Silverblue, Kinoite) where DKMS does not work (the immutable
# layout makes /var/lib/dkms read-only during the rpm-ostree transaction).
#
# ############################################################################
# ##  VERIFIED on Fedora Silverblue 44 (kernel 7.1.3-200.fc44): builds in a ##
# ##  toolbox, layers with `rpm-ostree install`, and after reboot the       ##
# ##  module loads (modprobe hid-logitech-dd) and registers the logitech-dd ##
# ##  driver with the 3 wheel modaliases. A wheel physically binding was    ##
# ##  not tested in the VM (none attached), but that path is proven on bare ##
# ##  metal. Full user steps: docs/GETTING_STARTED.md.                      ##
# ##  NOTE: a static kmod does NOT auto-rebuild on kernel updates - rebuild ##
# ##  and re-layer after each kernel bump (or publish an akmod/COPR).       ##
# ############################################################################
#
# Modeled on the RPM Fusion kmodtool convention. Builds the same scoped
# module the DKMS package does: hid-logitech-dd.ko, from mainline/ via the
# repo Kbuild (which renames the object to hid-logitech-dd and trims the
# id_table to the direct-drive wheel PIDs).

%global kmod_name       logitech-trueforce
%global upstream_ver    0.15.0
# Out-of-tree kmod: no separate debug/debugsource package.
%global debug_package %{nil}

Name:           %{kmod_name}-kmod
Version:        %{upstream_ver}
Release:        1%{?dist}
Summary:        Kernel module for Logitech TrueForce direct-drive wheels (RS50, G PRO)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        %{url}/archive/refs/tags/v%{upstream_ver}.tar.gz#/%{name}-%{upstream_ver}.tar.gz

BuildRequires:  kmodtool
BuildRequires:  gcc, make, kernel-rpm-macros
# Userspace companions (logi-ffb, logi-dd) built alongside the module.
BuildRequires:  cargo, rust

# Two build modes from one spec, selected by whether `kernels` is defined:
#   * kernels defined      -> compile per-kernel kmod-%%{kmod_name}-<kver>
#     packages for exactly those kernels. This is the toolbox / atomic
#     static-kmod path (see docs/GETTING_STARTED.md section 1a); it needs
#     kernel-devel for each listed kernel but no RPM Fusion buildsys.
#   * kernels NOT defined   -> emit the akmod-%%{kmod_name} package only. It
#     embeds the SRPM and rebuilds on the user's machine via the akmods
#     service, so it needs no kernel-devel at build time. This is the COPR
#     path: one build serves every kernel the user ever runs.
%{expand:%(kmodtool --target %{_target_cpu} --kmodname %{kmod_name} %{?kernels:--for-kernels "%{?kernels}"} %{!?kernels:--akmod} 2>/dev/null)}

%description
Out-of-tree kernel module (hid-logitech-dd) for Logitech direct-drive
racing wheels: force feedback, TrueForce texture routing, and G Hub-
equivalent settings via sysfs. Scoped to the direct-drive wheel USB IDs
(RS50 c276, G PRO c272/c268); it coexists with the in-tree
hid-logitech-hidpp, which continues to serve every other Logitech device.

# Kernel-independent shared files (the udev rule) go in a noarch -common
# package - the kmod-%%{kmod_name}-<kver> subpackages that kmodtool
# generates only carry the .ko.
%package -n %{kmod_name}-kmod-common
Summary:        udev rule for the %{kmod_name} kernel module
BuildArch:      noarch
# kmodtool wires each generated kmod-%%{kmod_name}-<kver> package to Require
# this -kmod-common package, so installing the driver always pulls the
# userspace tools in too.
Requires:       %{kmod_name}-tools

%description -n %{kmod_name}-kmod-common
udev rule granting the "input" group read/write access to the wheel's
wheel_* sysfs attributes and hidraw nodes, so settings do not need root.

%files -n %{kmod_name}-kmod-common
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules

# Userspace companions are ordinary compiled binaries (arch-specific, not
# tied to a kernel version), so they get their own subpackage rather than
# joining the noarch -common package or the per-kernel kmod packages.
# logi-ffb/logi-dd are GPL-2.0-only (the kmod_name-kmod package's license);
# logi-dd-gui is GPL-3.0-or-later, so this subpackage carries both.
%package -n %{kmod_name}-tools
Summary:        Userspace tools for the %{kmod_name} direct-drive wheel driver
License:        GPL-2.0-only AND GPL-3.0-or-later
BuildRequires:  cargo, rust
# logi-dd-gui (Slint GUI) runtime stack: windowing (Wayland/X11), input
# (xkbcommon), and GL/EGL rendering. Derived from `ldd`/`strings` on the
# built binary; Slint dlopen's the wayland/X11/GL bits at runtime rather
# than linking them, so ldd alone would miss them. Fedora tracks current
# Rust, so logi-dd-gui's MSRV (1.92, from Slint 1.17.1) always builds here;
# no version guard needed (contrast packaging/debian/rules).
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

%description -n %{kmod_name}-tools
logi-ffb, a DirectInput force-feedback proxy, logi-dd, a terminal settings
UI, and logi-dd-gui, a graphical settings app (GPL-3.0-or-later), for the
Logitech direct-drive wheel driver. Includes the udev rule granting the
"input" group access to /dev/uhid, which logi-ffb needs to create its
virtual force-feedback device.

%files -n %{kmod_name}-tools
%{_bindir}/logi-ffb
%{_bindir}/logi-dd
%{_bindir}/logi-dd-gui
%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules

%prep
%setup -q -n logitech-trueforce-linux-driver-%{upstream_ver}
# One build tree per target kernel (kmodtool convention).
for kver in %{?kernel_versions}; do
    cp -a mainline _kmod_build_${kver%%___*}
    echo "v%{upstream_ver}" > _kmod_build_${kver%%___*}/.git_hash
done

%build
for kver in %{?kernel_versions}; do
    make -C "${kver##*___}" M="$PWD/_kmod_build_${kver%%___*}" modules
done
# Userspace companions: not kernel-specific, built once regardless of the
# akmod-vs-static-kmod mode selected above. This also builds logi-dd-gui
# (the Slint GUI): Fedora's rustc is always new enough for its MSRV, so
# unlike packaging/debian/rules no version guard is needed here.
# cargo fetches crate dependencies over the network at build time (nothing
# is vendored); enable networking for the COPR project or the build.
cargo build --release --manifest-path userspace/logi-dd/Cargo.toml

%install
for kver in %{?kernel_versions}; do
    install -D -m 0644 _kmod_build_${kver%%___*}/hid-logitech-dd.ko \
        "%{buildroot}%{kmodinstdir_prefix}/${kver%%___*}/%{kmodinstdir_postfix}/hid-logitech-dd.ko"
done
%{?akmod_install}

# Shared, kernel-independent bits (udev rule) ship in the -kmod-common package.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    "%{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules"

# Userspace binaries + their udev rule ship in the -tools package.
install -D -m 0755 userspace/logi-dd/target/release/logi-ffb \
    "%{buildroot}%{_bindir}/logi-ffb"
install -D -m 0755 userspace/logi-dd/target/release/logi-dd \
    "%{buildroot}%{_bindir}/logi-dd"
install -D -m 0755 userspace/logi-dd/target/release/logi-dd-gui \
    "%{buildroot}%{_bindir}/logi-dd-gui"
install -D -m 0644 udev/71-logi-ffb-uhid.rules \
    "%{buildroot}%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules"

%changelog
* Sat Jul 18 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.15.0-1
- Add a logitech-trueforce-tools subpackage with logi-ffb (DirectInput
  force-feedback proxy), logi-dd (settings TUI), and logi-dd-gui (graphical
  settings app, GPL-3.0-or-later), built from the userspace/logi-dd Rust
  workspace, plus the udev rule for /dev/uhid access and the GUI's
  windowing/rendering runtime dependencies.

* Thu Jul 09 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.12.1-1
- kmod package for atomic distros (Bazzite/Silverblue/Kinoite). Verified on
  Fedora Silverblue 44: builds in a toolbox, layers with rpm-ostree, and the
  module loads on the running kernel.
