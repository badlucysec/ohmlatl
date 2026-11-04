#!/bin/sh
# Usage: check-kconfig.sh <fragment> <final .config>
# Every "=y" of the fragment must stay "=y"; every "is not set" must stay off.
set -eu
frag=$1 conf=$2 bad=0
while IFS= read -r line; do
	case $line in
	CONFIG_*=y)
		grep -qx "$line" "$conf" || { echo "check-kconfig: ${line%=y} is not =y" >&2; bad=1; } ;;
	"# CONFIG_"*" is not set")
		opt=${line#\# }; opt=${opt%% *}
		if grep -Eq "^$opt=(y|m)" "$conf"; then echo "check-kconfig: $opt is enabled" >&2; bad=1; fi ;;
	esac
done < "$frag"
[ "$bad" = 0 ] && echo "check-kconfig: ok ($conf)"
exit "$bad"
