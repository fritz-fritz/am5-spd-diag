#!/usr/bin/python3
"""Send a persistent desktop notice and handle D-Bus activation clicks.

libnotify actions are bound to the sender's unique bus name. Capture uses a
one-shot busctl/Notify call, so Plasma/GNOME drop the buttons and ignore
clicks. A dbus-monitor waiter is a different unique name, so it never sees
those clicks either.

This helper is a D-Bus-activatable org.freedesktop.Application. Click runs
status; Analyze and Report are buttons.

FDO notification actions are delivered only to the connection that called
Notify. Capture therefore starts notify-app and leaves that connection up
until the notice is clicked or dismissed. That is the same notification, not
a second watcher: busctl-and-exit drops the buttons, and dbus-monitor uses a
different unique name so it never sees the click.

On GNOME, org.gtk.Notifications can outlive the sender via D-Bus activation.

FDO actions are the portable path for Plasma, XFCE, Cinnamon, MATE, LXQt,
and dunst. Wayland compositors also emit ActivationToken before
ActionInvoked; that token must be passed as XDG_ACTIVATION_TOKEN or the
terminal window is often never mapped. Capture runs as root, so the helper
imports DISPLAY/WAYLAND_DISPLAY from the user systemd session before launch.
A click or button replaces the resident notice with a 1ms copy so the banner
goes away while Plasma keeps the history row. CloseNotification would delete it.
"""
from __future__ import annotations

import os
import select
import shlex
import shutil
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

APP_ID = "org.opensuse.am5spdDiag"
OBJECT_PATH = "/org/opensuse/am5spdDiag"
NOTIFY_ID = "spd-corruption"
DEFAULT_ACTION = "app.status"
ANALYZE_ACTION = "app.analyze"
REPORT_ACTION = "app.report"
ACTIONS = ("status", "analyze", "report")
NOTIFY_ACTIONS = ("default", "Status", "analyze", "Analyze", "report", "Report")
APP_INTERFACE = "org.freedesktop.Application"
DBUS_NAME = "org.freedesktop.DBus"
DBUS_PATH = "/org/freedesktop/DBus"
DBUS_IFACE = "org.freedesktop.DBus"
FDO_DEST = "org.freedesktop.Notifications"
FDO_PATH = "/org/freedesktop/Notifications"
FDO_IFACE = "org.freedesktop.Notifications"

MSG_METHOD_CALL = 1
MSG_METHOD_RETURN = 2
MSG_ERROR = 3
MSG_SIGNAL = 4
HDR_PATH = 1
HDR_INTERFACE = 2
HDR_MEMBER = 3
HDR_ERROR_NAME = 4
HDR_REPLY_SERIAL = 5
HDR_DESTINATION = 6
HDR_SENDER = 7
HDR_SIGNATURE = 8
REQUEST_NAME_PRIMARY = 1
REQUEST_NAME_REPLACE_EXISTING = 0x2
REQUEST_NAME_DO_NOT_QUEUE = 0x4
CLOSE_GRACE_SEC = 0.4
SESSION_ENV_KEYS = (
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "HOME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "PATH",
    "QT_QPA_PLATFORM",
    "QT_QPA_PLATFORMTHEME",
)
_SESSION_ENV_LOADED = False


def open_term_path() -> Path:
    here = Path(__file__).resolve().parent
    candidate = here / "open-term"
    if candidate.is_file():
        return candidate
    return Path("/usr/libexec/am5-spd-diag/open-term")


def normalize_action(name: str) -> str:
    action = name.strip()
    if action.startswith("app."):
        action = action[4:]
    if action in {"", "default"}:
        return "status"
    if action in ACTIONS:
        return action
    return "status"


def parse_systemd_env_file(text: str) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, rest = line.partition("=")
        if not key or not all(ch.isalnum() or ch == "_" for ch in key):
            continue
        try:
            parsed = shlex.split(rest, posix=True)
        except ValueError:
            parsed = [rest]
        env[key] = parsed[0] if parsed else ""
    return env


def probe_wayland_display(runtime: str) -> str | None:
    try:
        names = sorted(os.listdir(runtime))
    except OSError:
        return None
    for name in names:
        if not name.startswith("wayland-") or name.endswith(".lock"):
            continue
        path = os.path.join(runtime, name)
        if os.path.exists(path):
            return name
    return None


def systemd_user_environment() -> dict[str, str]:
    try:
        proc = subprocess.run(
            ["systemctl", "--user", "show-environment"],
            capture_output=True,
            text=True,
            timeout=2,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return {}
    if proc.returncode != 0 or not proc.stdout:
        return {}
    return parse_systemd_env_file(proc.stdout)


def ensure_session_env() -> None:
    global _SESSION_ENV_LOADED
    if _SESSION_ENV_LOADED:
        return
    _SESSION_ENV_LOADED = True
    imported = systemd_user_environment()
    for key in SESSION_ENV_KEYS:
        value = imported.get(key, "")
        if value and not os.environ.get(key):
            os.environ[key] = value
    runtime = os.environ.get("XDG_RUNTIME_DIR", "")
    if runtime and not os.environ.get("WAYLAND_DISPLAY"):
        probed = probe_wayland_display(runtime)
        if probed:
            os.environ["WAYLAND_DISPLAY"] = probed
    if not os.environ.get("DISPLAY") and os.path.exists("/tmp/.X11-unix/X0"):
        os.environ["DISPLAY"] = ":0"


def systemd_user_run_argv(
    argv: list[str],
    env: dict[str, str],
    *,
    runner: str | None = None,
) -> list[str] | None:
    if runner is None:
        runner = shutil.which("systemd-run")
    if not runner:
        return None
    if not env.get("XDG_RUNTIME_DIR"):
        return None
    cmd = [runner, "--user", "--collect", "--quiet"]
    for key in (
        "XDG_ACTIVATION_TOKEN",
        "DESKTOP_STARTUP_ID",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
    ):
        value = env.get(key) or ""
        if value:
            cmd.append(f"--setenv={key}={value}")
    cmd.append("--")
    cmd.extend(argv)
    return cmd


def launch_env(activation_token: str | None = None) -> dict[str, str]:
    ensure_session_env()
    env = os.environ.copy()
    token = (activation_token or env.get("XDG_ACTIVATION_TOKEN") or "").strip()
    if token:
        env["XDG_ACTIVATION_TOKEN"] = token
        env["DESKTOP_STARTUP_ID"] = token
    return env


def launch_action(action: str, activation_token: str | None = None) -> None:
    dismiss_gtk_notification()
    helper = open_term_path()
    if not helper.is_file():
        return
    argv = [str(helper), normalize_action(action)]
    env = launch_env(activation_token)
    run_argv = systemd_user_run_argv(argv, env)
    try:
        if run_argv is not None:
            proc = subprocess.run(
                run_argv,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=8,
                check=False,
            )
            if proc.returncode == 0:
                return
        subprocess.Popen(
            argv,
            env=env,
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
        )
    except (OSError, subprocess.TimeoutExpired):
        return


def action_from_call(msg: dict[str, object]) -> str:
    member = str(msg.get("member") or "")
    if member != "ActivateAction":
        return "status"
    body = msg.get("body") or b""
    if not isinstance(body, (bytes, bytearray)) or not body:
        return "status"
    try:
        _, name = _parse_string(bytes(body), 0)
    except (ValueError, struct.error, IndexError, UnicodeDecodeError):
        return "status"
    return normalize_action(name)


def _align(pos: int, size: int) -> int:
    return (pos + size - 1) // size * size


def _pad(buf: bytearray, size: int) -> None:
    pad = _align(len(buf), size) - len(buf)
    if pad:
        buf.extend(b"\0" * pad)


def _append_u32(buf: bytearray, value: int) -> None:
    _pad(buf, 4)
    buf.extend(struct.pack("<I", value))


def _append_string(buf: bytearray, value: str, extra_nul: bool = True) -> None:
    _pad(buf, 4)
    encoded = value.encode()
    buf.extend(struct.pack("<I", len(encoded)))
    buf.extend(encoded)
    if extra_nul:
        buf.append(0)


def _append_signature(buf: bytearray, sig: str) -> None:
    encoded = sig.encode()
    buf.append(len(encoded))
    buf.extend(encoded)
    buf.append(0)


def _append_i32(buf: bytearray, value: int) -> None:
    _pad(buf, 4)
    buf.extend(struct.pack("<i", value))


def _append_string_array(buf: bytearray, items: list[str]) -> None:
    _pad(buf, 4)
    len_pos = len(buf)
    buf.extend(b"\0\0\0\0")
    data_start = len(buf)
    for item in items:
        _append_string(buf, item)
    struct.pack_into("<I", buf, len_pos, len(buf) - data_start)


def _append_variant(buf: bytearray, sig: str, value: object) -> None:
    _append_signature(buf, sig)
    if sig == "y":
        buf.append(int(value) & 0xFF)
    elif sig == "b":
        _append_u32(buf, 1 if value else 0)
    elif sig == "s":
        _append_string(buf, str(value))
    else:
        raise ValueError(sig)


def _append_hints(buf: bytearray, hints: list[tuple[str, str, object]]) -> None:
    _pad(buf, 4)
    len_pos = len(buf)
    buf.extend(b"\0\0\0\0")
    _pad(buf, 8)
    for key, sig, value in hints:
        _pad(buf, 8)
        _append_string(buf, key)
        _append_variant(buf, sig, value)
    struct.pack_into("<I", buf, len_pos, len(buf) - (len_pos + 4))


def pack_notify_body(
    title: str,
    body: str,
    *,
    replaces_id: int = 0,
    actions: list[str] | None = None,
    resident: bool = True,
    timeout: int = 0,
) -> bytes:
    buf = bytearray()
    _append_string(buf, APP_ID)
    _append_u32(buf, replaces_id)
    _append_string(buf, "dialog-warning")
    _append_string(buf, title)
    _append_string(buf, body)
    _append_string_array(buf, list(NOTIFY_ACTIONS if actions is None else actions))
    hints: list[tuple[str, str, object]] = [
        ("urgency", "y", 2),
        ("desktop-entry", "s", APP_ID),
    ]
    if resident:
        hints.append(("resident", "b", True))
    _append_hints(buf, hints)
    _append_i32(buf, timeout)
    return bytes(buf)


def pack_dismiss_body(nid: int, title: str, body: str) -> bytes:
    # Replace the resident notice with a 1ms copy and no actions. Plasma treats
    # CloseNotification as "remove from history"; expiry keeps the history row
    # and only drops the popup.
    return pack_notify_body(
        title,
        body,
        replaces_id=nid,
        actions=[],
        resident=False,
        timeout=1,
    )


def parse_action_invoked(body: bytes) -> tuple[int, str]:
    pos = _align(0, 4)
    (nid,) = struct.unpack_from("<I", body, pos)
    pos += 4
    _, action = _parse_string(body, pos)
    return nid, action


def _append_header_field(buf: bytearray, code: int, sig: str, value: str | int) -> None:
    _pad(buf, 8)
    buf.append(code)
    _append_signature(buf, sig)
    if sig in {"s", "o"}:
        _append_string(buf, str(value))
    elif sig == "u":
        _append_u32(buf, int(value))
    else:
        raise ValueError(sig)


def pack_message(
    msg_type: int,
    serial: int,
    *,
    path: str | None = None,
    interface: str | None = None,
    member: str | None = None,
    destination: str | None = None,
    sender: str | None = None,
    reply_serial: int | None = None,
    error_name: str | None = None,
    signature: str = "",
    body: bytes = b"",
) -> bytes:
    fields = bytearray()
    if path is not None:
        _append_header_field(fields, HDR_PATH, "o", path)
    if interface is not None:
        _append_header_field(fields, HDR_INTERFACE, "s", interface)
    if member is not None:
        _append_header_field(fields, HDR_MEMBER, "s", member)
    if error_name is not None:
        _append_header_field(fields, HDR_ERROR_NAME, "s", error_name)
    if reply_serial is not None:
        _append_header_field(fields, HDR_REPLY_SERIAL, "u", reply_serial)
    if destination is not None:
        _append_header_field(fields, HDR_DESTINATION, "s", destination)
    if sender is not None:
        _append_header_field(fields, HDR_SENDER, "s", sender)
    if signature:
        _pad(fields, 8)
        fields.append(HDR_SIGNATURE)
        _append_signature(fields, "g")
        _append_signature(fields, signature)
    header = bytearray()
    header.extend(
        struct.pack(
            "<BBBBIII",
            ord("l"),
            msg_type,
            0,
            1,
            len(body),
            serial,
            len(fields),
        )
    )
    header.extend(fields)
    _pad(header, 8)
    return bytes(header + body)


def _parse_string(data: bytes, pos: int) -> tuple[int, str]:
    pos = _align(pos, 4)
    (length,) = struct.unpack_from("<I", data, pos)
    pos += 4
    value = data[pos : pos + length].decode()
    pos += length + 1
    return pos, value


def _parse_signature(data: bytes, pos: int) -> tuple[int, str]:
    length = data[pos]
    pos += 1
    value = data[pos : pos + length].decode()
    pos += length + 1
    return pos, value


def parse_header_fields(data: bytes) -> dict[int, str | int]:
    fields: dict[int, str | int] = {}
    pos = 0
    while pos < len(data):
        pos = _align(pos, 8)
        if pos >= len(data):
            break
        code = data[pos]
        pos += 1
        pos, sig = _parse_signature(data, pos)
        if sig in {"s", "o"}:
            pos, value = _parse_string(data, pos)
            fields[code] = value
        elif sig == "u":
            pos = _align(pos, 4)
            (value_u,) = struct.unpack_from("<I", data, pos)
            pos += 4
            fields[code] = value_u
        elif sig == "g":
            pos, value = _parse_signature(data, pos)
            fields[code] = value
        else:
            break
    return fields


def parse_message(blob: bytes) -> tuple[dict[str, object], int]:
    if len(blob) < 16 or blob[0:1] != b"l":
        raise ValueError("not a little-endian D-Bus message")
    msg_type = blob[1]
    body_len, serial, fields_len = struct.unpack_from("<III", blob, 4)
    header_end = 16 + fields_len
    padded = _align(header_end, 8)
    total = padded + body_len
    if len(blob) < total:
        raise ValueError("truncated D-Bus message")
    fields = parse_header_fields(blob[16:header_end])
    body = blob[padded:total]
    parsed: dict[str, object] = {
        "type": msg_type,
        "serial": serial,
        "body": body,
        "path": fields.get(HDR_PATH),
        "interface": fields.get(HDR_INTERFACE),
        "member": fields.get(HDR_MEMBER),
        "destination": fields.get(HDR_DESTINATION),
        "sender": fields.get(HDR_SENDER),
        "reply_serial": fields.get(HDR_REPLY_SERIAL),
        "signature": fields.get(HDR_SIGNATURE, ""),
        "error_name": fields.get(HDR_ERROR_NAME),
    }
    return parsed, total


def session_bus_path() -> str | None:
    addr = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    for part in addr.split(";"):
        for item in part.split(","):
            if item.startswith("path="):
                return item[5:]
            if item.startswith("unix:path="):
                return item[len("unix:path=") :]
    runtime = os.environ.get("XDG_RUNTIME_DIR", "")
    if runtime:
        candidate = Path(runtime) / "bus"
        if candidate.exists():
            return str(candidate)
    return None


def is_application_call(msg: dict[str, object]) -> bool:
    if msg.get("type") != MSG_METHOD_CALL:
        return False
    member = str(msg.get("member") or "")
    interface = str(msg.get("interface") or "")
    return interface in {"", APP_INTERFACE} and member in {"Activate", "ActivateAction", "Open"}


class SessionBus:
    def __init__(self, path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect(path)
        self.serial = 0
        self.unique_name = ""
        self.buf = bytearray()
        self.pending_calls: list[dict[str, object]] = []
        self.pending_signals: list[dict[str, object]] = []
        self._auth()

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def _auth(self) -> None:
        hex_uid = str(os.getuid()).encode().hex()
        self.sock.sendall(b"\0AUTH EXTERNAL " + hex_uid.encode() + b"\r\n")
        reply = b""
        while b"\r\n" not in reply:
            chunk = self.sock.recv(256)
            if not chunk:
                raise ConnectionError("D-Bus auth closed")
            reply += chunk
            if len(reply) > 4096:
                raise ConnectionError("D-Bus auth overflow")
        line, rest = reply.split(b"\r\n", 1)
        if not line.startswith(b"OK "):
            raise ConnectionError(f"D-Bus auth failed: {line!r}")
        if rest:
            self.buf.extend(rest)
        self.sock.sendall(b"BEGIN\r\n")

    def _next_serial(self) -> int:
        self.serial += 1
        return self.serial

    def send_msg(self, blob: bytes) -> None:
        self.sock.sendall(blob)

    def recv_msg(self, timeout: float | None) -> dict[str, object]:
        self.sock.settimeout(timeout)
        while True:
            try:
                parsed, used = parse_message(bytes(self.buf))
                del self.buf[:used]
                return parsed
            except ValueError:
                pass
            ready, _, _ = select.select([self.sock], [], [], timeout)
            if not ready:
                raise TimeoutError("D-Bus read timed out")
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("D-Bus connection closed")
            self.buf.extend(chunk)

    def call(
        self,
        destination: str,
        path: str,
        interface: str,
        member: str,
        signature: str = "",
        body: bytes = b"",
        timeout: float = 5,
    ) -> dict[str, object]:
        serial = self._next_serial()
        self.send_msg(
            pack_message(
                MSG_METHOD_CALL,
                serial,
                path=path,
                interface=interface,
                member=member,
                destination=destination,
                signature=signature,
                body=body,
            )
        )
        while True:
            msg = self.recv_msg(timeout)
            if msg["type"] in {MSG_METHOD_RETURN, MSG_ERROR} and msg.get("reply_serial") == serial:
                return msg
            if is_application_call(msg):
                self.pending_calls.append(msg)
            elif msg["type"] == MSG_SIGNAL:
                self.pending_signals.append(msg)

    def hello(self) -> None:
        reply = self.call(DBUS_NAME, DBUS_PATH, DBUS_IFACE, "Hello")
        if reply["type"] != MSG_METHOD_RETURN:
            raise ConnectionError("Hello failed")
        _, self.unique_name = _parse_string(reply["body"], 0)  # type: ignore[arg-type]

    def request_name(self, name: str) -> bool:
        body = bytearray()
        _append_string(body, name)
        _append_u32(body, REQUEST_NAME_REPLACE_EXISTING | REQUEST_NAME_DO_NOT_QUEUE)
        reply = self.call(
            DBUS_NAME,
            DBUS_PATH,
            DBUS_IFACE,
            "RequestName",
            "su",
            bytes(body),
        )
        if reply["type"] != MSG_METHOD_RETURN:
            return False
        raw: bytes = reply["body"]  # type: ignore[assignment]
        if len(raw) < 4:
            return False
        (result,) = struct.unpack_from("<I", raw, 0)
        return result == REQUEST_NAME_PRIMARY

    def reply(self, call: dict[str, object]) -> None:
        sender = call.get("sender")
        serial = call.get("serial")
        if not isinstance(sender, str) or not isinstance(serial, int):
            return
        self.send_msg(
            pack_message(
                MSG_METHOD_RETURN,
                self._next_serial(),
                destination=sender,
                reply_serial=serial,
            )
        )


def send_fdo_and_wait(title: str, body: str) -> None:
    try:
        send_fdo_gio_and_wait(title, body)
        return
    except Exception:
        pass
    send_fdo_raw_and_wait(title, body)


def send_fdo_gio_and_wait(title: str, body: str) -> None:
    import gi

    gi.require_version("Gio", "2.0")
    gi.require_version("GLib", "2.0")
    from gi.repository import Gio, GLib

    ensure_session_env()
    loop = GLib.MainLoop()
    conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    proxy = Gio.DBusProxy.new_sync(
        conn,
        Gio.DBusProxyFlags.NONE,
        None,
        FDO_DEST,
        FDO_PATH,
        FDO_IFACE,
        None,
    )
    state: dict[str, object] = {"nid": None, "token": "", "done": False}

    def dismiss_popup() -> None:
        nid = state["nid"]
        if nid is None:
            return
        try:
            proxy.call_sync(
                "Notify",
                GLib.Variant(
                    "(susssasa{sv}i)",
                    (
                        APP_ID,
                        int(nid),
                        "dialog-warning",
                        title,
                        body,
                        [],
                        {
                            "urgency": GLib.Variant("y", 2),
                            "desktop-entry": GLib.Variant("s", APP_ID),
                        },
                        1,
                    ),
                ),
                Gio.DBusCallFlags.NONE,
                5000,
                None,
            )
        except Exception:
            pass

    def finish(action: str | None) -> bool:
        if state["done"]:
            return False
        state["done"] = True
        if action is not None:
            dismiss_popup()
            launch_action(action, str(state.get("token") or "") or None)
        loop.quit()
        return False

    def on_signal(_conn, _sender, _path, _iface, signal, params, *_args) -> None:
        nid = state["nid"]
        if nid is None:
            return
        values = params.unpack()
        if not values:
            return
        if int(values[0]) != int(nid):
            return
        if signal == "ActivationToken":
            state["token"] = str(values[1])
        elif signal == "ActionInvoked":
            GLib.idle_add(finish, str(values[1]))
        elif signal == "NotificationClosed":
            GLib.timeout_add(int(CLOSE_GRACE_SEC * 1000), finish, None)

    conn.signal_subscribe(
        FDO_DEST,
        FDO_IFACE,
        None,
        FDO_PATH,
        None,
        Gio.DBusSignalFlags.NONE,
        on_signal,
    )
    hints = {
        "urgency": GLib.Variant("y", 2),
        "desktop-entry": GLib.Variant("s", APP_ID),
        "resident": GLib.Variant("b", True),
    }
    state["nid"] = proxy.call_sync(
        "Notify",
        GLib.Variant(
            "(susssasa{sv}i)",
            (
                APP_ID,
                0,
                "dialog-warning",
                title,
                body,
                list(NOTIFY_ACTIONS),
                hints,
                0,
            ),
        ),
        Gio.DBusCallFlags.NONE,
        5000,
        None,
    ).unpack()[0]
    loop.run()


def send_fdo_raw_and_wait(title: str, body: str) -> None:
    path = session_bus_path()
    if not path:
        return
    bus = SessionBus(path)
    try:
        bus.hello()
        reply = bus.call(
            FDO_DEST,
            FDO_PATH,
            FDO_IFACE,
            "Notify",
            "susssasa{sv}i",
            pack_notify_body(title, body),
        )
        if reply["type"] != MSG_METHOD_RETURN:
            return
        raw: bytes = reply["body"]  # type: ignore[assignment]
        if len(raw) < 4:
            return
        (nid,) = struct.unpack_from("<I", raw, 0)
        match = bytearray()
        _append_string(match, "type='signal',interface='org.freedesktop.Notifications'")
        try:
            bus.call(DBUS_NAME, DBUS_PATH, DBUS_IFACE, "AddMatch", "s", bytes(match))
        except (OSError, ConnectionError, TimeoutError, ValueError, struct.error):
            pass
        token = ""
        closing_until: float | None = None
        while True:
            timeout = None
            if closing_until is not None:
                remaining = closing_until - time.monotonic()
                if remaining <= 0:
                    return
                timeout = remaining
            if bus.pending_signals:
                msg = bus.pending_signals.pop(0)
            else:
                try:
                    msg = bus.recv_msg(timeout)
                except TimeoutError:
                    if closing_until is not None:
                        return
                    continue
            if is_application_call(msg):
                bus.reply(msg)
                dismiss_fdo_on_bus(bus, nid, title, body)
                launch_action(action_from_call(msg), token or None)
                return
            if msg["type"] != MSG_SIGNAL:
                continue
            if str(msg.get("interface") or "") != FDO_IFACE:
                continue
            payload = msg.get("body") or b""
            if not isinstance(payload, (bytes, bytearray)):
                continue
            member = str(msg.get("member") or "")
            if member == "ActivationToken":
                got_id, value = parse_action_invoked(bytes(payload))
                if got_id == nid:
                    token = value
            elif member == "ActionInvoked":
                got_id, action = parse_action_invoked(bytes(payload))
                if got_id == nid:
                    dismiss_fdo_on_bus(bus, nid, title, body)
                    launch_action(action, token or None)
                    return
            elif member == "NotificationClosed":
                (got_id,) = struct.unpack_from("<I", payload, 0)
                if got_id == nid:
                    closing_until = time.monotonic() + CLOSE_GRACE_SEC
    finally:
        bus.close()


def dismiss_fdo_on_bus(bus: SessionBus, nid: int, title: str, body: str) -> None:
    try:
        bus.call(
            FDO_DEST,
            FDO_PATH,
            FDO_IFACE,
            "Notify",
            "susssasa{sv}i",
            pack_dismiss_body(nid, title, body),
        )
    except (OSError, ConnectionError, TimeoutError, ValueError, struct.error):
        pass


def serve_application(timeout: float = 25) -> bool:
    path = session_bus_path()
    if not path:
        return False
    bus = SessionBus(path)
    try:
        bus.hello()
        if not bus.request_name(APP_ID):
            return True
        end = time.monotonic() + timeout
        while True:
            if bus.pending_calls:
                msg = bus.pending_calls.pop(0)
            else:
                remaining = end - time.monotonic()
                if remaining <= 0:
                    return False
                try:
                    msg = bus.recv_msg(remaining)
                except TimeoutError:
                    return False
            if is_application_call(msg):
                action = action_from_call(msg)
                bus.reply(msg)
                launch_action(action)
                return True
            if str(msg.get("interface") or "") == "org.freedesktop.DBus.Peer" and str(
                msg.get("member") or ""
            ) == "Ping":
                bus.reply(msg)
    finally:
        bus.close()


def notification_body_args(title: str, body: str, *, persistent_hint: bool = False) -> list[str]:
    args = [
        "title",
        "s",
        title,
        "body",
        "s",
        body,
        "priority",
        "s",
        "urgent",
        "default-action",
        "s",
        DEFAULT_ACTION,
        "buttons",
        "aa{sv}",
        "2",
        "2",
        "label",
        "s",
        "Analyze",
        "action",
        "s",
        ANALYZE_ACTION,
        "2",
        "label",
        "s",
        "Report",
        "action",
        "s",
        REPORT_ACTION,
    ]
    count = 5
    if persistent_hint:
        count = 6
        args.extend(["display-hint", "as", "1", "persistent"])
    return [str(count), *args]


def gtk_remove_argv() -> list[str]:
    return [
        "busctl",
        "--user",
        "call",
        "--",
        "org.gtk.Notifications",
        "/org/gtk/Notifications",
        "org.gtk.Notifications",
        "RemoveNotification",
        "ss",
        APP_ID,
        NOTIFY_ID,
    ]


def gtk_notify_argv(title: str, body: str) -> list[str]:
    return [
        "busctl",
        "--user",
        "call",
        "--",
        "org.gtk.Notifications",
        "/org/gtk/Notifications",
        "org.gtk.Notifications",
        "AddNotification",
        "ssa{sv}",
        APP_ID,
        NOTIFY_ID,
        *notification_body_args(title, body),
    ]


def portal_notify_argv(title: str, body: str, *, persistent_hint: bool = True) -> list[str]:
    return [
        "busctl",
        "--user",
        "call",
        "--",
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Notification",
        "AddNotification",
        "sa{sv}",
        NOTIFY_ID,
        *notification_body_args(title, body, persistent_hint=persistent_hint),
    ]


def kde_impl_notify_argv(
    title: str,
    body: str,
    dest: str = "org.freedesktop.impl.portal.desktop.plasmanotify",
    *,
    persistent_hint: bool = True,
) -> list[str]:
    return [
        "busctl",
        "--user",
        "call",
        "--",
        dest,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.impl.portal.Notification",
        "AddNotification",
        "ssa{sv}",
        APP_ID,
        NOTIFY_ID,
        *notification_body_args(title, body, persistent_hint=persistent_hint),
    ]


def fdo_notify_argv(title: str, body: str) -> list[str]:
    """Persistent FDO notice; actions work only while this sender stays connected."""
    return [
        "busctl",
        "--user",
        "call",
        "--",
        FDO_DEST,
        FDO_PATH,
        FDO_IFACE,
        "Notify",
        "susssasa{sv}i",
        APP_ID,
        "0",
        "dialog-warning",
        title,
        body,
        "6",
        "default",
        "Status",
        "analyze",
        "Analyze",
        "report",
        "Report",
        "3",
        "urgency",
        "y",
        "2",
        "desktop-entry",
        "s",
        APP_ID,
        "resident",
        "b",
        "true",
        "0",
    ]


def fdo_dismiss_argv(nid: int, title: str, body: str) -> list[str]:
    """Replace a resident notice so it expires into history instead of being deleted."""
    return [
        "busctl",
        "--user",
        "call",
        "--",
        FDO_DEST,
        FDO_PATH,
        FDO_IFACE,
        "Notify",
        "susssasa{sv}i",
        APP_ID,
        str(nid),
        "dialog-warning",
        title,
        body,
        "0",
        "2",
        "urgency",
        "y",
        "2",
        "desktop-entry",
        "s",
        APP_ID,
        "1",
    ]


def name_has_owner_argv(name: str) -> list[str]:
    return [
        "busctl",
        "--user",
        "call",
        "--",
        DBUS_NAME,
        DBUS_PATH,
        DBUS_IFACE,
        "NameHasOwner",
        "s",
        name,
    ]


def _run(argv: list[str]) -> subprocess.CompletedProcess[str] | None:
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=3, check=False)
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return None


def name_has_owner(name: str) -> bool:
    proc = _run(name_has_owner_argv(name))
    if proc is None or proc.returncode != 0:
        return False
    return "true" in (proc.stdout or "").lower()


def try_call(argv: list[str]) -> bool:
    proc = _run(argv)
    return proc is not None and proc.returncode == 0


def dismiss_gtk_notification() -> None:
    if name_has_owner("org.gtk.Notifications"):
        try_call(gtk_remove_argv())


PORTAL_IMPL_NAMES = (
    "org.freedesktop.impl.portal.desktop.plasmanotify",
    "org.freedesktop.impl.portal.desktop.kde",
)


def send_notification(title: str, body: str) -> None:
    ensure_session_env()
    if name_has_owner("org.gtk.Notifications") and try_call(gtk_notify_argv(title, body)):
        return
    send_fdo_and_wait(title, body)


def cmd_activated() -> int:
    try:
        if serve_application():
            return 0
    except (OSError, ConnectionError, TimeoutError, ValueError, struct.error):
        pass
    launch_action("status")
    return 0


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if args[:1] == ["--notify"]:
        if len(args) < 3:
            print("usage: notify-app --notify TITLE BODY", file=sys.stderr)
            return 2
        send_notification(args[1], args[2])
        return 0
    if args[:1] == ["--activated"]:
        return cmd_activated()
    if args[:1] in (["--status"], ["--analyze"], ["--report"]):
        launch_action(args[0][2:])
        return 0
    launch_action(args[0] if args else "status")
    return 0


if __name__ == "__main__":
    sys.exit(main())
