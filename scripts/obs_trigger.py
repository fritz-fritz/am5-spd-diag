#!/usr/bin/env python3
"""Fire an OBS rebuild trigger token (Profile → Tokens → rebuild)."""
from __future__ import annotations

import argparse
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_API = "https://api.opensuse.org"


def rebuild_url(api: str, project: str, package: str) -> str:
    query = urllib.parse.urlencode({"project": project, "package": package})
    return f"{api.rstrip('/')}/trigger/rebuild?{query}"


def trigger_rebuild(
    token: str,
    project: str,
    package: str,
    api: str = DEFAULT_API,
) -> str:
    request = urllib.request.Request(
        rebuild_url(api, project, package),
        data=b"",
        method="POST",
        headers={"Authorization": f"Token {token}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as err:
        detail = err.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OBS rebuild trigger failed: HTTP {err.code}\n{detail}") from err


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--token", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--api", default=DEFAULT_API)
    args = parser.parse_args(argv)
    try:
        body = trigger_rebuild(args.token, args.project, args.package, api=args.api)
    except RuntimeError as err:
        print(err, file=sys.stderr)
        return 1
    sys.stdout.write(body)
    if body and not body.endswith("\n"):
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
