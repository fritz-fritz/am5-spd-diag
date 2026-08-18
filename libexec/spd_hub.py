#!/usr/bin/python3
"""SPD5118 hub probe/recover via SMBus ioctl (i2c-tools optional)."""
from __future__ import annotations

import argparse
import ctypes
import fcntl
import glob
import json
import os
import pwd
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

I2C_SLAVE = 0x0703
I2C_SMBUS = 0x0720
I2C_SMBUS_READ = 1
I2C_SMBUS_WRITE = 0
I2C_SMBUS_BYTE_DATA = 2
I2C_SMBUS_WORD_DATA = 3

MR11 = 0x0B
HUB_ADDRS = (0x50, 0x51, 0x52, 0x53)
STUCK_MR11 = 0x08
FORUM_URL = (
    "https://forum-en.msi.com/index.php?threads/"
    "ddr5-module-detected-as-2gb-ghost-dimm-after-s3-sleep-on-am5-root-cause-found.419787/"
)
PLACEHOLDERS = {"", "unknown", "not specified", "none", "not provided", "n/a", "to be filled by o.e.m."}
GHOST_SERIALS = {"00206200", "00-20-62-00", "00 20 62 00"}
# JEDEC JEP106 last byte; used when SMBIOS Manufacturer is Unknown.
MODULE_MFG_ID = {
    "9e": "Corsair",
    "ad": "SK Hynix",
    "ce": "Samsung",
    "2c": "Micron",
    "c1": "Infineon",
    "98": "Kingston",
}


class I2CSmbusData(ctypes.Union):
    _fields_ = [
        ("byte", ctypes.c_uint8),
        ("word", ctypes.c_uint16),
        ("block", ctypes.c_uint8 * 34),
    ]


class I2CSmbusIoctl(ctypes.Structure):
    _fields_ = [
        ("read_write", ctypes.c_uint8),
        ("command", ctypes.c_uint8),
        ("size", ctypes.c_uint32),
        ("data", ctypes.POINTER(I2CSmbusData)),
    ]


def is_placeholder(value: str) -> bool:
    return value.strip().lower() in PLACEHOLDERS


def parse_dimm_summary(text: str) -> list[dict[str, str]]:
    dimms: list[dict[str, str]] = []
    for line in text.splitlines():
        line = line.strip()
        if not line or "=" not in line:
            continue
        row: dict[str, str] = {}
        for part in line.split("|"):
            if "=" not in part:
                continue
            k, v = part.split("=", 1)
            row[k.strip()] = v.strip()
        if row:
            apply_jedec_manufacturer(row)
            dimms.append(row)
    return dimms


def _decode_mfg_id(value: str) -> str:
    m = re.search(r"hex\s+0x([0-9a-f]{2})", value.lower())
    if not m:
        return ""
    return MODULE_MFG_ID.get(m.group(1), "")


def is_populated_dimm(dimm: dict[str, str]) -> bool:
    size = dimm.get("size", "").strip()
    loc = dimm.get("locator", "").strip()
    if not size or "no module installed" in size.lower():
        return False
    if not re.search(r"\d", size):
        return False
    if not loc or loc.lower().startswith("locator:"):
        return False
    if "channel" in loc.lower() and "dimm" not in loc.lower():
        return False
    return True


def _empty_device() -> dict[str, str]:
    return {
        "locator": "",
        "size": "",
        "total_width": "",
        "data_width": "",
        "manufacturer": "",
        "serial": "",
        "part": "",
        "speed": "",
        "mem_type": "",
        "form_factor": "",
        "rank": "",
        "voltage": "",
    }


def apply_jedec_manufacturer(dimm: dict[str, str]) -> None:
    if not is_placeholder(dimm.get("manufacturer", "")):
        return
    decoded = _decode_mfg_id(dimm.get("mfg_id") or "")
    if decoded:
        dimm["manufacturer"] = decoded


def parse_dmidecode_memory(text: str) -> list[dict[str, str]]:
    """Parse `dmidecode -t memory`. Size/width come before Locator; skip empty slots."""
    devices: list[dict[str, str]] = []
    cur: dict[str, str] | None = None
    in_device = False

    def finish() -> None:
        nonlocal cur, in_device
        if cur and is_populated_dimm(cur):
            apply_jedec_manufacturer(cur)
            devices.append({k: v for k, v in cur.items() if k != "mfg_id"})
        cur = None
        in_device = False

    def start_device() -> None:
        nonlocal cur, in_device
        finish()
        in_device = True
        cur = _empty_device()

    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("Handle "):
            if "DMI type 17," in line:
                start_device()
            else:
                finish()
            continue
        if "DMI type 16," in line:
            finish()
            continue
        if "DMI type 17," in line:
            if not in_device:
                start_device()
            continue
        if not in_device or cur is None or ":" not in line:
            continue
        key, val = line.split(":", 1)
        key, val = key.strip(), val.strip()
        if key == "Locator":
            cur["locator"] = val
        elif key == "Size":
            cur["size"] = val
        elif key == "Total Width":
            cur["total_width"] = val
        elif key == "Data Width":
            cur["data_width"] = val
        elif key == "Manufacturer":
            cur["manufacturer"] = val
        elif key == "Serial Number":
            cur["serial"] = val
        elif key == "Part Number":
            cur["part"] = val
        elif key == "Configured Memory Speed":
            cur["speed"] = val
        elif key == "Type" and not val.lower().startswith("detail"):
            cur["mem_type"] = val
        elif key == "Form Factor":
            cur["form_factor"] = val
        elif key == "Rank":
            cur["rank"] = val
        elif key == "Configured Voltage":
            cur["voltage"] = val
        elif key == "Module Manufacturer ID":
            cur["mfg_id"] = val
    finish()
    return devices


def format_dimm_summary(dimms: list[dict[str, str]]) -> str:
    lines: list[str] = []
    for d in dimms:
        extra = ""
        for key in ("mem_type", "form_factor", "rank", "voltage", "mfg_id"):
            val = d.get(key) or ""
            if val:
                extra += f"|{key}={val}"
        lines.append(
            "locator={loc}|size={size}|total_width={tw}|data_width={dw}|"
            "manufacturer={man}|serial={ser}|part={part}|speed={speed}{extra}".format(
                loc=d.get("locator", ""),
                size=d.get("size", ""),
                tw=d.get("total_width", ""),
                dw=d.get("data_width", ""),
                man=d.get("manufacturer", ""),
                ser=d.get("serial", ""),
                part=d.get("part", ""),
                speed=d.get("speed", ""),
                extra=extra,
            )
        )
    return ("\n".join(lines) + "\n") if lines else ""


DMI_TABLE = Path("/sys/firmware/dmi/tables/DMI")
DMI_ENTRIES = Path("/sys/firmware/dmi/entries")


def _u16(data: bytes, off: int) -> int:
    return int.from_bytes(data[off : off + 2], "little")


def _u32(data: bytes, off: int) -> int:
    return int.from_bytes(data[off : off + 4], "little")


def _smbios_string(strings: list[str], idx: int) -> str:
    if idx <= 0 or idx > len(strings):
        return ""
    return strings[idx - 1].strip()


def _parse_string_table(blob: bytes, start: int) -> tuple[list[str], int]:
    n = len(blob)
    if start >= n:
        return [], start
    if blob[start] == 0:
        nxt = start + 1
        if nxt < n and blob[nxt] == 0:
            nxt += 1
        return [], nxt
    strings: list[str] = []
    i = start
    while i < n:
        if blob[i] == 0:
            i += 1
            break
        end = blob.find(b"\x00", i)
        if end < 0:
            strings.append(blob[i:].decode("latin-1", "replace"))
            return strings, n
        strings.append(blob[i:end].decode("latin-1", "replace"))
        i = end + 1
        if i < n and blob[i] == 0:
            i += 1
            break
    return strings, i


def iter_smbios_structures(blob: bytes):
    off = 0
    n = len(blob)
    while off + 4 <= n:
        typ = blob[off]
        length = blob[off + 1]
        handle = _u16(blob, off + 2)
        if length < 4 or off + length > n:
            break
        formatted = blob[off : off + length]
        strings, nxt = _parse_string_table(blob, off + length)
        yield typ, handle, formatted, strings
        if typ == 127:
            break
        if nxt <= off:
            break
        off = nxt


def _format_width(code: int) -> str:
    if code in (0, 0xFFFF):
        return "Unknown"
    return f"{code} bits"


def _format_size(formatted: bytes) -> str:
    if len(formatted) < 0x0E:
        return "Unknown"
    code = _u16(formatted, 0x0C)
    if code == 0:
        return "No Module Installed"
    if code == 0xFFFF:
        return "Unknown"
    if len(formatted) >= 0x20 and code == 0x7FFF:
        mb = _u32(formatted, 0x1C) & 0x7FFFFFFF
        nbytes = mb * 1024 * 1024
    elif code & 0x8000:
        nbytes = (code & 0x7FFF) * 1024
    else:
        nbytes = (code & 0x7FFF) * 1024 * 1024
    for unit, step in (("GiB", 1024**3), ("MiB", 1024**2), ("KiB", 1024)):
        if nbytes >= step and nbytes % step == 0:
            return f"{nbytes // step} {unit}"
    return f"{nbytes} bytes"


def _format_speed(formatted: bytes) -> str:
    if len(formatted) < 0x22:
        return ""
    code1 = _u16(formatted, 0x20)
    ext = _u32(formatted, 0x58) if len(formatted) >= 0x5C else 0
    if code1 == 0xFFFF:
        return "Unknown" if ext == 0 else f"{ext} MT/s"
    if code1 == 0:
        return "Unknown"
    return f"{code1} MT/s"


def _format_mfg_id(code: int) -> str:
    if code == 0:
        return "Unknown"
    return f"Bank {(code & 0x7F) + 1}, Hex 0x{code >> 8:02X}"


FORM_FACTORS = {0x08: "RIMM", 0x09: "DIMM", 0x0D: "SODIMM", 0x0F: "FB-DIMM"}
MEMORY_TYPES = {0x18: "DDR3", 0x1A: "DDR3", 0x1E: "DDR4", 0x1F: "LPDDR4", 0x22: "DDR5", 0x23: "LPDDR5"}


def _format_voltage_mv(code: int) -> str:
    if code in (0, 0xFFFF):
        return ""
    if code % 1000 == 0:
        return f"{code // 1000} V"
    return f"{code / 1000:.1f} V".replace(".0 V", " V")


def type17_display_fields(formatted: bytes, strings: list[str]) -> dict[str, str]:
    """dmidecode-style keys for a type 17 Memory Device."""
    empty = len(formatted) >= 0x0E and _u16(formatted, 0x0C) == 0
    fields = {
        "Total Width": _format_width(_u16(formatted, 0x08)) if len(formatted) >= 0x0A else "Unknown",
        "Data Width": _format_width(_u16(formatted, 0x0A)) if len(formatted) >= 0x0C else "Unknown",
        "Size": _format_size(formatted),
        "Locator": _smbios_string(strings, formatted[0x10] if len(formatted) > 0x10 else 0),
        "Bank Locator": _smbios_string(strings, formatted[0x11] if len(formatted) > 0x11 else 0),
    }
    if empty:
        return fields
    if len(formatted) > 0x0E:
        ff = FORM_FACTORS.get(formatted[0x0E])
        if ff:
            fields["Form Factor"] = ff
    if len(formatted) > 0x12:
        mt = MEMORY_TYPES.get(formatted[0x12])
        if mt:
            fields["Type"] = mt
    if len(formatted) > 0x17:
        fields["Manufacturer"] = _smbios_string(strings, formatted[0x17])
    if len(formatted) > 0x18:
        fields["Serial Number"] = _smbios_string(strings, formatted[0x18])
    if len(formatted) > 0x1A:
        fields["Part Number"] = _smbios_string(strings, formatted[0x1A])
    if len(formatted) > 0x1B:
        rank = formatted[0x1B] & 0x0F
        if rank:
            fields["Rank"] = str(rank)
    speed = _format_speed(formatted)
    if speed:
        fields["Configured Memory Speed"] = speed
    if len(formatted) >= 0x2E:
        fields["Module Manufacturer ID"] = _format_mfg_id(_u16(formatted, 0x2C))
    if len(formatted) >= 0x34:
        volt = _format_voltage_mv(_u16(formatted, 0x32))
        if volt:
            fields["Configured Voltage"] = volt
    return fields


def format_smbios_memory_dump(blob: bytes, source: str = "sysfs") -> str:
    lines = [
        "# am5-spd-diag SMBIOS memory dump",
        f"# source: {source}",
    ]
    found = False
    for typ, handle, formatted, strings in iter_smbios_structures(blob):
        if typ != 17:
            continue
        found = True
        fields = type17_display_fields(formatted, strings)
        lines.append("")
        lines.append(f"Handle 0x{handle:04X}, DMI type 17, {len(formatted)} bytes")
        lines.append("Memory Device")
        for key, val in fields.items():
            lines.append(f"\t{key}: {val}")
    if not found:
        lines.append("")
        lines.append("# no SMBIOS type 17 Memory Device structures")
    lines.append("")
    return "\n".join(lines)


def parse_smbios_memory(blob: bytes) -> list[dict[str, str]]:
    return parse_dmidecode_memory(format_smbios_memory_dump(blob, source="blob"))


def looks_like_text_dump(data: bytes) -> bool:
    stripped = data.lstrip()
    if not stripped:
        return True
    if stripped.startswith((b"#", b"Handle", b"Getting ", b"SMBIOS", b"Memory Device")):
        return True
    if stripped[0] < 32 and stripped[0] not in (9, 10, 13):
        return False
    return b"\x00" not in stripped[:64]


def parse_memory_devices(data: bytes) -> list[dict[str, str]]:
    if looks_like_text_dump(data):
        return parse_dmidecode_memory(data.decode("utf-8", "replace"))
    return parse_smbios_memory(data)


def read_sysfs_dmi_table() -> bytes | None:
    try:
        return DMI_TABLE.read_bytes()
    except OSError:
        return None


def read_sysfs_type17_entries() -> bytes | None:
    if not DMI_ENTRIES.is_dir():
        return None
    blobs: list[bytes] = []
    try:
        dirs = sorted(DMI_ENTRIES.glob("17-*"))
    except OSError:
        return None
    for d in dirs:
        try:
            blobs.append((d / "raw").read_bytes())
        except OSError:
            continue
    return b"".join(blobs) if blobs else None


def dump_from_dmidecode() -> str | None:
    try:
        out = subprocess.check_output(["dmidecode", "-t", "memory"], stderr=subprocess.DEVNULL)
    except (FileNotFoundError, subprocess.CalledProcessError, OSError):
        return None
    text = out.decode("utf-8", "replace")
    if "DMI type 17" in text or "Memory Device" in text:
        return text
    return None


def collect_memory_dump(*, table: Path | None = None, allow_dmidecode: bool = True) -> tuple[str, str]:
    if table is not None:
        data = table.read_bytes()
        if looks_like_text_dump(data):
            return data.decode("utf-8", "replace"), "file"
        return format_smbios_memory_dump(data, source=str(table)), "file"
    blob = read_sysfs_dmi_table()
    source = "sysfs /sys/firmware/dmi/tables/DMI"
    if not blob:
        blob = read_sysfs_type17_entries()
        source = "sysfs /sys/firmware/dmi/entries/17-*"
    if blob:
        text = format_smbios_memory_dump(blob, source=source)
        if "DMI type 17" in text:
            return text, "sysfs"
    if allow_dmidecode:
        text = dump_from_dmidecode()
        if text:
            return text, "dmidecode"
    return (
        "# SMBIOS memory dump unavailable (no sysfs DMI table and no dmidecode)\n",
        "none",
    )


def redact_dmi_secrets(text: str) -> str:
    """Omit system UUID and asset tags. Keep motherboard and DIMM serials."""
    lines: list[str] = []
    for line in text.splitlines():
        stripped = line.lstrip()
        key = stripped.split(":", 1)[0].strip().lower()
        if key in {"uuid", "asset tag"}:
            indent = line[: len(line) - len(stripped)]
            lines.append(f"{indent}{stripped.split(':', 1)[0]}: [redacted]")
        else:
            lines.append(line)
    return ("\n".join(lines) + "\n") if lines else ""


def _mhz(code: int) -> str:
    if code in (0, 0xFFFF):
        return "Unknown"
    return f"{code} MHz"


def type0_fields(formatted: bytes, strings: list[str]) -> dict[str, str]:
    fields: dict[str, str] = {}
    if len(formatted) > 0x05:
        fields["Vendor"] = _smbios_string(strings, formatted[0x04])
        fields["Version"] = _smbios_string(strings, formatted[0x05])
    if len(formatted) > 0x08:
        fields["Release Date"] = _smbios_string(strings, formatted[0x08])
    if len(formatted) >= 0x18:
        if formatted[0x14] != 0xFF and formatted[0x15] != 0xFF:
            fields["BIOS Revision"] = f"{formatted[0x14]}.{formatted[0x15]}"
        if formatted[0x16] != 0xFF and formatted[0x17] != 0xFF:
            fields["Firmware Revision"] = f"{formatted[0x16]}.{formatted[0x17]}"
    return fields


def type1_fields(formatted: bytes, strings: list[str]) -> dict[str, str]:
    fields: dict[str, str] = {}
    if len(formatted) > 0x06:
        fields["Manufacturer"] = _smbios_string(strings, formatted[0x04])
        fields["Product Name"] = _smbios_string(strings, formatted[0x05])
        fields["Version"] = _smbios_string(strings, formatted[0x06])
    if len(formatted) > 0x1A:
        fields["SKU Number"] = _smbios_string(strings, formatted[0x19])
        fields["Family"] = _smbios_string(strings, formatted[0x1A])
    return fields


def type2_fields(formatted: bytes, strings: list[str]) -> dict[str, str]:
    fields: dict[str, str] = {}
    if len(formatted) > 0x06:
        fields["Manufacturer"] = _smbios_string(strings, formatted[0x04])
        fields["Product Name"] = _smbios_string(strings, formatted[0x05])
        fields["Version"] = _smbios_string(strings, formatted[0x06])
    if len(formatted) > 0x07:
        serial = _smbios_string(strings, formatted[0x07])
        if serial:
            fields["Serial Number"] = serial
    if len(formatted) > 0x0D:
        board_types = {
            1: "Other",
            2: "Unknown",
            3: "Server Blade",
            10: "Motherboard",
        }
        fields["Type"] = board_types.get(formatted[0x0D], f"0x{formatted[0x0D]:02X}")
    return fields


def type4_fields(formatted: bytes, strings: list[str]) -> dict[str, str] | None:
    if len(formatted) < 0x1A:
        return None
    if not (formatted[0x18] & (1 << 6)):
        return None
    fields = {
        "Socket Designation": _smbios_string(strings, formatted[0x04]),
        "Manufacturer": _smbios_string(strings, formatted[0x07]),
        "Version": _smbios_string(strings, formatted[0x10]),
        "Max Speed": _mhz(_u16(formatted, 0x14)),
        "Current Speed": _mhz(_u16(formatted, 0x16)),
    }
    return fields


def format_smbios_system_dump(blob: bytes, source: str = "sysfs") -> str:
    titles = {0: "BIOS Information", 1: "System Information", 2: "Base Board Information", 4: "Processor Information"}
    parsers = {0: type0_fields, 1: type1_fields, 2: type2_fields, 4: type4_fields}
    lines = [
        "# am5-spd-diag SMBIOS system dump",
        f"# source: {source}",
        "# system UUID and asset tags are omitted; board and DIMM serials are kept",
    ]
    found = False
    for typ, handle, formatted, strings in iter_smbios_structures(blob):
        parser = parsers.get(typ)
        if not parser:
            continue
        fields = parser(formatted, strings)
        if not fields:
            continue
        found = True
        lines.append("")
        lines.append(f"Handle 0x{handle:04X}, DMI type {typ}, {len(formatted)} bytes")
        lines.append(titles[typ])
        for key, val in fields.items():
            if val:
                lines.append(f"\t{key}: {val}")
    if not found:
        lines.append("")
        lines.append("# no SMBIOS BIOS/system/board/processor structures")
    lines.append("")
    return "\n".join(lines)


def dump_system_from_dmidecode() -> str | None:
    try:
        out = subprocess.check_output(
            ["dmidecode", "-t", "bios", "-t", "system", "-t", "baseboard", "-t", "processor"],
            stderr=subprocess.DEVNULL,
        )
    except (FileNotFoundError, subprocess.CalledProcessError, OSError):
        return None
    text = out.decode("utf-8", "replace")
    if "BIOS Information" in text or "Base Board" in text:
        return redact_dmi_secrets(text)
    return None


def collect_system_dump(*, table: Path | None = None, allow_dmidecode: bool = True) -> tuple[str, str]:
    if table is not None:
        data = table.read_bytes()
        if looks_like_text_dump(data):
            return redact_dmi_secrets(data.decode("utf-8", "replace")), "file"
        return format_smbios_system_dump(data, source=str(table)), "file"
    blob = read_sysfs_dmi_table()
    source = "sysfs /sys/firmware/dmi/tables/DMI"
    if blob:
        text = format_smbios_system_dump(blob, source=source)
        if "DMI type" in text:
            return text, "sysfs"
    if allow_dmidecode:
        text = dump_system_from_dmidecode()
        if text:
            return text, "dmidecode"
    return (
        "# SMBIOS system dump unavailable (no sysfs DMI table and no dmidecode)\n",
        "none",
    )


def dimm_flags(dimm: dict[str, str]) -> list[str]:
    if not is_populated_dimm(dimm):
        return []
    flags: list[str] = []
    part = dimm.get("part", "")
    serial = dimm.get("serial", "").replace(":", "").replace(" ", "")
    tw = dimm.get("total_width", "").lower()
    dw = dimm.get("data_width", "").lower()
    if is_placeholder(part):
        flags.append("unknown_part")
    if "8 bit" in tw or "8 bit" in dw:
        flags.append("dimm_8bit_width")
    serial_cmp = serial.lower().replace("-", "")
    if serial_cmp in {s.replace("-", "").replace(" ", "").lower() for s in GHOST_SERIALS} and is_placeholder(part):
        flags.append("ghost_page0")
    return flags


def summary_flags(text: str) -> list[str]:
    seen: list[str] = []
    for dimm in parse_dimm_summary(text):
        for flag in dimm_flags(dimm):
            if flag not in seen:
                seen.append(flag)
    return seen


def _smbus_xfer(fd: int, rw: int, command: int, size: int, data: I2CSmbusData) -> None:
    args = I2CSmbusIoctl(rw, command, size, ctypes.pointer(data))
    fcntl.ioctl(fd, I2C_SMBUS, args)


def smbus_read_byte(dev: str, addr: int, command: int) -> int | None:
    data = I2CSmbusData()
    try:
        with open(dev, "rb+", buffering=0) as fh:
            fcntl.ioctl(fh.fileno(), I2C_SLAVE, addr)
            _smbus_xfer(fh.fileno(), I2C_SMBUS_READ, command, I2C_SMBUS_BYTE_DATA, data)
        return int(data.byte)
    except OSError:
        return None


def read_spd_page0(dev: str, addr: int, bus: int | None = None, length: int = 128) -> bytes | None:
    """Read 128 bytes with 1-byte addressing (hub window / page 0). Not full EEPROM."""
    out = bytearray()
    for cmd in range(length):
        val = smbus_read_byte(dev, addr, cmd)
        if val is None and bus is not None:
            val = i2c_tools_get(bus, addr, cmd)
        if val is None:
            break
        out.append(val)
    return bytes(out) if len(out) >= 16 else None


def format_spd_page0_text(sysfs: str, data: bytes) -> str:
    lines = [
        "# SPD hub window (page 0 / 1-byte addressing), not full EEPROM",
        f"# device {sysfs} first {len(data)} bytes",
    ]
    for i in range(0, len(data), 16):
        chunk = data[i : i + 16]
        hexpart = " ".join(f"{b:02x}" for b in chunk)
        lines.append(f"{i:04x}: {hexpart}")
    return "\n".join(lines) + "\n"


def write_spd_page0_files(directory: Path, probe: dict[str, Any]) -> list[str]:
    written: list[str] = []
    directory.mkdir(parents=True, exist_ok=True)
    for row in probe.get("stuck") or []:
        hx = row.get("spd_page0") or ""
        sysfs = str(row.get("sysfs") or "unknown").replace("/", "_")
        if not hx:
            continue
        try:
            data = bytes.fromhex(hx)
        except ValueError:
            continue
        path = directory / f"spd-page0-{sysfs}.txt"
        path.write_text(format_spd_page0_text(sysfs, data), encoding="utf-8")
        written.append(str(path))
    return written


def smbus_write_word(dev: str, addr: int, command: int, word: int) -> bool:
    data = I2CSmbusData()
    data.word = word & 0xFFFF
    try:
        with open(dev, "rb+", buffering=0) as fh:
            fcntl.ioctl(fh.fileno(), I2C_SLAVE, addr)
            _smbus_xfer(fh.fileno(), I2C_SMBUS_WRITE, command, I2C_SMBUS_WORD_DATA, data)
        return True
    except OSError:
        return False


def i2c_tools_get(bus: int, addr: int, command: int) -> int | None:
    try:
        out = subprocess.check_output(
            ["i2cget", "-y", str(bus), f"0x{addr:02x}", f"0x{command:02x}", "b"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return int(out.strip(), 16)
    except (FileNotFoundError, subprocess.CalledProcessError, ValueError):
        return None


def i2c_tools_set_word(bus: int, addr: int, command: int, word: int) -> bool:
    try:
        subprocess.check_call(
            ["i2cset", "-y", str(bus), f"0x{addr:02x}", f"0x{command:02x}", f"0x{word:04x}", "w"],
            stderr=subprocess.DEVNULL,
        )
        return True
    except (FileNotFoundError, subprocess.CalledProcessError):
        return False


def i2c_devices() -> list[tuple[int, str]]:
    found: list[tuple[int, str]] = []
    for path in sorted(glob.glob("/dev/i2c-*")):
        try:
            bus = int(path.rsplit("-", 1)[1])
        except ValueError:
            continue
        found.append((bus, path))
    return found


def adapter_name(bus: int) -> str:
    path = Path(f"/sys/class/i2c-adapter/i2c-{bus}/name")
    if path.is_file():
        return path.read_text(errors="replace").strip()
    return ""


SMBUS_ADAPTER_RE = re.compile(r"smbus|piix4|i801|fch|amd.*smb|\bsb-?t?si\b", re.I)
NON_SMBUS_ADAPTER_RE = re.compile(
    r"nvidia|geforce|nouveau|designware|synopsys|cros-ec|aux\b|ddc|gpu",
    re.I,
)


def is_smbus_adapter(name: str, *, bus: int | None = None) -> bool:
    """True for host SMBus controllers used for SPD (not GPU/aux/touchpad buses)."""
    n = (name or "").strip()
    if not n and bus is not None:
        n = adapter_name(bus)
    if not n:
        return False
    if SMBUS_ADAPTER_RE.search(n):
        return True
    if NON_SMBUS_ADAPTER_RE.search(n):
        return False
    return False


def spd5118_dmesg() -> list[str]:
    try:
        out = subprocess.check_output(["dmesg"], stderr=subprocess.DEVNULL, text=True, errors="replace")
    except (FileNotFoundError, subprocess.CalledProcessError, OSError):
        return []
    return [ln for ln in out.splitlines() if "spd5118" in ln.lower()]


def parse_stuck_from_dmesg(lines: list[str]) -> list[str]:
    stuck: list[str] = []
    for ln in lines:
        if "16-bit register" not in ln.lower() and "does not support" not in ln.lower():
            continue
        m = re.search(r"(\d+-00[0-9a-fA-F]{2})", ln)
        if m and m.group(1) not in stuck:
            stuck.append(m.group(1))
    return stuck


def probe_hubs() -> dict[str, Any]:
    dmesg_lines = spd5118_dmesg()
    result: dict[str, Any] = {
        "dmesg": dmesg_lines[-20:],
        "dmesg_stuck": parse_stuck_from_dmesg(dmesg_lines),
        "adapters": [],
        "hubs": [],
        "stuck": [],
        "method": "none",
    }
    devices = i2c_devices()
    if not devices:
        return result
    for bus, dev in devices:
        name = adapter_name(bus)
        result["adapters"].append({"bus": bus, "dev": dev, "name": name, "smbus": is_smbus_adapter(name)})
        if not is_smbus_adapter(name):
            continue
        for addr in HUB_ADDRS:
            val = smbus_read_byte(dev, addr, MR11)
            method = "ioctl"
            if val is None:
                val = i2c_tools_get(bus, addr, MR11)
                method = "i2cget" if val is not None else method
            if val is None:
                continue
            result["method"] = method
            row = {
                "bus": bus,
                "dev": dev,
                "adapter": name,
                "addr": addr,
                "addr_hex": f"0x{addr:02x}",
                "sysfs": f"{bus}-00{addr:02x}",
                "mr11": val,
                "mr11_hex": f"0x{val:02x}",
                "stuck": val == STUCK_MR11,
            }
            if row["stuck"]:
                page = read_spd_page0(dev, addr, bus)
                if page:
                    row["spd_page0"] = page.hex()
                    row["spd_page0_head"] = page[:16].hex()
            result["hubs"].append(row)
            if row["stuck"]:
                result["stuck"].append(row)
    return result


def recover_stuck(probe: dict[str, Any] | None = None) -> dict[str, Any]:
    probe = probe or probe_hubs()
    actions: list[dict[str, Any]] = []
    eligible = []
    for hub in probe.get("stuck") or []:
        name = str(hub.get("adapter") or "")
        bus = hub.get("bus")
        if is_smbus_adapter(name, bus=int(bus) if bus is not None else None):
            eligible.append(hub)
    if not eligible:
        return {"ok": False, "reason": "no_stuck_hub", "probe": probe, "actions": actions}
    ok_all = True
    for hub in eligible:
        bus = int(hub["bus"])
        addr = int(hub["addr"])
        dev = str(hub["dev"])
        wrote = smbus_write_word(dev, addr, MR11, 0x0000)
        method = "ioctl"
        if not wrote:
            wrote = i2c_tools_set_word(bus, addr, MR11, 0x0000)
            method = "i2cset"
        verify = smbus_read_byte(dev, addr, MR11)
        if verify is None:
            verify = i2c_tools_get(bus, addr, MR11)
        cleared = verify == 0x00
        ok_all = ok_all and wrote and cleared
        actions.append(
            {
                "sysfs": hub["sysfs"],
                "wrote": wrote,
                "method": method,
                "mr11_after": verify,
                "cleared": cleared,
            }
        )
    return {"ok": ok_all, "reason": "ok" if ok_all else "verify_failed", "probe": probe, "actions": actions}


def notify_app_path() -> Path:
    here = Path(__file__).resolve().parent
    for name in ("notify-app", "notify_app.py"):
        candidate = here / name
        if candidate.is_file():
            return candidate
    return Path("/usr/libexec/am5-spd-diag/notify-app")


def uid_from_bus_path(bus_path: str) -> int | None:
    parts = Path(bus_path).resolve().parts
    try:
        idx = parts.index("user")
        return int(parts[idx + 1])
    except (ValueError, IndexError):
        return None


def notify_user_argv(bus_path: str, title: str, body: str) -> list[str]:
    """Run notify-app as the session user on that user's bus."""
    uid = uid_from_bus_path(bus_path)
    argv = [
        "env",
        f"XDG_RUNTIME_DIR=/run/user/{uid}",
        f"DBUS_SESSION_BUS_ADDRESS=unix:path={bus_path}",
        sys.executable,
        str(notify_app_path()),
        "--notify",
        title,
        body,
    ]
    if os.geteuid() == 0 and uid not in (None, 0):
        try:
            name = pwd.getpwuid(uid).pw_name
        except KeyError:
            return argv
        return ["runuser", "-u", name, "--", *argv]
    return argv


def notify_desktop(title: str, body: str) -> None:
    helper = notify_app_path()
    if not helper.is_file():
        return
    for bus_path in glob.glob("/run/user/*/bus"):
        uid = uid_from_bus_path(bus_path)
        if uid in (None, 0):
            continue
        try:
            subprocess.Popen(
                notify_user_argv(bus_path, title, body),
                start_new_session=True,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                close_fds=True,
            )
        except OSError:
            continue


def notify_all(message: str) -> None:
    try:
        subprocess.run(["logger", "-p", "user.alert", "-t", "am5-spd-diag", message], check=False)
    except OSError:
        pass
    try:
        subprocess.run(["wall", "-n", message], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError:
        pass
    notify_desktop("SPD corruption detected", message)


def write_baseline(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    txt = path.with_suffix(".txt")
    lines = [
        f"captured={payload.get('ts', '')}",
        f"memtotal_kb={payload.get('memtotal_kb', '')}",
        f"cpu={payload.get('cpu', '')}",
        f"board={payload.get('dmi', {}).get('board_name', '')}",
        f"bios={payload.get('dmi', {}).get('bios_version', '')}",
    ]
    for dimm in payload.get("dimms") or []:
        lines.append(
            "{loc} {size} {man} {part}".format(
                loc=dimm.get("locator", "?"),
                size=dimm.get("size", "?"),
                man=dimm.get("manufacturer", "?"),
                part=dimm.get("part", "?"),
            )
        )
    txt.write_text("\n".join(lines) + "\n", encoding="utf-8")


def cmd_flags(args: argparse.Namespace) -> int:
    text = Path(args.summary).read_text(errors="replace") if args.summary != "-" else sys.stdin.read()
    flags = summary_flags(text)
    print(",".join(flags))
    return 0


def cmd_summarize(args: argparse.Namespace) -> int:
    data = Path(args.dmidecode).read_bytes() if args.dmidecode != "-" else sys.stdin.buffer.read()
    sys.stdout.write(format_dimm_summary(parse_memory_devices(data)))
    return 0


def cmd_dump_memory(args: argparse.Namespace) -> int:
    table = Path(args.table) if args.table else None
    text, source = collect_memory_dump(table=table, allow_dmidecode=not args.sysfs_only)
    sys.stdout.write(text if text.endswith("\n") else text + "\n")
    if args.source:
        print(f"# resolved-source: {source}", file=sys.stderr)
    return 0 if source != "none" else 1


def cmd_dump_system(args: argparse.Namespace) -> int:
    table = Path(args.table) if args.table else None
    text, source = collect_system_dump(table=table, allow_dmidecode=not args.sysfs_only)
    sys.stdout.write(text if text.endswith("\n") else text + "\n")
    if args.source:
        print(f"# resolved-source: {source}", file=sys.stderr)
    return 0 if source != "none" else 1


def cmd_probe(args: argparse.Namespace) -> int:
    probe = probe_hubs()
    if args.json:
        print(json.dumps(probe, indent=2))
    else:
        if probe["dmesg_stuck"]:
            print("dmesg stuck:", ", ".join(probe["dmesg_stuck"]))
        if not probe["hubs"]:
            print("No SPD hubs readable (need root, i2c-dev, and /dev/i2c-*).")
        for hub in probe["hubs"]:
            state = "STUCK MR11=0x08" if hub["stuck"] else f"mr11={hub['mr11_hex']}"
            print(f"bus {hub['bus']} {hub['addr_hex']} ({hub['sysfs']}) {state}")
    return 0


def cmd_recover(args: argparse.Namespace) -> int:
    print("WARNING: experimental in-band recover for a stuck SPD5118 hub (MR11).")
    print("This does not rewrite EEPROM. BIOS may show 'Devices Changed' and retrain.")
    print(f"Source: {FORUM_URL}")
    print("This tool will NOT reboot the machine. A warm reboot is required after a successful clear.")
    print()
    if not args.yes:
        try:
            reply = input("Type YES to probe and clear stuck hubs: ").strip()
        except EOFError:
            reply = ""
        if reply != "YES":
            print("Aborted.")
            return 2
    result = recover_stuck()
    print(json.dumps({k: result[k] for k in ("ok", "reason", "actions")}, indent=2))
    if not result["ok"]:
        if result["reason"] == "no_stuck_hub":
            print("No hub with MR11=0x08 was found. SMBIOS Unknown/missing part can still be present.")
        return 1
    print("MR11 cleared. Warm reboot now so firmware re-reads the real SPD.")
    return 0


def cmd_write_spd_pages(args: argparse.Namespace) -> int:
    probe = json.loads(Path(args.hub_json).read_text(encoding="utf-8"))
    write_spd_page0_files(Path(args.out_dir), probe)
    return 0


def cmd_notify(args: argparse.Namespace) -> int:
    notify_all(args.message)
    return 0


def cmd_baseline(args: argparse.Namespace) -> int:
    payload = json.loads(Path(args.json_in).read_text(encoding="utf-8") if args.json_in != "-" else sys.stdin.read())
    existing = Path(args.out)
    if existing.is_file() and not args.force:
        print(existing)
        return 0
    write_baseline(existing, payload)
    print(existing)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="am5-spd-diag-hub",
        description="Internal SMBIOS/SPD5118 helpers. Prefer: am5-spd-diag probe|recover",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_flags = sub.add_parser("flags", help="corruption flags from a dimm-summary.txt")
    p_flags.add_argument("summary")
    p_flags.set_defaults(func=cmd_flags)

    p_sum = sub.add_parser("summarize", help="DIMM summary from dmidecode text or SMBIOS blob")
    p_sum.add_argument("dmidecode")
    p_sum.set_defaults(func=cmd_summarize)

    p_dump = sub.add_parser("dump-memory", help="SMBIOS type 17 dump (sysfs, else dmidecode)")
    p_dump.add_argument("--table", help="SMBIOS blob or dmidecode text file")
    p_dump.add_argument("--sysfs-only", action="store_true", help="do not fall back to dmidecode")
    p_dump.add_argument("--source", action="store_true", help="print resolved source on stderr")
    p_dump.set_defaults(func=cmd_dump_memory)

    p_sys = sub.add_parser("dump-system", help="SMBIOS BIOS/board/CPU dump (sysfs, else dmidecode)")
    p_sys.add_argument("--table", help="SMBIOS blob or dmidecode text file")
    p_sys.add_argument("--sysfs-only", action="store_true", help="do not fall back to dmidecode")
    p_sys.add_argument("--source", action="store_true", help="print resolved source on stderr")
    p_sys.set_defaults(func=cmd_dump_system)

    p_probe = sub.add_parser("probe", help="read SPD5118 MR11 on 0x50–0x53")
    p_probe.add_argument("--json", action="store_true", help="machine-readable output")
    p_probe.set_defaults(func=cmd_probe)

    p_rec = sub.add_parser("recover", help="clear stuck MR11=0x08; does not reboot")
    p_rec.add_argument("--yes", action="store_true", help="do not prompt")
    p_rec.set_defaults(func=cmd_recover)

    p_spd = sub.add_parser("write-spd-pages", help="write spd-page0-*.txt from a hub.json")
    p_spd.add_argument("hub_json")
    p_spd.add_argument("out_dir")
    p_spd.set_defaults(func=cmd_write_spd_pages)

    p_note = sub.add_parser("notify", help="journal/wall/desktop notice")
    p_note.add_argument("message")
    p_note.set_defaults(func=cmd_notify)

    p_base = sub.add_parser("baseline", help="write healthy baseline.json")
    p_base.add_argument("out")
    p_base.add_argument("json_in")
    p_base.add_argument("--force", action="store_true")
    p_base.set_defaults(func=cmd_baseline)

    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
