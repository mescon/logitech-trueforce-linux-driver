# DKMS RPM for the Open Build Service (openSUSE Tumbleweed/Leap and Fedora).
# noarch: it ships only the module source; DKMS compiles on the user's machine
# and rebuilds on kernel updates. The same source, dkms.conf, and udev rule as
# every other channel; the module builds as hid-logitech-dd.
%global module   logitech-trueforce
%global modver   0.12.1

Name:           logitech-trueforce-dkms
Version:        0.12.1
Release:        0
Summary:        DKMS kernel driver for Logitech TrueForce direct-drive wheels (RS50, G PRO)
License:        GPL-2.0-only
URL:            https://github.com/mescon/logitech-trueforce-linux-driver
Source0:        logitech-trueforce-linux-driver-%{version}.tar.gz
BuildArch:      noarch
Requires:       dkms >= 2.1.0.0
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

%prep
%autosetup -n logitech-trueforce-linux-driver-%{version}

%build
# Nothing to compile here: DKMS builds the module on the target machine.

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

%files
%license COPYING
%doc README.md docs/GETTING_STARTED.md
%{_usrsrc}/%{module}-%{modver}/
%{_prefix}/lib/udev/rules.d/70-logitech-trueforce.rules
%{_bindir}/logitech-trueforce-install-shim

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
