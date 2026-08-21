#!/usr/bin/env python3
"""Wait until OBS has versioned binaries for every enabled repo."""
from __future__ import annotations

import argparse
import os
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


def osc_bytes(config: str | None, *args: str) -> bytes:
    cmd = ["osc"]
    if config:
        cmd.extend(["-c", config])
    cmd.extend(args)
    return subprocess.check_output(cmd)


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


# Official OBS Leap repo name is "16.0". The spec encodes OpenSUSE in
# Release so that rpm is archived under its OBS basename.
KEEP_OBS_BASENAME = {"16.0"}


def github_asset_name(repo: str, filename: str) -> str:
    """GitHub asset name for an OBS binary.

    Most repos share ``name-version-release.arch.rpm`` / the same ``.deb``,
    so GitHub gets ``stem.repo.ext``. Leap 16.0 already has a unique OBS
    filename; keep it so the archive matches what OBS publishes.
    """
    if repo in KEEP_OBS_BASENAME:
        return filename
    if "." not in filename:
        return f"{filename}.{repo}"
    stem, ext = filename.rsplit(".", 1)
    return f"{stem}.{repo}.{ext}"


def finished_ok(
    pending: list[str],
    ready: list[str],
    failed: list[str] | None = None,
    *,
    retries_exhausted: bool = False,
) -> bool | None:
    """None = keep polling. True = every enabled repo has a payload. False = give up.

    Remaining ``failed`` is never success. Stale failed rows after ``osc commit``
    keep polling until OBS schedules the new build, a retry runs, or timeout.
    After each failed target has used its one rebuild and is still failed,
    return False so Release does not collect an incomplete set.
    """
    if pending:
        return None
    if failed:
        return False if retries_exhausted else None
    if ready:
        return True
    return False


def maybe_rebuild(
    failed_targets: list[tuple[str, str]],
    *,
    seen_live: set[tuple[str, str]],
    retried: set[tuple[str, str]],
    pending: list[str],
    ready: list[str],
) -> list[tuple[str, str]]:
    """(repo, arch) pairs to rebuild this tick. At most one rebuild per pair ever."""
    wave_done = not pending and bool(ready)
    out: list[tuple[str, str]] = []
    for pair in failed_targets:
        if pair in retried:
            continue
        if pair in seen_live or wave_done:
            out.append(pair)
    return out


def rebuild_package(
    config: str | None, project: str, package: str, repo: str, arch: str
) -> None:
    osc(config, "rebuildpac", project, package, "-r", repo, "-a", arch)


def snapshot(
    config: str | None,
    project: str,
    package: str,
    version: str,
    *,
    rows: list[tuple[str, str, str]] | None = None,
) -> tuple[list[str], list[str], list[str], list[str]]:
    """Classify one `_result` fetch. Pass `rows` so wait/collect share a tick."""
    if rows is None:
        rows = results(config, project, package)
    pending: list[str] = []
    failed: list[str] = []
    skipped: list[str] = []
    ready: list[str] = []
    for repo, arch, code in rows:
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
    rows = results(config, project, package)
    pending, failed, skipped, ready = snapshot(
        config, project, package, version, rows=rows
    )
    if skipped:
        print("skip:   " + ", ".join(skipped))
    if failed:
        print("failed: " + ", ".join(failed), file=sys.stderr)
    if pending or failed or not ready:
        print("obs_wait: binaries for this version are not ready yet", file=sys.stderr)
        return 1
    count = 0
    for repo, arch, code in rows:
        if code in SKIP or code in BAD or code not in DONE:
            continue
        names = [
            name
            for name in binary_names(config, project, package, repo, arch)
            if is_payload(name, version)
        ]
        if not names:
            continue
        target = f"{dest.rstrip('/')}/{repo}/{arch}"
        os.makedirs(target, exist_ok=True)
        print(f"collect: {repo}/{arch} -> {target}", flush=True)
        for name in names:
            path = os.path.join(target, name)
            data = osc_bytes(
                config,
                "api",
                f"/build/{project}/{repo}/{arch}/{package}/{name}",
            )
            with open(path, "wb") as fh:
                fh.write(data)
            print(f"downloaded {repo}/{arch}/{name}", flush=True)
        count += 1
    if count == 0:
        print("obs_wait: no repositories to download", file=sys.stderr)
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project")
    parser.add_argument("--package")
    parser.add_argument("--version")
    parser.add_argument("--config", help="osc config file")
    parser.add_argument("--timeout", type=int, default=5400, help="seconds")
    parser.add_argument("--interval", type=int, default=30, help="poll interval seconds")
    parser.add_argument(
        "--getbinaries",
        metavar="DIR",
        help="download versioned rpm/deb payloads to DIR instead of waiting",
    )
    parser.add_argument(
        "--asset-name",
        nargs=2,
        metavar=("REPO", "FILE"),
        help="print the GitHub asset name for an OBS binary and exit",
    )
    args = parser.parse_args(argv)
    if args.asset_name:
        print(github_asset_name(args.asset_name[0], args.asset_name[1]))
        return 0
    if not args.project or not args.package or not args.version:
        parser.error("--project, --package, and --version are required")
    if args.getbinaries:
        try:
            return download_binaries(
                args.config, args.project, args.package, args.version, args.getbinaries
            )
        except subprocess.CalledProcessError as err:
            print(f"obs_wait: osc failed: {err}", file=sys.stderr)
            return 1

    deadline = time.monotonic() + args.timeout
    seen_live: set[tuple[str, str]] = set()
    retried: set[tuple[str, str]] = set()
    awaiting_schedule: set[tuple[str, str]] = set()
    while True:
        try:
            rows = results(args.config, args.project, args.package)
            pending, failed, skipped, ready = snapshot(
                args.config,
                args.project,
                args.package,
                args.version,
                rows=rows,
            )
        except subprocess.CalledProcessError as err:
            print(f"obs_wait: osc failed: {err}", file=sys.stderr)
            if time.monotonic() >= deadline:
                return 1
            time.sleep(args.interval)
            continue
        for repo, arch, code in rows:
            pair = (repo, arch)
            if code in LIVE:
                seen_live.add(pair)
                awaiting_schedule.discard(pair)
        failed_pairs = [(repo, arch) for repo, arch, code in rows if code in BAD]
        to_retry = maybe_rebuild(
            failed_pairs,
            seen_live=seen_live,
            retried=retried,
            pending=pending,
            ready=ready,
        )
        print("ready:  " + (", ".join(ready) if ready else "(none)"))
        print("wait:   " + (", ".join(pending) if pending else "(none)"))
        print("skip:   " + (", ".join(skipped) if skipped else "(none)"))
        if failed:
            print("failed: " + ", ".join(failed), file=sys.stderr)
        if to_retry:
            for repo, arch in to_retry:
                print(f"obs_wait: retry {repo}/{arch} (once)", flush=True)
                try:
                    rebuild_package(
                        args.config, args.project, args.package, repo, arch
                    )
                except subprocess.CalledProcessError as err:
                    print(f"obs_wait: rebuild {repo}/{arch} failed: {err}", file=sys.stderr)
                    return 1
                retried.add((repo, arch))
                awaiting_schedule.add((repo, arch))
            if time.monotonic() >= deadline:
                print("obs_wait: timed out waiting for OBS binaries", file=sys.stderr)
                return 1
            time.sleep(args.interval)
            continue
        retries_exhausted = (
            bool(failed_pairs)
            and not awaiting_schedule
            and all(pair in retried for pair in failed_pairs)
        )
        outcome = finished_ok(
            pending, ready, failed, retries_exhausted=retries_exhausted
        )
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
