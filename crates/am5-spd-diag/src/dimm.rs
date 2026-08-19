use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const PLACEHOLDERS: &[&str] = &[
    "",
    "unknown",
    "not specified",
    "none",
    "not provided",
    "n/a",
    "to be filled by o.e.m.",
];

pub const GHOST_SERIALS: &[&str] = &["00206200", "00-20-62-00", "00 20 62 00"];

pub fn module_mfg_id() -> &'static BTreeMap<&'static str, &'static str> {
    static MAP: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        BTreeMap::from([
            ("9e", "Corsair"),
            ("ad", "SK Hynix"),
            ("ce", "Samsung"),
            ("2c", "Micron"),
            ("c1", "Infineon"),
            ("98", "Kingston"),
        ])
    })
}

pub fn is_placeholder(value: &str) -> bool {
    PLACEHOLDERS.contains(&value.trim().to_ascii_lowercase().as_str())
}

pub fn parse_dimm_summary(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut dimms = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || !line.contains('=') {
            continue;
        }
        let mut row = BTreeMap::new();
        for part in line.split('|') {
            if let Some((k, v)) = part.split_once('=') {
                row.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        if !row.is_empty() {
            apply_jedec_manufacturer(&mut row);
            dimms.push(row);
        }
    }
    dimms
}

fn decode_mfg_id(value: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)hex\s+0x([0-9a-f]{2})").unwrap());
    re.captures(&value.to_ascii_lowercase())
        .and_then(|c| c.get(1))
        .and_then(|m| module_mfg_id().get(m.as_str()).copied())
        .unwrap_or("")
        .to_string()
}

pub fn is_populated_dimm(dimm: &BTreeMap<String, String>) -> bool {
    let size = dimm.get("size").map(|s| s.trim()).unwrap_or("");
    let loc = dimm.get("locator").map(|s| s.trim()).unwrap_or("");
    if size.is_empty() || size.to_ascii_lowercase().contains("no module installed") {
        return false;
    }
    if !size.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    if loc.is_empty() || loc.to_ascii_lowercase().starts_with("locator:") {
        return false;
    }
    if loc.to_ascii_lowercase().contains("channel") && !loc.to_ascii_lowercase().contains("dimm") {
        return false;
    }
    true
}

fn empty_device() -> BTreeMap<String, String> {
    [
        "locator",
        "size",
        "total_width",
        "data_width",
        "manufacturer",
        "serial",
        "part",
        "speed",
        "mem_type",
        "form_factor",
        "rank",
        "voltage",
    ]
    .into_iter()
    .map(|k| (k.to_string(), String::new()))
    .collect()
}

pub fn apply_jedec_manufacturer(dimm: &mut BTreeMap<String, String>) {
    if !is_placeholder(dimm.get("manufacturer").map(String::as_str).unwrap_or("")) {
        return;
    }
    let decoded = decode_mfg_id(dimm.get("mfg_id").map(String::as_str).unwrap_or(""));
    if !decoded.is_empty() {
        dimm.insert("manufacturer".into(), decoded);
    }
}

pub fn parse_dmidecode_memory(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut devices = Vec::new();
    let mut cur: Option<BTreeMap<String, String>> = None;
    let mut in_device = false;

    let finish = |cur: &mut Option<BTreeMap<String, String>>,
                  in_device: &mut bool,
                  devices: &mut Vec<BTreeMap<String, String>>| {
        if let Some(mut row) = cur.take() {
            if is_populated_dimm(&row) {
                apply_jedec_manufacturer(&mut row);
                row.remove("mfg_id");
                devices.push(row);
            }
        }
        *in_device = false;
    };

    let start_device = |cur: &mut Option<BTreeMap<String, String>>,
                        in_device: &mut bool,
                        devices: &mut Vec<BTreeMap<String, String>>| {
        finish(cur, in_device, devices);
        *in_device = true;
        *cur = Some(empty_device());
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("Handle ") {
            if line.contains("DMI type 17,") {
                start_device(&mut cur, &mut in_device, &mut devices);
            } else {
                finish(&mut cur, &mut in_device, &mut devices);
            }
            continue;
        }
        if line.contains("DMI type 16,") {
            finish(&mut cur, &mut in_device, &mut devices);
            continue;
        }
        if line.contains("DMI type 17,") {
            if !in_device {
                start_device(&mut cur, &mut in_device, &mut devices);
            }
            continue;
        }
        if !in_device || cur.is_none() || !line.contains(':') {
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        let Some(row) = cur.as_mut() else { continue };
        match key {
            "Locator" => {
                row.insert("locator".into(), val.to_string());
            }
            "Size" => {
                row.insert("size".into(), val.to_string());
            }
            "Total Width" => {
                row.insert("total_width".into(), val.to_string());
            }
            "Data Width" => {
                row.insert("data_width".into(), val.to_string());
            }
            "Manufacturer" => {
                row.insert("manufacturer".into(), val.to_string());
            }
            "Serial Number" => {
                row.insert("serial".into(), val.to_string());
            }
            "Part Number" => {
                row.insert("part".into(), val.to_string());
            }
            "Configured Memory Speed" => {
                row.insert("speed".into(), val.to_string());
            }
            "Type" if !val.to_ascii_lowercase().starts_with("detail") => {
                row.insert("mem_type".into(), val.to_string());
            }
            "Form Factor" => {
                row.insert("form_factor".into(), val.to_string());
            }
            "Rank" => {
                row.insert("rank".into(), val.to_string());
            }
            "Configured Voltage" => {
                row.insert("voltage".into(), val.to_string());
            }
            "Module Manufacturer ID" => {
                row.insert("mfg_id".into(), val.to_string());
            }
            _ => {}
        }
    }
    finish(&mut cur, &mut in_device, &mut devices);
    devices
}

pub fn format_dimm_summary(dimms: &[BTreeMap<String, String>]) -> String {
    let mut lines = Vec::new();
    for d in dimms {
        let mut extra = String::new();
        for key in ["mem_type", "form_factor", "rank", "voltage", "mfg_id"] {
            if let Some(val) = d.get(key) {
                if !val.is_empty() {
                    extra.push_str(&format!("|{key}={val}"));
                }
            }
        }
        lines.push(format!(
            "locator={}|size={}|total_width={}|data_width={}|manufacturer={}|serial={}|part={}|speed={}{}",
            d.get("locator").map(String::as_str).unwrap_or(""),
            d.get("size").map(String::as_str).unwrap_or(""),
            d.get("total_width").map(String::as_str).unwrap_or(""),
            d.get("data_width").map(String::as_str).unwrap_or(""),
            d.get("manufacturer").map(String::as_str).unwrap_or(""),
            d.get("serial").map(String::as_str).unwrap_or(""),
            d.get("part").map(String::as_str).unwrap_or(""),
            d.get("speed").map(String::as_str).unwrap_or(""),
            extra,
        ));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

pub fn dimm_flags(dimm: &BTreeMap<String, String>) -> Vec<String> {
    if !is_populated_dimm(dimm) {
        return Vec::new();
    }
    let mut flags = Vec::new();
    let part = dimm.get("part").map(String::as_str).unwrap_or("");
    let serial = dimm
        .get("serial")
        .map(|s| s.replace(':', "").replace(' ', ""))
        .unwrap_or_default();
    let tw = dimm
        .get("total_width")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let dw = dimm
        .get("data_width")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if is_placeholder(part) {
        flags.push("unknown_part".into());
    }
    if tw.contains("8 bit") || dw.contains("8 bit") {
        flags.push("dimm_8bit_width".into());
    }
    let serial_cmp = serial.to_ascii_lowercase().replace('-', "");
    let ghosts: Vec<String> = GHOST_SERIALS
        .iter()
        .map(|s| s.replace('-', "").replace(' ', "").to_ascii_lowercase())
        .collect();
    if ghosts.contains(&serial_cmp) && is_placeholder(part) {
        flags.push("ghost_page0".into());
    }
    flags
}

pub fn summary_flags(text: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for dimm in parse_dimm_summary(text) {
        for flag in dimm_flags(&dimm) {
            if !seen.contains(&flag) {
                seen.push(flag);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::repo_root;
    use std::fs;

    #[test]
    fn dmidecode_healthy() {
        let text = fs::read_to_string(repo_root().join("tests/dmidecode-healthy.txt")).unwrap();
        let dimms = parse_dmidecode_memory(&text);
        let locs: Vec<_> = dimms.iter().map(|d| d["locator"].as_str()).collect();
        assert_eq!(locs, ["DIMMA2", "DIMMB2"]);
        for d in &dimms {
            assert_eq!(d["size"], "16 GiB");
            assert_eq!(d["total_width"], "64 bits");
            assert_eq!(d["data_width"], "64 bits");
            assert_eq!(d["part"], "CMH32GX5M2M6000Z36");
            assert_eq!(d["manufacturer"], "Corsair");
            assert_eq!(d["speed"], "6000 MT/s");
            assert_eq!(d["mem_type"], "DDR5");
            assert!(dimm_flags(d).is_empty());
        }
        let summary = format_dimm_summary(&dimms);
        assert!(!summary.contains("CHANNEL"));
        assert!(!summary.contains("DIMMA1"));
        assert!(summary_flags(&summary).is_empty());
    }

    #[test]
    fn dmidecode_corrupt() {
        let text = fs::read_to_string(repo_root().join("tests/dmidecode-corrupt.txt")).unwrap();
        let dimms = parse_dmidecode_memory(&text);
        let by_loc: BTreeMap<_, _> = dimms.iter().map(|d| (d["locator"].clone(), d.clone())).collect();
        assert_eq!(by_loc.len(), 2);
        assert!(dimm_flags(&by_loc["DIMMA2"]).is_empty());
        let flags = dimm_flags(&by_loc["DIMMB2"]);
        assert!(flags.contains(&"unknown_part".into()));
        assert!(flags.contains(&"dimm_8bit_width".into()));
        assert!(flags.contains(&"ghost_page0".into()));
    }

    #[test]
    fn ignore_bank_locator_garbage() {
        let garbage = concat!(
            "locator=Locator: P0 CHANNEL A|size=16 GiB|total_width=64 bits|",
            "data_width=64 bits|manufacturer=Unknown|serial=|part=|speed=\n",
            "locator=DIMMA2|size=|total_width=?|data_width=?|manufacturer=|",
            "serial=|part=|speed=\n",
            "locator=DIMMB2|size=Size: None|total_width=|data_width=|",
            "manufacturer=|serial=|part=|speed=\n",
        );
        assert!(summary_flags(garbage).is_empty());
    }

    #[test]
    fn jedec_unknown_manufacturer() {
        let text = concat!(
            "locator=DIMMA2|size=16 GiB|total_width=64 bits|data_width=64 bits|",
            "manufacturer=Unknown|serial=B5066693|part=CMH32GX5M2M6000Z36|",
            "mfg_id=Bank 3, Hex 0x9E\n",
        );
        let dimms = parse_dimm_summary(text);
        assert_eq!(dimms[0]["manufacturer"], "Corsair");
    }
}
