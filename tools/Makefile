# Windows-side helpers, built with a mingw cross compiler.
#
# These are not part of the kernel module or the Rust workspace; they are the
# pieces that have to run inside a Wine/Proton prefix alongside the game.
#
#   tf-range-proxy.dll  answers the SDK's rotation question from sysfs and
#                       forwards everything else to Logitech's own library
#   tf-pipe-probe.exe   listens on the pipe the SDK looks for, for diagnosis
#
# Requires: mingw-w64-gcc (Arch: pacman -S mingw-w64-gcc)
#
# tf-range-proxy.dll is COMMITTED to the repository, unlike everything else
# built here. It has to be: it is installed into a prefix by
# install-tf-shim.sh --range-proxy, and the people who need it are running
# Linux without a Windows cross compiler. If you change tf-range-proxy.c,
# run make here and commit the rebuilt DLL with it, or the shipped binary
# and the source it claims to be drift apart. CI compares commit ages
# (check-committed-dlls.sh --check); before committing, run
# check-committed-dlls.sh --verify-bytes to prove the DLL you built
# really is this source.

CC_WIN := x86_64-w64-mingw32-gcc
CFLAGS_WIN := -O2 -Wall

all: tf-range-proxy.dll tf-pipe-probe.exe

# -lws2_32: the proxy copies the game's own TrueForce to logi-tf-sim over
# localhost UDP, so it needs winsock. Wine implements it.
tf-range-proxy.dll: tf-range-proxy.c tf-range-proxy.def
	$(CC_WIN) $(CFLAGS_WIN) -shared -o $@ tf-range-proxy.c tf-range-proxy.def -lws2_32

tf-pipe-probe.exe: tf-pipe-probe.c
	$(CC_WIN) $(CFLAGS_WIN) -o $@ $<

clean:
	rm -f tf-range-proxy.dll tf-pipe-probe.exe

.PHONY: all clean
