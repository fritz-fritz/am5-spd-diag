#!/usr/bin/env python3
"""Package version bump patches Cargo, packaging, man, and OBS .changes."""
from __future__ import annotations

import os
import re
import subprocess
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
    assert wait.github_asset_name(
        "Fedora_44", "am5-spd-diag-1.0.2-0.x86_64.rpm"
    ) == "am5-spd-diag-1.0.2-0.x86_64.Fedora_44.rpm"
    assert wait.github_asset_name(
        "xUbuntu_24.04", "am5-spd-diag_1.0.2-1_amd64.deb"
    ) == "am5-spd-diag_1.0.2-1_amd64.xUbuntu_24.04.deb"
    assert wait.github_asset_name(
        "16.0", "am5-spd-diag-1.0.2-0.openSUSE_Leap_16.0.x86_64.rpm"
    ) == "am5-spd-diag-1.0.2-0.openSUSE_Leap_16.0.x86_64.rpm"
    assert wait.classify_codes(["succeeded", "unresolvable"]) == "unresolvable"
    assert wait.classify_codes(["succeeded", "building"]) == "building"
    assert wait.classify_codes(["succeeded", "excluded"]) == "excluded"
    assert wait.classify_codes(["failed"]) == "failed"
    # finished/signing stay live until succeeded: GitHub Releases cannot grow
    # assets after publish, so collect must wait through OBS signing.
    assert wait.classify_codes(["finished"]) == "finished"
    assert wait.classify_codes(["signing"]) == "signing"
    assert wait.classify_codes(["succeeded", "finished"]) == "finished"
    assert wait.classify_codes(["finished", "building"]) == "building"
    assert wait.status_label("Fedora_44", "x86_64", "finished") == (
        "Fedora_44/x86_64: finished (web UI may already show succeeded)"
    )
    assert wait.finished_ok(["tw/x86_64: building"], []) is None
    assert wait.finished_ok(["tw/x86_64: binaries not listed yet"], []) is None
    assert wait.finished_ok([], ["tw/x86_64: succeeded"]) is True
    assert wait.finished_ok([], []) is False
    assert wait.finished_ok([], [], ["tw/x86_64: failed"]) is None
    assert wait.finished_ok([], ["tw/x86_64: succeeded"], ["ubuntu/x86_64: failed"]) is None
    assert (
        wait.finished_ok(
            [],
            ["tw/x86_64: succeeded"],
            ["ubuntu/x86_64: failed"],
            retries_exhausted=True,
        )
        is False
    )
    seen_live: set[tuple[str, str]] = set()
    retried: set[tuple[str, str]] = set()
    assert wait.maybe_rebuild(
        [("xUbuntu_24.10", "x86_64")],
        seen_live=seen_live,
        retried=retried,
        pending=[],
        ready=[],
    ) == []
    assert wait.maybe_rebuild(
        [("xUbuntu_24.10", "x86_64")],
        seen_live=seen_live,
        retried=retried,
        pending=["Fedora_44/x86_64: building"],
        ready=[],
    ) == []
    seen_live.add(("xUbuntu_24.10", "x86_64"))
    assert wait.maybe_rebuild(
        [("xUbuntu_24.10", "x86_64")],
        seen_live=seen_live,
        retried=retried,
        pending=["Fedora_44/x86_64: building"],
        ready=[],
    ) == [("xUbuntu_24.10", "x86_64")]
    retried.add(("xUbuntu_24.10", "x86_64"))
    assert wait.maybe_rebuild(
        [("xUbuntu_24.10", "x86_64")],
        seen_live=seen_live,
        retried=retried,
        pending=[],
        ready=["Fedora_44/x86_64: succeeded"],
    ) == []
    retried.clear()
    seen_live.clear()
    assert wait.maybe_rebuild(
        [("xUbuntu_24.10", "x86_64")],
        seen_live=seen_live,
        retried=retried,
        pending=[],
        ready=["Fedora_44/x86_64: succeeded"],
    ) == [("xUbuntu_24.10", "x86_64")]
    assert wait.collapse_results(
        [
            ("Debian_12", "x86_64", "succeeded"),
            ("Debian_12", "x86_64", "unresolvable"),
            ("openSUSE_Tumbleweed", "x86_64", "building"),
        ]
    ) == [
        ("Debian_12", "x86_64", "unresolvable"),
        ("openSUSE_Tumbleweed", "x86_64", "building"),
    ]
    assert wait.collapse_results(
        [
            ("Fedora_44", "x86_64", "finished"),
            ("Fedora_43", "x86_64", "building"),
        ]
    ) == [
        ("Fedora_44", "x86_64", "finished"),
        ("Fedora_43", "x86_64", "building"),
    ]

    orig_results = wait.results
    orig_names = wait.binary_names
    wait.results = lambda *_a, **_k: [
        ("Fedora_44", "x86_64", "finished"),
        ("Fedora_43", "x86_64", "building"),
    ]
    wait.binary_names = lambda *_a, **_k: ["am5-spd-diag-1.0.1-0.x86_64.rpm"]
    try:
        pending, failed, skipped, ready = wait.snapshot(None, "home:x", "am5-spd-diag", "1.0.1")
        assert failed == []
        assert skipped == []
        assert ready == []
        assert pending == [
            "Fedora_44/x86_64: finished (web UI may already show succeeded)",
            "Fedora_43/x86_64: building",
        ]
        wait.results = lambda *_a, **_k: [("Fedora_44", "x86_64", "succeeded")]
        pending, failed, skipped, ready = wait.snapshot(None, "home:x", "am5-spd-diag", "1.0.1")
        assert pending == []
        assert ready == ["Fedora_44/x86_64: succeeded"]
        wait.binary_names = lambda *_a, **_k: []
        pending, failed, skipped, ready = wait.snapshot(None, "home:x", "am5-spd-diag", "1.0.1")
        assert ready == []
        assert pending == ["Fedora_44/x86_64: binaries not listed yet"]

        orig_bytes = wait.osc_bytes
        api_calls: list[tuple[str, ...]] = []

        def fake_bytes(_config: str | None, *args: str) -> bytes:
            api_calls.append(args)
            assert args[0] == "api"
            assert "getbinaries" not in args
            return b"rpm-bytes"

        wait.osc_bytes = fake_bytes
        wait.results = lambda *_a, **_k: [("Fedora_44", "x86_64", "succeeded")]
        wait.binary_names = lambda *_a, **_k: [
            "am5-spd-diag-1.0.1-0.x86_64.rpm",
            "am5-spd-diag-1.0.1-0.src.rpm",
            "rpmlint.log",
        ]
        dest = Path(tempfile.mkdtemp())
        try:
            assert wait.download_binaries(None, "home:x", "am5-spd-diag", "1.0.1", str(dest)) == 0
            rpm = dest / "Fedora_44" / "x86_64" / "am5-spd-diag-1.0.1-0.x86_64.rpm"
            assert rpm.read_bytes() == b"rpm-bytes"
            assert not (dest / "Fedora_44" / "x86_64" / "am5-spd-diag-1.0.1-0.src.rpm").exists()
            assert len(api_calls) == 1
            assert api_calls[0] == (
                "api",
                "/build/home:x/Fedora_44/x86_64/am5-spd-diag/am5-spd-diag-1.0.1-0.x86_64.rpm",
            )
        finally:
            wait.osc_bytes = orig_bytes

        fetches = {"n": 0}
        seq = [
            [("xUbuntu_24.10", "x86_64", "failed")],
            [("xUbuntu_24.10", "x86_64", "succeeded")],
        ]

        def flipping_results(*_a, **_k):
            rows = seq[min(fetches["n"], 1)]
            fetches["n"] += 1
            return rows

        wait.results = flipping_results
        wait.binary_names = lambda *_a, **_k: ["am5-spd-diag-1.0.1-0.amd64.deb"]
        rows = wait.results(None, "home:x", "am5-spd-diag")
        pending, failed, skipped, ready = wait.snapshot(
            None, "home:x", "am5-spd-diag", "1.0.1", rows=rows
        )
        assert fetches["n"] == 1
        assert failed == ["xUbuntu_24.10/x86_64: failed"]
        assert ready == []
        failed_pairs = [(repo, arch) for repo, arch, code in rows if code in wait.BAD]
        assert failed_pairs == [("xUbuntu_24.10", "x86_64")]
        assert wait.maybe_rebuild(
            failed_pairs,
            seen_live={("xUbuntu_24.10", "x86_64")},
            retried=set(),
            pending=pending,
            ready=ready,
        ) == [("xUbuntu_24.10", "x86_64")]

        fetches["n"] = 0
        wait.results = flipping_results
        with tempfile.TemporaryDirectory() as dest2:
            assert wait.download_binaries(None, "home:x", "am5-spd-diag", "1.0.1", dest2) == 1
            assert fetches["n"] == 1
    finally:
        wait.results = orig_results
        wait.binary_names = orig_names


def test_obs_release_gate() -> None:
    sys.path.insert(0, str(ROOT / "scripts"))
    import obs_release as rel  # noqa: E402

    assert rel.parse_spec_version("Name: x\nVersion: 1.0.0\n") == "1.0.0"
    assert rel.allow_commit("1.0.0", None, False)
    assert rel.allow_commit("1.0.1", "1.0.0", False)
    assert rel.allow_commit("1.0.0", "1.0.0", False)
    assert not rel.allow_commit("1.0.0", "1.0.1", False)
    assert rel.allow_commit("1.0.0", "1.0.1", True)


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
    assert "SHA256SUMS" in later
    assert "gh release verify v1.0.1" in later
    assert "GitHub attached assets are archival copies of release packages." in later
    assert "convenience copies" not in later
    assert "convenience copies" not in pending


def test_dist_keeps_packaging_metadata_in_source0() -> None:
    """OBS %check runs the tagged Makefile, which reads .changes, spec, and debian/."""
    dist = (ROOT / "Makefile").read_text(encoding="utf-8").split("\ndist:", 1)[1]
    dist = dist.split("\n\n", 1)[0]
    assert "*.tar.xz" in dist
    assert "$(NAME).spec" not in dist
    assert "$(NAME).changes" not in dist
    assert "/debian" not in dist
    assert (ROOT / "am5-spd-diag.changes").is_file()
    assert (ROOT / "am5-spd-diag.spec").is_file()
    assert (ROOT / "debian/changelog").is_file()


def test_dist_splits_vendor_and_skips_rustc() -> None:
    makefile = (ROOT / "Makefile").read_text(encoding="utf-8")
    dist = makefile.split("\ndist:", 1)[1].split("\n\n", 1)[0]
    assert "-vendor.tar.zst" in dist
    assert "rm -rf" in dist
    assert "vendor" in dist
    assert "'/vendor/'" in dist or '"/vendor/"' in dist or "/vendor/" in dist
    assert "static.rust-lang.org" not in dist
    assert "osc-fetch-rust" in makefile
    assert (ROOT / "obs/rust-dist.txt").is_file()
    assert (ROOT / "rust-toolchain.toml").is_file()
    import check_rust_pin

    assert check_rust_pin.main() == 0
    pin = (ROOT / "obs/rust-dist.txt").read_text(encoding="utf-8")
    assert "https://static.rust-lang.org/dist/rust-" in pin
    assert "VERSION=" in pin
    ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    assert "dtolnay/rust-toolchain@stable" not in ci
    assert "dtolnay/rust-toolchain@stable" not in release
    assert "dtolnay/rust-toolchain@1." not in ci
    assert "dtolnay/rust-toolchain@1." not in release
    assert "toolchain: ${{ steps.rust.outputs.version }}" in ci
    assert "toolchain: ${{ steps.rust.outputs.version }}" in release
    assert "rust_pin.sh channel" in ci
    assert "rust_pin.sh channel" in release
    assert "print-osc-repos" in makefile
    assert "print-osc-repos" in ci
    repos = subprocess.check_output(
        ["make", "-s", "print-osc-repos"], cwd=ROOT, text=True
    ).split()
    assert "xUbuntu_24.10" in repos
    assert "openSUSE_Tumbleweed" in repos
    assert len(repos) >= 13
    assert "fail-fast: true" in ci
    assert "fromJSON(needs.dist.outputs.matrix)" in ci
    assert "needs: [test, dist, osc-build]" in ci
    assert "if: always()" in ci
    assert "github.event_name == 'pull_request'" in ci
    assert "github.event_name == 'workflow_dispatch'" in ci
    assert "OSC_VM_TYPE: chroot" in ci
    assert "OSC_PRELOAD" in ci
    assert "packagecachedir" in ci
    assert "OSC_PACKAGE_CACHE_DIR" in ci
    assert "scripts/osc_build.sh" in ci
    assert "obs_build_cmd.sh" in ci
    assert not re.search(r"^\s+osc(\s+-c\s+\S+)?\s+commit\b", ci, re.MULTILINE)
    osc_build = (ROOT / "scripts" / "osc_build.sh").read_text(encoding="utf-8")
    assert "OSC_VM_TYPE" in osc_build
    assert "OSC_PRELOAD" in osc_build
    assert "obs_build_cmd.sh" in osc_build
    assert '--config "$OSC_RC"' in osc_build
    assert not re.search(r'cmd\+=\(-c ', osc_build)
    assert "/usr/bin/obs-build" in (ROOT / "scripts" / "obs_build_cmd.sh").read_text(
        encoding="utf-8"
    )
    assert "/opt/obs-build/build" in (ROOT / "scripts" / "obs_build_cmd.sh").read_text(
        encoding="utf-8"
    )
    assert "openSUSE:/Tools/xUbuntu_24.04" in ci
    assert "ubuntu-24.04" in ci
    assert not re.search(r"^\s*osc(\s+-c\s+\S+)?\s+commit\b", osc_build, re.MULTILINE)
    assert "package-ecosystem: rust-toolchain" in (
        ROOT / ".github/dependabot.yml"
    ).read_text(encoding="utf-8")
    assert "needs.gate.outputs.obs" in release
    assert "ahead_by" in release
    assert "github.event_name == 'push' || inputs.commit_obs" not in release
    assert "actions/attest@" in release
    assert "SHA256SUMS" in release


def test_github_actions_pinned_to_full_sha() -> None:
    uses_re = re.compile(r"^\s+- uses:\s+(\S+)\s*(?:#.*)?$", re.MULTILINE)
    sha_re = re.compile(r"^[0-9a-f]{40}$")
    workflows = ROOT / ".github" / "workflows"
    found = 0
    for path in sorted(workflows.glob("*.yml")):
        text = path.read_text(encoding="utf-8")
        for match in uses_re.finditer(text):
            spec = match.group(1)
            if spec.startswith("./") or spec.startswith("docker://"):
                continue
            found += 1
            _action, sep, ref = spec.partition("@")
            assert sep, f"{path.name}: {spec} is missing @ref"
            assert sha_re.fullmatch(ref), (
                f"{path.name}: {spec} must pin a full 40-char commit SHA"
            )
    assert found >= 5


def test_obs_package_meta_disables_unwanted_repos() -> None:
    meta = (ROOT / "obs/package-meta.xml").read_text(encoding="utf-8")
    for repo in ("Fedora_Rawhide", "AppImage"):
        assert f'<disable repository="{repo}"/>' in meta
    for repo in (
        "xUbuntu_25.10",
        "xUbuntu_25.04",
        "xUbuntu_24.10",
        "xUbuntu_24.04",
        "Debian_12",
    ):
        assert f'repository="{repo}"' not in meta
    makefile = (ROOT / "Makefile").read_text(encoding="utf-8")
    for repo in (
        "xUbuntu_26.04",
        "xUbuntu_25.10",
        "xUbuntu_25.04",
        "xUbuntu_24.10",
        "xUbuntu_24.04",
    ):
        assert repo in makefile
    spec = (ROOT / "am5-spd-diag.spec").read_text(encoding="utf-8")
    assert '"%{?_repository}" == "16.0"' in spec
    assert "Release:        0.openSUSE_Leap_16.0" in spec
    prjconf = (ROOT / "obs/prjconf").read_text(encoding="utf-8")
    assert "Prefer: libselinux-dev" in prjconf
    assert "Prefer: libjpeg-dev" in prjconf
    assert "%if 0%{?suse_version}" in prjconf
    assert "Preinstall: shadow" in prjconf
    fmt = (ROOT / "debian/source/format").read_text(encoding="utf-8").strip()
    assert fmt == "1.0", fmt


def test_obs_build_cmd_preinstalls_shadow() -> None:
    wrapper = ROOT / "scripts" / "obs_build_cmd.sh"
    rpmlist = (
        "filesystem /cache/filesystem.rpm\n"
        "dbus-1-common /cache/dbus.rpm\n"
        "shadow /cache/shadow.rpm\n"
        "preinstall: filesystem\n"
        "vminstall: \n"
        "runscripts: \n"
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "rpmlist"
        path.write_text(rpmlist, encoding="utf-8")
        path.chmod(0o444)
        fake = Path(tmp) / "fake-build"
        args_file = Path(tmp) / "args"
        fake.write_text(
            "#!/bin/bash\nprintf '%s\\n' \"$@\" > \"$OBS_BUILD_ARGS\"\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        env = os.environ.copy()
        env["OBS_BUILD_REAL"] = str(fake)
        env["OBS_BUILD_ARGS"] = str(args_file)
        subprocess.check_call(
            ["bash", str(wrapper), f"--rpmlist={path}"], env=env
        )
        assert path.read_text(encoding="utf-8") == rpmlist
        passed = args_file.read_text(encoding="utf-8").strip()
        assert passed.startswith("--rpmlist=")
        patched = Path(passed.split("=", 1)[1])
        assert patched != path
        assert "preinstall: filesystem shadow" in patched.read_text(encoding="utf-8")
        env_miss = os.environ.copy()
        env_miss["OBS_BUILD_REAL"] = str(Path(tmp) / "no-such-build")
        proc = subprocess.run(
            ["bash", str(wrapper), f"--rpmlist={path}"],
            env=env_miss,
            capture_output=True,
            text=True,
        )
        assert proc.returncode != 0
        assert "not executable" in proc.stderr
        deb = Path(tmp) / "deb-rpmlist"
        deb.write_text(
            "libc6 /cache/libc6.deb\npreinstall: build-essential\n",
            encoding="utf-8",
        )
        deb.chmod(0o444)
        subprocess.check_call(
            ["bash", str(wrapper), f"--rpmlist={deb}"], env=env
        )
        assert "preinstall: build-essential" in deb.read_text(encoding="utf-8")
        passed_deb = args_file.read_text(encoding="utf-8").strip()
        assert passed_deb == f"--rpmlist={deb}"


def test_release_profile_and_rpmlint() -> None:
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    assert "[profile.release]" in cargo
    assert "strip = true" in cargo
    assert 'lto = "thin"' in cargo
    spec = (ROOT / "am5-spd-diag.spec").read_text(encoding="utf-8")
    assert "%define debug_package %{nil}" in spec
    assert "%service_add_pre am5-spd-diag.service" in spec
    assert "%service_add_post am5-spd-diag.service" in spec
    assert "%service_del_preun am5-spd-diag.service" in spec
    assert "%service_del_postun_without_restart am5-spd-diag.service" in spec
    assert "%systemd_postun am5-spd-diag.service" in spec
    assert "%systemd_postun_with_restart" not in spec
    assert "%service_del_postun am5-spd-diag.service\n" not in spec
    assert re.search(r"^%pre\b", spec, re.MULTILINE)
    units = (
        "am5-spd-diag.service",
        "am5-spd-diag-pre-sleep.service",
        "am5-spd-diag-post-sleep.service",
    )
    for unit in units:
        assert f"enable {unit}" in (ROOT / "systemd/50-am5-spd-diag.preset").read_text(
            encoding="utf-8"
        )
        assert unit in spec
    assert "%{_prefix}/lib/systemd/system-preset/50-%{name}.preset" in spec
    assert "systemctl --no-reload preset" in spec
    assert "is-active --quiet am5-spd-diag.service" in spec
    assert "systemctl start am5-spd-diag.service" in spec
    assert "systemctl start am5-spd-diag-pre-sleep.service" not in spec
    assert "systemctl start am5-spd-diag-post-sleep.service" not in spec
    for i, line in enumerate(spec.splitlines(), 1):
        stripped = line.lstrip()
        if not stripped.startswith("#"):
            continue
        if re.search(r"(?<!%)%(service_|systemd_)", stripped.replace("%%", "\0")):
            raise AssertionError(
                f"am5-spd-diag.spec:{i}: unescaped systemd macro in comment: {line}"
            )
    makefile = (ROOT / "Makefile").read_text(encoding="utf-8")
    assert "ln -f $(DESTDIR)$(LIBEXECDIR)/$(NAME) $(DESTDIR)$(LIBEXECDIR)/pkexec-snapshot" in makefile
    assert "ln -sf ../libexec/$(NAME)/$(NAME) $(DESTDIR)$(BINDIR)/$(NAME)" in makefile
    assert "ln -f $(DESTDIR)$(BINDIR)/$(NAME) $(DESTDIR)$(LIBEXECDIR)/pkexec-snapshot" not in makefile
    assert "systemd/50-$(NAME).preset" in makefile
    for rules in (
        (ROOT / "debian.rules").read_text(encoding="utf-8"),
        (ROOT / "debian" / "rules").read_text(encoding="utf-8"),
    ):
        assert "noautodbgsym" in rules
        assert "--no-start" in rules
        assert "--no-enable" not in rules
        assert "am5-spd-diag-pre-sleep.service" in rules
        assert "am5-spd-diag-post-sleep.service" in rules
        boot_starts = [
            line
            for line in rules.splitlines()
            if "dh_installsystemd" in line and "am5-spd-diag.service" in line
        ]
        assert boot_starts, "debian rules must start the boot unit"
        assert all("--no-start" not in line for line in boot_starts)
    rpmlintrc = (ROOT / "am5-spd-diag.rpmlintrc").read_text(encoding="utf-8")
    for check in (
        "polkit-user-privilege",
        "polkit-untracked-privilege",
        "polkit-file-unauthorized",
    ):
        assert check in rpmlintrc
        assert f'setBadness("{check}", 0)' in rpmlintrc
    assert "addFilter(" not in rpmlintrc
    osc_build = (ROOT / "scripts" / "osc_build.sh").read_text(encoding="utf-8")
    assert "$NAME.rpmlintrc" in osc_build
    release = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
    assert "$OBS_PACKAGE.rpmlintrc" in release
    assert "scripts/obs_commit_msg.py" in release
    assert "osc commit -F" in release
    assert 'osc commit -m "Release $VERSION from $TAG ($SOURCE_SHA)"' not in release


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
    test_release_notes_mentions_obs()
    test_dist_keeps_packaging_metadata_in_source0()
    test_dist_splits_vendor_and_skips_rustc()
    test_github_actions_pinned_to_full_sha()
    test_obs_package_meta_disables_unwanted_repos()
    test_release_profile_and_rpmlint()
    print("ok")
