#!/bin/sh
# Download the pinned official rustc tarball (OBS Source2).
# Usage: scripts/osc_fetch_rust.sh [DESTDIR] [obs/rust-dist.txt]
set -eu

DEST=${1:-.}
TXT=${2:-}
if [ -z "$TXT" ]; then
	TXT=$(dirname "$0")/../obs/rust-dist.txt
fi
if [ ! -f "$TXT" ]; then
	printf 'osc_fetch_rust: missing %s\n' "$TXT" >&2
	exit 1
fi

mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)

URL=
SHA256=
while IFS= read -r line || [ -n "$line" ]; do
	case "$line" in
	'' | \#*) continue ;;
	URL=*) URL=${line#URL=} ;;
	SHA256=*) SHA256=${line#SHA256=} ;;
	esac
done <"$TXT"

if [ -z "$URL" ] || [ -z "$SHA256" ]; then
	printf 'osc_fetch_rust: URL and SHA256 are required in %s\n' "$TXT" >&2
	exit 1
fi

FILE=$(basename "$URL")
OUT="$DEST/$FILE"

if [ -f "$OUT" ]; then
	echo "$SHA256  $OUT" | sha256sum -c -
	exit 0
fi

tmp=$OUT.$$
curl -fL --retry 5 --retry-delay 2 -o "$tmp" "$URL"
echo "$SHA256  $tmp" | sha256sum -c -
mv -f "$tmp" "$OUT"
printf 'Wrote %s\n' "$OUT"
