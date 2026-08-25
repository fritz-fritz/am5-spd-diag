#!/usr/bin/env python3
"""Run osc after giving urllib3 a connect timeout.

osc 1.27 constructs PoolManager/HTTPSConnectionPool with timeout=None, so a
dead mirror hangs until the TCP stack gives up (CI logs show connect
timeout=None for 7-9 minutes). Read timeout stays unlimited so large CPIO
fetches can finish.
"""
from __future__ import annotations

import os
import sys


def install() -> None:
    try:
        connect = float(os.environ.get("OSC_HTTP_CONNECT_TIMEOUT", "30"))
    except ValueError:
        connect = 30.0
    if connect <= 0:
        return

    import urllib3

    timeout = urllib3.Timeout(connect=connect, read=None)

    orig_pm = urllib3.PoolManager.__init__

    def pm_init(self, *args, **kwargs):
        kwargs.setdefault("timeout", timeout)
        orig_pm(self, *args, **kwargs)

    urllib3.PoolManager.__init__ = pm_init  # type: ignore[method-assign]

    orig_https = urllib3.HTTPSConnectionPool.__init__

    def https_init(self, *args, **kwargs):
        if "timeout" not in kwargs and len(args) < 3:
            kwargs["timeout"] = timeout
        orig_https(self, *args, **kwargs)

    urllib3.HTTPSConnectionPool.__init__ = https_init  # type: ignore[method-assign]

    try:
        import osc.connection as conn
    except ImportError:
        return
    conn.POOL_MANAGER = urllib3.PoolManager()


def main() -> int:
    install()
    sys.argv[0] = "/usr/bin/osc"
    from osc.babysitter import main as osc_main

    return osc_main()


if __name__ == "__main__":
    raise SystemExit(main())
