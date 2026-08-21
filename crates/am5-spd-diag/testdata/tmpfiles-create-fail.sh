#!/bin/sh
# Stub systemd-tmpfiles for purge tests: succeed unless --create.
for a in "$@"; do
	case "$a" in
	--create) exit 1 ;;
	esac
done
exit 0
