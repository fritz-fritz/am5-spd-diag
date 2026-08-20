#!/bin/sh
# Unpack OBS extra sources: vendor archive + pinned official rustc.
# Usage: scripts/obs_prep.sh [vendor.tar.zst] [rust.tar.xz]
# Vendor is always re-extracted so a prior dh_clean cannot leave a tree
# missing vendor/*/Cargo.toml.orig. Rust is installed to /tmp/am5-rust
# (no ':' in the path). Callers put that bin dir first on PATH.
set -eu

RUST_PREFIX=/tmp/am5-rust
PIN_TXT=$(dirname "$0")/../obs/rust-dist.txt

log() {
	printf 'obs_prep: %s\n' "$*" >&2
}

find_file() {
	pattern=$1
	explicit=${2:-}
	if [ -n "$explicit" ] && [ -f "$explicit" ]; then
		printf '%s\n' "$explicit"
		return 0
	fi
	if [ -n "$explicit" ] && [ -d "$explicit" ]; then
		for f in "$explicit"/$pattern; do
			if [ -f "$f" ]; then
				printf '%s\n' "$f"
				return 0
			fi
		done
	fi
	for dir in "${RPM_SOURCE_DIR:-}" "${_sourcedir:-}" .. . /usr/src/packages/SOURCES /home/abuild/rpmbuild/SOURCES; do
		[ -n "$dir" ] || continue
		[ -d "$dir" ] || continue
		for f in "$dir"/$pattern; do
			if [ -f "$f" ]; then
				printf '%s\n' "$f"
				return 0
			fi
		done
	done
	return 1
}

extract_vendor() {
	archive=$1
	log "extracting vendor from $archive"
	rm -rf vendor
	mkdir -p .cargo
	if tar --help 2>/dev/null | grep -q -- --zstd; then
		tar --zstd -xf "$archive"
	else
		zstd -dc "$archive" | tar -xf -
	fi
	test -d vendor
	test -f .cargo/config.toml
}

pin_rust_file() {
	if [ -f "$PIN_TXT" ]; then
		url=
		while IFS= read -r line || [ -n "$line" ]; do
			case "$line" in
			URL=*) url=${line#URL=} ;;
			esac
		done <"$PIN_TXT"
		if [ -n "$url" ]; then
			basename "$url"
			return 0
		fi
	fi
	printf '%s\n' 'rust-*-x86_64-unknown-linux-gnu.tar.xz'
}

install_rust() {
	archive=$1
	rust_dir=$(basename "$archive" .tar.xz)
	want=${rust_dir#rust-}
	want=${want%-x86_64-unknown-linux-gnu}
	if [ -x "$RUST_PREFIX/bin/rustc" ]; then
		have=$("$RUST_PREFIX/bin/rustc" --version)
		case "$have" in
		*" $want "*)
			log "reusing $RUST_PREFIX ($have)"
			return 0
			;;
		esac
		log "replacing $RUST_PREFIX ($have, want $want)"
		rm -rf "$RUST_PREFIX"
	fi
	log "installing rustc from $archive -> $RUST_PREFIX"
	stage=$(mktemp -d /tmp/am5-rust-dist.XXXXXX)
	tar -C "$stage" -xf "$archive"
	installer="$stage/$rust_dir/install.sh"
	if [ ! -f "$installer" ]; then
		log "install.sh missing under $stage"
		find "$stage" -maxdepth 3 -type f >&2 || true
		rm -rf "$stage"
		return 1
	fi
	chmod +x "$installer"
	"$installer" --prefix="$RUST_PREFIX" --disable-ldconfig
	rm -rf "$stage"
	test -x "$RUST_PREFIX/bin/rustc"
	test -x "$RUST_PREFIX/bin/cargo"
	log "$("$RUST_PREFIX/bin/rustc" --version)"
}

VENDOR_ARG=${1:-}
RUST_ARG=${2:-}

if VENDOR=$(find_file 'am5-spd-diag-*-vendor.tar.zst' "$VENDOR_ARG"); then
	extract_vendor "$VENDOR"
elif [ -d vendor ]; then
	log "using existing vendor/"
else
	log "vendor archive not found"
	exit 1
fi

RUST_FILE=$(pin_rust_file)
if RUST=$(find_file "$RUST_FILE" "$RUST_ARG"); then
	install_rust "$RUST"
else
	log "rust dist $RUST_FILE not found; distro rustc on PATH is the fallback"
fi
