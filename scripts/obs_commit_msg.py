#!/usr/bin/env python3
"""Format the osc commit message from changelog entries since the last release."""
from __future__ import annotations

import argparse
import sys
import textwrap
from datetime import datetime, timezone
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))
import gen_changelogs as gen  # noqa: E402


def parse_since(raw: str) -> datetime:
    text = raw.strip().replace("Z", "+00:00")
    when = datetime.fromisoformat(text)
    if when.tzinfo is None:
        when = when.replace(tzinfo=timezone.utc)
    return when.astimezone(timezone.utc)


def bullets_since_last_release(
    entries: list[gen.ChangeEntry],
    since: datetime | None,
) -> list[str]:
    """Changelog bullets newer than *since*. If none match, use the newest entry."""
    if not entries:
        return []
    if since is None:
        return list(entries[0].bullets)
    found: list[str] = []
    for entry in entries:
        if entry.when <= since:
            break
        found.extend(entry.bullets)
    return found or list(entries[0].bullets)


def format_obs_commit_message(
    version: str,
    tag: str,
    sha: str,
    bullets: list[str],
) -> str:
    header = f"Release {version} from {tag}"
    if sha:
        header = f"{header} ({sha[:12]})"
    lines = [header]
    if bullets:
        lines.append("")
        for bullet in bullets:
            lines.append(
                textwrap.fill(
                    bullet,
                    width=76,
                    initial_indent="- ",
                    subsequent_indent="  ",
                    break_long_words=False,
                    break_on_hyphens=False,
                )
            )
    return "\n".join(lines) + "\n"


def obs_commit_message(
    version: str,
    tag: str,
    sha: str,
    changes_text: str,
    since: datetime | None = None,
) -> str:
    entries = gen.parse_changes(changes_text) if changes_text.strip() else []
    return format_obs_commit_message(
        version, tag, sha, bullets_since_last_release(entries, since)
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", default="")
    parser.add_argument(
        "--changes",
        type=Path,
        default=SCRIPTS.parent / "am5-spd-diag.changes",
    )
    parser.add_argument(
        "--since",
        help="ISO-8601 timestamp of the previous release; include newer changelog entries",
    )
    args = parser.parse_args(argv)
    since = parse_since(args.since) if args.since else None
    sys.stdout.write(
        obs_commit_message(
            args.version,
            args.tag,
            args.sha,
            args.changes.read_text(encoding="utf-8"),
            since=since,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
