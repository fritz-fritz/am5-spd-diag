#!/bin/sh
# Print one field from the rustc pin.
# Usage:
#   scripts/rust_pin.sh channel [rust-toolchain.toml]
#   scripts/rust_pin.sh version|url|sha256|file [obs/rust-dist.txt]
set -eu

CMD=${1:-file}
ARG=${2:-}
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

channel_from_toml() {
	file=$1
	if [ ! -f "$file" ]; then
		printf 'rust_pin: missing %s\n' "$file" >&2
		exit 1
	fi
	channel=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$file" | head -n 1)
	if [ -z "$channel" ]; then
		printf 'rust_pin: no channel in %s\n' "$file" >&2
		exit 1
	fi
	printf '%s\n' "$channel"
}

if [ "$CMD" = "channel" ]; then
	channel_from_toml "${ARG:-$ROOT/rust-toolchain.toml}"
	exit 0
fi

TXT=${ARG:-$ROOT/obs/rust-dist.txt}
if [ ! -f "$TXT" ]; then
	printf 'rust_pin: missing %s\n' "$TXT" >&2
	exit 1
fi

VERSION=
URL=
SHA256=
while IFS= read -r line || [ -n "$line" ]; do
	case "$line" in
	'' | \#*) continue ;;
	VERSION=*) VERSION=${line#VERSION=} ;;
	URL=*) URL=${line#URL=} ;;
	SHA256=*) SHA256=${line#SHA256=} ;;
	esac
done <"$TXT"

if [ -z "$URL" ] || [ -z "$SHA256" ]; then
	printf 'rust_pin: URL and SHA256 are required in %s\n' "$TXT" >&2
	exit 1
fi

FILE=$(basename "$URL")
if [ -z "$VERSION" ]; then
	VERSION=${FILE#rust-}
	VERSION=${VERSION%-x86_64-unknown-linux-gnu.tar.xz}
fi

case "$CMD" in
version) printf '%s\n' "$VERSION" ;;
url) printf '%s\n' "$URL" ;;
sha256) printf '%s\n' "$SHA256" ;;
file) printf '%s\n' "$FILE" ;;
*)
	printf 'rust_pin: unknown field %s (want channel|version|url|sha256|file)\n' "$CMD" >&2
	exit 2
	;;
esac
