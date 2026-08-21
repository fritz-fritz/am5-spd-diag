#!/bin/bash
# osc build-cmd wrapper: put `shadow` in the chroot preinstall set when the
# rpmlist already has that package. Leap 16 installs dbus-1-common before
# shadow; rpm --nodeps ignores Requires(pre): useradd, so %prein dies with
# "neither useradd nor busybox found". OBS kvm uses a preinstall image;
# GitHub's --vm-type=chroot does not.
set -euo pipefail

REAL=${OBS_BUILD_REAL:-/usr/bin/build}

patch_rpmlist() {
	local file=$1
	python3 - "$file" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
lines = path.read_text(encoding="utf-8").splitlines(True)
has_shadow = False
for line in lines:
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        continue
    key, _, rest = stripped.partition(":")
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
path.write_text("".join(out), encoding="utf-8")
PY
}

for arg in "$@"; do
	case "$arg" in
	--rpmlist=*)
		patch_rpmlist "${arg#--rpmlist=}"
		;;
	esac
done

exec "$REAL" "$@"
