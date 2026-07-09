# logitech-trueforce kmod/akmod spec for Fedora atomic distros
# (Bazzite, Silverblue, Kinoite) where DKMS does not work (the immutable
# layout makes /var/lib/dkms read-only during the rpm-ostree transaction).
#
# ############################################################################
# ##  UNTESTED. This is a starting scaffold, NOT a verified package.        ##
# ##  It has not been built or loaded on a real Bazzite/atomic system.      ##
# ##  It needs a build-and-load test cycle (matching kernel-devel, akmods   ##
# ##  rebuild on kernel change, module actually binding the wheel) before   ##
# ##  it can be advertised as supported. See the atomic-distro note in      ##
# ##  docs/GETTING_STARTED.md. Contributions from a Bazzite owner welcome.  ##
# ############################################################################
#
# Modeled on the RPM Fusion kmodtool convention. Builds the same scoped
# module the DKMS package does: hid-logitech-dd.ko, from mainline/ via the
# repo Kbuild (which renames the object to hid-logitech-dd and trims the
# id_table to the direct-drive wheel PIDs).

%global kmod_name       logitech-trueforce
%global upstream_ver    0.12.0

Name:           %{kmod_name}-kmod
Version:        %{upstream_ver}
Release:        1%{?dist}
Summary:        Kernel module for Logitech TrueForce direct-drive wheels (RS50, G PRO)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        %{url}/archive/refs/tags/v%{upstream_ver}.tar.gz#/%{name}-%{upstream_ver}.tar.gz

BuildRequires:  kmodtool
BuildRequires:  gcc, make, kernel-rpm-macros
# Pulls in the kernel-devel set to build against (RPM Fusion buildsys macro).
%{!?kernels:BuildRequires: buildsys-build-rpmfusion-kerneldevpkgs-current}

# kmodtool emits the per-kernel kmod-%{kmod_name}-<kver> subpackages and the
# akmod-%{kmod_name} package that rebuilds on kernel change.
%{expand:%(kmodtool --target %{_target_cpu} --kmodname %{kmod_name} %{?kernels:--for-kernels "%{?kernels}"} 2>/dev/null)}

%description
Out-of-tree kernel module (hid-logitech-dd) for Logitech direct-drive
racing wheels: force feedback, TrueForce texture routing, and G Hub-
equivalent settings via sysfs. Scoped to the direct-drive wheel USB IDs
(RS50 c276, G PRO c272/c268); it coexists with the in-tree
hid-logitech-hidpp, which continues to serve every other Logitech device.

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

%install
for kver in %{?kernel_versions}; do
    install -D -m 0644 _kmod_build_${kver%%___*}/hid-logitech-dd.ko \
        "%{buildroot}%{kmodinstdir_prefix}/${kver%%___*}/%{kmodinstdir_postfix}/hid-logitech-dd.ko"
done
%{?akmod_install}

# Shared, kernel-independent bits (udev rule) ship in the -kmod-common package.
install -D -m 0644 udev/70-logitech-trueforce.rules \
    "%{buildroot}%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules"

%changelog
* Wed Jul 09 2026 mescon <5875228+mescon@users.noreply.github.com> - 0.12.0-1
- Initial UNTESTED akmods scaffold for atomic distros (Bazzite/Silverblue).
