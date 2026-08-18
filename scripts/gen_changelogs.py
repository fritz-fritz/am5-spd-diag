#!/usr/bin/env python3
"""Generate Debian and RPM spec changelogs from the OBS .changes file."""
from __future__ import annotations

import argparse
import re
import sys
import textwrap
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import format_datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHANGES_PATH = ROOT / "am5-spd-diag.changes"
SPEC_PATH = ROOT / "am5-spd-diag.spec"
DSC_PATH = ROOT / "am5-spd-diag.dsc"
DEBIAN_CHANGELOGS = (
    ROOT / "debian.changelog",
    ROOT / "debian" / "changelog",
)
PACKAGE = "am5-spd-diag"
HEADER_RE = re.compile(
    r"^(?P<date>\w{3} \w{3}\s+\d{1,2} \d{2}:\d{2}:\d{2} UTC \d{4}) - (?P<author>.+)$"
)
SEP_RE = re.compile(r"^-+$", re.MULTILINE)


@dataclass(frozen=True)
class ChangeEntry:
    when: datetime
    author: str
    bullets: tuple[str, ...]


def parse_changes(text: str) -> list[ChangeEntry]:
    entries: list[ChangeEntry] = []
    for block in SEP_RE.split(text):
        lines = [ln.rstrip() for ln in block.strip().splitlines()]
        if not lines:
            continue
        match = HEADER_RE.match(lines[0].strip())
        if not match:
            raise ValueError(f"unrecognized .changes header: {lines[0]!r}")
        when = datetime.strptime(match.group("date"), "%a %b %d %H:%M:%S UTC %Y").replace(
            tzinfo=timezone.utc
        )
        bullets: list[str] = []
        current: list[str] = []
        for raw in lines[1:]:
            if not raw.strip():
                continue
            if raw.startswith("- "):
                if current:
                    bullets.append(" ".join(current))
                current = [raw[2:].strip()]
            elif raw.startswith("  ") and current:
                current.append(raw.strip())
            else:
                raise ValueError(f"unrecognized .changes line: {raw!r}")
        if current:
            bullets.append(" ".join(current))
        if not bullets:
            raise ValueError(f"changelog entry has no bullets: {lines[0]!r}")
        entries.append(
            ChangeEntry(
                when=when,
                author=match.group("author").strip(),
                bullets=tuple(bullets),
            )
        )
    return entries


def _field(path: Path, name: str) -> str:
    prefix = f"{name}:"
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith(prefix):
            return line.split(":", 1)[1].strip()
    raise ValueError(f"{path.name} is missing {name}")


def debian_changelog(entries: list[ChangeEntry], version: str) -> str:
    newest = entries[0]
    bullets = [bullet for entry in entries for bullet in entry.bullets]
    wrapped = [
        textwrap.fill(
            bullet,
            width=76,
            initial_indent="  * ",
            subsequent_indent="    ",
            break_long_words=False,
            break_on_hyphens=False,
        )
        for bullet in bullets
    ]
    return (
        f"{PACKAGE} ({version}) unstable; urgency=medium\n"
        "\n"
        + "\n".join(wrapped)
        + "\n"
        "\n"
        f" -- {newest.author}  {format_datetime(newest.when)}\n"
    )


def spec_changelog(entries: list[ChangeEntry], version: str) -> str:
    blocks: list[str] = []
    for entry in entries:
        header = entry.when.strftime("%a %b %d %Y")
        bullets = [
            textwrap.fill(
                bullet.replace("%", "%%"),
                width=76,
                initial_indent="- ",
                subsequent_indent="  ",
                break_long_words=False,
                break_on_hyphens=False,
            )
            for bullet in entry.bullets
        ]
        blocks.append(f"* {header} {entry.author} - {version}\n" + "\n".join(bullets))
    return "%changelog\n" + "\n\n".join(blocks) + "\n"


def replace_spec_changelog(spec_text: str, changelog: str) -> str:
    marker = "%changelog"
    idx = spec_text.find(marker)
    if idx < 0:
        raise ValueError("spec file is missing %changelog")
    return spec_text[:idx] + changelog


def render(changes_text: str, spec_text: str, dsc_version: str) -> tuple[str, str]:
    entries = parse_changes(changes_text)
    if not entries:
        raise ValueError("no changelog entries found")
    rpm_version = _field_from_text(spec_text, "Version")
    return debian_changelog(entries, dsc_version), spec_changelog(entries, rpm_version)


def _field_from_text(text: str, name: str) -> str:
    prefix = f"{name}:"
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith(prefix):
            return stripped.split(":", 1)[1].strip()
    raise ValueError(f"missing {name}")


def write_outputs(debian_text: str, spec_text: str) -> None:
    for path in DEBIAN_CHANGELOGS:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(debian_text, encoding="utf-8")
    SPEC_PATH.write_text(spec_text, encoding="utf-8")


def check_outputs(debian_text: str, spec_text: str) -> list[str]:
    errors: list[str] = []
    for path in DEBIAN_CHANGELOGS:
        current = path.read_text(encoding="utf-8") if path.exists() else ""
        if current != debian_text:
            errors.append(f"{path.relative_to(ROOT)} is stale; run python3 scripts/gen_changelogs.py")
    if SPEC_PATH.read_text(encoding="utf-8") != spec_text:
        errors.append(f"{SPEC_PATH.name} %changelog is stale; run python3 scripts/gen_changelogs.py")
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if generated changelogs are out of date",
    )
    args = parser.parse_args(argv)
    debian_text, spec_changelog_text = render(
        CHANGES_PATH.read_text(encoding="utf-8"),
        SPEC_PATH.read_text(encoding="utf-8"),
        _field(DSC_PATH, "Version"),
    )
    spec_text = replace_spec_changelog(SPEC_PATH.read_text(encoding="utf-8"), spec_changelog_text)
    if args.check:
        errors = check_outputs(debian_text, spec_text)
        if errors:
            print("\n".join(errors), file=sys.stderr)
            return 1
        return 0
    write_outputs(debian_text, spec_text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
