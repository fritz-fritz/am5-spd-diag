#!/usr/bin/env python3
"""Build a synthetic capture tree for analyzer smoke tests."""
from __future__ import annotations

import json
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent / "fixture"
BOOT_A = "aaaaaaaa-1111-1111-1111-111111111111"
BOOT_B = "bbbbbbbb-2222-2222-2222-222222222222"

HEALTHY = (
    "locator=DIMMA2|size=16 GiB|total_width=64 bits|data_width=64 bits|"
    "manufacturer=Corsair|serial=B5066693|part=CMH32GX5M2M6000Z36|speed=6000 MT/s|"
    "mem_type=DDR5|form_factor=DIMM|rank=1|voltage=1.1 V|mfg_id=Bank 3, Hex 0x9E\n"
    "locator=DIMMB2|size=16 GiB|total_width=64 bits|data_width=64 bits|"
    "manufacturer=Corsair|serial=C0FFEE00|part=CMH32GX5M2M6000Z36|speed=6000 MT/s|"
    "mem_type=DDR5|form_factor=DIMM|rank=1|voltage=1.1 V|mfg_id=Bank 3, Hex 0x9E\n"
)
CORRUPT = (
    "locator=DIMMA2|size=16 GiB|total_width=64 bits|data_width=64 bits|"
    "manufacturer=Corsair|serial=B5066693|part=CMH32GX5M2M6000Z36|speed=6000 MT/s|"
    "mem_type=DDR5|form_factor=DIMM|rank=1|voltage=1.1 V|mfg_id=Bank 3, Hex 0x9E\n"
    "locator=DIMMB2|size=2 GiB|total_width=8 bits|data_width=8 bits|"
    "manufacturer=Unknown|serial=00206200|part=Unknown|speed=6000 MT/s|"
    "mem_type=DDR5|form_factor=DIMM\n"
)
DMI = """bios_vendor=American Megatrends International, LLC.
bios_version=2.A52
bios_date=06/29/2026
bios_release=5.41
board_vendor=Micro-Star International Co., Ltd.
board_name=PRO X870E-P WIFI (MS-7E70)
board_version=2.0
board_serial=07E701234567
sys_vendor=Micro-Star International Co., Ltd.
product_name=MS-7E70
product_version=Default string
product_family=To Be Filled By O.E.M.
chassis_vendor=Micro-Star International Co., Ltd.
chassis_type=3
"""
SYSTEM = {
    "dmi": {
        "bios_vendor": "American Megatrends International, LLC.",
        "bios_version": "2.A52",
        "bios_date": "06/29/2026",
        "bios_release": "5.41",
        "board_vendor": "Micro-Star International Co., Ltd.",
        "board_name": "PRO X870E-P WIFI (MS-7E70)",
        "board_version": "2.0",
        "board_serial": "07E701234567",
        "sys_vendor": "Micro-Star International Co., Ltd.",
        "product_name": "MS-7E70",
    },
    "cpu": {
        "model_name": "AMD Ryzen 7 9800X3D 8-Core Processor",
        "vendor_id": "AuthenticAMD",
        "family": "26",
        "model": "32",
        "stepping": "0",
        "microcode": "0xb404023",
    },
    "os": {
        "NAME": "openSUSE Tumbleweed",
        "ID": "opensuse-tumbleweed",
        "VERSION_ID": "20260815",
        "PRETTY_NAME": "openSUSE Tumbleweed",
    },
    "kernel": {
        "release": "7.1.5-1-default",
        "machine": "x86_64",
        "version": "#1 SMP PREEMPT_DYNAMIC",
    },
    "boot_mode": "UEFI",
    "mem_sleep": "s2idle [deep]",
}
ALERT_FLAGS = "unknown_part,dimm_8bit_width,ghost_page0"


def write_event(
    name: str,
    event: str,
    boot: str,
    ts: str,
    kb: int,
    alert: bool,
    dimms: str,
    sleep_type: str = "",
    boot_kind: str = "",
) -> dict:
    d = ROOT / "events" / name
    d.mkdir(parents=True, exist_ok=True)
    flags = ALERT_FLAGS if alert else ""
    (d / "dimm-summary.txt").write_text(dimms)
    (d / "dmi-sysfs.txt").write_text(DMI)
    (d / "system.json").write_text(json.dumps(SYSTEM) + "\n")
    (d / "meta.txt").write_text(
        f"ts={ts}\nevent={event}\nsleep_type={sleep_type}\nboot_id={boot}\n"
        f"memtotal_kb={kb}\nmem_sleep=s2idle [deep]\nboot_kind={boot_kind}\n"
        f"alert={str(alert).lower()}\nflags={flags}\n"
    )
    if alert:
        (d / "ALERT.flags").write_text(flags + "\n")
        (d / "hub.json").write_text(
            json.dumps(
                {
                    "dmesg_stuck": ["1-0053"],
                    "dmesg": [
                        "spd5118 1-0053: Adapter does not support 16-bit register addresses"
                    ],
                    "stuck": [
                        {
                            "bus": 1,
                            "addr": 0x53,
                            "addr_hex": "0x53",
                            "sysfs": "1-0053",
                            "mr11": 8,
                            "mr11_hex": "0x08",
                            "stuck": True,
                            "adapter": "SMBus PIIX4 adapter",
                            "spd_page0_head": "230c4d08000000000020620000000000",
                        }
                    ],
                    "hubs": [],
                }
            )
            + "\n"
        )
        (d / "dmesg-spd5118.txt").write_text(
            "spd5118 1-0053: Adapter does not support 16-bit register addresses\n"
        )
        (d / "e820.txt").write_text(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n"
            "BIOS-e820: [mem 0x0000000100000000-0x000000041fffffff] System RAM\n"
        )
        (d / "e820-system-ram.txt").write_text(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x0000000100000000-0x000000041fffffff] System RAM\n"
        )
        (d / "spd-page0-1-0053.txt").write_text(
            "# SPD hub window (page 0 / 1-byte addressing), not full EEPROM\n"
            "# device 1-0053 first 16 bytes\n"
            "0000: 23 0c 4d 08 00 00 00 00 00 20 62 00 00 00 00 00\n"
        )
    else:
        (d / "e820.txt").write_text(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n"
            "BIOS-e820: [mem 0x0000000100000000-0x000000047fffffff] System RAM\n"
        )
        (d / "e820-system-ram.txt").write_text(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n"
            "BIOS-e820: [mem 0x0000000100000000-0x000000047fffffff] System RAM\n"
        )
    return {
        "ts": ts,
        "event": event,
        "boot_id": boot,
        "memtotal_kb": kb,
        "mem_sleep": "s2idle [deep]",
        "sleep_type": sleep_type,
        "flags": flags,
        "alert": alert,
        "dir": str(d),
        "boot_kind": boot_kind,
        "hub_stuck": "yes" if alert else "no",
    }


def main() -> None:
    if ROOT.exists():
        shutil.rmtree(ROOT)
    (ROOT / "events").mkdir(parents=True, exist_ok=True)

    events = [
        write_event("20260817T040000.0Z-boot", "boot", BOOT_A, "2026-08-17T00:00:00-04:00", 32000000, False, HEALTHY),
        write_event("20260817T041000.0Z-suspend-pre", "suspend-pre", BOOT_A, "2026-08-17T00:10:00-04:00", 32000000, False, HEALTHY, "suspend"),
        write_event("20260817T041100.0Z-suspend-post", "suspend-post", BOOT_A, "2026-08-17T00:11:00-04:00", 32000000, False, HEALTHY, "suspend"),
        write_event("20260817T042000.0Z-suspend-pre", "suspend-pre", BOOT_A, "2026-08-17T00:20:00-04:00", 32000000, False, HEALTHY, "suspend"),
        write_event("20260817T042100.0Z-suspend-post", "suspend-post", BOOT_A, "2026-08-17T00:21:00-04:00", 32000000, False, HEALTHY, "suspend"),
        write_event("20260817T043000.0Z-reboot", "reboot", BOOT_A, "2026-08-17T00:30:00-04:00", 32000000, False, HEALTHY),
        write_event(
            "20260817T043030.0Z-boot",
            "boot",
            BOOT_B,
            "2026-08-17T00:30:30-04:00",
            17800092,
            True,
            CORRUPT,
            boot_kind="warm_reboot",
        ),
    ]
    (ROOT / "timeline.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
    (ROOT / "ALERTS.log").write_text(
        f"2026-08-17T00:30:30-04:00 ALERT event=boot flags={ALERT_FLAGS} memtotal_kb=17800092 boot_kind=warm_reboot\n"
    )
    (ROOT / "SPD_NOW").write_text("corrupt\n")
    (ROOT / "NOTICE").write_text("SPD corruption is current (flags=unknown_part,dimm_8bit_width,ghost_page0).\n")
    (ROOT / "baseline.json").write_text(
        json.dumps(
            {
                "ts": "2026-08-17T00:00:00-04:00",
                "memtotal_kb": 32000000,
                "cpu": "AMD Ryzen 7 9800X3D",
                "dmi": {
                    "board_name": "PRO X870E-P WIFI (MS-7E70)",
                    "bios_version": "2.A52",
                    "bios_date": "06/29/2026",
                    "bios_release": "5.41",
                    "board_vendor": "Micro-Star International Co., Ltd.",
                    "board_serial": "07E701234567",
                    "sys_vendor": "Micro-Star International Co., Ltd.",
                },
                "dimms": [
                    {
                        "locator": "DIMMA2",
                        "size": "16 GiB",
                        "total_width": "64 bits",
                        "data_width": "64 bits",
                        "manufacturer": "Corsair",
                        "part": "CMH32GX5M2M6000Z36",
                        "serial": "B5066693",
                        "speed": "6000 MT/s",
                        "mem_type": "DDR5",
                    },
                    {
                        "locator": "DIMMB2",
                        "size": "16 GiB",
                        "total_width": "64 bits",
                        "data_width": "64 bits",
                        "manufacturer": "Corsair",
                        "part": "CMH32GX5M2M6000Z36",
                        "serial": "C0FFEE00",
                        "speed": "6000 MT/s",
                        "mem_type": "DDR5",
                    },
                ],
            },
            indent=2,
        )
        + "\n"
    )
    print(ROOT)


if __name__ == "__main__":
    main()
