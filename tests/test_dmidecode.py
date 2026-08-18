#!/usr/bin/env python3
"""Parser checks for dmidecode Memory Device records."""
from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "libexec"))
import spd_hub  # noqa: E402

HERE = Path(__file__).resolve().parent


def test_healthy() -> None:
    text = (HERE / "dmidecode-healthy.txt").read_text(encoding="utf-8")
    dimms = spd_hub.parse_dmidecode_memory(text)
    locs = [d["locator"] for d in dimms]
    assert locs == ["DIMMA2", "DIMMB2"], locs
    for d in dimms:
        assert d["size"] == "16 GiB", d
        assert d["total_width"] == "64 bits", d
        assert d["data_width"] == "64 bits", d
        assert d["part"] == "CMH32GX5M2M6000Z36", d
        assert d["manufacturer"] == "Corsair", d
        assert d["speed"] == "6000 MT/s", d
        assert d["mem_type"] == "DDR5", d
        assert not spd_hub.dimm_flags(d), d
    summary = spd_hub.format_dimm_summary(dimms)
    assert "CHANNEL" not in summary
    assert "DIMMA1" not in summary
    assert spd_hub.summary_flags(summary) == []


def test_corrupt() -> None:
    text = (HERE / "dmidecode-corrupt.txt").read_text(encoding="utf-8")
    dimms = spd_hub.parse_dmidecode_memory(text)
    by_loc = {d["locator"]: d for d in dimms}
    assert set(by_loc) == {"DIMMA2", "DIMMB2"}
    assert not spd_hub.dimm_flags(by_loc["DIMMA2"])
    flags = spd_hub.dimm_flags(by_loc["DIMMB2"])
    assert "unknown_part" in flags, flags
    assert "dimm_8bit_width" in flags, flags
    assert "ghost_page0" in flags, flags
    summary_flags = spd_hub.summary_flags(spd_hub.format_dimm_summary(dimms))
    assert "unknown_part" in summary_flags
    assert "dimm_8bit_width" in summary_flags


def test_ignore_bank_locator_garbage() -> None:
    garbage = (
        "locator=Locator: P0 CHANNEL A|size=16 GiB|total_width=64 bits|"
        "data_width=64 bits|manufacturer=Unknown|serial=|part=|speed=\n"
        "locator=DIMMA2|size=|total_width=?|data_width=?|manufacturer=|"
        "serial=|part=|speed=\n"
        "locator=DIMMB2|size=Size: None|total_width=|data_width=|"
        "manufacturer=|serial=|part=|speed=\n"
    )
    assert spd_hub.summary_flags(garbage) == []


def main() -> int:
    test_healthy()
    test_corrupt()
    test_ignore_bank_locator_garbage()
    print("dmidecode parser ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
