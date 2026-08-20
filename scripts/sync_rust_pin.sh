#!/bin/sh
# Point OBS Source2, Cargo.toml rust-version, and spec Source2 at VERSION.
# rust-toolchain.toml is the Dependabot-owned pin. With no argument, read it.
# Usage: scripts/sync_rust_pin.sh [VERSION]
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOST=x86_64-unknown-linux-gnu
TOML=$ROOT/rust-toolchain.toml
DIST=$ROOT/obs/rust-dist.txt

VERSION=${1:-}
if [ -z "$VERSION" ]; then
	VERSION=$("$ROOT/scripts/rust_pin.sh" channel "$TOML")
fi
case "$VERSION" in
[0-9]*.[0-9]*.[0-9]*) ;;
*)
	printf 'sync_rust_pin: VERSION must be X.Y.Z (got %s)\n' "$VERSION" >&2
	exit 1
	;;
esac

FILE=rust-${VERSION}-${HOST}.tar.xz
URL=https://static.rust-lang.org/dist/$FILE

if [ -f "$DIST" ]; then
	cur=$("$ROOT/scripts/rust_pin.sh" version "$DIST")
	cur_url=$("$ROOT/scripts/rust_pin.sh" url "$DIST")
	cur_sha=$("$ROOT/scripts/rust_pin.sh" sha256 "$DIST")
	channel=$("$ROOT/scripts/rust_pin.sh" channel "$TOML")
	msrv=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
	spec=$(sed -n 's/^Source2:[[:space:]]*//p' "$ROOT/am5-spd-diag.spec" | head -n 1)
	if [ "$cur" = "$VERSION" ] && [ "$cur_url" = "$URL" ] && [ -n "$cur_sha" ] &&
		[ "$channel" = "$VERSION" ] && [ "$msrv" = "$VERSION" ] &&
		[ "$spec" = "$FILE" ]; then
		printf 'sync_rust_pin: already at %s\n' "$VERSION"
		exit 0
	fi
fi

printf 'sync_rust_pin: fetching SHA256 for %s\n' "$FILE" >&2
SHA256=$(curl -fsSL "$URL.sha256" | awk '{print $1}')
case "$SHA256" in
[0-9a-f][0-9a-f][0-9a-f][0-9a-f]*)
	[ "${#SHA256}" -eq 64 ]
	;;
*)
	printf 'sync_rust_pin: bad SHA256 from %s.sha256\n' "$URL" >&2
	exit 1
	;;
esac

{
	printf '%s\n' \
		'# Official standalone rustc used as OBS Source2 (build-time only).' \
		'# The compiler is built against old glibc (~2.17) so it runs on Ubuntu 24.04,' \
		'# Debian 12, Debian 13, Fedora 43, and Leap 16. Each OBS chroot still links' \
		'# the *output* binary against that distro'\''s glibc and GTK.' \
		'# make dist does not download this. GHA and `make osc-fetch-rust` do.' \
		'#' \
		'# rust-toolchain.toml is the pin Dependabot bumps. sync_rust_pin.sh copies' \
		'# that channel into VERSION, URL, SHA256, Cargo.toml rust-version, and spec' \
		'# Source2. CI/Release parse the channel and pass it to dtolnay/rust-toolchain.' \
		"VERSION=$VERSION" \
		"URL=$URL" \
		"SHA256=$SHA256"
} >"$DIST.tmp"
mv -f "$DIST.tmp" "$DIST"

cat >"$TOML" <<EOF
# Dependabot package-ecosystem: rust-toolchain bumps channel.
# scripts/sync_rust_pin.sh copies it to OBS Source2 and rust-version.
# Workflows parse channel and pass it as dtolnay/rust-toolchain's toolchain.
[toolchain]
channel = "$VERSION"
components = ["rustfmt", "clippy"]
EOF

python3 - "$ROOT/Cargo.toml" "$VERSION" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
version = sys.argv[2]
lines = []
done = False
for line in path.read_text(encoding="utf-8").splitlines(True):
    if not done and line.startswith("rust-version = "):
        lines.append(f'rust-version = "{version}"\n')
        done = True
    else:
        lines.append(line)
if not done:
    sys.exit("sync_rust_pin: rust-version not found in Cargo.toml")
path.write_text("".join(lines), encoding="utf-8")
PY

python3 - "$ROOT/am5-spd-diag.spec" "$FILE" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
filename = sys.argv[2]
lines = []
found = False
for line in path.read_text(encoding="utf-8").splitlines(True):
    if line.startswith("Source2:"):
        lines.append(f"Source2:        {filename}\n")
        found = True
    else:
        lines.append(line)
if not found:
    sys.exit("sync_rust_pin: Source2 not found in spec")
path.write_text("".join(lines), encoding="utf-8")
PY

python3 "$ROOT/scripts/check_rust_pin.py"
printf 'sync_rust_pin: now %s (%s)\n' "$VERSION" "$SHA256"
