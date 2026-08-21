#!/bin/bash
# osc build-cmd wrapper: put `shadow` in the chroot preinstall set when the
# rpmlist already has that package. Leap 16 installs dbus-1-common before
# shadow; rpm --nodeps ignores Requires(pre): useradd, so %prein dies with
# "neither useradd nor busybox found". OBS kvm uses a preinstall image;
# GitHub's --vm-type=chroot does not.
#
# osc's rpmlist is a 0600 tempfile in sticky /tmp. sudo/build cannot rewrite
# it (PermissionError), so patch a copy we own and pass that path through.
set -euo pipefail

# Ubuntu/Debian ship obs-build as /usr/bin/obs-build, not /usr/bin/build.
# sudo may strip OBS_BUILD_REAL, so resolve from well-known paths here.
find_build() {
	if [ -n "${OBS_BUILD_REAL:-}" ]; then
		if [ -x "$OBS_BUILD_REAL" ]; then
			printf '%s\n' "$OBS_BUILD_REAL"
			return
		fi
		printf 'obs_build_cmd: OBS_BUILD_REAL is not executable: %s\n' "$OBS_BUILD_REAL" >&2
		return 1
	fi
	local c
	for c in /usr/bin/build /usr/bin/obs-build /usr/lib/obs-build/build; do
		if [ -x "$c" ]; then
			printf '%s\n' "$c"
			return
		fi
	done
	printf 'obs_build_cmd: no obs-build binary (tried /usr/bin/build, /usr/bin/obs-build, /usr/lib/obs-build/build)\n' >&2
	return 1
}

REAL=$(find_build)

patched_rpmlist() {
	python3 - "$1" <<'PY'
import os
import pathlib
import sys
import tempfile

src = pathlib.Path(sys.argv[1])
lines = src.read_text(encoding="utf-8").splitlines(True)
has_shadow = False
for line in lines:
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        continue
    key, _, _rest = stripped.partition(":")
    if key in {
        "preinstall",
        "vminstall",
        "runscripts",
        "noinstall",
        "installonly",
        "sysroot",
        "preinstallimage",
        "preinstallimagesource",
        "preinstallimageinfo",
    }:
        continue
    if stripped.split()[0] == "shadow":
        has_shadow = True
        break
if not has_shadow:
    raise SystemExit(0)
out = []
for line in lines:
    if line.startswith("preinstall:"):
        names = line.split(":", 1)[1].split()
        if "shadow" not in names:
            names.append("shadow")
        line = "preinstall: " + " ".join(names) + "\n"
    out.append(line)
fd, name = tempfile.mkstemp(prefix="rpmlist-patched.")
try:
    os.write(fd, "".join(out).encode("utf-8"))
finally:
    os.close(fd)
print(name)
PY
}

args=()
for arg in "$@"; do
	case "$arg" in
	--rpmlist=*)
		src=${arg#--rpmlist=}
		out=$(patched_rpmlist "$src")
		if [ -n "$out" ]; then
			arg="--rpmlist=$out"
		fi
		;;
	esac
	args+=("$arg")
done

exec "$REAL" "${args[@]}"
