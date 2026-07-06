PREFIX ?= /usr/local
LIBDIR ?= $(PREFIX)/lib
INCDIR ?= $(PREFIX)/include
UDEVDIR ?= /etc/udev/rules.d

CC ?= cc
CFLAGS ?= -O2 -g -Wall -Wextra -Wno-unused-parameter -fPIC
CPPFLAGS ?= -Iinclude
LDFLAGS ?=
LDLIBS := -lpthread

SRCS := src/discovery.c src/exports.c src/kf.c src/session.c src/status.c src/stream.c src/sysfs.c
OBJS := $(SRCS:.c=.o)

SONAME := libtrueforce.so.1
FULL := $(SONAME).3.11
LIB  := libtrueforce.so
STATIC := libtrueforce.a

TESTS := tests/discover tests/init_session tests/sine tests/kf tests/angle tests/unit

.PHONY: all clean install udev-install tests check

all: $(FULL) $(STATIC) tests

$(FULL): $(OBJS)
	$(CC) $(LDFLAGS) -shared -Wl,-soname,$(SONAME) -o $@ $^ $(LDLIBS)
	ln -sf $(FULL) $(SONAME)
	ln -sf $(SONAME) $(LIB)

# Static archive used by the wine PE shim so the resulting .dll.so has
# no DT_NEEDED for libtrueforce.so.1 -- pressure-vessel doesn't expose
# /usr/lib from the host, so per-prefix install needs a self-contained
# shim.
$(STATIC): $(OBJS)
	$(AR) rcs $@ $^

%.o: %.c
	$(CC) $(CFLAGS) $(CPPFLAGS) -c -o $@ $<

tests: $(TESTS)

tests/discover: tests/discover.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -Isrc -L. -o $@ $< -ltrueforce -Wl,-rpath,'$$ORIGIN/..'

tests/init_session: tests/init_session.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -Isrc -L. -o $@ $< -ltrueforce -Wl,-rpath,'$$ORIGIN/..'

tests/sine: tests/sine.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -L. -o $@ $< -ltrueforce -lm -Wl,-rpath,'$$ORIGIN/..'

tests/kf: tests/kf.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -L. -o $@ $< -ltrueforce -Wl,-rpath,'$$ORIGIN/..'

tests/angle: tests/angle.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -L. -o $@ $< -ltrueforce -Wl,-rpath,'$$ORIGIN/..'

tests/unit: tests/unit.c $(FULL)
	$(CC) $(CFLAGS) $(CPPFLAGS) -Isrc -L. -o $@ $< -ltrueforce -Wl,-rpath,'$$ORIGIN/..'

# Pure-logic tests that don't need a wheel attached. Safe in CI.
check: tests/unit
	./tests/unit

install: all
	install -d $(DESTDIR)$(LIBDIR) $(DESTDIR)$(INCDIR)
	install -m 0644 $(FULL) $(DESTDIR)$(LIBDIR)/
	ln -sf $(FULL) $(DESTDIR)$(LIBDIR)/$(SONAME)
	ln -sf $(SONAME) $(DESTDIR)$(LIBDIR)/$(LIB)
	install -m 0644 include/trueforce.h $(DESTDIR)$(INCDIR)/

udev-install:
	install -d $(DESTDIR)$(UDEVDIR)
	install -m 0644 udev/99-logitech-trueforce.rules $(DESTDIR)$(UDEVDIR)/

clean:
	rm -f $(OBJS) $(FULL) $(SONAME) $(LIB) $(STATIC) $(TESTS)
