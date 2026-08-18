#!/usr/bin/python3
"""Analyze AM5 SPD capture timelines and emit vendor-agnostic reports/packages."""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from collections import Counter, OrderedDict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import spd_hub  # noqa: E402

FORUM_URL = spd_hub.FORUM_URL


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def iso_now() -> str:
    return datetime.now().astimezone().isoformat()


def parse_conf(path: Path) -> dict[str, str]:
    cfg: dict[str, str] = {}
    if not path.is_file():
        return cfg
    for raw in path.read_text(errors="replace").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, val = line.split("=", 1)
        cfg[key.strip()] = val.strip().strip("'\"")
    return cfg


def load_config(prefix: Path) -> dict[str, str]:
    cfg = {
        "STATE_DIR": os.environ.get("AM5_SPD_DIAG_STATE_DIR", "/var/log/am5-spd-diag"),
        "FALLBACK_BOARD": "unknown AM5 board",
        "FALLBACK_CPU": "AMD Ryzen AM5",
        "FALLBACK_MEMORY": "DDR5 UDIMM kit",
        "FALLBACK_BIOS": "unknown",
    }
    share = Path(os.environ.get("AM5_SPD_DIAG_SHARE") or str(prefix))
    for path in (share / "config" / "default.conf", Path("/etc/am5-spd-diag.conf")):
        cfg.update(parse_conf(path))
    if os.environ.get("AM5_SPD_DIAG_STATE_DIR"):
        cfg["STATE_DIR"] = os.environ["AM5_SPD_DIAG_STATE_DIR"]
    return cfg


def is_alert(ev: dict[str, Any]) -> bool:
    val = ev.get("alert", False)
    if isinstance(val, bool):
        return val
    return str(val).lower() in {"true", "1", "yes"}


def parse_dimm_summary(text: str) -> list[dict[str, str]]:
    return spd_hub.parse_dimm_summary(text)


def read_kv_file(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    if not path.is_file():
        return data
    for line in path.read_text(errors="replace").splitlines():
        if "=" not in line:
            continue
        k, v = line.split("=", 1)
        data[k.strip()] = v.strip()
    return data


def load_json(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def load_event_extras(ev: dict[str, Any]) -> dict[str, Any]:
    directory = Path(str(ev.get("dir") or ""))
    extras: dict[str, Any] = {
        "dimms": [],
        "meta": {},
        "dmi": {},
        "hub": {},
        "system": {},
        "e820": "",
        "dmesg_spd": "",
        "spd_page0": [],
        "dir_exists": directory.is_dir(),
    }
    if directory.is_dir():
        raw = directory / "dmidecode-memory.txt"
        if raw.is_file():
            extras["dimms"] = spd_hub.parse_dmidecode_memory(raw.read_text(errors="replace"))
        if not extras["dimms"]:
            extras["dimms"] = parse_dimm_summary(
                (directory / "dimm-summary.txt").read_text(errors="replace")
                if (directory / "dimm-summary.txt").is_file()
                else ""
            )
        extras["meta"] = read_kv_file(directory / "meta.txt")
        extras["dmi"] = read_kv_file(directory / "dmi-sysfs.txt")
        extras["hub"] = load_json(directory / "hub.json")
        extras["system"] = load_json(directory / "system.json")
        extras["e820"] = ""
        for name in ("e820.txt", "e820-system-ram.txt"):
            path = directory / name
            if path.is_file():
                extras["e820"] = path.read_text(errors="replace")
                break
        extras["dmesg_spd"] = (
            (directory / "dmesg-spd5118.txt").read_text(errors="replace")
            if (directory / "dmesg-spd5118.txt").is_file()
            else ""
        )
        extras["spd_page0"] = []
        for path in sorted(directory.glob("spd-page0-*.txt")):
            extras["spd_page0"].append(
                {"name": path.name, "text": path.read_text(errors="replace")}
            )
        if not ev.get("boot_kind"):
            ev["boot_kind"] = extras["meta"].get("boot_kind", "")
        if not ev.get("hub_stuck"):
            ev["hub_stuck"] = extras["meta"].get("hub_stuck", "")
    return extras


def iter_json_objects(text: str):
    """Yield dicts from JSONL, including multiple objects jammed on one line."""
    decoder = json.JSONDecoder()
    idx = 0
    n = len(text)
    while idx < n:
        while idx < n and text[idx] in " \t\r\n":
            idx += 1
        if idx >= n:
            break
        try:
            obj, end = decoder.raw_decode(text, idx)
        except json.JSONDecodeError:
            nxt = text.find("{", idx + 1)
            if nxt < 0:
                break
            idx = nxt
            continue
        if isinstance(obj, dict):
            yield obj
        idx = end


def load_timeline(state_dir: Path) -> list[dict[str, Any]]:
    path = state_dir / "timeline.jsonl"
    events: list[dict[str, Any]] = []
    if not path.is_file():
        return events
    for ev in iter_json_objects(path.read_text(errors="replace")):
        ev.update(load_event_extras(ev))
        events.append(ev)
    return events


def load_baseline(state_dir: Path) -> dict[str, Any]:
    return load_json(state_dir / "baseline.json")


DMI_SYSFS_KEYS = (
    "bios_vendor",
    "bios_version",
    "bios_date",
    "bios_release",
    "board_vendor",
    "board_name",
    "board_version",
    "board_serial",
    "sys_vendor",
    "product_name",
    "product_version",
    "product_family",
    "product_sku",
    "chassis_vendor",
    "chassis_type",
    "chassis_version",
)
CHASSIS_TYPES = {
    "1": "Other",
    "2": "Unknown",
    "3": "Desktop",
    "4": "Low Profile Desktop",
    "5": "Pizza Box",
    "6": "Mini Tower",
    "7": "Tower",
    "8": "Portable",
    "9": "Laptop",
    "10": "Notebook",
    "11": "Hand Held",
    "14": "Sub Notebook",
    "30": "Tablet",
    "31": "Convertible",
    "32": "Detachable",
}


def _sysfs_text(path: Path) -> str:
    try:
        return path.read_text(errors="replace").strip()
    except OSError:
        return ""


def live_dmi() -> dict[str, str]:
    base = Path("/sys/class/dmi/id")
    out: dict[str, str] = {}
    for name in DMI_SYSFS_KEYS:
        val = _sysfs_text(base / name)
        if val:
            out[name] = val
    return out


def live_cpu() -> str:
    return live_cpu_info().get("model_name", "")


def live_cpu_info() -> dict[str, str]:
    path = Path("/proc/cpuinfo")
    info: dict[str, str] = {}
    if not path.is_file():
        return info
    for line in path.read_text(errors="replace").splitlines():
        if ":" not in line:
            continue
        key, val = line.split(":", 1)
        key, val = key.strip().lower(), val.strip()
        mapping = {
            "model name": "model_name",
            "vendor_id": "vendor_id",
            "cpu family": "family",
            "model": "model",
            "stepping": "stepping",
            "microcode": "microcode",
        }
        dest = mapping.get(key)
        if dest and dest not in info:
            info[dest] = val
    return info


def live_os_info() -> dict[str, str]:
    data = read_kv_file(Path("/etc/os-release"))
    return {k: v.strip().strip('"') for k, v in data.items()}


def live_os() -> str:
    data = live_os_info()
    return data.get("PRETTY_NAME") or data.get("NAME") or ""


def live_kernel_info() -> dict[str, str]:
    u = os.uname()
    info = {
        "sysname": u.sysname,
        "release": u.release,
        "version": u.version,
        "machine": u.machine,
        "proc_version": _sysfs_text(Path("/proc/version")),
    }
    return {k: v for k, v in info.items() if v}


def live_kernel() -> str:
    return os.uname().release


def boot_mode() -> str:
    return "UEFI" if Path("/sys/firmware/efi").is_dir() else "legacy"


def collect_system_info() -> dict[str, Any]:
    cpu = live_cpu_info()
    osinfo = live_os_info()
    kernel = live_kernel_info()
    return {
        "dmi": live_dmi(),
        "cpu": cpu,
        "os": osinfo,
        "kernel": kernel,
        "boot_mode": boot_mode(),
        "mem_sleep": mem_sleep(),
    }


def memtotal_kb() -> int:
    path = Path("/proc/meminfo")
    if not path.is_file():
        return 0
    for line in path.read_text(errors="replace").splitlines():
        if line.startswith("MemTotal:"):
            return int(line.split()[1])
    return 0


def mem_sleep() -> str:
    path = Path("/sys/power/mem_sleep")
    if not path.is_file():
        return "unknown"
    return path.read_text(errors="replace").strip()


def kb_to_gib(kb: int) -> str:
    if kb <= 0:
        return "unknown"
    return f"{kb / 1024 / 1024:.2f} GiB"


def group_boots(events: list[dict[str, Any]]) -> OrderedDict[str, list[dict[str, Any]]]:
    boots: OrderedDict[str, list[dict[str, Any]]] = OrderedDict()
    for ev in events:
        bid = str(ev.get("boot_id") or "unknown")
        boots.setdefault(bid, []).append(ev)
    return boots


def sleep_cycles(boot_events: list[dict[str, Any]]) -> int:
    return sum(
        1
        for ev in boot_events
        if ev.get("event") in {"suspend-pre", "hibernate-pre"}
    )


def dimm_table(dimms: list[dict[str, str]]) -> str:
    if not dimms:
        return "_No populated DIMM summary captured (SMBIOS/dmidecode may have been unavailable)._"
    lines = [
        "| Locator | Size | Width | Speed | Type | Manufacturer | Part | Serial |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for d in dimms:
        width = f"{d.get('total_width', '?')} / {d.get('data_width', '?')}"
        lines.append(
            "| {loc} | {size} | {width} | {speed} | {typ} | {man} | {part} | {ser} |".format(
                loc=d.get("locator", "?"),
                size=d.get("size", "?"),
                width=width,
                speed=d.get("speed") or "?",
                typ=d.get("mem_type") or "?",
                man=d.get("manufacturer", "?"),
                part=d.get("part", "?"),
                ser=d.get("serial", "?"),
            )
        )
    return "\n".join(lines)


def slot_map_line(dimms: list[dict[str, str]]) -> str:
    if not dimms:
        return "no populated DIMMs"
    locs = [d.get("locator") or "?" for d in dimms]
    sizes = [d.get("size") or "?" for d in dimms]
    if len(set(sizes)) == 1:
        return f"{len(dimms)}×{sizes[0]} in {'+'.join(locs)}"
    return "; ".join(f"{sz} in {loc}" for loc, sz in zip(locs, sizes))


def memory_from_dimms(dimms: list[dict[str, str]]) -> str:
    if not dimms:
        return ""
    parts: list[str] = []
    for d in dimms:
        bits = [d.get("locator", ""), d.get("size", ""), d.get("manufacturer", ""), d.get("part", "")]
        parts.append(" ".join(x for x in bits if x))
    return "; ".join(parts)


def event_row(ev: dict[str, Any]) -> str:
    flags = ev.get("flags") or ""
    mark = "ALERT" if is_alert(ev) else "ok"
    kind = ev.get("boot_kind") or ""
    extra = f" {kind}" if kind and kind not in {"unknown", "same_boot", ""} else ""
    return (
        f"| {ev.get('ts', '')} | {ev.get('event', '')}{extra} | {mark} | "
        f"{ev.get('memtotal_kb', '')} | {flags} |"
    )


def boot_kind_from_previous_event(prev_event: str) -> str:
    name = str(prev_event or "")
    if name == "reboot":
        return "warm_reboot"
    if name in {"poweroff", "shutdown", "halt"}:
        return "shutdown_poweroff"
    if name:
        return "unexpected_power_loss"
    return "unknown"


def infer_boot_kind(events: list[dict[str, Any]], index: int) -> str:
    ev = events[index]
    kind = str(ev.get("boot_kind") or ev.get("meta", {}).get("boot_kind") or "")
    if kind and kind not in {"unknown", "same_boot"}:
        return kind
    bid = ev.get("boot_id")
    for j in range(index - 1, -1, -1):
        prev = events[j]
        if prev.get("boot_id") == bid:
            continue
        return boot_kind_from_previous_event(str(prev.get("event") or ""))
    return "unknown"


def latest_boot_start_kind(events: list[dict[str, Any]]) -> str:
    for i in range(len(events) - 1, -1, -1):
        if events[i].get("event") == "boot":
            return infer_boot_kind(events, i)
    return ""


def previous_boot_id(events: list[dict[str, Any]], index: int) -> str | None:
    bid = events[index].get("boot_id")
    for j in range(index - 1, -1, -1):
        if events[j].get("boot_id") != bid:
            return str(events[j].get("boot_id") or "")
    return None


def find_transitions(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    boots = group_boots(events)
    transitions: list[dict[str, Any]] = []
    last_alert_boot: str | None = None
    for i, ev in enumerate(events):
        if not is_alert(ev):
            continue
        bid = str(ev.get("boot_id") or "")
        if bid and bid == last_alert_boot:
            continue
        last_alert_boot = bid or None
        prev_healthy = None
        chain: list[dict[str, Any]] = []
        for j in range(i - 1, -1, -1):
            prev = events[j]
            chain.append(prev)
            name = prev.get("event")
            if not is_alert(prev) and prev_healthy is None:
                prev_healthy = prev
            if name == "boot" and prev.get("boot_id") != ev.get("boot_id"):
                if not is_alert(prev) and prev_healthy is None:
                    prev_healthy = prev
                break
            if is_alert(prev) and name == "boot":
                break
        chain.reverse()
        prev_bid = previous_boot_id(events, i)
        sleep_count = sleep_cycles(boots.get(prev_bid or "", []))
        boot_kind = infer_boot_kind(events, i)
        reboot_between = boot_kind == "warm_reboot" or any(
            p.get("event") == "reboot" for p in chain
        )
        stuck = []
        hub = ev.get("hub") or {}
        for row in hub.get("stuck") or []:
            stuck.append(str(row.get("sysfs") or row.get("addr_hex") or ""))
        last_pre = next((p for p in reversed(chain) if p.get("event") in {"suspend-pre", "hibernate-pre"}), None)
        mem_sleep_used = ""
        if last_pre:
            mem_sleep_used = str((last_pre.get("meta") or {}).get("mem_sleep") or last_pre.get("mem_sleep") or "")
        bad_dimms = [d for d in (ev.get("dimms") or []) if spd_hub.dimm_flags(d)]
        transitions.append(
            {
                "alert_event": ev,
                "prev_healthy": prev_healthy,
                "sleep_count": sleep_count,
                "boot_kind": boot_kind,
                "reboot_between": reboot_between,
                "chain": chain + [ev],
                "stuck_hubs": [s for s in stuck if s],
                "bad_dimms": bad_dimms,
                "mem_sleep": mem_sleep_used,
            }
        )
    return transitions


def render_pattern(events: list[dict[str, Any]], boots: OrderedDict[str, list[dict[str, Any]]], transitions: list[dict[str, Any]]) -> str:
    if not transitions:
        healthy_sleep = sum(1 for bid, evs in boots.items() if sleep_cycles(evs) and not any(is_alert(e) for e in evs))
        extra = (
            f" {healthy_sleep} boot(s) had sleep cycles without an SPD identity alert."
            if healthy_sleep
            else ""
        )
        return (
            "No corruption events recorded yet. Leave the monitor enabled through sleep "
            f"and the next reboot.{extra}"
        )
    n = len(transitions)
    kinds = Counter(tr["boot_kind"] for tr in transitions)
    sleep2 = sum(1 for tr in transitions if tr["sleep_count"] >= 2)
    sleep1 = sum(1 for tr in transitions if tr["sleep_count"] == 1)
    sleep0 = sum(1 for tr in transitions if tr["sleep_count"] == 0)
    lines = [
        f"{n} corruption snapshot(s) recorded.",
        f"- Boot kind: warm reboot {kinds.get('warm_reboot', 0)}, "
        f"shutdown/poweroff {kinds.get('shutdown_poweroff', 0)}, "
        f"unexpected power loss {kinds.get('unexpected_power_loss', 0)}, "
        f"unknown {kinds.get('unknown', 0) + kinds.get('same_boot', 0)}.",
        f"- Sleep cycles on the previous boot: ≥2 in {sleep2}, exactly 1 in {sleep1}, none in {sleep0}.",
    ]
    if sleep0:
        lines.append(
            "- POST with no sleep this boot matches the forum report that firmware can "
            "write MR11 during POST, not only on S3 resume."
        )
    healthy_after_sleep = 0
    boot_ids = list(boots.keys())
    for i, bid in enumerate(boot_ids):
        evs = boots[bid]
        if any(is_alert(e) for e in evs) or sleep_cycles(evs) < 2:
            continue
        if i + 1 < len(boot_ids) and any(is_alert(e) for e in boots[boot_ids[i + 1]]):
            continue
        healthy_after_sleep += 1
    if healthy_after_sleep:
        lines.append(
            f"- Intermittent: {healthy_after_sleep} boot(s) had ≥2 suspends and did not show "
            "SPD identity corruption (same sequence is not a guaranteed trigger)."
        )
    return "\n".join(lines)


def _md_cell(value: str) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ").strip()


def is_dmi_placeholder(value: str) -> bool:
    return value.strip().lower() in {
        "",
        "unknown",
        "none",
        "n/a",
        "not specified",
        "not provided",
        "default string",
        "to be filled by o.e.m.",
        "to be filled by o.e.m",
        "to be filled by oem",
    }


def md_kv_table(rows: list[tuple[str, str]]) -> str:
    lines = ["| Item | Details |", "|---|---|"]
    kept = 0
    for key, val in rows:
        if val is None:
            continue
        text = _md_cell(val)
        if not text or is_dmi_placeholder(text):
            continue
        lines.append(f"| {key} | {text} |")
        kept += 1
    if kept == 0:
        return "_No system details available._"
    return "\n".join(lines)


def dimm_key(dimm: dict[str, str]) -> tuple[str, ...]:
    return tuple(
        (dimm.get(k) or "").strip()
        for k in ("locator", "size", "total_width", "data_width", "manufacturer", "part", "serial")
    )


def dimms_match(a: list[dict[str, str]], b: list[dict[str, str]]) -> bool:
    if not a or not b:
        return False
    return [dimm_key(x) for x in a] == [dimm_key(x) for x in b]


def hardware_table(
    cfg: dict[str, str],
    dmi: dict[str, str],
    cpu: str,
    os_name: str,
    kernel: str,
    baseline: dict[str, Any],
    system: dict[str, Any] | None = None,
) -> str:
    system = system or {}
    dmi = dict(dmi or {})
    dmi.update({k: v for k, v in (system.get("dmi") or {}).items() if v})
    cpu_info = system.get("cpu") or {}
    osinfo = system.get("os") or {}
    kinfo = system.get("kernel") or {}
    board = dmi.get("board_name") or (baseline.get("dmi") or {}).get("board_name") or cfg.get("FALLBACK_BOARD", "")
    bios = dmi.get("bios_version") or (baseline.get("dmi") or {}).get("bios_version") or cfg.get("FALLBACK_BIOS", "")
    bios_date = dmi.get("bios_date") or (baseline.get("dmi") or {}).get("bios_date", "")
    vendor = dmi.get("board_vendor") or dmi.get("sys_vendor") or (baseline.get("dmi") or {}).get("board_vendor", "")
    cpu = cpu_info.get("model_name") or cpu or baseline.get("cpu") or cfg.get("FALLBACK_CPU", "")
    memory = memory_from_dimms(list(baseline.get("dimms") or [])) or cfg.get("FALLBACK_MEMORY", "")
    base_kb = int(baseline.get("memtotal_kb") or 0)
    if base_kb:
        memory = f"{memory} (healthy MemTotal {base_kb} kB / {kb_to_gib(base_kb)})"
    chassis = dmi.get("chassis_type", "")
    chassis_s = CHASSIS_TYPES.get(chassis, chassis)
    cpu_ids = ""
    if cpu_info.get("family"):
        cpu_ids = (
            f"family {cpu_info.get('family')} model {cpu_info.get('model')} "
            f"stepping {cpu_info.get('stepping')}"
        )
    os_line = osinfo.get("PRETTY_NAME") or os_name or "unknown"
    if osinfo.get("VERSION_ID"):
        os_line = f"{os_line} ({osinfo.get('ID', '')} {osinfo.get('VERSION_ID')})".strip()
    kernel_line = kinfo.get("release") or kernel or "unknown"
    if kinfo.get("machine"):
        kernel_line = f"{kernel_line} {kinfo['machine']}"
    product = dmi.get("product_name", "")
    if product and board and product in board:
        product = ""
    sys_vendor = dmi.get("sys_vendor", "")
    if sys_vendor and vendor and sys_vendor == vendor:
        sys_vendor = ""
    chassis_vendor = dmi.get("chassis_vendor", "")
    chassis_line = chassis_s
    if chassis_vendor and chassis_vendor != vendor:
        chassis_line = f"{chassis_vendor} {chassis_s}".strip()
    board_ver = dmi.get("board_version", "")
    board_line = board
    if board_ver and not is_dmi_placeholder(board_ver):
        board_line = f"{board} rev {board_ver}"
    product_ver = dmi.get("product_version", "")
    if product_ver and product_ver == board_ver:
        product_ver = ""
    board_serial = dmi.get("board_serial", "") or (baseline.get("dmi") or {}).get("board_serial", "")
    rows = [
        ("Vendor", vendor),
        ("System vendor", sys_vendor),
        ("Motherboard", board_line),
        ("Board serial", board_serial),
        ("Product", product),
        ("Product family", dmi.get("product_family", "")),
        ("Product version", product_ver),
        ("SKU", dmi.get("product_sku", "")),
        ("Chassis", chassis_line),
        ("BIOS vendor", dmi.get("bios_vendor", "")),
        ("BIOS version", f"{bios} ({bios_date})".strip() if bios_date else bios),
        ("BIOS revision", dmi.get("bios_release", "")),
        ("Firmware boot mode", system.get("boot_mode") or boot_mode()),
        ("CPU", cpu),
        ("CPU ID", cpu_ids),
        ("CPU microcode", cpu_info.get("microcode", "")),
        ("Memory (healthy baseline)", memory or "no healthy baseline yet"),
        ("OS", os_line),
        ("Kernel", kernel_line),
        ("Kernel build", kinfo.get("version", "")),
        ("Sleep policy", system.get("mem_sleep") or mem_sleep()),
    ]
    return md_kv_table(rows)


def system_oneliner(system: dict[str, Any] | None = None, dmi: dict[str, str] | None = None, cpu: str = "", kernel: str = "") -> str:
    system = system or collect_system_info()
    dmi = dmi or system.get("dmi") or {}
    board = dmi.get("board_name") or "unknown board"
    bios = dmi.get("bios_version") or "unknown BIOS"
    cpu = (system.get("cpu") or {}).get("model_name") or cpu or "unknown CPU"
    krel = (system.get("kernel") or {}).get("release") or kernel or live_kernel()
    os_name = (system.get("os") or {}).get("PRETTY_NAME") or live_os() or "unknown OS"
    mode = system.get("boot_mode") or boot_mode()
    return f"\n · BOARD: {board}\n · BIOS: {bios}\n · CPU: {cpu}\n · OS: {os_name}\n · KERNEL: {krel}\n · BOOT MODE: {mode}"


def _capture_ident(ev: dict[str, Any]) -> dict[str, str]:
    meta = ev.get("meta") or {}
    dmi = ev.get("dmi") or {}
    sysinfo = ev.get("system") or {}
    kdmi = sysinfo.get("dmi") or dmi
    kinfo = sysinfo.get("kernel") or {}
    osinfo = sysinfo.get("os") or {}
    return {
        "ts": str(ev.get("ts") or meta.get("ts") or ""),
        "event": str(ev.get("event") or meta.get("event") or ""),
        "bios": kdmi.get("bios_version", ""),
        "bios_date": kdmi.get("bios_date", ""),
        "board": kdmi.get("board_name", ""),
        "os": osinfo.get("PRETTY_NAME") or osinfo.get("NAME") or "",
        "kernel": kinfo.get("release") or meta.get("uname") or "",
    }


def captured_system_table(events: list[dict[str, Any]], live: dict[str, Any] | None = None) -> str:
    if not events:
        return "_No captures yet._"
    live = live or collect_system_info()
    live_d = live.get("dmi") or {}
    live_k = (live.get("kernel") or {}).get("release") or live_kernel()
    live_os_name = (live.get("os") or {}).get("PRETTY_NAME") or live_os()
    live_bios = live_d.get("bios_version", "")
    live_board = live_d.get("board_name", "")
    latest = _capture_ident(events[-1])
    alert_ev = next((e for e in reversed(events) if is_alert(e)), None)
    lines = [
        f"- Latest capture: `{latest['ts']}` event `{latest['event']}`",
    ]
    diffs: list[str] = []
    if latest["bios"] and live_bios and latest["bios"] != live_bios:
        diffs.append(f"BIOS {latest['bios']} (live {live_bios})")
    if latest["kernel"] and live_k and latest["kernel"] != live_k:
        diffs.append(f"kernel {latest['kernel']} (live {live_k})")
    if latest["os"] and live_os_name and latest["os"] != live_os_name:
        diffs.append(f"OS {latest['os']} (live {live_os_name})")
    if latest["board"] and live_board and latest["board"] != live_board:
        diffs.append(f"board {latest['board']}")
    if diffs:
        lines.append("- Latest capture differs from live: " + "; ".join(diffs))
    else:
        lines.append("- Latest capture BIOS/board/OS/kernel match the live table above.")
    if alert_ev:
        alert = _capture_ident(alert_ev)
        if alert["ts"] != latest["ts"] or alert["event"] != latest["event"]:
            extra = []
            if alert["bios"]:
                extra.append(f"BIOS {alert['bios']}")
            if alert["kernel"]:
                extra.append(f"kernel {alert['kernel']}")
            suffix = f" ({', '.join(extra)})" if extra else ""
            lines.append(f"- Last alert: `{alert['ts']}` event `{alert['event']}`{suffix}")
    return "\n".join(lines)


def dimm_report_section(
    current: list[dict[str, str]],
    healthy: list[dict[str, str]],
    corrupt: list[dict[str, str]],
) -> str:
    parts = ["### Current", "", dimm_table(current)]
    if healthy and not dimms_match(current, healthy):
        parts.extend(["", "### Last healthy baseline (differs from current)", "", dimm_table(healthy)])
    if corrupt and not dimms_match(current, corrupt):
        parts.extend(["", "### Last corrupt snapshot", "", dimm_table(corrupt)])
    elif not corrupt:
        parts.extend(["", "_No corrupt DIMM snapshot recorded yet._"])
    return "\n".join(parts)


def pick_dmi(events: list[dict[str, Any]]) -> dict[str, str]:
    live = live_dmi()
    if live:
        return live
    for ev in reversed(events):
        dmi = ev.get("dmi") or {}
        if dmi:
            return dmi
    return {}


def pick_dimms(ev: dict[str, Any] | None) -> list[dict[str, str]]:
    if not ev:
        return []
    return list(ev.get("dimms") or [])


def corrupt_dimm_lines(dimms: list[dict[str, str]]) -> list[str]:
    lines: list[str] = []
    for d in dimms:
        flags = spd_hub.dimm_flags(d)
        if not flags:
            continue
        why = ", ".join(flags)
        lines.append(
            f"  {d.get('locator', '?')}: part {d.get('part') or 'empty'}, "
            f"width {d.get('total_width') or '?'}/{d.get('data_width') or '?'}"
            f" ({why})"
        )
    return lines


def spd_now_from_state(state_dir: Path, events: list[dict[str, Any]]) -> tuple[str, list[str]]:
    latest = events[-1] if events else None
    dimms = pick_dimms(latest)
    flag_list: list[str] = []
    if latest:
        raw = str(latest.get("flags") or "")
        flag_list = [f for f in raw.split(",") if f]
        if not flag_list:
            flag_list = []
            for d in dimms:
                flag_list.extend(spd_hub.dimm_flags(d))
        hub = latest.get("hub") or {}
        if hub.get("stuck") and "hub_mr11_stuck" not in flag_list:
            flag_list.append("hub_mr11_stuck")
    spd_file = (state_dir / "SPD_NOW").read_text(errors="replace").strip() if (state_dir / "SPD_NOW").is_file() else ""
    if flag_list or spd_file == "corrupt" or (latest and is_alert(latest)):
        return "corrupted", corrupt_dimm_lines(dimms) or [f"  flags: {', '.join(flag_list) or 'alert'}"]
    if not dimms and not events:
        return "unknown", ["  No DIMM snapshot yet. Run: am5-spd-diag snapshot"]
    if not dimms:
        return "unknown", ["  Latest capture has no DIMM summary (SMBIOS/dmidecode missing?)."]
    return "healthy", []


def dimm_change_lines(before: list[dict[str, str]], after: list[dict[str, str]]) -> list[str]:
    by_b = {d.get("locator", "?"): d for d in before}
    by_a = {d.get("locator", "?"): d for d in after}
    lines: list[str] = []
    for loc in list(dict.fromkeys([*by_b, *by_a])):
        b, a = by_b.get(loc), by_a.get(loc)
        if not b or not a:
            lines.append(f"- {loc}: missing from {'before' if not b else 'after'}")
            continue
        if dimm_key(b) == dimm_key(a):
            lines.append(f"- {loc}: unchanged ({b.get('size') or '?'} {b.get('part') or 'empty'})")
            continue
        lines.append(
            f"- {loc}: {b.get('size') or '?'} {b.get('part') or 'empty'} "
            f"{b.get('total_width') or '?'} → "
            f"{a.get('size') or '?'} {a.get('part') or 'empty'} "
            f"{a.get('total_width') or '?'}"
        )
    return lines


def render_transitions(transitions: list[dict[str, Any]]) -> str:
    if not transitions:
        return "_No healthy-to-corrupt transition captured yet. Keep the monitor enabled through sleep and a warm reboot._"
    blocks: list[str] = []
    for idx, tr in enumerate(transitions, 1):
        alert_ev = tr["alert_event"]
        healthy = tr["prev_healthy"]
        healthy_kb = int((healthy or {}).get("memtotal_kb") or 0)
        alert_kb = int(alert_ev.get("memtotal_kb") or 0)
        kind = tr.get("boot_kind") or "unknown"
        if kind == "unexpected_power_loss":
            kind = (
                "unexpected_power_loss (no reboot/poweroff capture; crash, reset, or power "
                "interruption — not a full AC-cord pull if 5VSB/DIMM RGB stayed up)"
            )
        elif kind == "unknown" and (tr["alert_event"].get("event") == "boot"):
            kind = "unknown (no reboot/poweroff capture)"
        blocks.append(f"### Transition {idx}")
        blocks.append("")
        blocks.append(
            f"- Last healthy: `{healthy.get('ts') if healthy else 'unknown'}` "
            f"event `{healthy.get('event') if healthy else 'unknown'}` "
            f"firmware published {healthy_kb} kB ({kb_to_gib(healthy_kb)})"
        )
        blocks.append(f"- Sleep cycles on the previous boot: **{tr['sleep_count']}**")
        if tr.get("mem_sleep"):
            blocks.append(f"- Sleep mode on last suspend-pre: `{tr['mem_sleep']}`")
        blocks.append(f"- How this boot started: **{kind}**")
        if tr.get("stuck_hubs"):
            blocks.append(f"- Stuck SPD5118 hub(s): `{', '.join(tr['stuck_hubs'])}` (MR11=0x08)")
        blocks.append(
            f"- Corrupt snapshot: `{alert_ev.get('ts')}` event `{alert_ev.get('event')}` "
            f"firmware published {alert_kb} kB ({kb_to_gib(alert_kb)}) flags `{alert_ev.get('flags')}`"
        )
        changes = dimm_change_lines(pick_dimms(healthy) if healthy else [], pick_dimms(alert_ev))
        if changes:
            blocks.append("- Identity change:")
            blocks.extend(f"  {line}" for line in changes)
        blocks.append("")
        blocks.append("**Event sequence**")
        blocks.append("")
        blocks.append("| Time | Event | State | MemTotal kB | Flags |")
        blocks.append("|---|---|---|---|---|")
        for ev in tr["chain"]:
            blocks.append(event_row(ev))
        blocks.append("")
    return "\n".join(blocks).rstrip()


def render_boot_timeline(boots: OrderedDict[str, list[dict[str, Any]]]) -> str:
    if not boots:
        return "_No events recorded yet._"
    blocks: list[str] = []
    boot_ids = list(boots.keys())
    for i, bid in enumerate(boot_ids):
        evs = boots[bid]
        first = evs[0]
        last = evs[-1]
        alerted = any(is_alert(e) for e in evs)
        boot_ev = next((e for e in evs if e.get("event") == "boot"), None)
        kind = str((boot_ev or {}).get("boot_kind") or "")
        if kind in {"", "unknown", "same_boot"}:
            if i == 0:
                kind = kind or "unknown"
            else:
                prev_last = boots[boot_ids[i - 1]][-1]
                kind = boot_kind_from_previous_event(str(prev_last.get("event") or ""))
        kind_bit = f" started={kind}" if kind and kind not in {"unknown", "same_boot"} else ""
        blocks.append(
            f"- boot `{bid[:8]}…`{kind_bit} events={len(evs)} sleep_cycles={sleep_cycles(evs)} "
            f"SPD={'corrupted' if alerted else 'healthy'} "
            f"first=`{first.get('event')}` last=`{last.get('event')}` "
            f"firmware {last.get('memtotal_kb')} kB"
        )
    return "\n".join(blocks)


def hub_section(events: list[dict[str, Any]], transitions: list[dict[str, Any]]) -> str:
    evidence: list[str] = []
    seen: set[str] = set()

    def add(item: str) -> None:
        if item and item not in seen:
            seen.add(item)
            evidence.append(item)

    ordered: list[dict[str, Any]] = []
    for ev in reversed(events or []):
        if is_alert(ev):
            ordered.append(ev)
    latest = events[-1] if events else None
    if latest and latest not in ordered:
        ordered.append(latest)
    for ev in ordered:
        hub = ev.get("hub") or {}
        ts = ev.get("ts") or ""
        event = ev.get("event") or ""
        label = f"`{ts}` `{event}`" if ts else "capture"
        if hub.get("dmesg_stuck"):
            add(
                f"{label}: kernel spd5118 unbound at {', '.join(hub['dmesg_stuck'])} "
                "(16-bit addressing refused)"
            )
        quoted_page0 = False
        for row in hub.get("stuck") or []:
            add(
                f"{label}: MR11={row.get('mr11_hex')} on {row.get('sysfs')} "
                f"({row.get('adapter') or 'i2c'})"
            )
            head = row.get("spd_page0_head") or ""
            if head:
                spaced = " ".join(head[i : i + 2] for i in range(0, min(len(head), 32), 2))
                add(
                    f"{label}: SPD page-0 window (not full EEPROM) first 16 bytes: `{spaced}`"
                )
                quoted_page0 = True
        dmesg_text = str(ev.get("dmesg_spd") or "")
        lines = [ln.strip() for ln in dmesg_text.splitlines() if ln.strip()]
        for ln in lines[:3]:
            add(f"{label} dmesg: `{ln}`")
        if not quoted_page0:
            for page in ev.get("spd_page0") or []:
                text = str(page.get("text") or "")
                for ln in text.splitlines():
                    if ln.startswith("0000:"):
                        add(f"{label} {page.get('name')}: `{ln}`")
                        break
    for tr in transitions:
        for d in tr.get("bad_dimms") or []:
            if "ghost_page0" in spd_hub.dimm_flags(d):
                add(
                    f"ghost page-0 serial `{d.get('serial')}` on {d.get('locator')} (empty part)"
                )
    if not evidence:
        return (
            "No SPD5118 hub probe evidence yet. Capture as root (or via the polkit snapshot "
            "helper) so `/dev/i2c-*` can be read, or look for "
            "`spd5118: Adapter does not support 16-bit register addresses` in dmesg."
        )
    lines = ["Evidence from this machine (corrupt captures kept even if identity later restored):"]
    lines.extend(f"- {item}" for item in evidence)
    return "\n".join(lines)


def current_state(state_dir: Path, events: list[dict[str, Any]], baseline: dict[str, Any]) -> str:
    now, _details = spd_now_from_state(state_dir, events)
    live_kb = memtotal_kb()
    base_kb = int(baseline.get("memtotal_kb") or 0)
    seen = "yes" if any(is_alert(e) for e in events) else "no"
    lines = [
        f"- **SPD now: {now}**",
    ]
    if live_kb and base_kb and live_kb == base_kb:
        lines.append(f"- Firmware published RAM: **{live_kb} kB** ({kb_to_gib(live_kb)}; matches healthy baseline)")
    else:
        live_line = f"- Firmware published RAM: **{live_kb} kB** ({kb_to_gib(live_kb)})"
        if base_kb:
            live_line += f"; last healthy baseline **{base_kb} kB** ({kb_to_gib(base_kb)})"
        lines.append(live_line)
    sleep_total = sum(1 for e in events if e.get("event") in {"suspend-pre", "hibernate-pre"})
    lines.append(
        f"- Suspends: kernel **{kernel_suspend_success()}** this boot; "
        f"package recorded **{sleep_total}**"
    )
    lines.append(f"- Corruption seen in earlier captures: **{seen}**")
    if now == "healthy" and seen == "yes":
        lines.append("- Identity looks restored since the last alert (AC power loss or `am5-spd-diag recover` + reboot).")
    boot_kind = latest_boot_start_kind(events)
    if boot_kind == "unexpected_power_loss":
        lines.append(
            "- This boot followed an **unexpected power loss** (previous boot had no reboot/poweroff "
            "capture: crash, reset, or power interruption). That is not a clean ACPI shutdown and is "
            "not the same as pulling the AC cord: 5VSB/VDDSPD may stay up (DIMM RGB often stays lit)."
        )
    elif boot_kind == "shutdown_poweroff":
        lines.append("- This boot followed a captured ACPI poweroff/shutdown.")
    elif boot_kind == "warm_reboot":
        lines.append("- This boot followed a captured warm reboot.")
    lines.append(f"- Timeline events: **{len(events)}**")
    notice = state_dir / "NOTICE"
    if notice.is_file() and now == "corrupted":
        lines.append(f"- Notice: {notice.read_text(errors='replace').strip()}")
    return "\n".join(lines)


def fill_template(template: str, mapping: dict[str, str]) -> str:
    def repl(match: re.Match[str]) -> str:
        return mapping.get(match.group(1), match.group(0))

    return re.sub(r"\{\{([A-Z0-9_]+)\}\}", repl, template)


def _systemctl_state(verb: str, name: str) -> str:
    try:
        proc = subprocess.run(
            ["systemctl", verb, name],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=False,
        )
    except (FileNotFoundError, OSError):
        return "unknown"
    out = (proc.stdout or "").strip()
    return out.splitlines()[0] if out else "unknown"


def unit_line(name: str) -> str:
    enabled = _systemctl_state("is-enabled", name)
    active = _systemctl_state("is-active", name)
    note = ""
    if name.endswith(("-pre-sleep.service", "-post-sleep.service")):
        if enabled == "enabled" and active == "inactive":
            note = " (oneshot; idle until sleep)"
    return f"{name}: {active}, {enabled}{note}"


def kernel_suspend_success() -> str:
    path = Path("/sys/power/suspend_stats/success")
    if not path.is_file():
        return "unknown"
    return path.read_text(errors="replace").strip() or "unknown"


def sleep_history_lines(events: list[dict[str, Any]], sleep_total: int) -> list[str]:
    kernel = kernel_suspend_success()
    lines = [
        f"Kernel suspends this boot: {kernel} (from /sys/power/suspend_stats/success)",
        f"Recorded by this package: {sleep_total} suspend-pre/hibernate-pre event(s)",
    ]
    if sleep_total == 0:
        lines.append(
            "  No sleep captures in this log yet. Pre/post-sleep units are oneshot, "
            "so inactive between sleeps is normal."
        )
    return lines


def build_context(cfg: dict[str, str], events: list[dict[str, Any]], state_dir: Path) -> dict[str, Any]:
    boots = group_boots(events)
    transitions = find_transitions(events)
    dmi = pick_dmi(events)
    last_alert = next((e for e in reversed(events) if is_alert(e)), None)
    last_healthy = next((e for e in reversed(events) if not is_alert(e)), None)
    latest = events[-1] if events else None
    baseline = load_baseline(state_dir)
    now, _details = spd_now_from_state(state_dir, events)
    return {
        "boots": boots,
        "transitions": transitions,
        "dmi": dmi,
        "last_alert": last_alert,
        "last_healthy": last_healthy,
        "latest": latest,
        "baseline": baseline,
        "sleep_total": sum(sleep_cycles(v) for v in boots.values()),
        "alert_count": sum(1 for e in events if is_alert(e)),
        "spd_now": now,
        "pattern": render_pattern(events, boots, transitions),
        "state_dir": state_dir,
    }


def report_title(dmi: dict[str, str], now: str, alert_count: int) -> str:
    board = dmi.get("board_name") or "AM5 board"
    bios = dmi.get("bios_version") or "unknown BIOS"
    if alert_count == 0:
        return f"{board} BIOS {bios}: AM5 SPD identity monitor (no corruption recorded)"
    if now == "corrupted":
        return f"{board} BIOS {bios}: DIMM identity lost after sleep + warm reboot"
    return f"{board} BIOS {bios}: DIMM identity was corrupted after sleep + warm reboot (now restored)"


def dimm_expect_line(dimms: list[dict[str, str]]) -> str:
    if not dimms:
        return "no DIMM snapshot"
    parts = []
    for d in dimms:
        parts.append(
            f"{d.get('locator', '?')} {d.get('size', '?')} "
            f"{d.get('total_width') or '?'}/{d.get('data_width') or '?'} "
            f"{d.get('part') or 'empty'} {d.get('speed') or ''} "
            f"serial {d.get('serial') or '?'}"
        )
    return "; ".join(" ".join(p.split()) for p in parts)


def expected_actual_section(
    now: str,
    baseline: dict[str, Any],
    current_dimms: list[dict[str, str]],
    healthy_dimms: list[dict[str, str]],
    corrupt_dimms: list[dict[str, str]],
    last_alert: dict[str, Any] | None,
    live_kb: int,
) -> str:
    base_kb = int(baseline.get("memtotal_kb") or 0)
    alert_kb = int((last_alert or {}).get("memtotal_kb") or 0)
    expected_dimms = healthy_dimms or list(baseline.get("dimms") or [])
    actual_dimms = current_dimms if now == "corrupted" else (corrupt_dimms or current_dimms)
    lines = [
        f"- **Slot map:** {slot_map_line(expected_dimms or current_dimms)}",
        f"- **Expected:** {dimm_expect_line(expected_dimms)}; "
        f"firmware MemTotal {base_kb} kB ({kb_to_gib(base_kb)})",
        f"- **Actual:** {dimm_expect_line(actual_dimms)}; "
        f"firmware MemTotal {alert_kb or live_kb} kB ({kb_to_gib(alert_kb or live_kb)}) "
        f"(live now {live_kb} kB / {kb_to_gib(live_kb)})",
        "- **Impact:** firmware published placeholder DIMM identity and a smaller memory map. "
        "Linux is reflecting SMBIOS/e820 from UEFI, not dropping RAM in the MM layer.",
    ]
    return "\n".join(lines)


E820_DMESG_PREFIX = re.compile(r"^(?:\[[^\]]+\]\s*)+")


def e820_display_line(ln: str) -> str:
    return E820_DMESG_PREFIX.sub("", ln.strip()).strip()


def e820_lines(ev: dict[str, Any] | None) -> list[str]:
    if not ev:
        return []
    text = str(ev.get("e820") or "")
    if not text.strip() and ev.get("dir"):
        path = Path(str(ev["dir"])) / "dmesg-filtered.txt"
        if path.is_file():
            text = "\n".join(
                ln for ln in path.read_text(errors="replace").splitlines() if "BIOS-e820" in ln
            )
    return [e820_display_line(ln) for ln in text.splitlines() if ln.strip() and not ln.startswith("#")]


def e820_high_end(lines: list[str]) -> str:
    ram = [ln for ln in lines if "System RAM" in ln]
    for ln in reversed(ram):
        match = re.search(r"\[mem 0x[0-9a-f]+-(0x[0-9a-f]+)\]", ln, re.I)
        if match:
            return match.group(1).lower()
    return ""


def e820_section(last_alert: dict[str, Any] | None, last_healthy: dict[str, Any] | None, baseline: dict[str, Any]) -> str:
    healthy_lines = e820_lines(last_healthy)
    alert_lines = e820_lines(last_alert)
    if not healthy_lines and not alert_lines:
        return "_No e820 map captured. Privileged snapshot records the full BIOS-e820 table at boot/manual/alert._"
    blocks = [
        "Full firmware e820 table (all types, not only System RAM). Kernel dmesg timestamps are omitted.",
        "",
    ]
    if healthy_lines and alert_lines and healthy_lines != alert_lines:
        healthy_end = e820_high_end(healthy_lines)
        alert_end = e820_high_end(alert_lines)
        if healthy_end and alert_end and healthy_end != alert_end:
            blocks.append(
                f"System RAM high range differs: healthy ends at `{healthy_end}`, "
                f"corrupt ends at `{alert_end}`."
            )
            blocks.append("")
        else:
            blocks.append("Healthy and corrupt e820 tables differ (see full lists below).")
            blocks.append("")
    elif healthy_lines and alert_lines:
        blocks.append(
            "Healthy and corrupt e820 tables match. Identity corruption on this boot did not "
            "change the firmware memory map (typical until the next POST/warm reboot)."
        )
        blocks.append("")
    base_kb = int(baseline.get("memtotal_kb") or 0)
    if last_healthy:
        blocks.append(
            f"Healthy `{last_healthy.get('ts')}` MemTotal {last_healthy.get('memtotal_kb')} kB "
            f"(baseline {base_kb} kB / {kb_to_gib(base_kb)}):"
        )
        blocks.extend(f"    {ln}" for ln in healthy_lines or ["_no e820 lines_"])
    if last_alert:
        if last_healthy:
            blocks.append("")
        blocks.append(
            f"Corrupt `{last_alert.get('ts')}` MemTotal {last_alert.get('memtotal_kb')} kB "
            f"({kb_to_gib(int(last_alert.get('memtotal_kb') or 0))}):"
        )
        blocks.extend(f"    {ln}" for ln in alert_lines or ["_no e820 lines_"])
    return "\n".join(blocks)


def attachment_checklist(events: list[dict[str, Any]], last_alert: dict[str, Any] | None, latest: dict[str, Any] | None, state_dir: Path) -> str:
    ev = last_alert or latest
    directory = Path(str((ev or {}).get("dir") or ""))
    names = [
        "hub.json",
        "dmidecode-memory.txt",
        "dimm-summary.txt",
        "e820.txt",
        "e820-system-ram.txt",
        "dmesg-spd5118.txt",
        "dmesg-filtered.txt",
        "dmi-sysfs.txt",
        "system.json",
    ]
    lines = [
        f"Capture logs: `{state_dir}` (timeline.jsonl, ALERTS.log, baseline.json, events/).",
        "`am5-spd-diag package` builds a tarball of the same.",
        "",
        "Files in the last corrupt (or latest) event directory:",
    ]
    if not directory.is_dir():
        lines.append("- _event directory missing_")
        return "\n".join(lines)
    for name in names:
        path = directory / name
        mark = "present" if path.is_file() and path.stat().st_size else "missing"
        lines.append(f"- `{name}`: {mark}")
    pages = sorted(directory.glob("spd-page0-*.txt"))
    if pages:
        for path in pages:
            lines.append(f"- `{path.name}`: present")
    else:
        lines.append("- `spd-page0-*.txt`: missing")
    return "\n".join(lines)


def mapping_from_context(cfg: dict[str, str], events: list[dict[str, Any]], ctx: dict[str, Any]) -> dict[str, str]:
    dmi = ctx["dmi"]
    last_alert = ctx["last_alert"]
    last_healthy = ctx["last_healthy"]
    latest = ctx["latest"]
    baseline = ctx["baseline"]
    live_kb = memtotal_kb()
    now = ctx["spd_now"]
    if ctx["alert_count"] == 0:
        summary = (
            "Monitor is installed. No SPD identity corruption has been recorded yet. "
            "Use the system normally (sleep/wake, then reboot) and re-run `am5-spd-diag report`."
        )
    elif now == "corrupted":
        summary = (
            "SPD identity is **currently corrupted**: firmware is publishing placeholder DIMM fields "
            "(Unknown/missing part and/or 8-bit width). Linux MemTotal is whatever firmware advertised, "
            "not a separate Linux bug. Warm reboot does not clear SPD5118 MR11 standby state; AC power "
            "loss does. Optional in-band recover: `sudo am5-spd-diag recover` then reboot."
        )
    else:
        summary = (
            "SPD identity looks **healthy now**, but earlier captures recorded corruption. "
            "See the last corrupt DIMM snapshot below. Restore is AC power loss "
            "or `am5-spd-diag recover` + reboot."
        )
    healthy_dimms = list(baseline.get("dimms") or pick_dimms(last_healthy))
    current_dimms = pick_dimms(latest or last_alert or last_healthy)
    corrupt_dimms = pick_dimms(last_alert)
    system = collect_system_info()
    attach = attachment_checklist(events, last_alert, latest, ctx["state_dir"])
    repro = (
        ctx["pattern"]
        + "\n\nSuggested loop: known-healthy boot → N suspend/resume cycles → "
        "warm reboot → inspect `am5-spd-diag status`."
    )
    title = report_title(dmi, now, ctx["alert_count"])
    return {
        "GENERATED_AT": iso_now(),
        "REPORT_TITLE": title,
        "SUMMARY": summary,
        "EXPECTED_ACTUAL": expected_actual_section(
            now, baseline, current_dimms, healthy_dimms, corrupt_dimms, last_alert, live_kb
        ),
        "HARDWARE_TABLE": hardware_table(
            cfg, dmi, live_cpu(), live_os(), live_kernel(), baseline, system
        ),
        "SYSTEM_CAPTURED": captured_system_table(events, system),
        "BIOS_VERSION": dmi.get("bios_version") or cfg.get("FALLBACK_BIOS", "unknown"),
        "CURRENT_STATE": current_state(ctx["state_dir"], events, baseline),
        "SPD_NOW": now,
        "PATTERN": ctx["pattern"],
        "HUB_SECTION": hub_section(events, ctx["transitions"]),
        "E820_SECTION": e820_section(last_alert, last_healthy, baseline),
        "DIMM_SECTION": dimm_report_section(current_dimms, healthy_dimms, corrupt_dimms),
        "DIMM_TABLE_CURRENT": dimm_table(current_dimms),
        "DIMM_TABLE_HEALTHY": dimm_table(healthy_dimms),
        "DIMM_TABLE_CORRUPT": dimm_table(corrupt_dimms),
        "TRANSITIONS": render_transitions(ctx["transitions"]),
        "BOOT_TIMELINE": render_boot_timeline(ctx["boots"]),
        "ALERT_COUNT": str(ctx["alert_count"]),
        "EVENT_COUNT": str(len(events)),
        "SLEEP_CYCLES": str(ctx["sleep_total"]),
        "CORRUPTION_SEEN": "yes" if ctx["alert_count"] else "no",
        "MEMTOTAL_CURRENT": f"{live_kb} kB ({kb_to_gib(live_kb)})",
        "MEM_SLEEP": mem_sleep(),
        "OS_RELEASE": live_os() or "unknown",
        "KERNEL": live_kernel(),
        "ATTACHMENTS": attach,
        "SEQUENCE": render_transitions(ctx["transitions"]),
        "REPRO_STEPS": repro,
        "FORUM_URL": FORUM_URL,
    }


def print_analyze(events: list[dict[str, Any]], ctx: dict[str, Any]) -> None:
    print(f"SPD now: {ctx['spd_now']}")
    print(f"System: {system_oneliner(dmi=ctx.get('dmi') or {}, cpu=live_cpu(), kernel=live_kernel())}")
    print(
        f"History: {len(events)} events, {ctx['alert_count']} alerts, "
        f"{len(ctx['boots'])} boots, {ctx['sleep_total']} recorded suspends"
    )
    for line in sleep_history_lines(events, ctx["sleep_total"]):
        print(line)
    print()
    print("Reproduction pattern")
    print(ctx["pattern"])
    print()
    print("Boots")
    print(render_boot_timeline(ctx["boots"]))
    if ctx["transitions"]:
        print()
        print("Corruption events")
        for i, tr in enumerate(ctx["transitions"], 1):
            ev = tr["alert_event"]
            loc = ",".join(d.get("locator", "?") for d in tr.get("bad_dimms") or []) or "?"
            print(
                f"  {i}. {ev.get('ts')} {ev.get('event')} "
                f"boot={tr['boot_kind']} sleeps_before={tr['sleep_count']} "
                f"dimms={loc} flags={ev.get('flags')}"
            )
    else:
        print()
        print("No corruption transitions yet.")
    print()
    print("SPD5118 hub")
    print(re.sub(r"_([^_\n]+)_", r"\1", hub_section(events, ctx["transitions"])))
    print()
    print("For a ticket: am5-spd-diag report")


def dimm_status_lines(dimms: list[dict[str, str]], now: str) -> list[str]:
    if now == "corrupted":
        return corrupt_dimm_lines(dimms) or ["  (alert; DIMM fields missing from latest capture)"]
    if not dimms:
        return ["  No populated DIMM snapshot yet. Run: am5-spd-diag snapshot"]
    lines: list[str] = []
    for d in dimms:
        man = d.get("manufacturer") or "Unknown"
        part = d.get("part") or "empty"
        lines.append(
            f"  {d.get('locator', '?')}: {d.get('size', '?')} {man} {part} "
            f"width {d.get('total_width') or '?'}/{d.get('data_width') or '?'}"
        )
    return lines


def print_status(events: list[dict[str, Any]], ctx: dict[str, Any]) -> None:
    now = ctx["spd_now"]
    baseline = ctx["baseline"]
    live_kb = memtotal_kb()
    base_kb = int(baseline.get("memtotal_kb") or 0)
    dimms = pick_dimms(ctx.get("latest")) or list(baseline.get("dimms") or [])
    print(f"SPD now: {now}")
    print(f"System: {system_oneliner(dmi=ctx.get('dmi') or {}, cpu=live_cpu(), kernel=live_kernel())}")
    for line in dimm_status_lines(dimms, now):
        print(f" · {line.strip()}")
    print(
        f"Firmware published RAM: {live_kb} kB ({kb_to_gib(live_kb)})"
        + (f"; healthy baseline {base_kb} kB ({kb_to_gib(base_kb)})" if base_kb else "")
    )
    print(f"Sleep policy: {mem_sleep()}")
    print(
        f"Log: {len(events)} events, {ctx['alert_count']} alert(s), "
        f"{ctx['sleep_total']} recorded suspend(s)"
    )
    print()
    print("Monitor")
    print(f"  {unit_line('am5-spd-diag.service')}")
    print(f"  {unit_line('am5-spd-diag-pre-sleep.service')}")
    print(f"  {unit_line('am5-spd-diag-post-sleep.service')}")
    hook = Path("/usr/lib/systemd/system-sleep/am5-spd-diag")
    print(f"  sleep hook: {'installed' if hook.is_file() else 'missing'}")
    notice = ctx["state_dir"] / "NOTICE"
    if notice.is_file() and now == "corrupted":
        print()
        print(notice.read_text(errors="replace").strip())
    print()
    print("For sleep/reboot history: am5-spd-diag analyze")


def user_artifact_dir(subdir: str) -> Path:
    home = Path.home()
    sudo_user = os.environ.get("SUDO_USER")
    if sudo_user and sudo_user != "root":
        try:
            import pwd

            home = Path(pwd.getpwnam(sudo_user).pw_dir)
        except Exception:
            pass
    return home / ".local/share/am5-spd-diag" / subdir


def write_report(prefix: Path, state_dir: Path, cfg: dict[str, str], events: list[dict[str, Any]], ctx: dict[str, Any], out: Path | None) -> Path:
    share = Path(os.environ.get("AM5_SPD_DIAG_SHARE") or str(prefix))
    template_path = share / "templates" / "ticket.md.tmpl"
    template = template_path.read_text(encoding="utf-8")
    text = fill_template(template, mapping_from_context(cfg, events, ctx))
    if out is None:
        reports = Path(os.environ.get("AM5_SPD_DIAG_REPORT_DIR") or str(Path(cfg["STATE_DIR"]) / "reports"))
        reports.mkdir(parents=True, exist_ok=True)
        out = reports / f"report-{utc_stamp()}.md"
    else:
        out.parent.mkdir(parents=True, exist_ok=True)
    try:
        out.write_text(text, encoding="utf-8")
    except PermissionError:
        fallback = user_artifact_dir("reports")
        fallback.mkdir(parents=True, exist_ok=True)
        out = fallback / f"report-{utc_stamp()}.md"
        out.write_text(text, encoding="utf-8")
    return out


def chain_dirs(events: list[dict[str, Any]], ctx: dict[str, Any], include_all: bool) -> list[Path]:
    if include_all:
        dirs = []
        for ev in events:
            p = Path(str(ev.get("dir") or ""))
            if p.is_dir():
                dirs.append(p)
        return dirs
    wanted: set[str] = set()
    boots_needed: set[str] = set()
    boot_ids = list(ctx["boots"].keys())
    for tr in ctx["transitions"]:
        for ev in tr["chain"]:
            if ev.get("dir"):
                wanted.add(str(ev["dir"]))
            if ev.get("boot_id"):
                boots_needed.add(str(ev["boot_id"]))
    for ev in events:
        if is_alert(ev):
            wanted.add(str(ev.get("dir") or ""))
            bid = str(ev.get("boot_id") or "")
            boots_needed.add(bid)
            if bid in boot_ids:
                idx = boot_ids.index(bid)
                if idx > 0:
                    boots_needed.add(boot_ids[idx - 1])
    for bid in boots_needed:
        for ev in ctx["boots"].get(bid, []):
            if ev.get("dir"):
                wanted.add(str(ev["dir"]))
    if not wanted and events:
        for ev in events[-12:]:
            if ev.get("dir"):
                wanted.add(str(ev["dir"]))
    dirs = [Path(p) for p in wanted if p and Path(p).is_dir()]
    dirs.sort()
    return dirs


def make_package(
    prefix: Path,
    state_dir: Path,
    cfg: dict[str, str],
    events: list[dict[str, Any]],
    ctx: dict[str, Any],
    package_dir: Path,
    include_all: bool,
) -> Path:
    package_dir.mkdir(parents=True, exist_ok=True)
    stamp = utc_stamp()
    name = f"am5-spd-diag-{stamp}"
    tar_path = package_dir / f"{name}.tar.gz"
    with tempfile.TemporaryDirectory(prefix="am5-spd-diag-pkg-") as tmp:
        root = Path(tmp) / name
        root.mkdir()
        report = write_report(prefix, state_dir, cfg, events, ctx, root / "report.md")
        for fname in ("timeline.jsonl", "ALERTS.log", "baseline.json", "baseline.txt"):
            src = state_dir / fname
            if src.is_file():
                shutil.copy2(src, root / fname)
        events_out = root / "events"
        events_out.mkdir()
        for d in chain_dirs(events, ctx, include_all):
            dest = events_out / d.name
            if dest.exists():
                continue
            shutil.copytree(d, dest, symlinks=True)
        manifest = {
            "generated": iso_now(),
            "include_all": include_all,
            "event_count": len(events),
            "alert_count": ctx["alert_count"],
            "spd_now": ctx["spd_now"],
            "report": str(report.name),
        }
        (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        with tarfile.open(tar_path, "w:gz") as tar:
            tar.add(root, arcname=name)
    return tar_path


def default_prefix() -> Path:
    for key in ("AM5_SPD_DIAG_SHARE", "AM5_SPD_DIAG_PREFIX"):
        env = os.environ.get(key)
        if env:
            return Path(env)
    here = Path(__file__).resolve().parent
    if (here.parent / "templates").is_dir():
        return here.parent
    return Path("/usr/share/am5-spd-diag")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="am5-spd-diag-analyze",
        description="Internal analyzer for am5-spd-diag. Prefer the am5-spd-diag wrapper.",
    )
    parser.add_argument(
        "command",
        choices=["summary", "status", "report", "package", "inventory"],
        help="summary=analyze history; status=current SPD + units; report/package=ticket artifacts; inventory=JSON system details",
    )
    parser.add_argument("--all", action="store_true", help="package every captured event directory")
    parser.add_argument("--out", help="report output path")
    parser.add_argument("--state-dir", help="capture log directory (default: STATE_DIR)")
    parser.add_argument("--package-dir", help="directory for evidence tarballs")
    args = parser.parse_args(argv)

    if args.command == "inventory":
        json.dump(collect_system_info(), sys.stdout, indent=2)
        sys.stdout.write("\n")
        return 0

    prefix = default_prefix()
    cfg = load_config(prefix)
    state_dir = Path(args.state_dir or cfg["STATE_DIR"])
    events = load_timeline(state_dir)
    ctx = build_context(cfg, events, state_dir)

    if args.command == "summary":
        print_analyze(events, ctx)
        return 0
    if args.command == "status":
        print_status(events, ctx)
        return 0
    if args.command == "report":
        out = Path(args.out) if args.out else None
        path = write_report(prefix, state_dir, cfg, events, ctx, out)
        print(path)
        return 0
    package_dir = Path(
        args.package_dir
        or os.environ.get("AM5_SPD_DIAG_PACKAGE_DIR")
        or str(Path(cfg["STATE_DIR"]) / "packages")
    )
    tar_path = make_package(prefix, state_dir, cfg, events, ctx, package_dir, args.all)
    print(tar_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
