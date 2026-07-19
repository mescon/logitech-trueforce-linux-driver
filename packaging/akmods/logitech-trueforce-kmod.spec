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
# logi-dd-gui's yeslogic-fontconfig-sys dependency links fontconfig/freetype
# at build time (build.rs calls pkg_config::find_library, no dlopen), so the
# devel package and pkg-config must be present or `cargo build` panics and
# aborts the whole %build. pkgconfig(fontconfig) pulls both on Fedora.
BuildRequires:  pkgconfig(fontconfig)

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

# Kernel-independent shared files (both udev rules) go in a noarch -common
# package - the kmod-%%{kmod_name}-<kver> subpackages that kmodtool
# generates only carry the .ko. kmodtool wires each generated
# kmod-%%{kmod_name}-<kver> package to Require this -kmod-common package.
%package -n %{kmod_name}-kmod-common
Summary:        udev rules for the %{kmod_name} kernel module
BuildArch:      noarch
# The pre-split -tools package shipped everything; recommending logi-dd
# keeps "install the driver, get the ecosystem" while still allowing a
# lean module-only install.
Recommends:     logi-dd

%description -n %{kmod_name}-kmod-common
udev rules granting the "input" group read/write access to the wheel's
wheel_* sysfs attributes and hidraw nodes (so settings do not need root)
and to /dev/uhid (which logi-ffb needs to create its virtual
force-feedback device).

%files -n %{kmod_name}-kmod-common
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules

# Userspace companions are ordinary compiled binaries (arch-specific, not
# tied to a kernel version), so they get their own subpackages rather than
# joining the noarch -common package or the per-kernel kmod packages.
# logi-dd is the complete headless install: driver <- logi-dd <-
# logi-dd-gui.
%package -n logi-dd
Summary:        Terminal tools for the %{kmod_name} direct-drive wheel driver
License:        GPL-2.0-only
BuildRequires:  cargo, rust
Requires:       %{kmod_name}-kmod-common
# The shim installer edits the wine prefix registry with python3.
Recommends:     python3

%description -n logi-dd
The complete headless toolset for the Logitech direct-drive wheel driver:
logi-dd, a terminal settings UI, logi-ffb, a DirectInput force-feedback
proxy, logi-tf-sim, a simulated-TrueForce daemon driven by game telemetry,
and logitech-trueforce-install-shim, the TrueForce SDK shim installer for
Proton prefixes.

%files -n logi-dd
%{_bindir}/logi-dd
%{_bindir}/logi-ffb
%{_bindir}/logi-tf-sim
%{_bindir}/logitech-trueforce-install-shim

%package -n logi-dd-gui
Summary:        Graphical settings app for the %{kmod_name} direct-drive wheel driver
License:        GPL-3.0-or-later
Requires:       logi-dd
# Owns the hicolor icon directories the GUI's launcher icon lands in.
Requires:       hicolor-icon-theme
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

%description -n logi-dd-gui
logi-dd-gui, a graphical settings app (GPL-3.0-or-later, with a desktop
menu entry) for the Logitech direct-drive wheel driver: wheel settings,
LIGHTSYNC, response curves, game-helper setup pages, and a test section.

%files -n logi-dd-gui
%{_bindir}/logi-dd-gui
%{_datadir}/applications/logi-dd-gui.desktop
%{_datadir}/icons/hicolor/scalable/apps/logi-dd-gui.svg

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

# Shared, kernel-independent bits (both udev rules) ship in -kmod-common.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    "%{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules"
install -D -m 0644 udev/71-logi-ffb-uhid.rules \
    "%{buildroot}%{_prefix}/lib/udev/rules.d/71-logi-ffb-uhid.rules"

# Headless toolset (the logi-dd package).
install -D -m 0755 userspace/logi-dd/target/release/logi-dd \
    "%{buildroot}%{_bindir}/logi-dd"
install -D -m 0755 userspace/logi-dd/target/release/logi-ffb \
    "%{buildroot}%{_bindir}/logi-ffb"
install -D -m 0755 userspace/logi-dd/target/release/logi-tf-sim \
    "%{buildroot}%{_bindir}/logi-tf-sim"
# TrueForce-in-Proton shim installer (no-op without the proprietary SDK
# DLLs; resolves the SDK dir via --sdk-dir / $LOGITECH_TRUEFORCE_SDK_DIR /
# ~/.local/share/logitech-trueforce/sdk).
install -D -m 0755 tools/install-tf-shim.sh \
    "%{buildroot}%{_bindir}/logitech-trueforce-install-shim"

# The GUI + its desktop integration (the logi-dd-gui package).
install -D -m 0755 userspace/logi-dd/target/release/logi-dd-gui \
    "%{buildroot}%{_bindir}/logi-dd-gui"
install -D -m 0644 desktop/logi-dd-gui.desktop \
    "%{buildroot}%{_datadir}/applications/logi-dd-gui.desktop"
install -D -m 0644 desktop/logi-dd-gui.svg \
    "%{buildroot}%{_datadir}/icons/hicolor/scalable/apps/logi-dd-gui.svg"

%changelog
* Sat Jul 18 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.15.0-1
- Ship the userspace ecosystem as layered subpackages: logi-dd (settings
  TUI, logi-ffb DirectInput force-feedback proxy, logi-tf-sim
  simulated-TrueForce daemon, and the TrueForce SDK shim installer;
  requires the driver's -kmod-common, which now carries both udev rules)
  and logi-dd-gui (graphical settings app, GPL-3.0-or-later, with desktop
  entry, icon, and the GUI's windowing/rendering runtime dependencies;
  requires logi-dd). Built from the userspace/logi-dd Rust workspace.

* Thu Jul 09 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.12.1-1
- kmod package for atomic distros (Bazzite/Silverblue/Kinoite). Verified on
  Fedora Silverblue 44: builds in a toolbox, layers with rpm-ostree, and the
  module loads on the running kernel.
