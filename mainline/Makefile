KVERSION := $(shell uname -r)
KDIR := /lib/modules/$(KVERSION)/build

# Module definition lives in Kbuild (obj-m, ccflags-y). This file is the
# user-facing wrapper: build/clean/install/load helpers and compiler
# auto-detection.

# Auto-detect kernel compiler and match it if available
# User can override with: make CC=gcc
ifeq ($(origin CC),default)
  KERNEL_CLANG := $(shell grep -s "^CONFIG_CC_IS_CLANG=y" $(KDIR)/.config)
  CLANG_AVAIL  := $(shell command -v clang >/dev/null 2>&1 && echo 1)
  ifeq ($(KERNEL_CLANG)$(CLANG_AVAIL),CONFIG_CC_IS_CLANG=y1)
    CC := clang
    LLVM := 1
  endif
endif

ifdef LLVM
  MAKE_OPTS := CC=$(CC) LLVM=$(LLVM)
endif

# Opt-in dev-only raw HID++ shell (/sys/.../wheel_hidpp_debug).
# Enable with: make DEBUG=1
ifdef DEBUG
  MAKE_OPTS += CONFIG_HID_LOGITECH_HIDPP_DEBUG=1
endif

all:
	$(MAKE) -C $(KDIR) M=$(PWD) $(MAKE_OPTS) modules

clean:
	$(MAKE) -C $(KDIR) M=$(PWD) clean

install: all
	$(MAKE) -C $(KDIR) M=$(PWD) modules_install
	depmod -a

load: all
	-sudo rmmod hid-logitech-dd 2>/dev/null || true
	sudo insmod ./hid-logitech-dd.ko

unload:
	-sudo rmmod hid-logitech-dd 2>/dev/null || true
