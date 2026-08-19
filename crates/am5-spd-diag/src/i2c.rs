use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

pub const I2C_SLAVE: libc::c_ulong = 0x0703;
/// Use the address even when a kernel driver (spd5118) already claimed it.
pub const I2C_SLAVE_FORCE: libc::c_ulong = 0x0706;
pub const I2C_SMBUS: libc::c_ulong = 0x0720;
pub const I2C_SMBUS_READ: u8 = 1;
pub const I2C_SMBUS_WRITE: u8 = 0;
pub const I2C_SMBUS_BYTE_DATA: u32 = 2;
pub const I2C_SMBUS_WORD_DATA: u32 = 3;
pub const MR11: u8 = 0x0B;
pub const HUB_ADDRS: [u8; 4] = [0x50, 0x51, 0x52, 0x53];
pub const STUCK_MR11: u8 = 0x08;

#[derive(Debug, Clone)]
pub struct ByteRead {
    pub value: u8,
    pub forced: bool,
    pub method: &'static str,
}

#[repr(C)]
union I2cSmbusData {
    byte: u8,
    word: u16,
    block: [u8; 34],
}

#[repr(C)]
struct I2cSmbusIoctl {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut I2cSmbusData,
}

fn smbus_xfer(fd: libc::c_int, rw: u8, command: u8, size: u32, data: &mut I2cSmbusData) -> io::Result<()> {
    let mut args = I2cSmbusIoctl {
        read_write: rw,
        command,
        size,
        data: data as *mut I2cSmbusData,
    };
    let rc = unsafe { libc::ioctl(fd, I2C_SMBUS, &mut args) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `I2C_SLAVE`, then `I2C_SLAVE_FORCE` if a kernel driver already owns the address.
fn smbus_select(fd: libc::c_int, addr: u8) -> io::Result<bool> {
    let addr = addr as libc::c_ulong;
    if unsafe { libc::ioctl(fd, I2C_SLAVE, addr) } >= 0 {
        return Ok(false);
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::EBUSY) {
        return Err(err);
    }
    if unsafe { libc::ioctl(fd, I2C_SLAVE_FORCE, addr) } >= 0 {
        return Ok(true);
    }
    Err(io::Error::last_os_error())
}

pub fn smbus_read_byte_detailed(dev: &str, addr: u8, command: u8) -> io::Result<ByteRead> {
    let file = OpenOptions::new().read(true).write(true).open(dev)?;
    let fd = file.as_raw_fd();
    let forced = smbus_select(fd, addr)?;
    let mut data = I2cSmbusData { byte: 0 };
    smbus_xfer(fd, I2C_SMBUS_READ, command, I2C_SMBUS_BYTE_DATA, &mut data)?;
    Ok(ByteRead {
        value: unsafe { data.byte },
        forced,
        method: "ioctl",
    })
}

pub fn smbus_read_byte(dev: &str, addr: u8, command: u8) -> Option<u8> {
    smbus_read_byte_detailed(dev, addr, command).ok().map(|r| r.value)
}

pub fn smbus_write_word(dev: &str, addr: u8, command: u8, word: u16) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(dev) else {
        return false;
    };
    let fd = file.as_raw_fd();
    if smbus_select(fd, addr).is_err() {
        return false;
    }
    let mut data = I2cSmbusData { word };
    smbus_xfer(fd, I2C_SMBUS_WRITE, command, I2C_SMBUS_WORD_DATA, &mut data).is_ok()
}

fn i2c_tools_get_once(bus: i32, addr: u8, command: u8, force: bool) -> Option<u8> {
    let mut args = vec!["-y".to_string()];
    if force {
        args.push("-f".into());
    }
    args.push(bus.to_string());
    args.push(format!("0x{addr:02x}"));
    args.push(format!("0x{command:02x}"));
    args.push("b".into());
    let out = Command::new("i2cget")
        .args(&args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    u8::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

pub fn i2c_tools_get(bus: i32, addr: u8, command: u8) -> Option<u8> {
    i2c_tools_get_once(bus, addr, command, false).or_else(|| i2c_tools_get_once(bus, addr, command, true))
}

fn i2c_tools_set_word_once(bus: i32, addr: u8, command: u8, word: u16, force: bool) -> bool {
    let mut args = vec!["-y".to_string()];
    if force {
        args.push("-f".into());
    }
    args.push(bus.to_string());
    args.push(format!("0x{addr:02x}"));
    args.push(format!("0x{command:02x}"));
    args.push(format!("0x{word:04x}"));
    args.push("w".into());
    Command::new("i2cset")
        .args(&args)
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn i2c_tools_set_word(bus: i32, addr: u8, command: u8, word: u16) -> bool {
    i2c_tools_set_word_once(bus, addr, command, word, false)
        || i2c_tools_set_word_once(bus, addr, command, word, true)
}

pub fn errno_name(code: i32) -> Option<&'static str> {
    Some(match code {
        libc::EACCES => "EACCES",
        libc::EPERM => "EPERM",
        libc::EBUSY => "EBUSY",
        libc::ENXIO => "ENXIO",
        libc::EIO => "EIO",
        libc::ENODEV => "ENODEV",
        libc::ENOENT => "ENOENT",
        libc::EINVAL => "EINVAL",
        _ => return None,
    })
}

pub fn i2c_client_sysfs(bus: i32, addr: u8) -> String {
    format!("{bus}-00{addr:02x}")
}

pub fn parse_i2c_client_id(id: &str) -> Option<(i32, u8)> {
    let (bus, addr) = id.split_once('-')?;
    if addr.len() != 4 || !addr.starts_with("00") {
        return None;
    }
    let bus = bus.parse().ok()?;
    let addr = u8::from_str_radix(addr, 16).ok()?;
    Some((bus, addr))
}

pub fn is_spd5118_client(client: Option<&str>, driver: Option<&str>) -> bool {
    client == Some("spd5118") || driver == Some("spd5118")
}

pub fn i2c_client_name_and_driver(bus: i32, addr: u8) -> (Option<String>, Option<String>) {
    let id = i2c_client_sysfs(bus, addr);
    let base = format!("/sys/bus/i2c/devices/{id}");
    let name = std::fs::read_to_string(format!("{base}/name"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let driver = std::fs::read_link(format!("{base}/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    (name, driver)
}

fn read_mr11(dev: &str, bus: i32, addr: u8) -> Result<ByteRead, io::Error> {
    match smbus_read_byte_detailed(dev, addr, MR11) {
        Ok(r) => Ok(r),
        Err(e) => {
            if let Some(value) = i2c_tools_get(bus, addr, MR11) {
                Ok(ByteRead {
                    value,
                    forced: true,
                    method: "i2cget",
                })
            } else {
                Err(e)
            }
        }
    }
}

pub fn i2c_devices() -> Vec<(i32, String)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return found;
    };
    let mut names: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("i2c-"))
                .unwrap_or(false)
        })
        .collect();
    names.sort();
    for path in names {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(num) = name.strip_prefix("i2c-") else { continue };
        if let Ok(bus) = num.parse::<i32>() {
            found.push((bus, path.display().to_string()));
        }
    }
    found
}

pub fn adapter_name(bus: i32) -> String {
    // Kernels without CONFIG_I2C_COMPAT omit /sys/class/i2c-adapter.
    let candidates = [
        format!("/sys/class/i2c-adapter/i2c-{bus}/name"),
        format!("/sys/class/i2c-dev/i2c-{bus}/name"),
        format!("/sys/bus/i2c/devices/i2c-{bus}/name"),
        format!("/sys/class/i2c-dev/i2c-{bus}/device/name"),
    ];
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(path) {
            let name = text.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    String::new()
}

fn smbus_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)smbus|piix4|i801|fch|amd.*smb|\bsb-?t?si\b").unwrap())
}

fn non_smbus_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)nvidia|geforce|nouveau|designware|synopsys|cros-ec|aux\b|ddc|gpu").unwrap()
    })
}

pub fn is_smbus_adapter(name: &str, bus: Option<i32>) -> bool {
    let mut n = name.trim().to_string();
    if n.is_empty() {
        if let Some(bus) = bus {
            n = adapter_name(bus);
        }
    }
    if n.is_empty() {
        return false;
    }
    if smbus_re().is_match(&n) {
        return true;
    }
    if non_smbus_re().is_match(&n) {
        return false;
    }
    false
}

pub fn read_spd_page0(dev: &str, addr: u8, bus: Option<i32>, length: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for cmd in 0..length {
        let mut val = smbus_read_byte(dev, addr, cmd as u8);
        if val.is_none() {
            if let Some(bus) = bus {
                val = i2c_tools_get(bus, addr, cmd as u8);
            }
        }
        match val {
            Some(v) => out.push(v),
            None => break,
        }
    }
    if out.len() >= 16 {
        Some(out)
    } else {
        None
    }
}

pub fn spd5118_dmesg() -> Vec<String> {
    let out = Command::new("dmesg")
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|ln| ln.to_ascii_lowercase().contains("spd5118"))
        .map(|s| s.to_string())
        .collect()
}

pub fn parse_stuck_from_dmesg(lines: &[String]) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(\d+-00[0-9a-fA-F]{2})").unwrap());
    let mut stuck = Vec::new();
    for ln in lines {
        let low = ln.to_ascii_lowercase();
        if !low.contains("16-bit register") && !low.contains("does not support") {
            continue;
        }
        if let Some(c) = re.captures(ln) {
            let id = c.get(1).unwrap().as_str().to_string();
            if !stuck.contains(&id) {
                stuck.push(id);
            }
        }
    }
    stuck
}

fn attempt_error_fields(err: &io::Error) -> Value {
    let mut row = json!({
        "ok": false,
        "error": err.to_string(),
    });
    if let Some(code) = err.raw_os_error() {
        row["errno"] = json!(code);
        if let Some(name) = errno_name(code) {
            row["errno_name"] = json!(name);
        }
    }
    row
}

#[derive(Clone, Debug)]
struct HubTarget {
    bus: i32,
    addr: u8,
    sysfs: String,
    client: Option<String>,
    driver: Option<String>,
    source: &'static str,
}

/// Kernel-enumerated SPD5118 hubs. Does not SMBus-scan empty 0x50–0x53 slots.
fn spd_hub_targets(dmesg_stuck: &[String]) -> Vec<HubTarget> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir("/sys/bus/i2c/devices") {
        for ent in entries.flatten() {
            let id = ent.file_name();
            let Some(id) = id.to_str() else { continue };
            let Some((bus, addr)) = parse_i2c_client_id(id) else { continue };
            if !HUB_ADDRS.contains(&addr) || !is_smbus_adapter("", Some(bus)) {
                continue;
            }
            let (client, driver) = i2c_client_name_and_driver(bus, addr);
            if !is_spd5118_client(client.as_deref(), driver.as_deref()) {
                continue;
            }
            if seen.insert((bus, addr)) {
                targets.push(HubTarget {
                    bus,
                    addr,
                    sysfs: id.to_string(),
                    client,
                    driver,
                    source: "sysfs",
                });
            }
        }
    }
    for id in dmesg_stuck {
        let Some((bus, addr)) = parse_i2c_client_id(id) else { continue };
        if !HUB_ADDRS.contains(&addr) || !is_smbus_adapter("", Some(bus)) {
            continue;
        }
        if !seen.insert((bus, addr)) {
            continue;
        }
        let (client, driver) = i2c_client_name_and_driver(bus, addr);
        targets.push(HubTarget {
            bus,
            addr,
            sysfs: id.clone(),
            client,
            driver,
            source: "dmesg",
        });
    }
    targets.sort_by_key(|t| (t.bus, t.addr));
    targets
}

pub fn probe_hubs() -> Value {
    let dmesg_lines = spd5118_dmesg();
    let dmesg_stuck = parse_stuck_from_dmesg(&dmesg_lines);
    let mut result = json!({
        "dmesg": dmesg_lines.iter().rev().take(20).rev().cloned().collect::<Vec<_>>(),
        "dmesg_stuck": dmesg_stuck,
        "adapters": [],
        "hubs": [],
        "stuck": [],
        "attempts": [],
        "method": "none",
    });
    let devices = i2c_devices();
    let mut by_bus = HashMap::new();
    for (bus, dev) in &devices {
        let name = adapter_name(*bus);
        result["adapters"].as_array_mut().unwrap().push(json!({
            "bus": bus,
            "dev": dev,
            "name": name,
            "smbus": is_smbus_adapter(&name, Some(*bus)),
        }));
        by_bus.insert(*bus, dev.clone());
    }
    if devices.is_empty() {
        return result;
    }
    let stuck_ids: Vec<String> = result["dmesg_stuck"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for target in spd_hub_targets(&stuck_ids) {
        let adapter = adapter_name(target.bus);
        if !is_smbus_adapter(&adapter, Some(target.bus)) {
            continue;
        }
        let mut attempt = json!({
            "bus": target.bus,
            "adapter": adapter,
            "addr": target.addr,
            "addr_hex": format!("0x{:02x}", target.addr),
            "sysfs": target.sysfs,
            "source": target.source,
        });
        if let Some(client) = &target.client {
            attempt["client"] = json!(client);
        }
        if let Some(driver) = &target.driver {
            attempt["driver"] = json!(driver);
        }
        let Some(dev) = by_bus.get(&target.bus) else {
            attempt["ok"] = json!(false);
            attempt["error"] = json!(format!("no /dev/i2c-{}", target.bus));
            result["attempts"].as_array_mut().unwrap().push(attempt);
            continue;
        };
        attempt["dev"] = json!(dev);
        match read_mr11(dev, target.bus, target.addr) {
            Ok(got) => {
                result["method"] = json!(got.method);
                let stuck = got.value == STUCK_MR11;
                attempt["ok"] = json!(true);
                attempt["mr11"] = json!(got.value);
                attempt["mr11_hex"] = json!(format!("0x{:02x}", got.value));
                attempt["forced"] = json!(got.forced);
                attempt["method"] = json!(got.method);
                attempt["stuck"] = json!(stuck);
                let mut row = json!({
                    "bus": target.bus,
                    "dev": dev,
                    "adapter": adapter,
                    "addr": target.addr,
                    "addr_hex": format!("0x{:02x}", target.addr),
                    "sysfs": target.sysfs,
                    "mr11": got.value,
                    "mr11_hex": format!("0x{:02x}", got.value),
                    "stuck": stuck,
                    "forced": got.forced,
                    "method": got.method,
                    "source": target.source,
                });
                if let Some(driver) = &target.driver {
                    row["driver"] = json!(driver);
                }
                if stuck {
                    if let Some(page) = read_spd_page0(dev, target.addr, Some(target.bus), 128) {
                        row["spd_page0"] = json!(hex_encode(&page));
                        row["spd_page0_head"] = json!(hex_encode(&page[..16.min(page.len())]));
                    }
                }
                result["hubs"].as_array_mut().unwrap().push(row.clone());
                if stuck {
                    result["stuck"].as_array_mut().unwrap().push(row);
                }
            }
            Err(err) => {
                let extra = attempt_error_fields(&err);
                if let Some(obj) = extra.as_object() {
                    for (k, v) in obj {
                        attempt[k] = v.clone();
                    }
                }
            }
        }
        result["attempts"].as_array_mut().unwrap().push(attempt);
    }
    result
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn recover_stuck(probe: Option<Value>) -> Value {
    let probe = probe.unwrap_or_else(probe_hubs);
    let mut actions = Vec::new();
    let mut eligible = Vec::new();
    if let Some(stuck) = probe.get("stuck").and_then(|v| v.as_array()) {
        for hub in stuck {
            let name = hub.get("adapter").and_then(|v| v.as_str()).unwrap_or("");
            let bus = hub.get("bus").and_then(|v| v.as_i64()).map(|i| i as i32);
            if is_smbus_adapter(name, bus) {
                eligible.push(hub.clone());
            }
        }
    }
    if eligible.is_empty() {
        return json!({"ok": false, "reason": "no_stuck_hub", "probe": probe, "actions": actions});
    }
    let mut ok_all = true;
    for hub in eligible {
        let bus = hub["bus"].as_i64().unwrap_or(0) as i32;
        let addr = hub["addr"].as_u64().unwrap_or(0) as u8;
        let dev = hub["dev"].as_str().unwrap_or("");
        let mut wrote = smbus_write_word(dev, addr, MR11, 0x0000);
        let mut method = "ioctl";
        if !wrote {
            wrote = i2c_tools_set_word(bus, addr, MR11, 0x0000);
            method = "i2cset";
        }
        let mut verify = smbus_read_byte(dev, addr, MR11);
        if verify.is_none() {
            verify = i2c_tools_get(bus, addr, MR11);
        }
        let cleared = verify == Some(0x00);
        ok_all = ok_all && wrote && cleared;
        actions.push(json!({
            "sysfs": hub["sysfs"],
            "wrote": wrote,
            "method": method,
            "mr11_after": verify,
            "cleared": cleared,
        }));
    }
    json!({
        "ok": ok_all,
        "reason": if ok_all { "ok" } else { "verify_failed" },
        "probe": probe,
        "actions": actions,
    })
}

pub fn uid_from_bus_path(bus_path: &str) -> Option<u32> {
    let path = PathBuf::from(bus_path);
    let parts: Vec<_> = path.iter().map(|s| s.to_string_lossy().into_owned()).collect();
    let idx = parts.iter().position(|p| p == "user")?;
    parts.get(idx + 1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist() {
        assert!(is_smbus_adapter("SMBus PIIX4 adapter port 0 at 0b00", None));
        assert!(is_smbus_adapter("SMBus PIIX4 adapter port 2 at 0b00", None));
        assert!(is_smbus_adapter("SMBus I801 adapter at e000", None));
        assert!(is_smbus_adapter("AMD SMBus", None));
        assert!(is_smbus_adapter("FCH SMBus", None));
        assert!(!is_smbus_adapter("NVIDIA i2c adapter 0", None));
        assert!(!is_smbus_adapter("Synopsys DesignWare I2C adapter", None));
        assert!(!is_smbus_adapter("i2c-NVIDIA-GPU", None));
        assert!(!is_smbus_adapter("", None));
        assert!(!is_smbus_adapter("cros-ec", None));
    }

    #[test]
    fn recover_skips_non_smbus() {
        let probe = json!({
            "stuck": [{
                "bus": 2,
                "addr": 0x50,
                "dev": "/dev/i2c-2",
                "adapter": "NVIDIA i2c adapter 0",
                "sysfs": "2-0050",
            }]
        });
        let result = recover_stuck(Some(probe));
        assert_eq!(result["ok"], false);
        assert_eq!(result["reason"], "no_stuck_hub");
        assert_eq!(result["actions"], json!([]));
    }

    #[test]
    fn uid_from_session_bus() {
        assert_eq!(uid_from_bus_path("/run/user/1000/bus"), Some(1000));
    }

    #[test]
    fn sysfs_id_zero_pads_addr() {
        assert_eq!(i2c_client_sysfs(1, 0x51), "1-0051");
        assert_eq!(i2c_client_sysfs(1, 0x53), "1-0053");
        assert_eq!(i2c_client_sysfs(12, 0x50), "12-0050");
        assert_eq!(parse_i2c_client_id("1-0051"), Some((1, 0x51)));
        assert_eq!(parse_i2c_client_id("1-0053"), Some((1, 0x53)));
        assert_eq!(parse_i2c_client_id("12-0050"), Some((12, 0x50)));
        assert_eq!(parse_i2c_client_id("i2c-1"), None);
        assert!(is_spd5118_client(Some("spd5118"), None));
        assert!(is_spd5118_client(None, Some("spd5118")));
        assert!(!is_spd5118_client(Some("eeprom"), None));
        assert!(!is_spd5118_client(None, None));
    }

    #[test]
    fn errno_names_cover_probe_failures() {
        assert_eq!(errno_name(libc::EBUSY), Some("EBUSY"));
        assert_eq!(errno_name(libc::ENXIO), Some("ENXIO"));
        assert_eq!(errno_name(libc::EACCES), Some("EACCES"));
        assert_eq!(errno_name(libc::EPERM), Some("EPERM"));
        assert_eq!(errno_name(libc::EIO), Some("EIO"));
    }
}
