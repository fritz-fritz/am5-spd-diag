#!/usr/bin/env python3
"""SMBIOS type 17 parser used when dmidecode is not installed."""
from __future__ import annotations

import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "libexec"))
import spd_hub  # noqa: E402


def pack_type17(
    handle: int,
    *,
    empty: bool = False,
    size_mb: int = 16384,
    total_width: int = 64,
    data_width: int = 64,
    locator: str = "DIMMA2",
    bank: str = "P0 CHANNEL A",
    manufacturer: str = "Unknown",
    serial: str = "B5066693",
    part: str = "CMH32GX5M2M6000Z36",
    speed: int = 6000,
    mfg_id: int = 0x9E02,
) -> bytes:
    length = 0x15 if empty else 0x34
    buf = bytearray(length)
    buf[0] = 17
    buf[1] = length
    buf[2:4] = handle.to_bytes(2, "little")
    buf[0x08:0x0A] = total_width.to_bytes(2, "little")
    buf[0x0A:0x0C] = data_width.to_bytes(2, "little")
    buf[0x0C:0x0E] = (0 if empty else size_mb).to_bytes(2, "little")
    if not empty:
        buf[0x0E] = 0x09  # DIMM
        buf[0x12] = 0x22  # DDR5
        buf[0x1B] = 1  # rank 1
        buf[0x32:0x34] = (1100).to_bytes(2, "little")  # 1.1 V
    strings = [locator, bank]
    buf[0x10] = 1
    buf[0x11] = 2
    if not empty:
        strings.extend([manufacturer, serial, part])
        buf[0x17] = 3
        buf[0x18] = 4
        buf[0x1A] = 5
        buf[0x20:0x22] = speed.to_bytes(2, "little")
        buf[0x2C:0x2E] = mfg_id.to_bytes(2, "little")
    blob = bytes(buf)
    for s in strings:
        blob += s.encode("ascii") + b"\x00"
    return blob + b"\x00"


def pack_type0(handle: int = 0) -> bytes:
    length = 0x18
    buf = bytearray(length)
    buf[0] = 0
    buf[1] = length
    buf[2:4] = handle.to_bytes(2, "little")
    strings = ["American Megatrends International, LLC.", "2.A52", "06/29/2026"]
    buf[0x04] = 1
    buf[0x05] = 2
    buf[0x08] = 3
    buf[0x14] = 5
    buf[0x15] = 41
    blob = bytes(buf)
    for s in strings:
        blob += s.encode("ascii") + b"\x00"
    return blob + b"\x00"


def pack_end() -> bytes:
    return bytes([127, 4, 0, 0]) + b"\x00\x00"


def test_system_dump() -> None:
    blob = pack_type0() + pack_end()
    text = spd_hub.format_smbios_system_dump(blob, source="test")
    assert "BIOS Information" in text
    assert "Version: 2.A52" in text
    assert "BIOS Revision: 5.41" in text
    assert "Serial Number" not in text


def test_sysfs_style_blob() -> None:
    blob = (
        pack_type17(0x000E, empty=True, locator="DIMMA1", total_width=0xFFFF, data_width=0xFFFF)
        + pack_type17(0x0010, locator="DIMMA2", serial="B5066693")
        + pack_type17(
            0x0015,
            locator="DIMMB2",
            bank="P0 CHANNEL B",
            serial="B506743D",
        )
        + pack_end()
    )
    dump = spd_hub.format_smbios_memory_dump(blob, source="test")
    assert "DMI type 17" in dump
    assert "Locator: DIMMA1" in dump
    assert "Size: No Module Installed" in dump
    assert "Locator: DIMMA2" in dump
    assert "Part Number: CMH32GX5M2M6000Z36" in dump
    assert "Module Manufacturer ID: Bank 3, Hex 0x9E" in dump
    dimms = spd_hub.parse_memory_devices(blob)
    assert [d["locator"] for d in dimms] == ["DIMMA2", "DIMMB2"]
    for d in dimms:
        assert d["size"] == "16 GiB", d
        assert d["part"] == "CMH32GX5M2M6000Z36", d
        assert d["manufacturer"] == "Corsair", d
        assert d["speed"] == "6000 MT/s", d
        assert d.get("mem_type") == "DDR5", d
        assert not spd_hub.dimm_flags(d), d


def test_corrupt_blob() -> None:
    blob = pack_type17(
        0x0015,
        locator="DIMMB2",
        size_mb=2048,
        total_width=8,
        data_width=8,
        manufacturer="Unknown",
        serial="00206200",
        part="Unknown",
        mfg_id=0,
    )
    dimms = spd_hub.parse_smbios_memory(blob)
    assert len(dimms) == 1
    flags = spd_hub.dimm_flags(dimms[0])
    assert "unknown_part" in flags, flags
    assert "dimm_8bit_width" in flags, flags
    assert "ghost_page0" in flags, flags


def test_extended_size() -> None:
    buf = bytearray(pack_type17(0x0010, size_mb=1))
    buf[0x0C:0x0E] = (0x7FFF).to_bytes(2, "little")
    buf[0x1C:0x20] = (32768).to_bytes(4, "little")  # 32 GiB in MB
    dimms = spd_hub.parse_smbios_memory(bytes(buf))
    assert dimms[0]["size"] == "32 GiB", dimms[0]


def test_dump_memory_cli_table() -> None:
    blob = pack_type17(0x0010) + pack_end()
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as fh:
        path = Path(fh.name)
        fh.write(blob)
    try:
        text, source = spd_hub.collect_memory_dump(table=path, allow_dmidecode=False)
        assert source == "file"
        assert "locator=DIMMA2" in spd_hub.format_dimm_summary(spd_hub.parse_dmidecode_memory(text))
    finally:
        path.unlink(missing_ok=True)


def main() -> int:
    test_sysfs_style_blob()
    test_corrupt_blob()
    test_extended_size()
    test_dump_memory_cli_table()
    test_system_dump()
    print("smbios parser ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
