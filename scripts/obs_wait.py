#!/usr/bin/env python3
"""Wait until OBS has versioned binaries. Sibling repo failures are skipped."""
from __future__ import annotations

import argparse
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from collections import defaultdict

SKIP = {"disabled", "excluded", "unresolvable"}
BAD = {"failed", "broken"}
DONE = {"succeeded"}
# `finished` then `signing` are real post-build steps. The web UI often already
# shows succeeded, but GitHub Releases are immutable: collect only after every
# enabled repo is succeeded and a versioned rpm/deb is listed.
LIVE_ACTIVE = {"scheduled", "dispatching", "building", "blocked"}
LIVE_POST = {"signing", "finished"}
LIVE = LIVE_ACTIVE | LIVE_POST


def osc(config: str | None, *args: str) -> str:
    cmd = ["osc"]
    if config:
        cmd.extend(["-c", config])
    cmd.extend(args)
    return subprocess.check_output(cmd, text=True)


def results(config: str | None, project: str, package: str) -> list[tuple[str, str, str]]:
    xml = osc(config, "api", f"/build/{project}/_result?package={package}")
    root = ET.fromstring(xml)
    rows: list[tuple[str, str, str]] = []
    for result in root.findall("result"):
        repo = result.get("repository") or ""
        arch = result.get("arch") or ""
        for status in result.findall("status"):
            if status.get("package") != package:
                continue
            rows.append((repo, arch, status.get("code") or "unknown"))
    return collapse_results(rows)


def collapse_results(rows: list[tuple[str, str, str]]) -> list[tuple[str, str, str]]:
    """One code per repo/arch. Prefer a live rebuild over a stale succeeded row."""
    grouped: dict[tuple[str, str], list[str]] = defaultdict(list)
    for repo, arch, code in rows:
        grouped[(repo, arch)].append(code)
    return [(repo, arch, classify_codes(codes)) for (repo, arch), codes in grouped.items()]


def classify_codes(codes: list[str]) -> str:
    if any(code in LIVE_ACTIVE for code in codes):
        return next(code for code in codes if code in LIVE_ACTIVE)
    if any(code in LIVE_POST for code in codes):
        return next(code for code in codes if code in LIVE_POST)
    if any(code in BAD for code in codes):
        return next(code for code in codes if code in BAD)
    if any(code in SKIP for code in codes):
        return next(code for code in codes if code in SKIP)
    if any(code in DONE for code in codes):
        return "succeeded"
    return codes[-1] if codes else "unknown"


def status_label(repo: str, arch: str, code: str) -> str:
    extra = ""
    if code in {"finished", "signing"}:
        extra = " (web UI may already show succeeded)"
    return f"{repo}/{arch}: {code}{extra}"


def binary_names(config: str | None, project: str, package: str, repo: str, arch: str) -> list[str]:
    try:
        xml = osc(config, "api", f"/build/{project}/{repo}/{arch}/{package}")
    except subprocess.CalledProcessError:
        return []
    root = ET.fromstring(xml)
    return [node.get("filename") or "" for node in root.findall("binary")]


def is_payload(name: str, version: str) -> bool:
    if version not in name:
        return False
    if name.endswith(".src.rpm"):
        return False
    lower = name.lower()
    if any(part in lower for part in ("debuginfo", "debugsource", "dbgsym")):
        return False
    return name.endswith(".rpm") or name.endswith(".deb")


def finished_ok(
    pending: list[str],
    ready: list[str],
    failed: list[str] | None = None,
) -> bool | None:
    """None = keep polling. True = versioned payloads exist. False = give up.

    Stale ``failed`` rows after ``osc commit`` must not abort: OBS often has
    not scheduled the rebuild yet. Keep polling until a live code appears,
    a payload shows up, or the timeout hits.
    """
    if pending:
        return None
    if ready:
        return True
    if failed:
        return None
    return False


def snapshot(
    config: str | None, project: str, package: str, version: str
) -> tuple[list[str], list[str], list[str], list[str]]:
    pending: list[str] = []
    failed: list[str] = []
    skipped: list[str] = []
    ready: list[str] = []
    for repo, arch, code in results(config, project, package):
        label = status_label(repo, arch, code)
        if code in SKIP:
            skipped.append(label)
            continue
        if code in BAD:
            failed.append(label)
            continue
        if code in LIVE:
            pending.append(label)
            continue
        has_payload = any(
            is_payload(name, version)
            for name in binary_names(config, project, package, repo, arch)
        )
        if has_payload:
            ready.append(label)
            continue
        if code in DONE:
            pending.append(f"{repo}/{arch}: binaries not listed yet")
        else:
            pending.append(label)
    return pending, failed, skipped, ready


def download_binaries(
    config: str | None,
    project: str,
    package: str,
    version: str,
    dest: str,
) -> int:
    pending, failed, skipped, ready = snapshot(config, project, package, version)
    if skipped:
        print("skip:   " + ", ".join(skipped))
    if failed:
        print("failed: " + ", ".join(failed), file=sys.stderr)
    if pending or not ready:
        print("obs_wait: binaries for this version are not ready yet", file=sys.stderr)
        return 1
    count = 0
    for repo, arch, code in results(config, project, package):
        if code in SKIP or code in BAD or code not in DONE:
            continue
        names = binary_names(config, project, package, repo, arch)
        if not any(is_payload(name, version) for name in names):
            continue
        target = f"{dest.rstrip('/')}/{repo}/{arch}"
        subprocess.check_call(["mkdir", "-p", target])
        cmd = ["osc"]
        if config:
            cmd.extend(["-c", config])
        cmd.extend(["getbinaries", project, package, repo, arch, "-d", target])
        print(f"collect: {repo}/{arch} -> {target}", flush=True)
        subprocess.check_call(cmd)
        count += 1
        print(f"downloaded {repo}/{arch}", flush=True)
    if count == 0:
        print("obs_wait: no repositories to download", file=sys.stderr)
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--config", help="osc config file")
    parser.add_argument("--timeout", type=int, default=5400, help="seconds")
    parser.add_argument("--interval", type=int, default=30, help="poll interval seconds")
    parser.add_argument(
        "--getbinaries",
        metavar="DIR",
        help="download versioned binaries to DIR instead of waiting",
    )
    args = parser.parse_args(argv)
    if args.getbinaries:
        try:
            return download_binaries(
                args.config, args.project, args.package, args.version, args.getbinaries
            )
        except subprocess.CalledProcessError as err:
            print(f"obs_wait: osc failed: {err}", file=sys.stderr)
            return 1

    deadline = time.monotonic() + args.timeout
    while True:
        try:
            pending, failed, skipped, ready = snapshot(
                args.config, args.project, args.package, args.version
            )
        except subprocess.CalledProcessError as err:
            print(f"obs_wait: osc failed: {err}", file=sys.stderr)
            if time.monotonic() >= deadline:
                return 1
            time.sleep(args.interval)
            continue
        print("ready:  " + (", ".join(ready) if ready else "(none)"))
        print("wait:   " + (", ".join(pending) if pending else "(none)"))
        print("skip:   " + (", ".join(skipped) if skipped else "(none)"))
        if failed:
            print("failed: " + ", ".join(failed), file=sys.stderr)
        outcome = finished_ok(pending, ready, failed)
        if outcome is True:
            print("obs_wait: versioned binaries are ready")
            return 0
        if outcome is False:
            print("obs_wait: no repositories produced versioned binaries", file=sys.stderr)
            return 1
        if time.monotonic() >= deadline:
            print("obs_wait: timed out waiting for OBS binaries", file=sys.stderr)
            return 1
        time.sleep(args.interval)


if __name__ == "__main__":
    raise SystemExit(main())
