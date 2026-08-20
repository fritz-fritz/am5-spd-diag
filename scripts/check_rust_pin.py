#!/usr/bin/env python3
"""Fail if rust-toolchain.toml, rust-version, spec Source2, OBS pin, and workflows disagree."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HOST = "x86_64-unknown-linux-gnu"
ACTION_TAG_RE = re.compile(r"dtolnay/rust-toolchain@([^\s]+)")


def fail(msg: str) -> None:
    print(f"check_rust_pin: {msg}", file=sys.stderr)
    raise SystemExit(1)


def parse_dist(path: Path) -> dict[str, str]:
    fields: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        fields[key] = value
    for key in ("VERSION", "URL", "SHA256"):
        if key not in fields:
            fail(f"{path} missing {key}")
    return fields


def toolchain_channel(path: Path) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("channel = "):
            return line.split("=", 1)[1].strip().strip('"')
    fail(f"{path} missing channel")
    raise AssertionError


def cargo_rust_version(path: Path) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("rust-version = "):
            return line.split("=", 1)[1].strip().strip('"')
    fail(f"{path} missing rust-version")
    raise AssertionError


def spec_source2(path: Path) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("Source2:"):
            return line.split(":", 1)[1].strip()
    fail(f"{path} missing Source2")
    raise AssertionError


def check_workflow(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(ROOT)
    tags = ACTION_TAG_RE.findall(text)
    if not tags:
        fail(f"{rel} has no dtolnay/rust-toolchain action")
    for tag in tags:
        tag = tag.strip()
        if tag in {"stable", "beta", "nightly"} or re.fullmatch(
            r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", tag
        ):
            fail(
                f"{rel} pins rustc via dtolnay/rust-toolchain@{tag}; "
                "parse rust-toolchain.toml and pass toolchain:"
            )
    if "toolchain: ${{ steps.rust.outputs.version }}" not in text:
        fail(f"{rel} must set with.toolchain from the parsed rust-toolchain.toml pin")
    if "rust_pin.sh channel" not in text:
        fail(f"{rel} must parse the pin with rust_pin.sh channel")


def main() -> int:
    channel = toolchain_channel(ROOT / "rust-toolchain.toml")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel):
        fail(f"rust-toolchain.toml channel must be X.Y.Z (got {channel})")

    dist = parse_dist(ROOT / "obs/rust-dist.txt")
    version = dist["VERSION"]
    if version != channel:
        fail(f"obs/rust-dist.txt VERSION {version} != rust-toolchain.toml {channel}")
    want_file = f"rust-{version}-{HOST}.tar.xz"
    want_url = f"https://static.rust-lang.org/dist/{want_file}"
    if dist["URL"] != want_url:
        fail(f"URL must be {want_url}")
    if not re.fullmatch(r"[0-9a-f]{64}", dist["SHA256"]):
        fail("SHA256 must be 64 lowercase hex chars")

    msrv = cargo_rust_version(ROOT / "Cargo.toml")
    if msrv != version:
        fail(f"Cargo.toml rust-version {msrv} != {version}")

    source2 = spec_source2(ROOT / "am5-spd-diag.spec")
    if source2 != want_file:
        fail(f"spec Source2 {source2} != {want_file}")

    for rel in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
        check_workflow(ROOT / rel)

    prep = (ROOT / "scripts/obs_prep.sh").read_text(encoding="utf-8")
    if re.search(r"rust-1\.[0-9]+\.[0-9]+", prep):
        fail("obs_prep.sh must not hardcode a rustc version; read obs/rust-dist.txt")

    osc = (ROOT / "scripts/osc_build.sh").read_text(encoding="utf-8")
    if re.search(r"rust-1\.[0-9]+\.[0-9]+", osc):
        fail("osc_build.sh must not hardcode a rustc version; use scripts/rust_pin.sh")

    release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    if re.search(r"rust-1\.[0-9]+\.[0-9]+-x86_64", release):
        fail("release.yml must not hardcode the rustc tarball name; use rust_pin.sh")

    makefile = (ROOT / "Makefile").read_text(encoding="utf-8")
    if re.search(r"RUST_DIST_FILE := rust-1\.", makefile):
        fail("Makefile must not hardcode RUST_DIST_FILE")

    print(f"check_rust_pin: rustc {version} ({want_file})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
