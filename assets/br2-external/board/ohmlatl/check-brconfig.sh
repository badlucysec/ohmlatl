#!/bin/sh
# Usage: check-brconfig.sh <Buildroot .config>
# Kconfig drops options with missing dependencies without a warning.
set -eu
bad=0
for opt in BR2_PACKAGE_BUSYBOX BR2_PACKAGE_CRYPTSETUP BR2_PACKAGE_E2FSPROGS; do
	grep -qx "$opt=y" "$1" || { echo "check-brconfig: $opt is not =y" >&2; bad=1; }
done
[ "$bad" = 0 ] && echo "check-brconfig: ok ($1)"
exit "$bad"
