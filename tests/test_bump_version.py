#!/usr/bin/env python3
"""Package version bump patches Cargo, packaging, man, and OBS .changes."""
from __future__ import annotations

import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import bump_version as bump  # noqa: E402
import gen_changelogs as gen  # noqa: E402

CARGO = """\
[workspace]
members = ["crates/am5-spd-diag"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["Fritz <code@fritztech.net>"]
"""

MAKEFILE = """\
NAME        := am5-spd-diag
VERSION     ?= 0.1.0

build:
	true
"""

SPEC = """\
Name:           am5-spd-diag
Version:        0.1.0
Release:        0

%changelog
* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0
- Initial package 0.1.0
"""

DSC = """\
Format: 1.0
DEBTRANSFORM-TAR: am5-spd-diag-0.1.0.tar.xz
DEBTRANSFORM-RELEASE: 1
Source: am5-spd-diag
Version: 0.1.0-1
Files:
 d57283ebb8157ae919762c58419353c8 133282 am5-spd-diag_0.1.0.orig.tar.gz
 2fecf324a32123b08cefc0f047bca5ee 63176 am5-spd-diag_0.1.0-1.diff.tar.gz
"""

MAN = """.TH AM5-SPD-DIAG 1 \"2026-08-17\" \"am5-spd-diag 0.1.0\" \"User Commands\"
.SH NAME
am5-spd-diag
"""

CHANGES = """\
-------------------------------------------------------------------
Mon Aug 17 06:00:00 UTC 2026 - Fritz <code@fritztech.net>

- Initial package 0.1.0: AM5 DDR5 SPD hub diagnostics after sleep
  and warm reboot.
"""


def test_patch_fields() -> None:
    assert bump.workspace_version(CARGO) == "0.1.0"
    patched = bump.apply_text_patches(
        {
            "Cargo.toml": CARGO,
            "Makefile": MAKEFILE,
            "am5-spd-diag.spec": SPEC,
            "am5-spd-diag.dsc": DSC,
            "man/am5-spd-diag.1": MAN,
        },
        "1.0.0",
    )
    assert bump.workspace_version(patched["Cargo.toml"]) == "1.0.0"
    assert 'version = "0.1.0"' not in patched["Cargo.toml"]
    assert "VERSION     ?= 1.0.0" in patched["Makefile"]
    assert "Version:        1.0.0" in patched["am5-spd-diag.spec"]
    assert "Version:        0.1.0" not in patched["am5-spd-diag.spec"]
    dsc = patched["am5-spd-diag.dsc"]
    assert "DEBTRANSFORM-TAR: am5-spd-diag-1.0.0.tar.xz" in dsc
    assert "Version: 1.0.0-1" in dsc
    assert "am5-spd-diag_1.0.0.orig.tar.gz" in dsc
    assert "am5-spd-diag_1.0.0-1.diff.tar.gz" in dsc
    assert "am5-spd-diag_0.1.0" not in dsc
    assert '"am5-spd-diag 1.0.0"' in patched["man/am5-spd-diag.1"]
    assert '"am5-spd-diag 0.1.0"' not in patched["man/am5-spd-diag.1"]


def test_refuse_same_version() -> None:
    try:
        bump.validate_semver("1.0")
    except ValueError:
        pass
    else:
        raise AssertionError("expected invalid semver")
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _write_tree(root)
        try:
            bump.bump(
                "0.1.0",
                root=root,
                generate_lockfile=False,
                generate_changelogs=False,
            )
        except ValueError as err:
            assert "already at 0.1.0" in str(err)
        else:
            raise AssertionError("expected refuse same version")


def test_changes_prepend_parses() -> None:
    when = datetime(2026, 8, 19, 9, 38, 0, tzinfo=timezone.utc)
    entry = bump.format_changes_entry(
        "First stable release (Rust rewrite of the Python/bash tool).",
        "Fritz <code@fritztech.net>",
        when=when,
    )
    text = bump.prepend_changes(CHANGES, entry)
    entries = gen.parse_changes(text)
    assert len(entries) == 2
    assert entries[0].author == "Fritz <code@fritztech.net>"
    assert entries[0].bullets[0].startswith("First stable release")
    assert entries[1].bullets[0].startswith("Initial package 0.1.0")


def test_bump_temp_tree() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _write_tree(root)
        when = datetime(2026, 8, 19, 9, 38, 0, tzinfo=timezone.utc)
        bumped = bump.bump(
            "1.0.0",
            "First stable release (Rust rewrite of the Python/bash tool).",
            root=root,
            generate_lockfile=False,
            generate_changelogs=False,
            when=when,
        )
        assert bumped == "1.0.0"
        cargo = (root / "Cargo.toml").read_text(encoding="utf-8")
        assert bump.workspace_version(cargo) == "1.0.0"
        assert "VERSION     ?= 1.0.0" in (root / "Makefile").read_text(encoding="utf-8")
        changes = (root / "am5-spd-diag.changes").read_text(encoding="utf-8")
        entries = gen.parse_changes(changes)
        assert entries[0].bullets[0].startswith("First stable release")


def test_check_agrees() -> None:
    files = {
        "Cargo.toml": CARGO,
        "Makefile": MAKEFILE,
        "am5-spd-diag.spec": SPEC,
        "am5-spd-diag.dsc": DSC,
        "man/am5-spd-diag.1": MAN,
    }
    assert bump.check_versions(files) == []
    assert bump.check_versions(files, "0.1.0") == []
    errors = bump.check_versions(files, "1.0.0")
    assert errors
    assert any("expected '1.0.0'" in line for line in errors)


def test_check_drift() -> None:
    files = {
        "Cargo.toml": CARGO,
        "Makefile": MAKEFILE.replace("0.1.0", "9.9.9", 1),
        "am5-spd-diag.spec": SPEC,
        "am5-spd-diag.dsc": DSC,
        "man/am5-spd-diag.1": MAN,
    }
    errors = bump.check_versions(files)
    assert any("Makefile VERSION" in line and "9.9.9" in line for line in errors)
    assert not any(line.startswith("Cargo.toml") for line in errors)


def test_check_tree() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _write_tree(root)
        assert bump.check_tree(root) == []
        assert bump.check_tree(root, "0.1.0") == []
        errors = bump.check_tree(root, "2.0.0")
        assert errors
        bump.bump(
            "1.0.0",
            root=root,
            generate_lockfile=False,
            generate_changelogs=False,
        )
        assert bump.check_tree(root) == []
        assert bump.check_tree(root, "1.0.0") == []


def test_obs_wait_payload() -> None:
    sys.path.insert(0, str(ROOT / "scripts"))
    import obs_wait as wait  # noqa: E402

    assert wait.is_payload("am5-spd-diag-1.0.0-0.x86_64.rpm", "1.0.0")
    assert wait.is_payload("am5-spd-diag_1.0.0-1_amd64.deb", "1.0.0")
    assert not wait.is_payload("am5-spd-diag-1.0.0-0.src.rpm", "1.0.0")
    assert not wait.is_payload("am5-spd-diag-1.0.0-0.x86_64.rpm", "2.0.0")
    assert not wait.is_payload("am5-spd-diag-debuginfo-1.0.0-0.x86_64.rpm", "1.0.0")


def test_obs_release_gate() -> None:
    sys.path.insert(0, str(ROOT / "scripts"))
    import obs_release as rel  # noqa: E402

    assert rel.parse_spec_version("Name: x\nVersion: 1.0.0\n") == "1.0.0"
    assert rel.allow_commit("1.0.0", None, False)
    assert rel.allow_commit("1.0.1", "1.0.0", False)
    assert rel.allow_commit("1.0.0", "1.0.0", False)
    assert not rel.allow_commit("1.0.0", "1.0.1", False)
    assert rel.allow_commit("1.0.0", "1.0.1", True)


def test_obs_rebuild_url() -> None:
    sys.path.insert(0, str(ROOT / "scripts"))
    import obs_trigger as trig  # noqa: E402

    url = trig.rebuild_url("https://api.opensuse.org", "home:fritz-fritz", "am5-spd-diag")
    assert url.startswith("https://api.opensuse.org/trigger/rebuild?")
    assert "project=home%3Afritz-fritz" in url
    assert "package=am5-spd-diag" in url


def test_release_notes_mentions_obs() -> None:
    sys.path.insert(0, str(ROOT / "scripts"))
    import release_notes as notes  # noqa: E402

    text = notes.notes("1.0.0", "abc", notes.DEFAULT_DOWNLOAD, ROOT / "am5-spd-diag.changes")
    assert "OBS download page" in text
    assert "abc" in text
    assert "1.0.0" in text
    assert "Ghost DIMM" in text
    assert "O_NOFOLLOW" not in text
    pending = notes.notes(
        "1.0.0",
        "abc",
        notes.DEFAULT_DOWNLOAD,
        ROOT / "am5-spd-diag.changes",
        obs_built=False,
    )
    assert "empty `OBS_PASSWORD`" not in pending
    assert "were built on the Open Build Service" not in pending
    later = notes.notes("1.0.1", "def", notes.DEFAULT_DOWNLOAD, ROOT / "am5-spd-diag.changes")
    assert "Ghost DIMM" in later
    assert "- " in later


def _write_tree(root: Path) -> None:
    (root / "man").mkdir()
    (root / "Cargo.toml").write_text(CARGO, encoding="utf-8")
    (root / "Makefile").write_text(MAKEFILE, encoding="utf-8")
    (root / "am5-spd-diag.spec").write_text(SPEC, encoding="utf-8")
    (root / "am5-spd-diag.dsc").write_text(DSC, encoding="utf-8")
    (root / "man" / "am5-spd-diag.1").write_text(MAN, encoding="utf-8")
    (root / "am5-spd-diag.changes").write_text(CHANGES, encoding="utf-8")


if __name__ == "__main__":
    test_patch_fields()
    test_refuse_same_version()
    test_changes_prepend_parses()
    test_bump_temp_tree()
    test_check_agrees()
    test_check_drift()
    test_check_tree()
    test_obs_wait_payload()
    test_obs_release_gate()
    test_obs_rebuild_url()
    test_release_notes_mentions_obs()
    print("ok")
