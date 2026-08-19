# PRO X870E-P WIFI (MS-7E70) BIOS 2.A52: DIMM identity lost after sleep + warm reboot

Generated: 2026-08-18T12:05:27.215870691-04:00

## Summary

SPD identity is **currently corrupted**: firmware is publishing placeholder DIMM fields (Unknown/missing part and/or 8-bit width). Linux MemTotal is whatever firmware advertised, not a separate Linux bug. Warm reboot does not clear SPD5118 MR11 standby state; AC power loss does. Optional in-band fix: `am5-spd-diag fix` then reboot.

## Expected / Actual / Impact

- **Slot map:** 2×16 GiB in DIMMA2+DIMMB2
- **Expected** firmware MemTotal 32000000 kB (30.52 GiB):
- **DIMMA2:** 16 GiB · 64 bits/64 bits · Corsair CMH32GX5M2M6000Z36 · 6000 MT/s · serial B5066693
- **DIMMB2:** 16 GiB · 64 bits/64 bits · Corsair CMH32GX5M2M6000Z36 · 6000 MT/s · serial C0FFEE00
- **Actual** firmware MemTotal 17800092 kB (16.98 GiB) (live now 32250752 kB / 30.76 GiB):
- **DIMMA2:** 16 GiB · 64 bits/64 bits · Corsair CMH32GX5M2M6000Z36 · 6000 MT/s · serial B5066693
- **DIMMB2:** 2 GiB · 8 bits/8 bits · Unknown Unknown · 6000 MT/s · serial 00206200
- **Impact:** firmware published placeholder DIMM identity and a smaller memory map. Linux is reflecting SMBIOS/e820 from UEFI, not dropping RAM in the MM layer.

## System

Live values when this report was generated. Motherboard and DIMM serials are included. System UUID and asset tags are omitted.

| Item | Details |
|---|---|
| Vendor | Micro-Star International Co., Ltd. |
| Motherboard | PRO X870E-P WIFI (MS-7E70) rev 2.0 |
| Board serial | 07E701234567 |
| Chassis | Desktop |
| BIOS vendor | American Megatrends International, LLC. |
| BIOS version | 2.A52 (06/29/2026) |
| BIOS revision | 5.41 |
| Firmware boot mode | UEFI |
| CPU | AMD Ryzen 7 9800X3D 8-Core Processor |
| CPU ID | family 26 model 68 stepping 0 |
| CPU microcode | 0xb404038 |
| Memory (healthy baseline) | DIMMA2 16 GiB Corsair CMH32GX5M2M6000Z36; DIMMB2 16 GiB Corsair CMH32GX5M2M6000Z36 (healthy MemTotal 32000000 kB / 30.52 GiB) |
| OS | openSUSE Tumbleweed (opensuse-tumbleweed 20260815) |
| Kernel | 7.1.8-1-default x86_64 |
| Kernel build | #1 SMP PREEMPT_DYNAMIC Mon Aug 10 05:03:20 UTC 2026 (f1071af) |
| Sleep policy | s2idle [deep] |

- Latest capture: `2026-08-17T00:30:30-04:00` event `boot`
- Latest capture differs from live: kernel 7.1.5-1-default (live 7.1.8-1-default)

## SPD identity

- **SPD now: corrupted**
- Firmware published RAM: **32250752 kB** (30.76 GiB); last healthy baseline **32000000 kB** (30.52 GiB)
- Suspends: kernel **1** this boot; package recorded **2**
- Corruption seen in earlier captures: **yes**
- This boot followed a captured warm reboot.
- Timeline events: **7**
- Notice: SPD corruption is current (flags=unknown_part,dimm_8bit_width,ghost_page0).

### Current

| Locator | Size | Width | Speed | Type | Manufacturer | Part | Serial |
|---|---|---|---|---|---|---|---|
| DIMMA2 | 16 GiB | 64 bits / 64 bits | 6000 MT/s | DDR5 | Corsair | CMH32GX5M2M6000Z36 | B5066693 |
| DIMMB2 | 2 GiB | 8 bits / 8 bits | 6000 MT/s | DDR5 | Unknown | Unknown | 00206200 |

### Last healthy baseline (differs from current)

| Locator | Size | Width | Speed | Type | Manufacturer | Part | Serial |
|---|---|---|---|---|---|---|---|
| DIMMA2 | 16 GiB | 64 bits / 64 bits | 6000 MT/s | DDR5 | Corsair | CMH32GX5M2M6000Z36 | B5066693 |
| DIMMB2 | 16 GiB | 64 bits / 64 bits | 6000 MT/s | DDR5 | Corsair | CMH32GX5M2M6000Z36 | C0FFEE00 |

## What happened on this machine

1 corruption snapshot(s) recorded.
- Boot kind: warm reboot 1, shutdown/poweroff 0, unexpected power loss 0, unknown 0.
- Sleep cycles on the previous boot: ≥2 in 1, exactly 1 in 0, none in 0.

Suggested loop: known-healthy boot → N suspend/resume cycles → warm reboot → inspect `am5-spd-diag status`.

### Sleep → reboot → corruption

### Transition 1

- Last healthy: `2026-08-17T00:30:00-04:00` event `reboot` firmware published 32000000 kB (30.52 GiB)
- Sleep cycles on the previous boot: **2**
- Sleep mode on last suspend-pre: `s2idle [deep]`
- How this boot started: **warm_reboot**
- Stuck SPD5118 hub(s): `1-0053` (MR11=0x08)
- Corrupt snapshot: `2026-08-17T00:30:30-04:00` event `boot` firmware published 17800092 kB (16.98 GiB) flags `unknown_part,dimm_8bit_width,ghost_page0`
- Identity change:
  - DIMMA2: unchanged (16 GiB CMH32GX5M2M6000Z36)
  - DIMMB2: 16 GiB CMH32GX5M2M6000Z36 64 bits → 2 GiB Unknown 8 bits

**Event sequence**

| Time | Event | State | MemTotal kB | Flags |
|---|---|---|---|---|
| 2026-08-17T00:00:00-04:00 | boot | ok | 32000000 |  |
| 2026-08-17T00:10:00-04:00 | suspend-pre | ok | 32000000 |  |
| 2026-08-17T00:11:00-04:00 | suspend-post | ok | 32000000 |  |
| 2026-08-17T00:20:00-04:00 | suspend-pre | ok | 32000000 |  |
| 2026-08-17T00:21:00-04:00 | suspend-post | ok | 32000000 |  |
| 2026-08-17T00:30:00-04:00 | reboot | ok | 32000000 |  |
| 2026-08-17T00:30:30-04:00 | boot warm_reboot | ALERT | 17800092 | unknown_part,dimm_8bit_width,ghost_page0 |

### Boot timeline

| Boot | Started | Events | Sleeps | SPD | First | Last | Firmware kB |
|---|---|---|---|---|---|---|---|
| `aaaaaaaa…` | — | 6 | 2 | healthy | `boot` | `reboot` | 32000000 |
| `bbbbbbbb…` | warm_reboot | 1 | 0 | corrupted | `boot` | `boot` | 17800092 |

## Firmware memory map (e820)

Full firmware e820 table (all types, not only System RAM). Kernel dmesg timestamps are omitted.

System RAM high range differs: healthy ends at `0x000000047fffffff`, corrupt ends at `0x000000041fffffff`.

Healthy `2026-08-17T00:30:00-04:00` MemTotal 32000000 kB (baseline 32000000 kB / 30.52 GiB):

```
BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM
BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved
BIOS-e820: [mem 0x0000000100000000-0x000000047fffffff] System RAM
```

Corrupt `2026-08-17T00:30:30-04:00` MemTotal 17800092 kB (16.98 GiB):

```
BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM
BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved
BIOS-e820: [mem 0x0000000100000000-0x000000041fffffff] System RAM
```

## SPD hub evidence

Evidence from this machine (corrupt captures kept even if identity later restored):
- `2026-08-17T00:30:30-04:00` `boot`: kernel spd5118 unbound at 1-0053 (16-bit addressing refused)
- `2026-08-17T00:30:30-04:00` `boot`: MR11=0x08 on 1-0053 (SMBus PIIX4 adapter)
- `2026-08-17T00:30:30-04:00` `boot`: SPD page-0 window (not full EEPROM) first 16 bytes: `23 0c 4d 08 00 00 00 00 00 20 62 00 00 00 00 00`
- `2026-08-17T00:30:30-04:00` `boot` dmesg: `spd5118 1-0053: Adapter does not support 16-bit register addresses`
- ghost page-0 serial `00206200` on DIMMB2 (empty part)

## For firmware vendors

Firmware/UEFI is the authority for DIMM identity. Placeholder part/width fields are not a Linux memory-accounting bug. Linux is the observer: the same SPD5118 MR11 latch survives a warm reboot on any OS until VDDSPD power loss (AC cut). A Windows-only check is not required to prove this.

Typical sequence: healthy baseline → sleep/wake → warm reboot (or POST) → Unknown/missing part and/or 8-bit width. Warm reboot does not clear SPD5118 MR11; AC power loss does. A dirty power blip or crash with no ACPI poweroff capture is not the same as pulling the cord: 5VSB/VDDSPD may stay up (DIMM RGB often stays lit). Optional in-band: `am5-spd-diag fix` then reboot.

Suspected component: Montage **SPD5118** MR11 (I2C 0x0B) latched to **0x08** (2-byte addressing) on VDDSPD standby. Details: https://forum-en.msi.com/index.php?threads/ddr5-module-detected-as-2gb-ghost-dimm-after-s3-sleep-on-am5-root-cause-found.419787/

Please:

1. Mask SPD hub page selects to 3 bits (`page & 7`) on **every** MR11 write, including ABL silent paths.
2. Write `MR11 = 0x00` for each DIMM **early in POST**, before SMBIOS / Memory-Z reads, so a stuck hub is self-healing on the next boot.
3. Confirm AGESA / vendor SPD code on AM5 after S0ix/S3 and warm reboot.
4. If needed: this tool’s `report` / `package` output is meant to go on a ticket as-is.

## Logs

Capture logs: `tests/fixture` (timeline.jsonl, ALERTS.log, baseline.json, events/).
`am5-spd-diag package` builds a tarball of the same.

Files in the last corrupt (or latest) event directory:
- `hub.json`: present
- `dmidecode-memory.txt`: missing
- `dimm-summary.txt`: present
- `e820.txt`: present
- `e820-system-ram.txt`: present
- `dmesg-spd5118.txt`: present
- `dmesg-filtered.txt`: missing
- `dmi-sysfs.txt`: present
- `system.json`: present
- `spd-page0-1-0053.txt`: present
