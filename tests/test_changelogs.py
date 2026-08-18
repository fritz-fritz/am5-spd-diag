#!/usr/bin/env python3
"""OBS .changes → Debian/RPM changelog generation."""
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import gen_changelogs as gen  # noqa: E402

SAMPLE = """\
-------------------------------------------------------------------
Mon Aug 17 08:40:00 UTC 2026 - Fritz <code@fritztech.net>

- Own /usr/lib/systemd/system-sleep so openSUSE post-build-checks
  does not fail. Drop duplicate share-dir listing.

-------------------------------------------------------------------
Mon Aug 17 06:00:00 UTC 2026 - Fritz <code@fritztech.net>

- Initial package 0.1.0: AM5 DDR5 SPD hub diagnostics after sleep
  and warm reboot.
"""


def test_parse_and_debian() -> None:
    entries = gen.parse_changes(SAMPLE)
    assert len(entries) == 2
    assert entries[0].author == "Fritz <code@fritztech.net>"
    assert entries[0].bullets[0].startswith("Own /usr/lib/systemd/system-sleep")
    debian = gen.debian_changelog(entries, "0.1.0-1")
    assert debian.startswith("am5-spd-diag (0.1.0-1) unstable; urgency=medium\n")
    assert "  * Own /usr/lib/systemd/system-sleep" in debian
    assert "  * Initial package 0.1.0" in debian
    assert debian.rstrip().endswith("Fritz <code@fritztech.net>  Mon, 17 Aug 2026 08:40:00 +0000")
    assert "\n -- " in debian


def test_spec_changelog() -> None:
    entries = gen.parse_changes(SAMPLE)
    spec = gen.spec_changelog(entries, "0.1.0")
    assert spec.startswith("%changelog\n")
    assert "* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0" in spec
    assert spec.count("* Mon Aug 17 2026") == 2
    rpm_macro = gen.spec_changelog(
        gen.parse_changes(
            "-------------------------------------------------------------------\n"
            "Mon Aug 17 08:30:00 UTC 2026 - Fritz <code@fritztech.net>\n"
            "\n"
            "- Install docs into %{_docdir} so openSUSE finds LICENSE/README.\n"
        ),
        "0.1.0",
    )
    assert "%%{_docdir}" in rpm_macro
    assert "%{_docdir}" not in rpm_macro.replace("%%{_docdir}", "")


def test_repo_changes_parse() -> None:
    path = ROOT / "am5-spd-diag.changes"
    if not path.exists():
        return
    entries = gen.parse_changes(path.read_text(encoding="utf-8"))
    assert entries
    assert all(entry.bullets for entry in entries)


if __name__ == "__main__":
    test_parse_and_debian()
    test_spec_changelog()
    test_repo_changes_parse()
    print("ok")
