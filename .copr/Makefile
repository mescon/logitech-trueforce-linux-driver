# COPR "Make SRPM" build entrypoint.
#
# COPR (Settings -> Build method: "make srpm") clones this repo and runs:
#     make -f .copr/Makefile srpm outdir=<dir>
# We build the source tarball from the checkout and emit an *akmod* SRPM
# (the spec builds akmod-only when `kernels` is undefined). COPR then rebuilds
# that SRPM per chroot into akmod-logitech-trueforce, which rebuilds the module
# on each user's kernel via the akmods service. No per-kernel build on COPR.

SPEC   := packaging/akmods/logitech-trueforce-kmod.spec
VER    := $(shell sed -n 's/^%global[[:space:]]*upstream_ver[[:space:]]*//p' $(SPEC))
PREFIX := logitech-trueforce-linux-driver-$(VER)
SRCDIR := $(HOME)/rpmbuild/SOURCES

srpm:
	dnf -y install rpm-build kmodtool kernel-rpm-macros git >/dev/null
	mkdir -p $(SRCDIR)
	git archive --prefix=$(PREFIX)/ -o $(SRCDIR)/logitech-trueforce-kmod-$(VER).tar.gz HEAD
	rpmbuild -bs $(SPEC) \
		--define "_sourcedir $(SRCDIR)" \
		--define "_srcrpmdir $(outdir)"
