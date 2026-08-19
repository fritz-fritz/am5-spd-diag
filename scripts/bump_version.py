#!/usr/bin/env python3
"""Bump the package version across Cargo, packaging, man, lockfile, and changelogs."""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
import textwrap
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = "am5-spd-diag"
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")
WORKSPACE_VERSION_RE = re.compile(
    r'(?P<pre>^\[workspace\.package\]\s*\n(?:(?!^\[).*\n)*?^version\s*=\s*")(?P<ver>[^"]+)(?P<post>")',
    re.MULTILINE,
)
AUTHORS_RE = re.compile(
    r'^\[workspace\.package\]\s*\n(?:(?!^\[).*\n)*?^authors\s*=\s*\[\s*"([^"]+)"',
    re.MULTILINE,
)
MAKEFILE_VERSION_RE = re.compile(r"^(VERSION\s+\?=\s*)(\S+)", re.MULTILINE)
SPEC_VERSION_RE = re.compile(r"^(Version:\s+)(\S+)", re.MULTILINE)
DSC_VERSION_RE = re.compile(r"^(Version:\s+)(\d+\.\d+\.\d+)(-\d+)", re.MULTILINE)
DSC_TAR_RE = re.compile(
    rf"^(DEBTRANSFORM-TAR:\s+{re.escape(PACKAGE)}-)(\d+\.\d+\.\d+)(\.tar\.xz)",
    re.MULTILINE,
)
MAN_VERSION_RE = re.compile(rf'("{re.escape(PACKAGE)} )(\d+\.\d+\.\d+)(")')
DSC_FILE_RE = re.compile(
    rf"({re.escape(PACKAGE)}_)(\d+\.\d+\.\d+)((?:-1)?\.(?:orig\.tar\.gz|diff\.tar\.gz))"
)


def validate_semver(version: str) -> str:
    if not SEMVER_RE.match(version):
        raise ValueError(f"version must be X.Y.Z (got {version!r})")
    return version


def workspace_version(cargo_toml: str) -> str:
    match = WORKSPACE_VERSION_RE.search(cargo_toml)
    if not match:
        raise ValueError("Cargo.toml is missing [workspace.package] version")
    return match.group("ver")


def workspace_author(cargo_toml: str) -> str:
    match = AUTHORS_RE.search(cargo_toml)
    if not match:
        raise ValueError("Cargo.toml is missing [workspace.package] authors")
    return match.group(1)


def patch_cargo_toml(text: str, new: str) -> str:
    if not WORKSPACE_VERSION_RE.search(text):
        raise ValueError("Cargo.toml is missing [workspace.package] version")
    return WORKSPACE_VERSION_RE.sub(rf"\g<pre>{new}\g<post>", text, count=1)


def patch_makefile(text: str, new: str) -> str:
    updated, n = MAKEFILE_VERSION_RE.subn(rf"\g<1>{new}", text, count=1)
    if n != 1:
        raise ValueError("Makefile is missing VERSION ?= …")
    return updated


def patch_spec(text: str, new: str) -> str:
    updated, n = SPEC_VERSION_RE.subn(rf"\g<1>{new}", text, count=1)
    if n != 1:
        raise ValueError("spec is missing Version:")
    return updated


def patch_dsc(text: str, new: str) -> str:
    updated, n = DSC_TAR_RE.subn(rf"\g<1>{new}\g<3>", text, count=1)
    if n != 1:
        raise ValueError("dsc is missing DEBTRANSFORM-TAR")
    updated, n = DSC_VERSION_RE.subn(rf"\g<1>{new}-1", updated, count=1)
    if n != 1:
        raise ValueError("dsc is missing Version:")
    files, n = DSC_FILE_RE.subn(rf"\g<1>{new}\g<3>", updated)
    if n < 1:
        raise ValueError("dsc Files: section is missing orig/diff names")
    return files


def patch_man(text: str, new: str) -> str:
    updated, n = MAN_VERSION_RE.subn(rf"\g<1>{new}\g<3>", text, count=1)
    if n != 1:
        raise ValueError("man page is missing .TH version field")
    return updated


def format_changes_entry(
    message: str,
    author: str,
    when: datetime | None = None,
) -> str:
    when = when or datetime.now(timezone.utc)
    header = when.strftime("%a %b %d %H:%M:%S UTC %Y")
    bullet = textwrap.fill(
        message.strip(),
        width=76,
        initial_indent="- ",
        subsequent_indent="  ",
        break_long_words=False,
        break_on_hyphens=False,
    )
    return f"-------------------------------------------------------------------\n{header} - {author}\n\n{bullet}\n\n"


def prepend_changes(text: str, entry: str) -> str:
    body = text if text.endswith("\n") or text == "" else text + "\n"
    return entry + body


def apply_text_patches(files: dict[str, str], new: str) -> dict[str, str]:
    out = dict(files)
    out["Cargo.toml"] = patch_cargo_toml(files["Cargo.toml"], new)
    out["Makefile"] = patch_makefile(files["Makefile"], new)
    out["am5-spd-diag.spec"] = patch_spec(files["am5-spd-diag.spec"], new)
    out["am5-spd-diag.dsc"] = patch_dsc(files["am5-spd-diag.dsc"], new)
    out["man/am5-spd-diag.1"] = patch_man(files["man/am5-spd-diag.1"], new)
    return out


def _require(match: re.Match[str] | None, err: str) -> re.Match[str]:
    if not match:
        raise ValueError(err)
    return match


def field_versions(files: dict[str, str]) -> dict[str, str]:
    """Map each patched field to the X.Y.Z version currently in that file."""
    found: dict[str, str] = {
        "Cargo.toml [workspace.package] version": workspace_version(files["Cargo.toml"]),
        "Makefile VERSION": _require(
            MAKEFILE_VERSION_RE.search(files["Makefile"]),
            "Makefile is missing VERSION ?= …",
        ).group(2),
        "am5-spd-diag.spec Version": _require(
            SPEC_VERSION_RE.search(files["am5-spd-diag.spec"]),
            "spec is missing Version:",
        ).group(2),
    }
    dsc = files["am5-spd-diag.dsc"]
    found["am5-spd-diag.dsc Version"] = _require(
        DSC_VERSION_RE.search(dsc),
        "dsc is missing Version:",
    ).group(2)
    found["am5-spd-diag.dsc DEBTRANSFORM-TAR"] = _require(
        DSC_TAR_RE.search(dsc),
        "dsc is missing DEBTRANSFORM-TAR",
    ).group(2)
    file_matches = list(DSC_FILE_RE.finditer(dsc))
    if not file_matches:
        raise ValueError("dsc Files: section is missing orig/diff names")
    for match in file_matches:
        found[f"am5-spd-diag.dsc Files {match.group(3).lstrip('-')}"] = match.group(2)
    found["man/am5-spd-diag.1"] = _require(
        MAN_VERSION_RE.search(files["man/am5-spd-diag.1"]),
        "man page is missing .TH version field",
    ).group(2)
    return found


def packaging_files(root: Path) -> dict[str, str]:
    return {
        "Cargo.toml": _read(root, "Cargo.toml"),
        "Makefile": _read(root, "Makefile"),
        "am5-spd-diag.spec": _read(root, "am5-spd-diag.spec"),
        "am5-spd-diag.dsc": _read(root, "am5-spd-diag.dsc"),
        "man/am5-spd-diag.1": _read(root, "man/am5-spd-diag.1"),
    }


def check_versions(files: dict[str, str], expected: str | None = None) -> list[str]:
    found = field_versions(files)
    if expected is None:
        target = found["Cargo.toml [workspace.package] version"]
        validate_semver(target)
    else:
        target = validate_semver(expected)
    return [
        f"{name}: {ver!r} (expected {target!r})"
        for name, ver in found.items()
        if ver != target
    ]


def check_tree(root: Path = ROOT, expected: str | None = None) -> list[str]:
    return check_versions(packaging_files(root), expected)


def _read(root: Path, rel: str) -> str:
    return (root / rel).read_text(encoding="utf-8")


def _write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def bump(
    new: str,
    message: str | None = None,
    *,
    root: Path = ROOT,
    generate_lockfile: bool = True,
    generate_changelogs: bool = True,
    when: datetime | None = None,
) -> str:
    new = validate_semver(new)
    cargo = _read(root, "Cargo.toml")
    current = workspace_version(cargo)
    if new == current:
        raise ValueError(f"already at {current}")
    patched = apply_text_patches(
        {
            "Cargo.toml": cargo,
            "Makefile": _read(root, "Makefile"),
            "am5-spd-diag.spec": _read(root, "am5-spd-diag.spec"),
            "am5-spd-diag.dsc": _read(root, "am5-spd-diag.dsc"),
            "man/am5-spd-diag.1": _read(root, "man/am5-spd-diag.1"),
        },
        new,
    )
    for rel, text in patched.items():
        _write(root, rel, text)
    if message:
        changes_path = "am5-spd-diag.changes"
        entry = format_changes_entry(message, workspace_author(cargo), when=when)
        existing = _read(root, changes_path) if (root / changes_path).is_file() else ""
        _write(root, changes_path, prepend_changes(existing, entry))
    if generate_lockfile:
        cmd = ["cargo", "generate-lockfile"]
        if (root / "vendor").is_dir():
            cmd.extend(["--offline", "--config", "net.offline=true"])
        subprocess.run(cmd, cwd=root, check=True)
    if generate_changelogs:
        scripts_dir = str(root / "scripts")
        if scripts_dir not in sys.path:
            sys.path.insert(0, scripts_dir)
        import gen_changelogs as gen

        rc = gen.main([])
        if rc:
            raise RuntimeError("gen_changelogs.py failed")
    return new


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        nargs="?",
        const="",
        default=None,
        metavar="VERSION",
        help="verify packaging versions agree; optionally require VERSION (X.Y.Z)",
    )
    parser.add_argument("version", nargs="?", help="new version (X.Y.Z)")
    parser.add_argument(
        "-m",
        "--message",
        help="prepend this bullet to am5-spd-diag.changes",
    )
    parser.add_argument(
        "--no-lockfile",
        action="store_true",
        help="skip cargo generate-lockfile",
    )
    parser.add_argument(
        "--no-changelogs",
        action="store_true",
        help="skip gen_changelogs.py",
    )
    args = parser.parse_args(argv)
    try:
        if args.check is not None:
            expected = args.check or None
            errors = check_tree(expected=expected)
            if errors:
                print("bump_version: version mismatch:", file=sys.stderr)
                for line in errors:
                    print(f"  {line}", file=sys.stderr)
                return 1
            print("versions ok" + (f" ({expected})" if expected else ""))
            return 0
        if not args.version:
            parser.error("version is required unless --check is set")
        bumped = bump(
            args.version,
            args.message,
            generate_lockfile=not args.no_lockfile,
            generate_changelogs=not args.no_changelogs,
        )
    except (ValueError, FileNotFoundError, subprocess.CalledProcessError) as err:
        print(f"bump_version: {err}", file=sys.stderr)
        return 1
    print(f"bumped to {bumped}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
