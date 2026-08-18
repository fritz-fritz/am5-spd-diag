#!/usr/bin/env python3
"""SMBus adapter allow-list for SPD probe/recover."""
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "libexec"))
import spd_hub  # noqa: E402


def test_allowlist() -> None:
    assert spd_hub.is_smbus_adapter("SMBus PIIX4 adapter")
    assert spd_hub.is_smbus_adapter("SMBus I801 adapter at e000")
    assert spd_hub.is_smbus_adapter("AMD SMBus")
    assert spd_hub.is_smbus_adapter("FCH SMBus")
    assert not spd_hub.is_smbus_adapter("NVIDIA i2c adapter 0")
    assert not spd_hub.is_smbus_adapter("Synopsys DesignWare I2C adapter")
    assert not spd_hub.is_smbus_adapter("i2c-NVIDIA-GPU")
    assert not spd_hub.is_smbus_adapter("")
    assert not spd_hub.is_smbus_adapter("cros-ec")


def test_recover_skips_non_smbus() -> None:
    probe = {
        "stuck": [
            {
                "bus": 2,
                "addr": 0x50,
                "dev": "/dev/i2c-2",
                "adapter": "NVIDIA i2c adapter 0",
                "sysfs": "2-0050",
            }
        ]
    }
    result = spd_hub.recover_stuck(probe)
    assert result["ok"] is False
    assert result["reason"] == "no_stuck_hub"
    assert result["actions"] == []


def test_spd_page0_format() -> None:
    text = spd_hub.format_spd_page0_text("1-0053", bytes.fromhex("230c4d08" + "00" * 12))
    assert "not full EEPROM" in text
    assert "0000:" in text
    assert "23 0c 4d 08" in text


def main() -> int:
    test_allowlist()
    test_recover_skips_non_smbus()
    test_spd_page0_format()
    print("i2c filter ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
