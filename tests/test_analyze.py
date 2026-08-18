#!/usr/bin/env python3
"""Analyzer transition/hub/ticket helpers."""
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "libexec"))
import analyze  # noqa: E402
import spd_hub  # noqa: E402


def _ev(event: str, boot: str, alert: bool, ts: str, **extra: object) -> dict:
    row = {
        "event": event,
        "boot_id": boot,
        "alert": alert,
        "ts": ts,
        "memtotal_kb": 32000000 if not alert else 17800092,
        "dimms": extra.pop("dimms", []),
        "hub": extra.pop("hub", {}),
        "meta": extra.pop("meta", {}),
        "mem_sleep": extra.pop("mem_sleep", "s2idle [deep]"),
    }
    row.update(extra)
    return row


CORRUPT_DIMM = {
    "locator": "DIMMB2",
    "size": "2 GiB",
    "total_width": "8 bits",
    "data_width": "8 bits",
    "manufacturer": "Unknown",
    "part": "Unknown",
    "serial": "00206200",
}
STUCK_HUB = {
    "dmesg_stuck": ["1-0053"],
    "stuck": [
        {
            "mr11_hex": "0x08",
            "sysfs": "1-0053",
            "adapter": "SMBus PIIX4 adapter",
            "spd_page0_head": "230c4d08",
        }
    ],
}


def test_find_transitions_resume_and_dedupe() -> None:
    events = [
        _ev("boot", "a", False, "t0"),
        _ev("suspend-pre", "a", False, "t1", mem_sleep="s2idle [deep]"),
        _ev("suspend-post", "a", True, "t2", flags="unknown_part", dimms=[CORRUPT_DIMM], hub=STUCK_HUB),
        _ev("boot", "a", True, "t3", flags="unknown_part", dimms=[CORRUPT_DIMM], hub=STUCK_HUB),
    ]
    trans = analyze.find_transitions(events)
    assert len(trans) == 1, trans
    assert trans[0]["alert_event"]["event"] == "suspend-post"
    assert trans[0]["mem_sleep"] == "s2idle [deep]"


def test_find_transitions_new_boot() -> None:
    events = [
        _ev("boot", "a", False, "t0"),
        _ev("reboot", "a", False, "t1"),
        _ev("boot", "b", True, "t2", boot_kind="warm_reboot", flags="unknown_part", dimms=[CORRUPT_DIMM], hub=STUCK_HUB),
    ]
    trans = analyze.find_transitions(events)
    assert len(trans) == 1
    assert trans[0]["boot_kind"] == "warm_reboot"


def test_hub_section_uses_alert_when_latest_healthy() -> None:
    alert = _ev("boot", "b", True, "t-alert", dimms=[CORRUPT_DIMM], hub=STUCK_HUB, dmesg_spd="spd5118 1-0053: Adapter does not support 16-bit register addresses")
    latest = _ev("boot", "c", False, "t-healthy", hub={"stuck": [], "dmesg_stuck": []})
    trans = analyze.find_transitions([alert])
    text = analyze.hub_section([alert, latest], trans)
    assert "0x08" in text
    assert "1-0053" in text
    assert "page-0" in text


def test_jedec_unknown_manufacturer() -> None:
    text = (
        "locator=DIMMA2|size=16 GiB|total_width=64 bits|data_width=64 bits|"
        "manufacturer=Unknown|serial=B5066693|part=CMH32GX5M2M6000Z36|"
        "mfg_id=Bank 3, Hex 0x9E\n"
    )
    dimms = spd_hub.parse_dimm_summary(text)
    assert dimms[0]["manufacturer"] == "Corsair"


def test_redact_keeps_board_serial() -> None:
    raw = "\tSerial Number: 07E701234567\n\tUUID: 12345678-1234-1234-1234-123456789abc\n\tAsset Tag: ABC\n"
    out = spd_hub.redact_dmi_secrets(raw)
    assert "07E701234567" in out
    assert "[redacted]" in out
    assert "12345678-1234" not in out


def test_slot_map() -> None:
    dimms = [
        {"locator": "DIMMA2", "size": "16 GiB"},
        {"locator": "DIMMB2", "size": "16 GiB"},
    ]
    assert analyze.slot_map_line(dimms) == "2×16 GiB in DIMMA2+DIMMB2"


def test_e820_section_points_at_high_range() -> None:
    healthy = _ev(
        "boot",
        "a",
        False,
        "t-healthy",
        e820=(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n"
            "BIOS-e820: [mem 0x0000000100000000-0x000000085de7ffff] System RAM\n"
        ),
    )
    alert = _ev(
        "boot",
        "b",
        True,
        "t-alert",
        e820=(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n"
            "BIOS-e820: [mem 0x0000000100000000-0x00000004dde7ffff] System RAM\n"
        ),
    )
    text = analyze.e820_section(alert, healthy, {"memtotal_kb": 32250768})
    assert "0x000000085de7ffff" in text
    assert "0x00000004dde7ffff" in text
    assert "System RAM high range differs" in text
    assert "reserved" in text
    assert "Full firmware e820 table" in text


def test_unexpected_power_loss() -> None:
    events = [
        _ev("boot", "a", False, "t0"),
        _ev("manual", "a", False, "t1"),
        _ev("boot", "b", False, "t2"),
    ]
    assert analyze.infer_boot_kind(events, 2) == "unexpected_power_loss"
    assert analyze.boot_kind_from_previous_event("manual") == "unexpected_power_loss"
    assert analyze.boot_kind_from_previous_event("reboot") == "warm_reboot"
    assert analyze.boot_kind_from_previous_event("poweroff") == "shutdown_poweroff"
    text = analyze.render_boot_timeline(analyze.group_boots(events))
    assert "started=unexpected_power_loss" in text
    state = analyze.current_state(Path("/nonexistent"), events, {"memtotal_kb": 32000000})
    assert "unexpected power loss" in state


def test_iter_json_objects_concatenated() -> None:
    text = '{"ts":"a","event":"manual"}{"ts":"b","event":"boot"}\n{"ts":"c","event":"manual"}\n'
    objs = list(analyze.iter_json_objects(text))
    assert [o["event"] for o in objs] == ["manual", "boot", "manual"]


def main() -> int:
    test_find_transitions_resume_and_dedupe()
    test_find_transitions_new_boot()
    test_hub_section_uses_alert_when_latest_healthy()
    test_jedec_unknown_manufacturer()
    test_redact_keeps_board_serial()
    test_slot_map()
    test_e820_section_points_at_high_range()
    test_unexpected_power_loss()
    test_iter_json_objects_concatenated()
    print("analyze helpers ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
