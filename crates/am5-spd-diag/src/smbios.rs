use crate::dimm::parse_dmidecode_memory;
use crate::safe_fs::{ensure_dir, write_nofollow};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DMI_TABLE: &str = "/sys/firmware/dmi/tables/DMI";
pub const DMI_ENTRIES: &str = "/sys/firmware/dmi/entries";

fn u16_at(data: &[u8], off: usize) -> u16 {
    if off + 1 >= data.len() {
        0
    } else {
        u16::from_le_bytes([data[off], data[off + 1]])
    }
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    if off + 3 >= data.len() {
        0
    } else {
        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    }
}

fn smbios_string(strings: &[String], idx: u8) -> String {
    if idx == 0 {
        return String::new();
    }
    strings
        .get((idx as usize).saturating_sub(1))
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parse_string_table(blob: &[u8], start: usize) -> (Vec<String>, usize) {
    let n = blob.len();
    if start >= n {
        return (Vec::new(), start);
    }
    if blob[start] == 0 {
        let mut nxt = start + 1;
        if nxt < n && blob[nxt] == 0 {
            nxt += 1;
        }
        return (Vec::new(), nxt);
    }
    let mut strings = Vec::new();
    let mut i = start;
    while i < n {
        if blob[i] == 0 {
            i += 1;
            break;
        }
        match blob[i..].iter().position(|&b| b == 0) {
            None => {
                strings.push(String::from_utf8_lossy(&blob[i..]).into_owned());
                return (strings, n);
            }
            Some(rel) => {
                let end = i + rel;
                strings.push(String::from_utf8_lossy(&blob[i..end]).into_owned());
                i = end + 1;
                if i < n && blob[i] == 0 {
                    i += 1;
                    break;
                }
            }
        }
    }
    (strings, i)
}

pub struct SmbiosStruct {
    pub typ: u8,
    pub handle: u16,
    pub formatted: Vec<u8>,
    pub strings: Vec<String>,
}

pub fn iter_smbios_structures(blob: &[u8]) -> Vec<SmbiosStruct> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let n = blob.len();
    while off + 4 <= n {
        let typ = blob[off];
        let length = blob[off + 1] as usize;
        let handle = u16_at(blob, off + 2);
        if length < 4 || off + length > n {
            break;
        }
        let formatted = blob[off..off + length].to_vec();
        let (strings, nxt) = parse_string_table(blob, off + length);
        out.push(SmbiosStruct {
            typ,
            handle,
            formatted,
            strings,
        });
        if typ == 127 {
            break;
        }
        if nxt <= off {
            break;
        }
        off = nxt;
    }
    out
}

fn format_width(code: u16) -> String {
    if code == 0 || code == 0xFFFF {
        "Unknown".into()
    } else {
        format!("{code} bits")
    }
}

fn format_size(formatted: &[u8]) -> String {
    if formatted.len() < 0x0E {
        return "Unknown".into();
    }
    let code = u16_at(formatted, 0x0C);
    if code == 0 {
        return "No Module Installed".into();
    }
    if code == 0xFFFF {
        return "Unknown".into();
    }
    let nbytes: u64 = if formatted.len() >= 0x20 && code == 0x7FFF {
        let mb = (u32_at(formatted, 0x1C) & 0x7FFF_FFFF) as u64;
        mb * 1024 * 1024
    } else if code & 0x8000 != 0 {
        (code as u64 & 0x7FFF) * 1024
    } else {
        (code as u64 & 0x7FFF) * 1024 * 1024
    };
    for (unit, step) in [
        ("GiB", 1024u64.pow(3)),
        ("MiB", 1024u64.pow(2)),
        ("KiB", 1024),
    ] {
        if nbytes >= step && nbytes.is_multiple_of(step) {
            return format!("{} {unit}", nbytes / step);
        }
    }
    format!("{nbytes} bytes")
}

fn format_speed(formatted: &[u8]) -> String {
    if formatted.len() < 0x22 {
        return String::new();
    }
    let code1 = u16_at(formatted, 0x20);
    let ext = if formatted.len() >= 0x5C {
        u32_at(formatted, 0x58)
    } else {
        0
    };
    if code1 == 0xFFFF {
        if ext == 0 {
            "Unknown".into()
        } else {
            format!("{ext} MT/s")
        }
    } else if code1 == 0 {
        "Unknown".into()
    } else {
        format!("{code1} MT/s")
    }
}

fn format_mfg_id(code: u16) -> String {
    if code == 0 {
        "Unknown".into()
    } else {
        format!("Bank {}, Hex 0x{:02X}", (code & 0x7F) + 1, code >> 8)
    }
}

const FORM_FACTORS: [(u8, &str); 4] = [
    (0x08, "RIMM"),
    (0x09, "DIMM"),
    (0x0D, "SODIMM"),
    (0x0F, "FB-DIMM"),
];
const MEMORY_TYPES: [(u8, &str); 6] = [
    (0x18, "DDR3"),
    (0x1A, "DDR3"),
    (0x1E, "DDR4"),
    (0x1F, "LPDDR4"),
    (0x22, "DDR5"),
    (0x23, "LPDDR5"),
];

fn format_voltage_mv(code: u16) -> String {
    if code == 0 || code == 0xFFFF {
        return String::new();
    }
    if code.is_multiple_of(1000) {
        format!("{} V", code / 1000)
    } else {
        let s = format!("{:.1} V", code as f64 / 1000.0);
        s.replace(".0 V", " V")
    }
}

pub fn type17_display_fields(formatted: &[u8], strings: &[String]) -> BTreeMap<String, String> {
    let empty = formatted.len() >= 0x0E && u16_at(formatted, 0x0C) == 0;
    let mut fields = BTreeMap::new();
    fields.insert(
        "Total Width".into(),
        if formatted.len() >= 0x0A {
            format_width(u16_at(formatted, 0x08))
        } else {
            "Unknown".into()
        },
    );
    fields.insert(
        "Data Width".into(),
        if formatted.len() >= 0x0C {
            format_width(u16_at(formatted, 0x0A))
        } else {
            "Unknown".into()
        },
    );
    fields.insert("Size".into(), format_size(formatted));
    fields.insert(
        "Locator".into(),
        smbios_string(
            strings,
            if formatted.len() > 0x10 {
                formatted[0x10]
            } else {
                0
            },
        ),
    );
    fields.insert(
        "Bank Locator".into(),
        smbios_string(
            strings,
            if formatted.len() > 0x11 {
                formatted[0x11]
            } else {
                0
            },
        ),
    );
    if empty {
        return fields;
    }
    if formatted.len() > 0x0E {
        if let Some((_, name)) = FORM_FACTORS.iter().find(|(c, _)| *c == formatted[0x0E]) {
            fields.insert("Form Factor".into(), (*name).into());
        }
    }
    if formatted.len() > 0x12 {
        if let Some((_, name)) = MEMORY_TYPES.iter().find(|(c, _)| *c == formatted[0x12]) {
            fields.insert("Type".into(), (*name).into());
        }
    }
    if formatted.len() > 0x17 {
        fields.insert(
            "Manufacturer".into(),
            smbios_string(strings, formatted[0x17]),
        );
    }
    if formatted.len() > 0x18 {
        fields.insert(
            "Serial Number".into(),
            smbios_string(strings, formatted[0x18]),
        );
    }
    if formatted.len() > 0x1A {
        fields.insert(
            "Part Number".into(),
            smbios_string(strings, formatted[0x1A]),
        );
    }
    if formatted.len() > 0x1B {
        let rank = formatted[0x1B] & 0x0F;
        if rank != 0 {
            fields.insert("Rank".into(), rank.to_string());
        }
    }
    let speed = format_speed(formatted);
    if !speed.is_empty() {
        fields.insert("Configured Memory Speed".into(), speed);
    }
    if formatted.len() >= 0x2E {
        fields.insert(
            "Module Manufacturer ID".into(),
            format_mfg_id(u16_at(formatted, 0x2C)),
        );
    }
    if formatted.len() >= 0x34 {
        let volt = format_voltage_mv(u16_at(formatted, 0x32));
        if !volt.is_empty() {
            fields.insert("Configured Voltage".into(), volt);
        }
    }
    fields
}

fn dump_fields(
    lines: &mut Vec<String>,
    handle: u16,
    typ: u8,
    formatted: &[u8],
    title: &str,
    fields: &BTreeMap<String, String>,
) {
    lines.push(String::new());
    lines.push(format!(
        "Handle 0x{handle:04X}, DMI type {typ}, {} bytes",
        formatted.len()
    ));
    lines.push(title.into());
    for (key, val) in fields {
        if !val.is_empty() {
            lines.push(format!("\t{key}: {val}"));
        }
    }
}

pub fn format_smbios_memory_dump(blob: &[u8], source: &str) -> String {
    let mut lines = vec![
        "# am5-spd-diag SMBIOS memory dump".into(),
        format!("# source: {source}"),
    ];
    let mut found = false;
    for st in iter_smbios_structures(blob) {
        if st.typ != 17 {
            continue;
        }
        found = true;
        let fields = type17_display_fields(&st.formatted, &st.strings);
        lines.push(String::new());
        lines.push(format!(
            "Handle 0x{:04X}, DMI type 17, {} bytes",
            st.handle,
            st.formatted.len()
        ));
        lines.push("Memory Device".into());
        // Preserve insertion-ish order used by Python (BTreeMap is sorted).
        // Python dict insertion: Total Width, Data Width, Size, Locator, Bank Locator, then optional.
        for key in [
            "Total Width",
            "Data Width",
            "Size",
            "Locator",
            "Bank Locator",
            "Form Factor",
            "Type",
            "Manufacturer",
            "Serial Number",
            "Part Number",
            "Rank",
            "Configured Memory Speed",
            "Module Manufacturer ID",
            "Configured Voltage",
        ] {
            if let Some(val) = fields.get(key) {
                lines.push(format!("\t{key}: {val}"));
            }
        }
    }
    if !found {
        lines.push(String::new());
        lines.push("# no SMBIOS type 17 Memory Device structures".into());
    }
    lines.push(String::new());
    lines.join("\n")
}

pub fn parse_smbios_memory(blob: &[u8]) -> Vec<BTreeMap<String, String>> {
    parse_dmidecode_memory(&format_smbios_memory_dump(blob, "blob"))
}

pub fn looks_like_text_dump(data: &[u8]) -> bool {
    let stripped = data
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .map(|i| &data[i..])
        .unwrap_or(data);
    if stripped.is_empty() {
        return true;
    }
    if stripped.starts_with(b"#")
        || stripped.starts_with(b"Handle")
        || stripped.starts_with(b"Getting ")
        || stripped.starts_with(b"SMBIOS")
        || stripped.starts_with(b"Memory Device")
    {
        return true;
    }
    if stripped[0] < 32 && !matches!(stripped[0], 9 | 10 | 13) {
        return false;
    }
    !stripped
        .get(..64.min(stripped.len()))
        .unwrap_or(stripped)
        .contains(&0)
}

pub fn parse_memory_devices(data: &[u8]) -> Vec<BTreeMap<String, String>> {
    if looks_like_text_dump(data) {
        parse_dmidecode_memory(&String::from_utf8_lossy(data))
    } else {
        parse_smbios_memory(data)
    }
}

pub fn read_sysfs_dmi_table() -> Option<Vec<u8>> {
    std::fs::read(DMI_TABLE).ok()
}

pub fn read_sysfs_type17_entries() -> Option<Vec<u8>> {
    let dir = Path::new(DMI_ENTRIES);
    if !dir.is_dir() {
        return None;
    }
    let mut blobs = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if !name.to_string_lossy().starts_with("17-") {
            continue;
        }
        if let Ok(raw) = std::fs::read(e.path().join("raw")) {
            blobs.extend(raw);
        }
    }
    if blobs.is_empty() {
        None
    } else {
        Some(blobs)
    }
}

fn dump_from_dmidecode() -> Option<String> {
    let out = Command::new("dmidecode")
        .arg("-t")
        .arg("memory")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.contains("DMI type 17") || text.contains("Memory Device") {
        Some(text)
    } else {
        None
    }
}

pub fn collect_memory_dump(table: Option<&Path>, allow_dmidecode: bool) -> (String, String) {
    if let Some(table) = table {
        let data = std::fs::read(table).unwrap_or_default();
        if looks_like_text_dump(&data) {
            return (String::from_utf8_lossy(&data).into_owned(), "file".into());
        }
        return (
            format_smbios_memory_dump(&data, &table.display().to_string()),
            "file".into(),
        );
    }
    let mut blob = read_sysfs_dmi_table();
    let mut source = "sysfs /sys/firmware/dmi/tables/DMI";
    if blob.is_none() {
        blob = read_sysfs_type17_entries();
        source = "sysfs /sys/firmware/dmi/entries/17-*";
    }
    if let Some(blob) = blob {
        let text = format_smbios_memory_dump(&blob, source);
        if text.contains("DMI type 17") {
            return (text, "sysfs".into());
        }
    }
    if allow_dmidecode {
        if let Some(text) = dump_from_dmidecode() {
            return (text, "dmidecode".into());
        }
    }
    (
        "# SMBIOS memory dump unavailable (no sysfs DMI table and no dmidecode)\n".into(),
        "none".into(),
    )
}

pub fn redact_dmi_secrets(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let stripped = line.trim_start();
        let key = stripped
            .split_once(':')
            .map(|(k, _)| k.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if key == "uuid" || key == "asset tag" {
            let indent_len = line.len() - stripped.len();
            let indent = &line[..indent_len];
            let label = stripped.split_once(':').map(|(k, _)| k).unwrap_or(stripped);
            lines.push(format!("{indent}{label}: [redacted]"));
        } else {
            lines.push(line.to_string());
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn mhz(code: u16) -> String {
    if code == 0 || code == 0xFFFF {
        "Unknown".into()
    } else {
        format!("{code} MHz")
    }
}

fn type0_fields(formatted: &[u8], strings: &[String]) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if formatted.len() > 0x05 {
        fields.insert("Vendor".into(), smbios_string(strings, formatted[0x04]));
        fields.insert("Version".into(), smbios_string(strings, formatted[0x05]));
    }
    if formatted.len() > 0x08 {
        fields.insert(
            "Release Date".into(),
            smbios_string(strings, formatted[0x08]),
        );
    }
    if formatted.len() >= 0x18 {
        if formatted[0x14] != 0xFF && formatted[0x15] != 0xFF {
            fields.insert(
                "BIOS Revision".into(),
                format!("{}.{}", formatted[0x14], formatted[0x15]),
            );
        }
        if formatted[0x16] != 0xFF && formatted[0x17] != 0xFF {
            fields.insert(
                "Firmware Revision".into(),
                format!("{}.{}", formatted[0x16], formatted[0x17]),
            );
        }
    }
    fields
}

fn type1_fields(formatted: &[u8], strings: &[String]) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if formatted.len() > 0x06 {
        fields.insert(
            "Manufacturer".into(),
            smbios_string(strings, formatted[0x04]),
        );
        fields.insert(
            "Product Name".into(),
            smbios_string(strings, formatted[0x05]),
        );
        fields.insert("Version".into(), smbios_string(strings, formatted[0x06]));
    }
    if formatted.len() > 0x1A {
        fields.insert("SKU Number".into(), smbios_string(strings, formatted[0x19]));
        fields.insert("Family".into(), smbios_string(strings, formatted[0x1A]));
    }
    fields
}

fn type2_fields(formatted: &[u8], strings: &[String]) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if formatted.len() > 0x06 {
        fields.insert(
            "Manufacturer".into(),
            smbios_string(strings, formatted[0x04]),
        );
        fields.insert(
            "Product Name".into(),
            smbios_string(strings, formatted[0x05]),
        );
        fields.insert("Version".into(), smbios_string(strings, formatted[0x06]));
    }
    if formatted.len() > 0x07 {
        let serial = smbios_string(strings, formatted[0x07]);
        if !serial.is_empty() {
            fields.insert("Serial Number".into(), serial);
        }
    }
    if formatted.len() > 0x0D {
        let t = formatted[0x0D];
        let name = match t {
            1 => "Other".into(),
            2 => "Unknown".into(),
            3 => "Server Blade".into(),
            10 => "Motherboard".into(),
            _ => format!("0x{t:02X}"),
        };
        fields.insert("Type".into(), name);
    }
    fields
}

fn type4_fields(formatted: &[u8], strings: &[String]) -> Option<BTreeMap<String, String>> {
    if formatted.len() < 0x1A {
        return None;
    }
    if formatted[0x18] & (1 << 6) == 0 {
        return None;
    }
    Some(BTreeMap::from([
        (
            "Socket Designation".into(),
            smbios_string(strings, formatted[0x04]),
        ),
        (
            "Manufacturer".into(),
            smbios_string(strings, formatted[0x07]),
        ),
        ("Version".into(), smbios_string(strings, formatted[0x10])),
        ("Max Speed".into(), mhz(u16_at(formatted, 0x14))),
        ("Current Speed".into(), mhz(u16_at(formatted, 0x16))),
    ]))
}

pub fn format_smbios_system_dump(blob: &[u8], source: &str) -> String {
    let mut lines = vec![
        "# am5-spd-diag SMBIOS system dump".into(),
        format!("# source: {source}"),
        "# system UUID and asset tags are omitted; board and DIMM serials are kept".into(),
    ];
    let mut found = false;
    for st in iter_smbios_structures(blob) {
        let (title, fields) = match st.typ {
            0 => (
                "BIOS Information",
                Some(type0_fields(&st.formatted, &st.strings)),
            ),
            1 => (
                "System Information",
                Some(type1_fields(&st.formatted, &st.strings)),
            ),
            2 => (
                "Base Board Information",
                Some(type2_fields(&st.formatted, &st.strings)),
            ),
            4 => (
                "Processor Information",
                type4_fields(&st.formatted, &st.strings),
            ),
            _ => continue,
        };
        let Some(fields) = fields else { continue };
        if fields.is_empty() {
            continue;
        }
        found = true;
        dump_fields(&mut lines, st.handle, st.typ, &st.formatted, title, &fields);
        // dump_fields skips empty; Python prints all non-empty in insertion order.
        // BTreeMap sorts keys. Tests only check Version/BIOS Revision presence.
        let _ = title;
    }
    if !found {
        lines.push(String::new());
        lines.push("# no SMBIOS BIOS/system/board/processor structures".into());
    }
    lines.push(String::new());
    lines.join("\n")
}

fn dump_system_from_dmidecode() -> Option<String> {
    let out = Command::new("dmidecode")
        .args([
            "-t",
            "bios",
            "-t",
            "system",
            "-t",
            "baseboard",
            "-t",
            "processor",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.contains("BIOS Information") || text.contains("Base Board") {
        Some(redact_dmi_secrets(&text))
    } else {
        None
    }
}

pub fn collect_system_dump(table: Option<&Path>, allow_dmidecode: bool) -> (String, String) {
    if let Some(table) = table {
        let data = std::fs::read(table).unwrap_or_default();
        if looks_like_text_dump(&data) {
            return (
                redact_dmi_secrets(&String::from_utf8_lossy(&data)),
                "file".into(),
            );
        }
        return (
            format_smbios_system_dump(&data, &table.display().to_string()),
            "file".into(),
        );
    }
    if let Some(blob) = read_sysfs_dmi_table() {
        let text = format_smbios_system_dump(&blob, "sysfs /sys/firmware/dmi/tables/DMI");
        if text.contains("DMI type") {
            return (text, "sysfs".into());
        }
    }
    if allow_dmidecode {
        if let Some(text) = dump_system_from_dmidecode() {
            return (text, "dmidecode".into());
        }
    }
    (
        "# SMBIOS system dump unavailable (no sysfs DMI table and no dmidecode)\n".into(),
        "none".into(),
    )
}

pub fn format_spd_page0_text(sysfs: &str, data: &[u8]) -> String {
    let mut lines = vec![
        "# SPD hub window (page 0 / 1-byte addressing), not full EEPROM".into(),
        format!("# device {sysfs} first {} bytes", data.len()),
    ];
    for i in (0..data.len()).step_by(16) {
        let chunk = &data[i..(i + 16).min(data.len())];
        let hexpart: Vec<_> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        lines.push(format!("{i:04x}: {}", hexpart.join(" ")));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn write_spd_page0_files(directory: &Path, probe: &serde_json::Value) -> Vec<PathBuf> {
    let mut written = Vec::new();
    let _ = ensure_dir(directory);
    let Some(stuck) = probe.get("stuck").and_then(|v| v.as_array()) else {
        return written;
    };
    for row in stuck {
        let hx = row.get("spd_page0").and_then(|v| v.as_str()).unwrap_or("");
        let sysfs = row
            .get("sysfs")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .replace('/', "_");
        if hx.is_empty() {
            continue;
        }
        let Ok(data) = hex_decode(hx) else { continue };
        let path = directory.join(format!("spd-page0-{sysfs}.txt"));
        if write_nofollow(&path, format_spd_page0_text(&sysfs, &data)).is_ok() {
            written.push(path);
        }
    }
    written
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimm::{dimm_flags, format_dimm_summary, parse_dmidecode_memory};

    #[allow(clippy::too_many_arguments)]
    fn pack_type17(
        handle: u16,
        empty: bool,
        size_mb: u16,
        total_width: u16,
        data_width: u16,
        locator: &str,
        bank: &str,
        manufacturer: &str,
        serial: &str,
        part: &str,
        speed: u16,
        mfg_id: u16,
    ) -> Vec<u8> {
        let length = if empty { 0x15 } else { 0x34 };
        let mut buf = vec![0u8; length];
        buf[0] = 17;
        buf[1] = length as u8;
        buf[2..4].copy_from_slice(&handle.to_le_bytes());
        buf[0x08..0x0A].copy_from_slice(&total_width.to_le_bytes());
        buf[0x0A..0x0C].copy_from_slice(&data_width.to_le_bytes());
        buf[0x0C..0x0E].copy_from_slice(&(if empty { 0 } else { size_mb }).to_le_bytes());
        if !empty {
            buf[0x0E] = 0x09;
            buf[0x12] = 0x22;
            buf[0x1B] = 1;
            buf[0x32..0x34].copy_from_slice(&1100u16.to_le_bytes());
        }
        let mut strings = vec![locator.to_string(), bank.to_string()];
        buf[0x10] = 1;
        buf[0x11] = 2;
        if !empty {
            strings.extend([manufacturer.into(), serial.into(), part.into()]);
            buf[0x17] = 3;
            buf[0x18] = 4;
            buf[0x1A] = 5;
            buf[0x20..0x22].copy_from_slice(&speed.to_le_bytes());
            buf[0x2C..0x2E].copy_from_slice(&mfg_id.to_le_bytes());
        }
        let mut blob = buf;
        for s in strings {
            blob.extend(s.as_bytes());
            blob.push(0);
        }
        blob.push(0);
        blob
    }

    fn pack_type0() -> Vec<u8> {
        let length = 0x18usize;
        let mut buf = vec![0u8; length];
        buf[0] = 0;
        buf[1] = length as u8;
        let strings = [
            "American Megatrends International, LLC.",
            "2.A52",
            "06/29/2026",
        ];
        buf[0x04] = 1;
        buf[0x05] = 2;
        buf[0x08] = 3;
        buf[0x14] = 5;
        buf[0x15] = 41;
        let mut blob = buf;
        for s in strings {
            blob.extend(s.as_bytes());
            blob.push(0);
        }
        blob.push(0);
        blob
    }

    fn pack_end() -> Vec<u8> {
        vec![127, 4, 0, 0, 0, 0]
    }

    #[test]
    fn system_dump() {
        let mut blob = pack_type0();
        blob.extend(pack_end());
        let text = format_smbios_system_dump(&blob, "test");
        assert!(text.contains("BIOS Information"));
        assert!(text.contains("Version: 2.A52"));
        assert!(text.contains("BIOS Revision: 5.41"));
        assert!(!text.contains("Serial Number"));
    }

    #[test]
    fn sysfs_style_blob() {
        let mut blob = pack_type17(
            0x000E,
            true,
            16384,
            0xFFFF,
            0xFFFF,
            "DIMMA1",
            "P0 CHANNEL A",
            "Unknown",
            "B5066693",
            "CMH32GX5M2M6000Z36",
            6000,
            0x9E02,
        );
        blob.extend(pack_type17(
            0x0010,
            false,
            16384,
            64,
            64,
            "DIMMA2",
            "P0 CHANNEL A",
            "Unknown",
            "B5066693",
            "CMH32GX5M2M6000Z36",
            6000,
            0x9E02,
        ));
        blob.extend(pack_type17(
            0x0015,
            false,
            16384,
            64,
            64,
            "DIMMB2",
            "P0 CHANNEL B",
            "Unknown",
            "B506743D",
            "CMH32GX5M2M6000Z36",
            6000,
            0x9E02,
        ));
        blob.extend(pack_end());
        let dump = format_smbios_memory_dump(&blob, "test");
        assert!(dump.contains("DMI type 17"));
        assert!(dump.contains("Locator: DIMMA1"));
        assert!(dump.contains("Size: No Module Installed"));
        assert!(dump.contains("Locator: DIMMA2"));
        assert!(dump.contains("Part Number: CMH32GX5M2M6000Z36"));
        assert!(dump.contains("Module Manufacturer ID: Bank 3, Hex 0x9E"));
        let dimms = parse_memory_devices(&blob);
        let locs: Vec<_> = dimms.iter().map(|d| d["locator"].as_str()).collect();
        assert_eq!(locs, ["DIMMA2", "DIMMB2"]);
        for d in &dimms {
            assert_eq!(d["size"], "16 GiB");
            assert_eq!(d["part"], "CMH32GX5M2M6000Z36");
            assert_eq!(d["manufacturer"], "Corsair");
            assert_eq!(d["speed"], "6000 MT/s");
            assert_eq!(d.get("mem_type").map(String::as_str), Some("DDR5"));
            assert!(dimm_flags(d).is_empty());
        }
    }

    #[test]
    fn corrupt_blob() {
        let blob = pack_type17(
            0x0015,
            false,
            2048,
            8,
            8,
            "DIMMB2",
            "P0 CHANNEL A",
            "Unknown",
            "00206200",
            "Unknown",
            6000,
            0,
        );
        let dimms = parse_smbios_memory(&blob);
        assert_eq!(dimms.len(), 1);
        let flags = dimm_flags(&dimms[0]);
        assert!(flags.contains(&"unknown_part".into()));
        assert!(flags.contains(&"dimm_8bit_width".into()));
        assert!(flags.contains(&"ghost_page0".into()));
    }

    #[test]
    fn extended_size() {
        let mut buf = pack_type17(
            0x0010,
            false,
            1,
            64,
            64,
            "DIMMA2",
            "P0 CHANNEL A",
            "Unknown",
            "B5066693",
            "CMH32GX5M2M6000Z36",
            6000,
            0x9E02,
        );
        buf[0x0C..0x0E].copy_from_slice(&0x7FFFu16.to_le_bytes());
        buf[0x1C..0x20].copy_from_slice(&32768u32.to_le_bytes());
        let dimms = parse_smbios_memory(&buf);
        assert_eq!(dimms[0]["size"], "32 GiB");
    }

    #[test]
    fn dump_memory_cli_table() {
        let mut blob = pack_type17(
            0x0010,
            false,
            16384,
            64,
            64,
            "DIMMA2",
            "P0 CHANNEL A",
            "Unknown",
            "B5066693",
            "CMH32GX5M2M6000Z36",
            6000,
            0x9E02,
        );
        blob.extend(pack_end());
        let dir = std::env::temp_dir();
        let path = dir.join("am5-spd-diag-smbios-test.bin");
        std::fs::write(&path, &blob).unwrap();
        let (text, source) = collect_memory_dump(Some(&path), false);
        assert_eq!(source, "file");
        let summary = format_dimm_summary(&parse_dmidecode_memory(&text));
        assert!(summary.contains("locator=DIMMA2"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn redact_keeps_board_serial() {
        let raw = "\tSerial Number: 07E701234567\n\tUUID: 12345678-1234-1234-1234-123456789abc\n\tAsset Tag: ABC\n";
        let out = redact_dmi_secrets(raw);
        assert!(out.contains("07E701234567"));
        assert!(out.contains("[redacted]"));
        assert!(!out.contains("12345678-1234"));
    }

    #[test]
    fn spd_page0_format() {
        let mut data = vec![0u8; 16];
        data[..4].copy_from_slice(&[0x23, 0x0c, 0x4d, 0x08]);
        let text = format_spd_page0_text("1-0053", &data);
        assert!(text.contains("not full EEPROM"));
        assert!(text.contains("0000:"));
        assert!(text.contains("23 0c 4d 08"));
    }
}
