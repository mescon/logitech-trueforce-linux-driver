#!/bin/sh
# Put hid-logitech-dd into the initramfs, and regenerate it.
#
# Why this exists (#90). The in-tree hid-logitech-hidpp driver claims the
# G923 too, and whichever of the two registers first owns the wheel.
# packaging/modprobe.d/hid-logitech-dd.conf makes loading the in-tree
# module pull ours in first, but only when both can be loaded: on a
# system whose initramfs carries the in-tree module (mkinitcpio's
# autodetect adds it whenever a G923 was plugged in at build time) and
# not ours, the in-tree driver binds the wheel a second after boot and
# the udev rebind rule has to take it back later. That unbind is the
# one the in-tree driver cannot survive while plymouth or Steam holds
# the wheel's event node open. So ours goes into the initramfs as well,
# together with the modprobe.d file the generators copy in, and the
# order holds from the first second.
#
# Installed as /usr/bin/logi-wheel-initramfs. Run by the packages after
# the module is built, by tools/dkms-update.sh, and by hand:
#
#   logi-wheel-initramfs           write the configuration if missing,
#                                  regenerate when it changed
#   logi-wheel-initramfs --force   regenerate even if nothing changed
#   logi-wheel-initramfs --check   report the state, change nothing;
#                                  exit 1 when the configuration is absent
#
# Knows mkinitcpio (Arch and derivatives), dracut (Fedora, openSUSE,
# others) and initramfs-tools (Debian, Ubuntu). Anything else: nothing
# to do, exit 0, and say so. Regeneration is skipped, with a note, when
# the module is not built for the running kernel: listing a module that
# does not exist makes mkinitcpio print an error, and the next kernel
# update regenerates the image with the module in place anyway.
set -u

MOD=hid-logitech-dd
mode="${1:-}"
case "$mode" in
	""|--force|--check) ;;
	*) echo "usage: $0 [--force | --check]" >&2; exit 2 ;;
esac

if command -v mkinitcpio >/dev/null 2>&1; then
	gen=mkinitcpio
	conf=/etc/mkinitcpio.conf.d/logitech-trueforce.conf
	want="MODULES+=($MOD)"
elif command -v dracut >/dev/null 2>&1; then
	gen=dracut
	conf=/etc/dracut.conf.d/logitech-trueforce.conf
	want="add_drivers+=\" $MOD \""
elif command -v update-initramfs >/dev/null 2>&1; then
	gen=initramfs-tools
	conf=/etc/initramfs-tools/modules
	want="$MOD"
else
	echo "no known initramfs generator here (mkinitcpio, dracut, initramfs-tools); nothing to do"
	exit 0
fi

# Present when the wanted line is in the file (the initramfs-tools list is
# shared with other entries, so it is a line match there, a whole-file
# match elsewhere).
present() {
	[ -f "$conf" ] && grep -qxF "$want" "$conf"
}

if [ "$mode" = "--check" ]; then
	if present; then
		echo "$MOD is listed for $gen in $conf"
		exit 0
	fi
	echo "$MOD is not listed for $gen ($conf); the in-tree driver may bind a G923 first at boot"
	exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
	echo "$0 needs root to write $conf and regenerate the initramfs" >&2
	exit 1
fi

changed=0
if ! present; then
	mkdir -p "$(dirname "$conf")"
	case "$gen" in
		initramfs-tools)
			printf '%s\n' "$want" >> "$conf"
			;;
		*)
			printf '%s\n' \
				"# Installed by logitech-trueforce (logi-wheel-initramfs). Puts the" \
				"# hid-logitech-dd module into the initramfs so it registers before the" \
				"# in-tree hid-logitech-hidpp driver can claim a G923 at boot (#90)." \
				"$want" > "$conf"
			;;
	esac
	echo "listed $MOD for $gen in $conf"
	changed=1
fi

if [ "$changed" -eq 0 ] && [ "$mode" != "--force" ]; then
	echo "$MOD already listed for $gen in $conf; initramfs left as it is"
	exit 0
fi

if ! modinfo -n "$MOD" >/dev/null 2>&1; then
	echo "$MOD is not built for the running kernel ($(uname -r)); not regenerating now."
	echo "The next kernel update, or a later 'logi-wheel-initramfs --force', puts it in the image."
	exit 0
fi

echo "regenerating the initramfs with $gen (this takes a moment)"
case "$gen" in
	mkinitcpio)      mkinitcpio -P ;;
	dracut)          dracut --regenerate-all --force ;;
	initramfs-tools) update-initramfs -u -k all ;;
esac
