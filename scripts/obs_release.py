#!/usr/bin/env python3
"""Decide whether a tagged release may overwrite the live OBS package."""
from __future__ import annotations

import argparse
import sys
from pathlib import Path


def parse_spec_version(text: str) -> str | None:
    for raw in text.splitlines():
        line = raw.strip()
        if line.lower().startswith("version:"):
            version = line.split(":", 1)[1].strip()
            return version or None
    return None


def version_parts(version: str) -> tuple[int, int, int]:
    nums = version.split(".")
    if len(nums) != 3 or not all(part.isdigit() for part in nums):
        raise ValueError(f"version must be X.Y.Z (got {version!r})")
    return int(nums[0]), int(nums[1]), int(nums[2])


def allow_commit(tag_version: str, obs_version: str | None, force: bool) -> bool:
    """Upload this tag's sources unless OBS already has a newer Version."""
    if force or not obs_version:
        return True
    return version_parts(tag_version) >= version_parts(obs_version)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Tag version (X.Y.Z)")
    parser.add_argument(
        "--spec",
        type=Path,
        help="OBS package spec (missing/empty means the package has no Version yet)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Allow replacing a newer OBS Version with this tag",
    )
    parser.add_argument(
        "--github-output",
        type=Path,
        help="Append commit=true|false for GitHub Actions",
    )
    args = parser.parse_args(argv)
    spec_text = ""
    if args.spec and args.spec.is_file():
        spec_text = args.spec.read_text(encoding="utf-8")
    obs_version = parse_spec_version(spec_text)
    commit = allow_commit(args.version, obs_version, args.force)
    if obs_version and not commit:
        print(
            f"OBS already has Version {obs_version}; not replacing it with {args.version}. "
            "Collect binaries for this tag if they are still published, or pass force_obs_commit "
            "to overwrite the live package.",
            file=sys.stderr,
        )
    elif obs_version:
        print(f"OBS Version {obs_version} -> commit {args.version}")
    else:
        print(f"OBS has no Version yet; commit {args.version}")
    line = f"commit={'true' if commit else 'false'}\n"
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as fh:
            fh.write(line)
    sys.stdout.write(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
