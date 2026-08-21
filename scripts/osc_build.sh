#!/bin/bash
# Stage Source0/Source1/Source2 plus packaging into a real osc checkout
# and run `osc build REPO x86_64` locally. Do not osc commit from here.
#
# Usage:
#   make dist && make osc-fetch-rust
#   scripts/osc_build.sh 16.0
#   OSC_OFFLINE=1 scripts/osc_build.sh openSUSE_Tumbleweed
#   OSC_VM_TYPE=chroot OSC_PRELOAD=0 scripts/osc_build.sh xUbuntu_24.10
#
# Do not osc commit from CI. Optional OSC_RC is an oscrc (API auth only).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
NAME=am5-spd-diag
OBS_PROJECT=${OBS_PROJECT:-home:fritz-fritz}
REPO=${1:-${REPO:-openSUSE_Tumbleweed}}
ARCH=${2:-${ARCH:-x86_64}}
OSC_WC=${OSC_WC:-/tmp/am5-spd-diag-osc-wc}
DIST_PARENT=${DIST_PARENT:-$(cd "$ROOT/.." && pwd)}
RUST_FILE=$("$ROOT/scripts/rust_pin.sh" file "$ROOT/obs/rust-dist.txt")

# Cursor (and other AppImage hosts) export APPIMAGE+OWD. osc's babysitter
# then chdirs to OWD ($HOME), so `osc build` from a checkout fails with
# "Directory '/home/…' is not a working copy".
osc() {
	local -a cmd
	cmd=(/usr/bin/osc)
	if [ -n "${OSC_RC:-}" ]; then
		cmd+=(-c "$OSC_RC")
	fi
	env -u APPIMAGE -u OWD "${cmd[@]}" "$@"
}

VERSION=$(awk '/^VERSION/{print $3; exit}' "$ROOT/Makefile")
if [ -z "$VERSION" ]; then
	printf 'osc_build: could not read VERSION from Makefile\n' >&2
	exit 1
fi

SRC0=$DIST_PARENT/$NAME-$VERSION.tar.xz
SRC1=$DIST_PARENT/$NAME-$VERSION-vendor.tar.zst
SRC2=$DIST_PARENT/$RUST_FILE

for f in "$SRC0" "$SRC1" "$SRC2"; do
	if [ ! -s "$f" ]; then
		printf 'osc_build: missing %s (run make dist && make osc-fetch-rust)\n' "$f" >&2
		exit 1
	fi
done

case "$REPO" in
xUbuntu_* | Debian_*) DESCR=$NAME.dsc ;;
*) DESCR=$NAME.spec ;;
esac

if [ ! -d "$OSC_WC/.osc" ]; then
	rm -rf "$OSC_WC"
	osc checkout -o "$OSC_WC" "$OBS_PROJECT" "$NAME"
fi

cp -f "$SRC0" "$SRC1" "$SRC2" "$OSC_WC/"
cp -f "$ROOT/$NAME.spec" "$ROOT/$NAME.changes" "$ROOT/$NAME.dsc" "$OSC_WC/"
cp -f "$ROOT/$NAME.rpmlintrc" "$OSC_WC/"
cp -f "$ROOT"/debian.control "$ROOT"/debian.changelog "$ROOT"/debian.rules \
	"$ROOT"/debian.compat "$ROOT"/debian.copyright "$OSC_WC/"

(
	cd "$OSC_WC"
	for old in "$NAME"-*.tar.xz; do
		[ -e "$old" ] || continue
		[ "$old" = "$NAME-$VERSION.tar.xz" ] && continue
		osc rm --force "$old" 2>/dev/null || rm -f "$old"
	done
	for old in "$NAME"-*-vendor.tar.zst; do
		[ -e "$old" ] || continue
		[ "$old" = "$NAME-$VERSION-vendor.tar.zst" ] && continue
		osc rm --force "$old" 2>/dev/null || rm -f "$old"
	done
	osc add "$NAME-$VERSION.tar.xz" 2>/dev/null || true
	osc add "$NAME-$VERSION-vendor.tar.zst" 2>/dev/null || true
	osc add "$RUST_FILE" 2>/dev/null || true
	osc add "$NAME.spec" "$NAME.changes" "$NAME.dsc" "$NAME.rpmlintrc" \
		debian.control debian.changelog debian.rules debian.compat debian.copyright \
		2>/dev/null || true
)

BUILD_ROOT=/var/tmp/build-root/${REPO}-${ARCH}
BUILD_FLAGS=(--trust-all-projects --no-verify)
if [ -n "${OSC_VM_TYPE:-}" ]; then
	BUILD_FLAGS+=(--vm-type="$OSC_VM_TYPE")
fi
if [ "${OSC_OFFLINE:-0}" = 1 ]; then
	BUILD_FLAGS+=(--offline)
	if [ -d "$BUILD_ROOT" ]; then
		BUILD_FLAGS+=(--noinit)
	fi
fi

cd "$OSC_WC"
if [ "${OSC_PRELOAD:-1}" = 1 ] && [ "${OSC_OFFLINE:-0}" != 1 ]; then
	osc build "${BUILD_FLAGS[@]}" --preload "$REPO" "$ARCH" "$DESCR"
fi
osc build "${BUILD_FLAGS[@]}" "$REPO" "$ARCH" "$DESCR"
