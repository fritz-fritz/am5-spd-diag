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

ABOUT = (
    "Linux helper for the AM5 DDR5 **Ghost DIMM**: after sleep, firmware can "
    "report a real module as an unknown 2 GB stick until AC is cut. This tool "
    "remembers a healthy kit, checks again after sleep and reboot, notifies "
    "when identity goes wrong, and can unstick the hub without crawling under "
    "the desk."
)

# First public tag: describe the product, not the last packaging PR.
INITIAL_1_0_0 = (
    "- First stable Linux release: `am5-spd-diag` CLI plus the **Ghost DIMM** GTK window.\n"
    "- Captures DDR5 SPD/DIMM identity at boot, shutdown, before sleep, and after resume.\n"
    "- Probe the SPD5118 hub and optionally clear a stuck MR11 in-band, then reboot — no wall-plug crawl.\n"
    "- Build a vendor-ticket report and an evidence tarball from the last capture."
)


def _change_bullets(version: str, changes_path: Path) -> str:
    if version == "1.0.0":
        return INITIAL_1_0_0
    entries = gen.parse_changes(changes_path.read_text(encoding="utf-8"))
    if not entries:
        return f"- {version}"
    return "\n".join(f"- {b}" for b in entries[0].bullets)


def notes(
    version: str,
    sha256: str,
    download: str,
    changes_path: Path,
    obs_built: bool = True,
) -> str:
    bullets = _change_bullets(version, changes_path)
    tag = f"v{version}"
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
            "attached in this run.\n"
            "\n"
            f"Install from the [OBS download page]({download}) once packages appear. "
            "GitHub attachments are convenience copies for this tag.\n"
        )
    return (
        f"{lead}"
        "\n"
        f"{ABOUT}\n"
        "\n"
        "## Changes\n"
        "\n"
        f"{bullets}\n"
        "\n"
        "## Source\n"
        "\n"
        f"`{PACKAGE}-{version}.tar.xz` SHA256: `{sha256}`\n"
        "\n"
        "`SHA256SUMS` lists every GitHub asset. The Release workflow attests that "
        "file and the source tarball (`gh attestation verify`). OBS rpm/deb "
        "binaries are built and signed on OBS, not on GitHub Actions.\n"
        "\n"
        f"Verify this release: `gh release verify {tag}` "
        f"and `gh release verify-asset {tag} {PACKAGE}-{version}.tar.xz`.\n"
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
