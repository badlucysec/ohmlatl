#!/bin/sh
# Keep only busybox, cryptsetup and e2fsck in the initramfs, with the loader and the
# libraries they need: PT_INTERP plus the recursive closure of DT_NEEDED.
set -eu
T=$1
keep=$(mktemp)
trap 'rm -f "$keep"' EXIT

findlib() {
	for d in /lib /usr/lib; do
		if [ -e "$T$d/$1" ] || [ -L "$T$d/$1" ]; then echo "$d/$1"; return; fi
	done
	echo "post-build: library $1 not found" >&2
	exit 1
}

need() {
	[ -e "$T$1" ] || [ -L "$T$1" ] || { echo "post-build: $1 is missing" >&2; exit 1; }
	grep -qxF "$1" "$keep" && return 0
	echo "$1" >> "$keep"
	if [ -L "$T$1" ]; then
		l=$(readlink "$T$1")
		case $l in /*) ;; *) l=$(dirname "$1")/$l ;; esac
		need "$(realpath -ms "$l")"
		return
	fi
	interp=$(readelf -l "$T$1" 2>/dev/null | sed -n 's/.*program interpreter: \(.*\)]$/\1/p')
	[ -z "$interp" ] || need "$interp"
	for lib in $(readelf -d "$T$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]$/\1/p'); do
		need "$(findlib "$lib")"
	done
}

need /bin/busybox
need /usr/sbin/cryptsetup
need /sbin/e2fsck

is_applet() { [ -L "$1" ] && [ "$(basename "$(readlink "$1")")" = busybox ]; }

for d in bin sbin usr/bin usr/sbin usr/libexec; do
	[ -d "$T/$d" ] && [ ! -L "$T/$d" ] || continue
	find "$T/$d" \( -type f -o -type l \) | while read -r f; do
		p=${f#"$T"}
		grep -qxF "$p" "$keep" || is_applet "$f" || rm -f "$f"
	done
done
for d in lib usr/lib; do
	[ -d "$T/$d" ] && [ ! -L "$T/$d" ] || continue
	find "$T/$d" \( -type f -o -type l \) \( -name '*.so*' -o -name '*.a' -o -name '*.la' \) | while read -r f; do
		grep -qxF "${f#"$T"}" "$keep" || rm -f "$f"
	done
done
echo "post-build: kept $(wc -l < "$keep") files:"
sed 's/^/  /' "$keep"
