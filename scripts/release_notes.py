#!/usr/bin/env python3
"""Print GitHub Release notes for an OBS-backed tag."""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = Path(__file__).resolve().parent
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))
import gen_changelogs as gen  # noqa: E402

PACKAGE = "am5-spd-diag"
DEFAULT_DOWNLOAD = (
    "https://software.opensuse.org/download/package"
    "?package=am5-spd-diag&project=home:fritz-fritz"
)


def notes(
    version: str,
    sha256: str,
    download: str,
    changes_path: Path,
    obs_built: bool = True,
) -> str:
    entries = gen.parse_changes(changes_path.read_text(encoding="utf-8"))
    bullets = "\n".join(f"- {b}" for b in entries[0].bullets) if entries else f"- {version}"
    if obs_built:
        lead = (
            f"Packages for {version} were built on the Open Build Service.\n"
            "\n"
            f"Install from the [OBS download page]({download}) (recommended). "
            "GitHub attachments are convenience copies for this tag.\n"
        )
    else:
        lead = (
            f"Source tarball for {version}. Open Build Service packages were not "
            "attached in this run (empty `OBS_PASSWORD` Actions secret).\n"
            "\n"
            f"Install from the [OBS download page]({download}) once packages appear. "
            "GitHub attachments are convenience copies for this tag.\n"
        )
    return (
        f"{lead}"
        "\n"
        "## Changes\n"
        "\n"
        f"{bullets}\n"
        "\n"
        "## Source\n"
        "\n"
        f"`{PACKAGE}-{version}.tar.xz` SHA256: `{sha256}`\n"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--download", default=DEFAULT_DOWNLOAD)
    parser.add_argument(
        "--changes",
        type=Path,
        default=ROOT / f"{PACKAGE}.changes",
    )
    parser.add_argument(
        "--pending-obs",
        action="store_true",
        help="Notes for a tarball-only GitHub Release (OBS skipped)",
    )
    args = parser.parse_args(argv)
    sys.stdout.write(
        notes(
            args.version,
            args.sha256,
            args.download,
            args.changes,
            obs_built=not args.pending_obs,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
