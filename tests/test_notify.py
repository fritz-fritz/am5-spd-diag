#!/usr/bin/env python3
"""Persistent desktop notification helpers."""
from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "libexec"))
import notify_app  # noqa: E402
import spd_hub  # noqa: E402


def test_fdo_notify_persists_with_actions() -> None:
    argv = notify_app.fdo_notify_argv("SPD corruption detected", "body")
    assert argv[argv.index("Notify") + 1] == "susssasa{sv}i"
    assert argv[-1] == "0"
    assert "15000" not in argv
    assert "resident" in argv
    assert argv[argv.index("desktop-entry") + 2] == notify_app.APP_ID
    assert argv[argv.index("urgency") + 2] == "2"
    assert argv[argv.index("susssasa{sv}i") + 6] == "6"
    assert "default" in argv
    assert "analyze" in argv
    assert "Analyze" in argv
    assert "report" in argv
    assert "Report" in argv


def test_pack_notify_body_is_urgent_persistent() -> None:
    body = notify_app.pack_notify_body("title", "hello")
    assert b"analyze" in body
    assert b"report" in body
    assert b"dialog-warning" in body
    assert body[-4:] == b"\x00\x00\x00\x00"
    assert notify_app.APP_ID.encode() in body


def test_pack_dismiss_body_expires_into_history() -> None:
    body = notify_app.pack_dismiss_body(12, "title", "hello")
    assert b"resident" not in body
    assert b"analyze" not in body
    assert body[-4:] == b"\x01\x00\x00\x00"
    pos, _ = notify_app._parse_string(body, 0)
    pos = notify_app._align(pos, 4)
    (replaces_id,) = __import__("struct").unpack_from("<I", body, pos)
    assert replaces_id == 12


def test_fdo_dismiss_replaces_without_close() -> None:
    argv = notify_app.fdo_dismiss_argv(7, "title", "body")
    assert argv[argv.index("Notify") + 1] == "susssasa{sv}i"
    assert "CloseNotification" not in argv
    assert "resident" not in argv
    assert argv[argv.index("susssasa{sv}i") + 2] == "7"
    assert argv[argv.index("susssasa{sv}i") + 6] == "0"
    assert argv[-1] == "1"


def test_gtk_remove_argv() -> None:
    argv = notify_app.gtk_remove_argv()
    assert "RemoveNotification" in argv
    assert notify_app.APP_ID in argv
    assert notify_app.NOTIFY_ID in argv


def test_parse_action_invoked() -> None:
    body = bytearray()
    notify_app._append_u32(body, 12)
    notify_app._append_string(body, "analyze")
    nid, action = notify_app.parse_action_invoked(bytes(body))
    assert nid == 12
    assert action == "analyze"


def test_portal_actions_are_status_analyze_report() -> None:
    for argv in (
        notify_app.gtk_notify_argv("t", "b"),
        notify_app.portal_notify_argv("t", "b"),
        notify_app.kde_impl_notify_argv("t", "b"),
    ):
        assert "app.status" in argv
        assert "app.analyze" in argv
        assert "app.report" in argv
        assert "Analyze" in argv
        assert "Report" in argv
        assert "urgent" in argv
        assert "default-action" in argv
        assert argv[argv.index("aa{sv}") + 1] == "2"


def test_portal_requests_persistent_display() -> None:
    portal = notify_app.portal_notify_argv("t", "b")
    kde = notify_app.kde_impl_notify_argv("t", "b")
    gtk = notify_app.gtk_notify_argv("t", "b")
    assert "display-hint" in portal
    assert "persistent" in portal
    assert "display-hint" in kde
    assert "persistent" in kde
    assert "display-hint" not in gtk


def test_plasma_impl_dest_is_explicit() -> None:
    argv = notify_app.kde_impl_notify_argv("t", "b")
    assert "org.freedesktop.impl.portal.desktop.plasmanotify" in argv
    gtk = notify_app.gtk_notify_argv("t", "b")
    kde = notify_app.kde_impl_notify_argv("t", "b")
    assert gtk[gtk.index("ssa{sv}") + 1] == notify_app.APP_ID
    assert kde[kde.index("ssa{sv}") + 1] == notify_app.APP_ID


def test_notify_user_argv_uses_session_bus() -> None:
    argv = spd_hub.notify_user_argv("/run/user/1000/bus", "title", "body")
    assert "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus" in argv
    assert "XDG_RUNTIME_DIR=/run/user/1000" in argv
    assert "--notify" in argv
    assert argv[-2:] == ["title", "body"]
    if os.geteuid() == 0:
        assert argv[0] == "runuser"
        assert argv[1] == "-u"
    else:
        assert argv[0] == "env"


def test_uid_from_bus_path() -> None:
    assert spd_hub.uid_from_bus_path("/run/user/1000/bus") == 1000


def test_dbus_hello_roundtrip() -> None:
    blob = notify_app.pack_message(
        notify_app.MSG_METHOD_CALL,
        1,
        path=notify_app.DBUS_PATH,
        interface=notify_app.DBUS_IFACE,
        member="Hello",
        destination=notify_app.DBUS_NAME,
    )
    parsed, used = notify_app.parse_message(blob)
    assert used == len(blob)
    assert blob[12] == 0x6D
    assert parsed["type"] == notify_app.MSG_METHOD_CALL
    assert parsed["serial"] == 1
    assert parsed["member"] == "Hello"
    assert parsed["destination"] == notify_app.DBUS_NAME
    assert parsed["path"] == notify_app.DBUS_PATH


def test_action_from_call() -> None:
    body = bytearray()
    notify_app._append_string(body, "app.report")
    assert (
        notify_app.action_from_call(
            {
                "type": notify_app.MSG_METHOD_CALL,
                "interface": notify_app.APP_INTERFACE,
                "member": "ActivateAction",
                "body": bytes(body),
            }
        )
        == "report"
    )
    assert notify_app.normalize_action("analyze") == "analyze"
    assert notify_app.normalize_action("app.status") == "status"
    assert notify_app.normalize_action("default") == "status"
    assert (
        notify_app.action_from_call(
            {
                "type": notify_app.MSG_METHOD_CALL,
                "interface": notify_app.APP_INTERFACE,
                "member": "Activate",
                "body": b"",
            }
        )
        == "status"
    )


def test_parse_systemd_env_file() -> None:
    parsed = notify_app.parse_systemd_env_file(
        "DISPLAY=:0\nWAYLAND_DISPLAY='wayland-0'\nPATH=/usr/bin\n# skip\n"
    )
    assert parsed["DISPLAY"] == ":0"
    assert parsed["WAYLAND_DISPLAY"] == "wayland-0"
    assert parsed["PATH"] == "/usr/bin"


def test_systemd_user_run_argv_forwards_token() -> None:
    env = {
        "XDG_RUNTIME_DIR": "/run/user/1000",
        "XDG_ACTIVATION_TOKEN": "tok",
        "DISPLAY": ":0",
        "WAYLAND_DISPLAY": "wayland-0",
    }
    argv = notify_app.systemd_user_run_argv(
        ["/helper", "analyze"],
        env,
        runner="/usr/bin/systemd-run",
    )
    assert argv is not None
    assert argv[:3] == ["/usr/bin/systemd-run", "--user", "--collect"]
    assert "--setenv=XDG_ACTIVATION_TOKEN=tok" in argv
    assert "--setenv=WAYLAND_DISPLAY=wayland-0" in argv
    assert argv[-3:] == ["--", "/helper", "analyze"]


def test_systemd_user_run_argv_requires_runtime_dir() -> None:
    assert (
        notify_app.systemd_user_run_argv(
            ["/helper", "status"],
            {},
            runner="/usr/bin/systemd-run",
        )
        is None
    )


def test_is_application_call() -> None:
    assert notify_app.is_application_call(
        {"type": notify_app.MSG_METHOD_CALL, "interface": notify_app.APP_INTERFACE, "member": "ActivateAction"}
    )
    assert notify_app.is_application_call(
        {"type": notify_app.MSG_METHOD_CALL, "interface": notify_app.APP_INTERFACE, "member": "Activate"}
    )
    assert not notify_app.is_application_call(
        {"type": notify_app.MSG_METHOD_RETURN, "interface": notify_app.APP_INTERFACE, "member": "Activate"}
    )


if __name__ == "__main__":
    test_fdo_notify_persists_with_actions()
    test_pack_notify_body_is_urgent_persistent()
    test_pack_dismiss_body_expires_into_history()
    test_fdo_dismiss_replaces_without_close()
    test_gtk_remove_argv()
    test_parse_action_invoked()
    test_portal_actions_are_status_analyze_report()
    test_portal_requests_persistent_display()
    test_plasma_impl_dest_is_explicit()
    test_notify_user_argv_uses_session_bus()
    test_uid_from_bus_path()
    test_dbus_hello_roundtrip()
    test_action_from_call()
    test_is_application_call()
    test_parse_systemd_env_file()
    test_systemd_user_run_argv_forwards_token()
    test_systemd_user_run_argv_requires_runtime_dir()
    print("notify helpers ok")
