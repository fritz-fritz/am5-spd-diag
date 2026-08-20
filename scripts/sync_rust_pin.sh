#!/bin/sh
# Point every rustc pin at VERSION (official linux-gnu tarball + toolchain).
# Usage:
#   scripts/sync_rust_pin.sh [VERSION]
# With no argument, use the dtolnay/rust-toolchain@X.Y.Z tag in ci.yml
# (the tag Dependabot bumps).
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOST=x86_64-unknown-linux-gnu
CI_YML=$ROOT/.github/workflows/ci.yml
REL_YML=$ROOT/.github/workflows/release.yml
DIST=$ROOT/obs/rust-dist.txt

toolchain_tag() {
	file=$1
	# Last explicit X.Y.Z tag wins; ignore @stable / @master / SHAs.
	grep -E 'dtolnay/rust-toolchain@[0-9]+\.[0-9]+\.[0-9]+' "$file" |
		sed -n 's/.*dtolnay\/rust-toolchain@\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' |
		tail -n 1
}

VERSION=${1:-}
if [ -z "$VERSION" ]; then
	ci=$(toolchain_tag "$CI_YML")
	rel=$(toolchain_tag "$REL_YML")
	VERSION=$(printf '%s\n' "$ci" "$rel" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' |
		sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
fi
if [ -z "$VERSION" ]; then
	printf 'sync_rust_pin: pass VERSION or pin dtolnay/rust-toolchain@X.Y.Z in ci.yml\n' >&2
	exit 1
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
	if [ "$cur" = "$VERSION" ] && [ "$cur_url" = "$URL" ] && [ -n "$cur_sha" ]; then
		ci_tag=$(toolchain_tag "$CI_YML")
		rel_tag=$(toolchain_tag "$REL_YML")
		channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml" | head -n 1)
		msrv=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
		spec=$(sed -n 's/^Source2:[[:space:]]*//p' "$ROOT/am5-spd-diag.spec" | head -n 1)
		if [ "$ci_tag" = "$VERSION" ] && [ "$rel_tag" = "$VERSION" ] &&
			[ "$channel" = "$VERSION" ] && [ "$msrv" = "$VERSION" ] &&
			[ "$spec" = "$FILE" ]; then
			printf 'sync_rust_pin: already at %s\n' "$VERSION"
			exit 0
		fi
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

TMP=$DIST.tmp
{
	printf '%s\n' \
		'# Official standalone rustc used as OBS Source2 (build-time only).' \
		'# The compiler is built against old glibc (~2.17) so it runs on Ubuntu 24.04,' \
		'# Debian 13, Fedora 43, and Leap 16. Each OBS chroot still links the *output*' \
		'# binary against that distro'\''s glibc and GTK.' \
		'# make dist does not download this. GHA and `make osc-fetch-rust` do.' \
		'#' \
		'# VERSION is the pin. Dependabot bumps dtolnay/rust-toolchain@VERSION in' \
		'# CI/Release; scripts/sync_rust_pin.sh refreshes URL, SHA256, rust-toolchain.toml,' \
		'# Cargo.toml rust-version, and spec Source2 on that PR.' \
		"VERSION=$VERSION" \
		"URL=$URL" \
		"SHA256=$SHA256"
} >"$TMP"
mv -f "$TMP" "$DIST"

cat >"$ROOT/rust-toolchain.toml" <<EOF
# Keep in sync with obs/rust-dist.txt and dtolnay/rust-toolchain@VERSION
# in .github/workflows. Dependabot bumps the workflow tags; the auto-merge
# workflow runs scripts/sync_rust_pin.sh so the OBS tarball follows.
[toolchain]
channel = "$VERSION"
components = ["rustfmt", "clippy"]
EOF

python3 - "$ROOT/Cargo.toml" "$VERSION" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
version = sys.argv[2]
text = path.read_text(encoding="utf-8")
old = text
lines = []
done = False
for line in text.splitlines(True):
    if not done and line.startswith("rust-version = "):
        lines.append(f'rust-version = "{version}"\n')
        done = True
    else:
        lines.append(line)
if not done:
    sys.exit("sync_rust_pin: rust-version not found in Cargo.toml")
path.write_text("".join(lines), encoding="utf-8")
if old == path.read_text(encoding="utf-8"):
    pass
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

pin_workflow() {
	yml=$1
	python3 - "$yml" "$VERSION" <<'PY'
from pathlib import Path
import re
import sys
path = Path(sys.argv[1])
version = sys.argv[2]
text = path.read_text(encoding="utf-8")
new, n = re.subn(
    r"(dtolnay/rust-toolchain@)(?:stable|beta|nightly|master|[0-9]+\.[0-9]+\.[0-9]+)",
    r"\g<1>" + version,
    text,
)
if n < 1:
    sys.exit(f"sync_rust_pin: no dtolnay/rust-toolchain pin in {path}")
path.write_text(new, encoding="utf-8")
PY
}

pin_workflow "$CI_YML"
pin_workflow "$REL_YML"

python3 "$ROOT/scripts/check_rust_pin.py"
printf 'sync_rust_pin: now %s (%s)\n' "$VERSION" "$SHA256"
