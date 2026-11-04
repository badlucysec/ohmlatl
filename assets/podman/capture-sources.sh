#!/bin/sh
# Runs inside the image of one podman-static stage, after the stage was built.
# Usage: capture-sources.sh <stage> <out dir> <shared aports dir>
#
# Writes to <out dir>:
#   apk-info.txt          `apk info -v` of the stage
#   <name>-<version>.tar  `git archive` of each component built in the stage (and its submodules)
#   static-libs.txt       installed packages that ship static archives, with their aports origin
#   notices/              LICENSE/NOTICE/COPYING of vendored Go modules, `cargo about` JSON for Rust
#   SOURCES               "<file> <sha256>" of each component tarball, compared against podman.lock
# and, once per package, the APKBUILD with its sources to <shared aports dir>/<origin>-<version>/.
set -eu
stage=$1 out=$2 aports=$3
mkdir -p "$out" "$aports"
git config --global --add safe.directory '*' 2>/dev/null || true

apk info -v | sort > "$out/apk-info.txt"

archive() { # <git dir> <name>
	v=$(git -C "$1" describe --tags --always)
	git -C "$1" archive --format=tar --prefix="$2-$v/" HEAD > "$out/$2-$v.tar"
	OUT=$out N=$2 V=$v git -C "$1" submodule --quiet foreach --recursive \
		'git archive --format=tar --prefix="$N-$V/$displaypath/" HEAD > "$OUT/$N-$V-$(echo "$displaypath" | tr / _).tar"'
}

notices() { # <src dir> <name>
	[ -d "$1/vendor" ] || return 0
	mkdir -p "$out/notices/$2"
	(cd "$1" && find vendor -type f \( -iname 'LICENSE*' -o -iname 'NOTICE*' -o -iname 'COPYING*' \) | tar -cf - -T -) \
		| tar -xf - -C "$out/notices/$2"
}

cargo_about() { # <src dir> <name>
	[ -x /opt/cargo-about/bin/cargo-about ] || cargo install --locked --features cli --root /opt/cargo-about cargo-about@0.9.3
	mkdir -p "$out/notices"
	cat > /tmp/about.toml <<'EOF'
accepted = ["Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "BSD-2-Clause", "BSD-3-Clause",
            "ISC", "Zlib", "Unicode-3.0", "Unicode-DFS-2016", "MPL-2.0", "CC0-1.0", "0BSD", "BSL-1.0",
            "Unlicense", "CDLA-Permissive-2.0"]
EOF
	(cd "$1" && /opt/cargo-about/bin/cargo-about generate --format json -c /tmp/about.toml) > "$out/notices/$2-cargo-about.json"
}

# APKBUILD and sources of every installed package that ships a static archive.
apkbuilds() {
	: > "$out/static-libs.txt"
	for p in $(apk info -q | sort); do
		apk info -qL "$p" | grep -q '\.a$' || continue
		set -- $(awk -v p="$p" '/^P:/{n=substr($0,3)} n==p && /^V:/{v=substr($0,3)} n==p && /^o:/{o=substr($0,3)} n==p && /^c:/{c=substr($0,3)} /^$/{if(n==p){print v, o, c; exit}}' /lib/apk/db/installed)
		ver=$1 origin=$2 commit=$3
		echo "$p $ver $origin $commit" >> "$out/static-libs.txt"
		d="$aports/$origin-$ver"
		[ -f "$d/.done" ] && continue
		rm -rf "$d"; mkdir -p "$d"
		for repo in main community; do
			if wget -qO /tmp/aports.tar.gz "https://gitlab.alpinelinux.org/alpine/aports/-/archive/$commit/aports-$commit.tar.gz?path=$repo/$origin" \
				&& tar -xzf /tmp/aports.tar.gz -C "$d" --strip-components=3 2>/dev/null && [ -f "$d/APKBUILD" ]; then
				break
			fi
		done
		[ -f "$d/APKBUILD" ] || { echo "capture-sources: no APKBUILD for $origin@$commit" >&2; exit 1; }
		for s in $(cd "$d" && CARCH=x86_64 CTARGET_ARCH=x86_64 CHOST=x86_64-alpine-linux-musl srcdir=/tmp sh -c '. ./APKBUILD >/dev/null 2>&1; echo $source'); do
			case $s in *://*) ;; *) continue ;; esac
			case $s in *::*) f=${s%%::*} url=${s#*::} ;; *) f=${s##*/} url=$s ;; esac
			wget -qO "$d/$f" "https://distfiles.alpinelinux.org/distfiles/v3.24/$f" || wget -qO "$d/$f" "$url" \
				|| { echo "capture-sources: cannot fetch $url" >&2; exit 1; }
		done
		touch "$d/.done"
	done
}

go=/go/src/github.com/containers
case $stage in
podman) archive $go/podman podman; notices $go/podman podman ;;
conmon) archive /conmon conmon; notices /conmon conmon ;;
netavark) archive /netavark netavark; cargo_about /netavark netavark ;;
aardvark-dns) archive /aardvark-dns aardvark-dns; cp /tmp/aardvark-dns-close_range.patch "$out/"; cargo_about /aardvark-dns aardvark-dns ;;
passt) archive /passt passt ;;
fuse-overlayfs) archive /libfuse libfuse; archive /fuse-overlayfs fuse-overlayfs ;;
catatonit) archive /catatonit catatonit ;;
crun) archive $go/crun crun ;;
*) echo "capture-sources: unknown stage $stage" >&2; exit 1 ;;
esac
apkbuilds
(cd "$out" && for f in *.tar; do echo "$f $(sha256sum "$f" | cut -d' ' -f1)"; done) > "$out/SOURCES"
