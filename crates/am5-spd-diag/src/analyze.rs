use crate::config::Config;
use crate::dimm::{dimm_flags, parse_dimm_summary, parse_dmidecode_memory};
use crate::schema::{
    event_from_value, iter_json_objects, load_json_object, value_as_i64, Baseline, Page0File,
    TimelineEvent, FORUM_URL,
};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const DMI_SYSFS_KEYS: &[&str] = &[
    "bios_vendor",
    "bios_version",
    "bios_date",
    "bios_release",
    "board_vendor",
    "board_name",
    "board_version",
    "board_serial",
    "sys_vendor",
    "product_name",
    "product_version",
    "product_family",
    "product_sku",
    "chassis_vendor",
    "chassis_type",
    "chassis_version",
];

fn chassis_types() -> &'static BTreeMap<&'static str, &'static str> {
    static MAP: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        BTreeMap::from([
            ("1", "Other"),
            ("2", "Unknown"),
            ("3", "Desktop"),
            ("4", "Low Profile Desktop"),
            ("5", "Pizza Box"),
            ("6", "Mini Tower"),
            ("7", "Tower"),
            ("8", "Portable"),
            ("9", "Laptop"),
            ("10", "Notebook"),
            ("11", "Hand Held"),
            ("14", "Sub Notebook"),
            ("30", "Tablet"),
            ("31", "Convertible"),
            ("32", "Detachable"),
        ])
    })
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub alert_event: TimelineEvent,
    pub prev_healthy: Option<TimelineEvent>,
    pub sleep_count: usize,
    pub boot_kind: String,
    pub reboot_between: bool,
    pub chain: Vec<TimelineEvent>,
    pub stuck_hubs: Vec<String>,
    pub bad_dimms: Vec<BTreeMap<String, String>>,
    pub mem_sleep: String,
}

#[derive(Clone, Debug)]
pub struct Context {
    pub boots: Vec<(String, Vec<TimelineEvent>)>,
    pub transitions: Vec<Transition>,
    pub dmi: BTreeMap<String, String>,
    pub last_alert: Option<TimelineEvent>,
    pub last_healthy: Option<TimelineEvent>,
    pub latest: Option<TimelineEvent>,
    pub baseline: Baseline,
    pub sleep_total: usize,
    pub alert_count: usize,
    pub spd_now: String,
    pub pattern: String,
    pub state_dir: PathBuf,
}

pub fn utc_stamp() -> String {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = t.as_secs() as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap())
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

pub fn iso_now() -> String {
    chrono::Local::now().to_rfc3339()
}

pub fn read_kv_file(path: &Path) -> BTreeMap<String, String> {
    let mut data = BTreeMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return data;
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            data.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    data
}

fn sysfs_text(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn load_event_extras(ev: &mut TimelineEvent) {
    let directory = PathBuf::from(&ev.dir);
    ev.dir_exists = directory.is_dir();
    if !directory.is_dir() {
        return;
    }
    let raw = directory.join("dmidecode-memory.txt");
    if raw.is_file() {
        if let Ok(text) = fs::read_to_string(&raw) {
            ev.dimms = parse_dmidecode_memory(&text);
        }
    }
    if ev.dimms.is_empty() {
        let summary = directory.join("dimm-summary.txt");
        let text = if summary.is_file() {
            fs::read_to_string(&summary).unwrap_or_default()
        } else {
            String::new()
        };
        ev.dimms = parse_dimm_summary(&text);
    }
    ev.meta = read_kv_file(&directory.join("meta.txt"));
    ev.dmi = read_kv_file(&directory.join("dmi-sysfs.txt"));
    ev.hub = load_json_object(&directory.join("hub.json"));
    ev.recover = load_json_object(&directory.join("recover.json"));
    ev.system = load_json_object(&directory.join("system.json"));
    ev.e820 = String::new();
    for name in ["e820.txt", "e820-system-ram.txt"] {
        let path = directory.join(name);
        if path.is_file() {
            ev.e820 = fs::read_to_string(&path).unwrap_or_default();
            break;
        }
    }
    let dmesg = directory.join("dmesg-spd5118.txt");
    ev.dmesg_spd = if dmesg.is_file() {
        fs::read_to_string(&dmesg).unwrap_or_default()
    } else {
        String::new()
    };
    ev.spd_page0.clear();
    if let Ok(rd) = fs::read_dir(&directory) {
        let mut pages: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("spd-page0-") && n.ends_with(".txt"))
                    .unwrap_or(false)
            })
            .collect();
        pages.sort();
        for path in pages {
            ev.spd_page0.push(Page0File {
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                text: fs::read_to_string(&path).unwrap_or_default(),
            });
        }
    }
    if ev.boot_kind.is_empty() {
        ev.boot_kind = ev.meta.get("boot_kind").cloned().unwrap_or_default();
    }
    if ev.hub_stuck.is_empty() {
        ev.hub_stuck = ev.meta.get("hub_stuck").cloned().unwrap_or_default();
    }
}

fn parse_timeline_file(path: &Path) -> Vec<TimelineEvent> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for obj in iter_json_objects(&text) {
        if let Some(ev) = event_from_value(obj) {
            events.push(ev);
        }
    }
    events
}

fn remap_event_dirs(events: &mut [TimelineEvent], events_dir: &Path) {
    for ev in events {
        if !ev.dir.is_empty() {
            let name = Path::new(&ev.dir).file_name().unwrap_or_default();
            ev.dir = events_dir.join(name).to_string_lossy().into_owned();
        }
        load_event_extras(ev);
    }
}

pub fn load_timeline(state_dir: &Path) -> Vec<TimelineEvent> {
    let mut events = parse_timeline_file(&state_dir.join("timeline.jsonl"));
    remap_event_dirs(&mut events, &state_dir.join("events"));
    events
}

/// Keep an extracted package tree alive for the duration of analyze/report.
pub struct PackageSession {
    _keep: Option<tempfile::TempDir>,
    pub root: PathBuf,
}

/// Open a `.tar.gz` package or an already-extracted directory that contains `timeline.jsonl`.
pub fn open_package(path: &Path) -> Result<PackageSession, String> {
    if path.is_dir() {
        let root = find_package_root(path).ok_or_else(|| {
            format!("no timeline.jsonl in {}", path.display())
        })?;
        return Ok(PackageSession {
            _keep: None,
            root,
        });
    }
    if !path.is_file() {
        return Err(format!("package not found: {}", path.display()));
    }
    let tmp = staging_tempdir("am5-spd-diag-from-")?;
    extract_package_archive(path, tmp.path())?;
    let root = find_package_root(tmp.path()).ok_or_else(|| {
        format!("archive has no timeline.jsonl: {}", path.display())
    })?;
    Ok(PackageSession {
        _keep: Some(tmp),
        root,
    })
}

fn extract_package_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(dec);
    for entry in tar.entries().map_err(|e| format!("tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let _ = entry
            .unpack_in(dest)
            .map_err(|e| format!("extract: {e}"))?;
    }
    Ok(())
}

fn find_package_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("timeline.jsonl").is_file() {
        return Some(dir.to_path_buf());
    }
    let mut hits: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("timeline.jsonl").is_file())
        .collect();
    if hits.len() == 1 {
        return hits.pop();
    }
    hits.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("am5-spd-diag-"))
            .unwrap_or(false)
    });
    if hits.len() == 1 {
        hits.pop()
    } else {
        None
    }
}

/// Load timeline.jsonl from a package and remap `dir` to `events/<basename>` in that tree.
pub fn load_timeline_from_package(root: &Path) -> Vec<TimelineEvent> {
    let mut events = parse_timeline_file(&root.join("timeline.jsonl"));
    remap_event_dirs(&mut events, &root.join("events"));
    events
}

pub fn load_baseline(state_dir: &Path) -> Baseline {
    let path = state_dir.join("baseline.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn live_dmi() -> BTreeMap<String, String> {
    let base = Path::new("/sys/class/dmi/id");
    let mut out = BTreeMap::new();
    for name in DMI_SYSFS_KEYS {
        let val = sysfs_text(&base.join(name));
        if !val.is_empty() {
            out.insert((*name).to_string(), val);
        }
    }
    out
}

pub fn live_cpu_info() -> BTreeMap<String, String> {
    let mut info = BTreeMap::new();
    let Ok(text) = fs::read_to_string("/proc/cpuinfo") else {
        return info;
    };
    let mapping = BTreeMap::from([
        ("model name", "model_name"),
        ("vendor_id", "vendor_id"),
        ("cpu family", "family"),
        ("model", "model"),
        ("stepping", "stepping"),
        ("microcode", "microcode"),
    ]);
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if let Some(dest) = mapping.get(key.as_str()) {
            if !info.contains_key(*dest) {
                info.insert((*dest).to_string(), val.trim().to_string());
            }
        }
    }
    info
}

pub fn live_cpu() -> String {
    live_cpu_info().get("model_name").cloned().unwrap_or_default()
}

pub fn live_os_info() -> BTreeMap<String, String> {
    read_kv_file(Path::new("/etc/os-release"))
        .into_iter()
        .map(|(k, v)| (k, v.trim().trim_matches('"').to_string()))
        .collect()
}

pub fn live_os() -> String {
    let data = live_os_info();
    data.get("PRETTY_NAME").cloned().or_else(|| data.get("NAME").cloned()).unwrap_or_default()
}

pub fn live_kernel_info() -> BTreeMap<String, String> {
    let u = uname();
    let mut info = BTreeMap::from([
        ("sysname".into(), u.0),
        ("release".into(), u.1.clone()),
        ("version".into(), u.2),
        ("machine".into(), u.3),
    ]);
    let proc_v = sysfs_text(Path::new("/proc/version"));
    if !proc_v.is_empty() {
        info.insert("proc_version".into(), proc_v);
    }
    info.retain(|_, v| !v.is_empty());
    info
}

fn uname() -> (String, String, String, String) {
    let mut u = unsafe { std::mem::zeroed::<libc::utsname>() };
    unsafe { libc::uname(&mut u) };
    let to_s = |buf: &[libc::c_char]| {
        let bytes: Vec<u8> = buf.iter().map(|c| *c as u8).take_while(|b| *b != 0).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    (
        to_s(&u.sysname),
        to_s(&u.release),
        to_s(&u.version),
        to_s(&u.machine),
    )
}

pub fn live_kernel() -> String {
    uname().1
}

pub fn boot_mode() -> String {
    if Path::new("/sys/firmware/efi").is_dir() {
        "UEFI".into()
    } else {
        "legacy".into()
    }
}

pub fn collect_system_info() -> Value {
    json!({
        "dmi": live_dmi(),
        "cpu": live_cpu_info(),
        "os": live_os_info(),
        "kernel": live_kernel_info(),
        "boot_mode": boot_mode(),
        "mem_sleep": mem_sleep(),
    })
}

pub fn memtotal_kb() -> i64 {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            if let Some(num) = rest.split_whitespace().next() {
                return num.parse().unwrap_or(0);
            }
        }
    }
    0
}

pub fn mem_sleep() -> String {
    fs::read_to_string("/sys/power/mem_sleep")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

pub fn kb_to_gib(kb: i64) -> String {
    if kb <= 0 {
        "unknown".into()
    } else {
        format!("{:.2} GiB", kb as f64 / 1024.0 / 1024.0)
    }
}

pub fn group_boots(events: &[TimelineEvent]) -> Vec<(String, Vec<TimelineEvent>)> {
    let mut boots: Vec<(String, Vec<TimelineEvent>)> = Vec::new();
    for ev in events {
        let bid = if ev.boot_id.is_empty() {
            "unknown".into()
        } else {
            ev.boot_id.clone()
        };
        if let Some((_, list)) = boots.iter_mut().find(|(id, _)| id == &bid) {
            list.push(ev.clone());
        } else {
            boots.push((bid, vec![ev.clone()]));
        }
    }
    boots
}

pub fn sleep_cycles(boot_events: &[TimelineEvent]) -> usize {
    boot_events
        .iter()
        .filter(|ev| ev.event == "suspend-pre" || ev.event == "hibernate-pre")
        .count()
}

pub fn dimm_table(dimms: &[BTreeMap<String, String>]) -> String {
    if dimms.is_empty() {
        return "_No populated DIMM summary captured (SMBIOS/dmidecode may have been unavailable)._".into();
    }
    let mut lines = vec![
        "| Locator | Size | Width | Speed | Type | Manufacturer | Part | Serial |".into(),
        "|---|---|---|---|---|---|---|---|".into(),
    ];
    for d in dimms {
        let width = format!(
            "{} / {}",
            d.get("total_width").map(String::as_str).unwrap_or("?"),
            d.get("data_width").map(String::as_str).unwrap_or("?")
        );
        lines.push(format!(
            "| {} | {} | {width} | {} | {} | {} | {} | {} |",
            d.get("locator").map(String::as_str).unwrap_or("?"),
            d.get("size").map(String::as_str).unwrap_or("?"),
            d.get("speed").map(|s| if s.is_empty() { "?" } else { s }).unwrap_or("?"),
            d.get("mem_type").map(|s| if s.is_empty() { "?" } else { s }).unwrap_or("?"),
            d.get("manufacturer").map(String::as_str).unwrap_or("?"),
            d.get("part").map(String::as_str).unwrap_or("?"),
            d.get("serial").map(String::as_str).unwrap_or("?"),
        ));
    }
    lines.join("\n")
}

pub fn slot_map_line(dimms: &[BTreeMap<String, String>]) -> String {
    if dimms.is_empty() {
        return "no populated DIMMs".into();
    }
    let locs: Vec<_> = dimms
        .iter()
        .map(|d| d.get("locator").map(String::as_str).unwrap_or("?"))
        .collect();
    let sizes: Vec<_> = dimms
        .iter()
        .map(|d| d.get("size").map(String::as_str).unwrap_or("?"))
        .collect();
    if sizes.iter().copied().collect::<HashSet<_>>().len() == 1 {
        format!("{}×{} in {}", dimms.len(), sizes[0], locs.join("+"))
    } else {
        locs.iter()
            .zip(sizes.iter())
            .map(|(loc, sz)| format!("{sz} in {loc}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn memory_from_dimms(dimms: &[BTreeMap<String, String>]) -> String {
    dimms
        .iter()
        .map(|d| {
            ["locator", "size", "manufacturer", "part"]
                .iter()
                .filter_map(|k| d.get(*k).filter(|s| !s.is_empty()).map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn event_row(ev: &TimelineEvent) -> String {
    let mark = if ev.is_alert() { "ALERT" } else { "ok" };
    let kind = ev.boot_kind.as_str();
    let extra = if !kind.is_empty() && kind != "unknown" && kind != "same_boot" {
        format!(" {kind}")
    } else {
        String::new()
    };
    format!(
        "| {} | {}{extra} | {mark} | {} | {} |",
        ev.ts,
        ev.event,
        ev.memtotal_kb_i64(),
        ev.flags
    )
}

pub fn boot_kind_from_previous_event(prev_event: &str) -> String {
    match prev_event {
        "reboot" => "warm_reboot".into(),
        "recover" => "warm_reboot".into(),
        "poweroff" | "shutdown" | "halt" => "shutdown_poweroff".into(),
        "" => "unknown".into(),
        _ => "unexpected_power_loss".into(),
    }
}

pub fn infer_boot_kind(events: &[TimelineEvent], index: usize) -> String {
    let ev = &events[index];
    let kind = if ev.boot_kind.is_empty() {
        ev.meta.get("boot_kind").cloned().unwrap_or_default()
    } else {
        ev.boot_kind.clone()
    };
    if !kind.is_empty() && kind != "unknown" && kind != "same_boot" {
        return kind;
    }
    let bid = &ev.boot_id;
    for j in (0..index).rev() {
        let prev = &events[j];
        if &prev.boot_id == bid {
            continue;
        }
        return boot_kind_from_previous_event(&prev.event);
    }
    "unknown".into()
}

pub fn latest_boot_start_kind(events: &[TimelineEvent]) -> String {
    for i in (0..events.len()).rev() {
        if events[i].event == "boot" {
            return infer_boot_kind(events, i);
        }
    }
    String::new()
}

pub fn recover_events(events: &[TimelineEvent]) -> Vec<&TimelineEvent> {
    events.iter().filter(|e| e.event == "recover").collect()
}

pub fn recover_cleared(ev: &TimelineEvent) -> bool {
    if let Some(ok) = ev.recover.get("ok").and_then(|v| v.as_bool()) {
        return ok;
    }
    ev.recover
        .get("actions")
        .and_then(|v| v.as_array())
        .map(|a| {
            !a.is_empty()
                && a.iter()
                    .all(|row| row.get("cleared").and_then(|v| v.as_bool()) == Some(true))
        })
        .unwrap_or(false)
}

pub fn recover_this_boot<'a>(events: &'a [TimelineEvent], boot_id: &str) -> Option<&'a TimelineEvent> {
    events
        .iter()
        .rev()
        .find(|e| e.event == "recover" && (boot_id.is_empty() || e.boot_id == boot_id))
}

pub fn recover_before_latest_boot(events: &[TimelineEvent]) -> Option<&TimelineEvent> {
    let boot_idx = events.iter().rposition(|e| e.event == "boot")?;
    events[..boot_idx].iter().rev().find(|e| e.event == "recover")
}

pub fn recover_status_lines(events: &[TimelineEvent], spd_now: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let latest_boot_id = events
        .iter()
        .rev()
        .find(|e| e.event == "boot")
        .map(|e| e.boot_id.as_str())
        .or_else(|| events.last().map(|e| e.boot_id.as_str()))
        .unwrap_or("");
    if let Some(ev) = recover_this_boot(events, latest_boot_id) {
        let ts = ev.ts.as_str();
        if recover_cleared(ev) {
            if spd_now == "corrupted" {
                lines.push(format!(
                    "- In-band MR11 clear applied at `{ts}` this boot. Firmware identity stays stale until a **warm reboot**."
                ));
            } else {
                lines.push(format!("- In-band MR11 clear applied at `{ts}` this boot."));
            }
        } else {
            lines.push(format!(
                "- In-band MR11 fix at `{ts}` this boot did not verify clear (reason `{}`).",
                ev.recover
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            ));
        }
    } else if spd_now == "healthy" {
        if let Some(ev) = recover_before_latest_boot(events) {
            if recover_cleared(ev) {
                lines.push(format!(
                    "- Identity looks restored after in-band MR11 clear (`{}`) and a warm reboot (not an AC power loss).",
                    ev.ts
                ));
            }
        }
    }
    lines
}

fn previous_boot_id(events: &[TimelineEvent], index: usize) -> Option<String> {
    let bid = &events[index].boot_id;
    for j in (0..index).rev() {
        if &events[j].boot_id != bid {
            return Some(events[j].boot_id.clone());
        }
    }
    None
}

pub fn find_transitions(events: &[TimelineEvent]) -> Vec<Transition> {
    let boots = group_boots(events);
    let mut transitions = Vec::new();
    let mut last_alert_boot: Option<String> = None;
    for (i, ev) in events.iter().enumerate() {
        if !ev.is_alert() || ev.event == "recover" {
            continue;
        }
        let bid = ev.boot_id.clone();
        if !bid.is_empty() && last_alert_boot.as_ref() == Some(&bid) {
            continue;
        }
        last_alert_boot = if bid.is_empty() { None } else { Some(bid) };
        let mut prev_healthy = None;
        let mut chain = Vec::new();
        for j in (0..i).rev() {
            let prev = &events[j];
            chain.push(prev.clone());
            if !prev.is_alert() && prev_healthy.is_none() {
                prev_healthy = Some(prev.clone());
            }
            if prev.event == "boot" && prev.boot_id != ev.boot_id {
                if !prev.is_alert() && prev_healthy.is_none() {
                    prev_healthy = Some(prev.clone());
                }
                break;
            }
            if prev.is_alert() && prev.event == "boot" {
                break;
            }
        }
        chain.reverse();
        let prev_bid = previous_boot_id(events, i).unwrap_or_default();
        let sleep_count = boots
            .iter()
            .find(|(id, _)| id == &prev_bid)
            .map(|(_, evs)| sleep_cycles(evs))
            .unwrap_or(0);
        let boot_kind = infer_boot_kind(events, i);
        let reboot_between = boot_kind == "warm_reboot" || chain.iter().any(|p| p.event == "reboot");
        let mut stuck = Vec::new();
        if let Some(arr) = ev.hub.get("stuck").and_then(|v| v.as_array()) {
            for row in arr {
                let s = row
                    .get("sysfs")
                    .and_then(|v| v.as_str())
                    .or_else(|| row.get("addr_hex").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if !s.is_empty() {
                    stuck.push(s.to_string());
                }
            }
        }
        let last_pre = chain
            .iter()
            .rev()
            .find(|p| p.event == "suspend-pre" || p.event == "hibernate-pre");
        let mut mem_sleep_used = String::new();
        if let Some(last_pre) = last_pre {
            mem_sleep_used = last_pre
                .meta
                .get("mem_sleep")
                .cloned()
                .unwrap_or_else(|| last_pre.mem_sleep.clone());
        }
        let bad_dimms: Vec<_> = ev.dimms.iter().filter(|d| !dimm_flags(d).is_empty()).cloned().collect();
        let mut full_chain = chain;
        full_chain.push(ev.clone());
        transitions.push(Transition {
            alert_event: ev.clone(),
            prev_healthy,
            sleep_count,
            boot_kind,
            reboot_between,
            chain: full_chain,
            stuck_hubs: stuck,
            bad_dimms,
            mem_sleep: mem_sleep_used,
        });
    }
    transitions
}

pub fn render_pattern(_events: &[TimelineEvent], boots: &[(String, Vec<TimelineEvent>)], transitions: &[Transition]) -> String {
    if transitions.is_empty() {
        let healthy_sleep = boots
            .iter()
            .filter(|(_, evs)| sleep_cycles(evs) > 0 && !evs.iter().any(|e| e.is_alert()))
            .count();
        let extra = if healthy_sleep > 0 {
            format!(" {healthy_sleep} boot(s) had sleep cycles without an SPD identity alert.")
        } else {
            String::new()
        };
        return format!(
            "No corruption events recorded yet. Leave the monitor enabled through sleep and the next reboot.{extra}"
        );
    }
    let n = transitions.len();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for tr in transitions {
        *kinds.entry(tr.boot_kind.clone()).or_default() += 1;
    }
    let sleep2 = transitions.iter().filter(|tr| tr.sleep_count >= 2).count();
    let sleep1 = transitions.iter().filter(|tr| tr.sleep_count == 1).count();
    let sleep0 = transitions.iter().filter(|tr| tr.sleep_count == 0).count();
    let mut lines = vec![
        format!("{n} corruption snapshot(s) recorded."),
        format!(
            "- Boot kind: warm reboot {}, shutdown/poweroff {}, unexpected power loss {}, unknown {}.",
            kinds.get("warm_reboot").copied().unwrap_or(0),
            kinds.get("shutdown_poweroff").copied().unwrap_or(0),
            kinds.get("unexpected_power_loss").copied().unwrap_or(0),
            kinds.get("unknown").copied().unwrap_or(0) + kinds.get("same_boot").copied().unwrap_or(0),
        ),
        format!("- Sleep cycles on the previous boot: ≥2 in {sleep2}, exactly 1 in {sleep1}, none in {sleep0}."),
    ];
    if sleep0 > 0 {
        lines.push(
            "- POST with no sleep this boot matches the forum report that firmware can write MR11 during POST, not only on S3 resume.".into(),
        );
    }
    let mut healthy_after_sleep = 0;
    for (i, (_bid, evs)) in boots.iter().enumerate() {
        if evs.iter().any(|e| e.is_alert()) || sleep_cycles(evs) < 2 {
            continue;
        }
        if i + 1 < boots.len() && boots[i + 1].1.iter().any(|e| e.is_alert()) {
            continue;
        }
        healthy_after_sleep += 1;
    }
    if healthy_after_sleep > 0 {
        lines.push(format!(
            "- Intermittent: {healthy_after_sleep} boot(s) had ≥2 suspends and did not show SPD identity corruption (same sequence is not a guaranteed trigger)."
        ));
    }
    lines.join("\n")
}

fn md_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ").trim().to_string()
}

fn is_dmi_placeholder(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "unknown"
            | "none"
            | "n/a"
            | "not specified"
            | "not provided"
            | "default string"
            | "to be filled by o.e.m."
            | "to be filled by o.e.m"
            | "to be filled by oem"
    )
}

fn md_kv_table(rows: &[(&str, String)]) -> String {
    let mut lines = vec!["| Item | Details |".into(), "|---|---|".into()];
    let mut kept = 0;
    for (key, val) in rows {
        let text = md_cell(val);
        if text.is_empty() || is_dmi_placeholder(&text) {
            continue;
        }
        lines.push(format!("| {key} | {text} |"));
        kept += 1;
    }
    if kept == 0 {
        "_No system details available._".into()
    } else {
        lines.join("\n")
    }
}

fn dimm_key(dimm: &BTreeMap<String, String>) -> Vec<String> {
    ["locator", "size", "total_width", "data_width", "manufacturer", "part", "serial"]
        .iter()
        .map(|k| dimm.get(*k).cloned().unwrap_or_default().trim().to_string())
        .collect()
}

fn dimms_match(a: &[BTreeMap<String, String>], b: &[BTreeMap<String, String>]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.iter().map(dimm_key).collect::<Vec<_>>() == b.iter().map(dimm_key).collect::<Vec<_>>()
}

fn map_str(m: &BTreeMap<String, String>, k: &str) -> String {
    m.get(k).cloned().unwrap_or_default()
}

fn value_map_str(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn hardware_table(
    cfg: &Config,
    dmi_in: &BTreeMap<String, String>,
    cpu: &str,
    os_name: &str,
    kernel: &str,
    baseline: &Baseline,
    system: &Value,
) -> String {
    let mut dmi = dmi_in.clone();
    if let Some(obj) = system.get("dmi").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    dmi.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    let cpu_info = system.get("cpu").cloned().unwrap_or(json!({}));
    let osinfo = system.get("os").cloned().unwrap_or(json!({}));
    let kinfo = system.get("kernel").cloned().unwrap_or(json!({}));
    let board = first_nonempty(&[
        &map_str(&dmi, "board_name"),
        &map_str(&baseline.dmi, "board_name"),
        cfg.get("FALLBACK_BOARD"),
    ]);
    let bios = first_nonempty(&[
        &map_str(&dmi, "bios_version"),
        &map_str(&baseline.dmi, "bios_version"),
        cfg.get("FALLBACK_BIOS"),
    ]);
    let bios_date = first_nonempty(&[&map_str(&dmi, "bios_date"), &map_str(&baseline.dmi, "bios_date")]);
    let vendor = first_nonempty(&[
        &map_str(&dmi, "board_vendor"),
        &map_str(&dmi, "sys_vendor"),
        &map_str(&baseline.dmi, "board_vendor"),
    ]);
    let cpu = first_nonempty(&[
        &value_map_str(&cpu_info, "model_name"),
        cpu,
        &baseline.cpu,
        cfg.get("FALLBACK_CPU"),
    ]);
    let mut memory = memory_from_dimms(&baseline.dimms);
    if memory.is_empty() {
        memory = cfg.get("FALLBACK_MEMORY").to_string();
    }
    let base_kb = value_as_i64(&baseline.memtotal_kb);
    if base_kb != 0 {
        memory = format!("{memory} (healthy MemTotal {base_kb} kB / {})", kb_to_gib(base_kb));
    }
    let chassis = map_str(&dmi, "chassis_type");
    let chassis_s = chassis_types()
        .get(chassis.as_str())
        .copied()
        .unwrap_or(&chassis)
        .to_string();
    let mut cpu_ids = String::new();
    if !value_map_str(&cpu_info, "family").is_empty() {
        cpu_ids = format!(
            "family {} model {} stepping {}",
            value_map_str(&cpu_info, "family"),
            value_map_str(&cpu_info, "model"),
            value_map_str(&cpu_info, "stepping")
        );
    }
    let mut os_line = first_nonempty(&[&value_map_str(&osinfo, "PRETTY_NAME"), os_name, "unknown"]);
    if !value_map_str(&osinfo, "VERSION_ID").is_empty() {
        os_line = format!(
            "{os_line} ({} {})",
            value_map_str(&osinfo, "ID"),
            value_map_str(&osinfo, "VERSION_ID")
        )
        .trim()
        .to_string();
    }
    let mut kernel_line = first_nonempty(&[&value_map_str(&kinfo, "release"), kernel, "unknown"]);
    if !value_map_str(&kinfo, "machine").is_empty() {
        kernel_line = format!("{kernel_line} {}", value_map_str(&kinfo, "machine"));
    }
    let mut product = map_str(&dmi, "product_name");
    if !product.is_empty() && !board.is_empty() && board.contains(&product) {
        product.clear();
    }
    let mut sys_vendor = map_str(&dmi, "sys_vendor");
    if !sys_vendor.is_empty() && !vendor.is_empty() && sys_vendor == vendor {
        sys_vendor.clear();
    }
    let chassis_vendor = map_str(&dmi, "chassis_vendor");
    let mut chassis_line = chassis_s.clone();
    if !chassis_vendor.is_empty() && chassis_vendor != vendor {
        chassis_line = format!("{chassis_vendor} {chassis_s}").trim().to_string();
    }
    let board_ver = map_str(&dmi, "board_version");
    let mut board_line = board.clone();
    if !board_ver.is_empty() && !is_dmi_placeholder(&board_ver) {
        board_line = format!("{board} rev {board_ver}");
    }
    let mut product_ver = map_str(&dmi, "product_version");
    if !product_ver.is_empty() && product_ver == board_ver {
        product_ver.clear();
    }
    let board_serial = first_nonempty(&[&map_str(&dmi, "board_serial"), &map_str(&baseline.dmi, "board_serial")]);
    let bios_line = if bios_date.is_empty() {
        bios.clone()
    } else {
        format!("{bios} ({bios_date})")
    };
    let boot = system
        .get("boot_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sleep = system
        .get("mem_sleep")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    md_kv_table(&[
        ("Vendor", vendor),
        ("System vendor", sys_vendor),
        ("Motherboard", board_line),
        ("Board serial", board_serial),
        ("Product", product),
        ("Product family", map_str(&dmi, "product_family")),
        ("Product version", product_ver),
        ("SKU", map_str(&dmi, "product_sku")),
        ("Chassis", chassis_line),
        ("BIOS vendor", map_str(&dmi, "bios_vendor")),
        ("BIOS version", bios_line),
        ("BIOS revision", map_str(&dmi, "bios_release")),
        ("Firmware boot mode", if boot.is_empty() { boot_mode() } else { boot }),
        ("CPU", first_nonempty(&[&value_map_str(&cpu_info, "model_name"), cpu.as_str(), &baseline.cpu, cfg.get("FALLBACK_CPU")])),
        ("CPU ID", cpu_ids),
        ("CPU microcode", value_map_str(&cpu_info, "microcode")),
        (
            "Memory (healthy baseline)",
            if memory.is_empty() {
                "no healthy baseline yet".into()
            } else {
                memory
            },
        ),
        ("OS", os_line),
        ("Kernel", kernel_line),
        ("Kernel build", value_map_str(&kinfo, "version")),
        ("Sleep policy", if sleep.is_empty() { mem_sleep() } else { sleep }),
    ])
}

fn first_nonempty(vals: &[&str]) -> String {
    vals.iter()
        .find(|s| !s.is_empty())
        .map(|s| (*s).to_string())
        .unwrap_or_default()
}

pub fn system_identity_rows(
    dmi: &BTreeMap<String, String>,
    cpu: &str,
    kernel: &str,
    system: Option<&Value>,
) -> Vec<(&'static str, String)> {
    let system_owned = system.cloned().unwrap_or_else(collect_system_info);
    let dmi_sys = system_owned.get("dmi").cloned().unwrap_or(json!({}));
    let board = first_nonempty(&[
        dmi.get("board_name").map(String::as_str).unwrap_or(""),
        dmi_sys.get("board_name").and_then(|v| v.as_str()).unwrap_or(""),
        "unknown board",
    ]);
    let bios = first_nonempty(&[
        dmi.get("bios_version").map(String::as_str).unwrap_or(""),
        dmi_sys.get("bios_version").and_then(|v| v.as_str()).unwrap_or(""),
        "unknown BIOS",
    ]);
    let cpu = first_nonempty(&[
        system_owned
            .pointer("/cpu/model_name")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        cpu,
        "unknown CPU",
    ]);
    let krel = first_nonempty(&[
        system_owned
            .pointer("/kernel/release")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        kernel,
        &live_kernel(),
    ]);
    let os_name = first_nonempty(&[
        system_owned
            .pointer("/os/PRETTY_NAME")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        &live_os(),
        "unknown OS",
    ]);
    let mode = first_nonempty(&[
        system_owned.get("boot_mode").and_then(|v| v.as_str()).unwrap_or(""),
        &boot_mode(),
    ]);
    vec![
        ("Board", board),
        ("BIOS", bios),
        ("CPU", cpu),
        ("OS", os_name),
        ("Kernel", krel),
        ("Boot mode", mode),
    ]
}

pub fn system_identity_table(
    dmi: &BTreeMap<String, String>,
    cpu: &str,
    kernel: &str,
    system: Option<&Value>,
) -> String {
    md_kv_table(&system_identity_rows(dmi, cpu, kernel, system))
}

pub fn system_oneliner(dmi: &BTreeMap<String, String>, cpu: &str, kernel: &str, system: Option<&Value>) -> String {
    system_identity_rows(dmi, cpu, kernel, system)
        .into_iter()
        .map(|(key, val)| format!("\n · {}: {val}", key.to_ascii_uppercase()))
        .collect()
}

fn capture_ident(ev: &TimelineEvent) -> BTreeMap<String, String> {
    let sysinfo = &ev.system;
    let kdmi = sysinfo.get("dmi").cloned().unwrap_or_else(|| json!(ev.dmi));
    let kinfo = sysinfo.get("kernel").cloned().unwrap_or(json!({}));
    let osinfo = sysinfo.get("os").cloned().unwrap_or(json!({}));
    BTreeMap::from([
        (
            "ts".into(),
            first_nonempty(&[&ev.ts, ev.meta.get("ts").map(String::as_str).unwrap_or("")]),
        ),
        (
            "event".into(),
            first_nonempty(&[&ev.event, ev.meta.get("event").map(String::as_str).unwrap_or("")]),
        ),
        ("bios".into(), value_map_str(&kdmi, "bios_version")),
        ("bios_date".into(), value_map_str(&kdmi, "bios_date")),
        ("board".into(), value_map_str(&kdmi, "board_name")),
        (
            "os".into(),
            first_nonempty(&[&value_map_str(&osinfo, "PRETTY_NAME"), &value_map_str(&osinfo, "NAME")]),
        ),
        (
            "kernel".into(),
            first_nonempty(&[
                &value_map_str(&kinfo, "release"),
                ev.meta.get("uname").map(String::as_str).unwrap_or(""),
            ]),
        ),
    ])
}

fn captured_system_table(events: &[TimelineEvent], live: &Value) -> String {
    if events.is_empty() {
        return "_No captures yet._".into();
    }
    let live_d = live.get("dmi").cloned().unwrap_or(json!({}));
    let live_k = first_nonempty(&[&value_map_str(live.get("kernel").unwrap_or(&json!({})), "release"), &live_kernel()]);
    let live_os_name = first_nonempty(&[&value_map_str(live.get("os").unwrap_or(&json!({})), "PRETTY_NAME"), &live_os()]);
    let live_bios = value_map_str(&live_d, "bios_version");
    let live_board = value_map_str(&live_d, "board_name");
    let latest = capture_ident(events.last().unwrap());
    let alert_ev = events.iter().rev().find(|e| e.is_alert());
    let mut lines = vec![format!(
        "- Latest capture: `{}` event `{}`",
        latest["ts"], latest["event"]
    )];
    let mut diffs = Vec::new();
    if !latest["bios"].is_empty() && !live_bios.is_empty() && latest["bios"] != live_bios {
        diffs.push(format!("BIOS {} (live {live_bios})", latest["bios"]));
    }
    if !latest["kernel"].is_empty() && !live_k.is_empty() && latest["kernel"] != live_k {
        diffs.push(format!("kernel {} (live {live_k})", latest["kernel"]));
    }
    if !latest["os"].is_empty() && !live_os_name.is_empty() && latest["os"] != live_os_name {
        diffs.push(format!("OS {} (live {live_os_name})", latest["os"]));
    }
    if !latest["board"].is_empty() && !live_board.is_empty() && latest["board"] != live_board {
        diffs.push(format!("board {}", latest["board"]));
    }
    if diffs.is_empty() {
        lines.push("- Latest capture BIOS/board/OS/kernel match the live table above.".into());
    } else {
        lines.push(format!("- Latest capture differs from live: {}", diffs.join("; ")));
    }
    if let Some(alert_ev) = alert_ev {
        let alert = capture_ident(alert_ev);
        if alert["ts"] != latest["ts"] || alert["event"] != latest["event"] {
            let mut extra = Vec::new();
            if !alert["bios"].is_empty() {
                extra.push(format!("BIOS {}", alert["bios"]));
            }
            if !alert["kernel"].is_empty() {
                extra.push(format!("kernel {}", alert["kernel"]));
            }
            let suffix = if extra.is_empty() {
                String::new()
            } else {
                format!(" ({})", extra.join(", "))
            };
            lines.push(format!(
                "- Last alert: `{}` event `{}`{suffix}",
                alert["ts"], alert["event"]
            ));
        }
    }
    lines.join("\n")
}

fn dimm_report_section(
    current: &[BTreeMap<String, String>],
    healthy: &[BTreeMap<String, String>],
    corrupt: &[BTreeMap<String, String>],
) -> String {
    let mut parts = vec!["### Current".into(), String::new(), dimm_table(current)];
    if !healthy.is_empty() && !dimms_match(current, healthy) {
        parts.extend([
            String::new(),
            "### Last healthy baseline (differs from current)".into(),
            String::new(),
            dimm_table(healthy),
        ]);
    }
    if !corrupt.is_empty() && !dimms_match(current, corrupt) {
        parts.extend([
            String::new(),
            "### Last corrupt snapshot".into(),
            String::new(),
            dimm_table(corrupt),
        ]);
    } else if corrupt.is_empty() {
        parts.extend([String::new(), "_No corrupt DIMM snapshot recorded yet._".into()]);
    }
    parts.join("\n")
}

pub fn pick_dmi(events: &[TimelineEvent]) -> BTreeMap<String, String> {
    let live = live_dmi();
    if !live.is_empty() {
        return live;
    }
    for ev in events.iter().rev() {
        if !ev.dmi.is_empty() {
            return ev.dmi.clone();
        }
    }
    BTreeMap::new()
}

pub fn pick_dimms(ev: Option<&TimelineEvent>) -> Vec<BTreeMap<String, String>> {
    ev.map(|e| e.dimms.clone()).unwrap_or_default()
}

fn corrupt_dimm_lines(dimms: &[BTreeMap<String, String>]) -> Vec<String> {
    let mut lines = Vec::new();
    for d in dimms {
        let flags = dimm_flags(d);
        if flags.is_empty() {
            continue;
        }
        lines.push(format!(
            "  {}: part {}, width {}/{} ({})",
            d.get("locator").map(String::as_str).unwrap_or("?"),
            d.get("part").map(|s| if s.is_empty() { "empty" } else { s }).unwrap_or("empty"),
            d.get("total_width").map(String::as_str).unwrap_or("?"),
            d.get("data_width").map(String::as_str).unwrap_or("?"),
            flags.join(", "),
        ));
    }
    lines
}

pub fn spd_now_from_state(state_dir: &Path, events: &[TimelineEvent]) -> (String, Vec<String>) {
    let latest = events.last();
    let dimms = pick_dimms(latest);
    let mut flag_list = Vec::new();
    if let Some(latest) = latest {
        flag_list = latest
            .flags
            .split(',')
            .filter(|f| !f.is_empty())
            .map(|s| s.to_string())
            .collect();
        if flag_list.is_empty() {
            for d in &dimms {
                flag_list.extend(dimm_flags(d));
            }
        }
        if latest.hub.get("stuck").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false)
            && !flag_list.iter().any(|f| f == "hub_mr11_stuck")
        {
            flag_list.push("hub_mr11_stuck".into());
        }
    }
    let spd_file = fs::read_to_string(state_dir.join("SPD_NOW"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !flag_list.is_empty() || spd_file == "corrupt" || latest.map(|e| e.is_alert()).unwrap_or(false) {
        let details = corrupt_dimm_lines(&dimms);
        let details = if details.is_empty() {
            vec![format!("  flags: {}", if flag_list.is_empty() { "alert".into() } else { flag_list.join(", ") })]
        } else {
            details
        };
        return ("corrupted".into(), details);
    }
    if dimms.is_empty() && events.is_empty() {
        return (
            "unknown".into(),
            vec!["  No DIMM snapshot yet. Run: am5-spd-diag snapshot".into()],
        );
    }
    if dimms.is_empty() {
        return (
            "unknown".into(),
            vec!["  Latest capture has no DIMM summary (SMBIOS/dmidecode missing?).".into()],
        );
    }
    ("healthy".into(), Vec::new())
}

fn dimm_change_lines(before: &[BTreeMap<String, String>], after: &[BTreeMap<String, String>]) -> Vec<String> {
    let by_b: BTreeMap<_, _> = before
        .iter()
        .map(|d| (d.get("locator").cloned().unwrap_or_else(|| "?".into()), d.clone()))
        .collect();
    let by_a: BTreeMap<_, _> = after
        .iter()
        .map(|d| (d.get("locator").cloned().unwrap_or_else(|| "?".into()), d.clone()))
        .collect();
    let mut locs: Vec<_> = by_b.keys().cloned().collect();
    for k in by_a.keys() {
        if !locs.contains(k) {
            locs.push(k.clone());
        }
    }
    let mut lines = Vec::new();
    for loc in locs {
        let b = by_b.get(&loc);
        let a = by_a.get(&loc);
        match (b, a) {
            (None, _) => lines.push(format!("- {loc}: missing from before")),
            (_, None) => lines.push(format!("- {loc}: missing from after")),
            (Some(b), Some(a)) if dimm_key(b) == dimm_key(a) => {
                lines.push(format!(
                    "- {loc}: unchanged ({} {})",
                    b.get("size").map(String::as_str).unwrap_or("?"),
                    b.get("part").map(|s| if s.is_empty() { "empty" } else { s }).unwrap_or("empty")
                ));
            }
            (Some(b), Some(a)) => lines.push(format!(
                "- {loc}: {} {} {} → {} {} {}",
                b.get("size").map(String::as_str).unwrap_or("?"),
                b.get("part").map(|s| if s.is_empty() { "empty" } else { s }).unwrap_or("empty"),
                b.get("total_width").map(String::as_str).unwrap_or("?"),
                a.get("size").map(String::as_str).unwrap_or("?"),
                a.get("part").map(|s| if s.is_empty() { "empty" } else { s }).unwrap_or("empty"),
                a.get("total_width").map(String::as_str).unwrap_or("?"),
            )),
        }
    }
    lines
}

pub fn render_transitions(transitions: &[Transition]) -> String {
    if transitions.is_empty() {
        return "_No healthy-to-corrupt transition captured yet. Keep the monitor enabled through sleep and a warm reboot._".into();
    }
    let mut blocks = Vec::new();
    for (idx, tr) in transitions.iter().enumerate() {
        let alert_ev = &tr.alert_event;
        let healthy_kb = tr.prev_healthy.as_ref().map(|h| h.memtotal_kb_i64()).unwrap_or(0);
        let alert_kb = alert_ev.memtotal_kb_i64();
        let mut kind = tr.boot_kind.clone();
        if kind == "unexpected_power_loss" {
            kind = "unexpected_power_loss (no reboot/poweroff capture; crash, reset, or power interruption — not a full AC-cord pull if 5VSB/DIMM RGB stayed up)".into();
        } else if kind == "unknown" && alert_ev.event == "boot" {
            kind = "unknown (no reboot/poweroff capture)".into();
        }
        blocks.push(format!("### Transition {}", idx + 1));
        blocks.push(String::new());
        let hts = tr.prev_healthy.as_ref().map(|h| h.ts.as_str()).unwrap_or("unknown");
        let hev = tr.prev_healthy.as_ref().map(|h| h.event.as_str()).unwrap_or("unknown");
        blocks.push(format!(
            "- Last healthy: `{hts}` event `{hev}` firmware published {healthy_kb} kB ({})",
            kb_to_gib(healthy_kb)
        ));
        blocks.push(format!("- Sleep cycles on the previous boot: **{}**", tr.sleep_count));
        if !tr.mem_sleep.is_empty() {
            blocks.push(format!("- Sleep mode on last suspend-pre: `{}`", tr.mem_sleep));
        }
        blocks.push(format!("- How this boot started: **{kind}**"));
        if !tr.stuck_hubs.is_empty() {
            blocks.push(format!(
                "- Stuck SPD5118 hub(s): `{}` (MR11=0x08)",
                tr.stuck_hubs.join(", ")
            ));
        }
        blocks.push(format!(
            "- Corrupt snapshot: `{}` event `{}` firmware published {alert_kb} kB ({}) flags `{}`",
            alert_ev.ts,
            alert_ev.event,
            kb_to_gib(alert_kb),
            alert_ev.flags
        ));
        let changes = dimm_change_lines(
            tr.prev_healthy.as_ref().map(|h| h.dimms.as_slice()).unwrap_or(&[]),
            &alert_ev.dimms,
        );
        if !changes.is_empty() {
            blocks.push("- Identity change:".into());
            blocks.extend(changes.into_iter().map(|l| format!("  {l}")));
        }
        blocks.push(String::new());
        blocks.push("**Event sequence**".into());
        blocks.push(String::new());
        blocks.push("| Time | Event | State | MemTotal kB | Flags |".into());
        blocks.push("|---|---|---|---|---|".into());
        for ev in &tr.chain {
            blocks.push(event_row(ev));
        }
        blocks.push(String::new());
    }
    blocks.join("\n").trim_end().to_string()
}

pub fn render_boot_timeline(boots: &[(String, Vec<TimelineEvent>)]) -> String {
    if boots.is_empty() {
        return "_No events recorded yet._".into();
    }
    let mut blocks = Vec::new();
    for (i, (bid, evs)) in boots.iter().enumerate() {
        let first = &evs[0];
        let last = evs.last().unwrap();
        let alerted = evs.iter().any(|e| e.is_alert());
        let boot_ev = evs.iter().find(|e| e.event == "boot");
        let mut kind = boot_ev.map(|e| e.boot_kind.clone()).unwrap_or_default();
        if kind.is_empty() || kind == "unknown" || kind == "same_boot" {
            if i == 0 {
                if kind.is_empty() {
                    kind = "unknown".into();
                }
            } else {
                let prev_last = boots[i - 1].1.last().unwrap();
                kind = boot_kind_from_previous_event(&prev_last.event);
            }
        }
        let started = if !kind.is_empty() && kind != "unknown" && kind != "same_boot" {
            kind
        } else {
            "—".into()
        };
        let short = if bid.len() >= 8 { &bid[..8] } else { bid };
        blocks.push(format!(
            "| `{short}…` | {started} | {} | {} | {} | `{}` | `{}` | {} |",
            evs.len(),
            sleep_cycles(evs),
            if alerted { "corrupted" } else { "healthy" },
            first.event,
            last.event,
            last.memtotal_kb_i64()
        ));
    }
    let mut out = vec![
        "| Boot | Started | Events | Sleeps | SPD | First | Last | Firmware kB |".into(),
        "|---|---|---|---|---|---|---|---|".into(),
    ];
    out.extend(blocks);
    out.join("\n")
}

pub fn hub_section(events: &[TimelineEvent], transitions: &[Transition]) -> String {
    let mut evidence = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |item: String| {
        if !item.is_empty() && seen.insert(item.clone()) {
            evidence.push(item);
        }
    };
    let mut ordered = Vec::new();
    for ev in events.iter().rev() {
        if ev.is_alert() || ev.event == "recover" {
            ordered.push(ev);
        }
    }
    if let Some(latest) = events.last() {
        if !ordered.iter().any(|e| std::ptr::eq(*e, latest)) {
            ordered.push(latest);
        }
    }
    for ev in ordered {
        let hub = &ev.hub;
        let ts = ev.ts.as_str();
        let event = ev.event.as_str();
        let label = if ts.is_empty() {
            "capture".into()
        } else {
            format!("`{ts}` `{event}`")
        };
        if ev.event == "recover" {
            add(format!(
                "{label}: in-band MR11 fix ok={} reason=`{}`",
                ev.recover.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                ev.recover.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown"),
            ));
        }
        if let Some(stuck) = hub.get("dmesg_stuck").and_then(|v| v.as_array()) {
            if !stuck.is_empty() {
                let ids: Vec<_> = stuck.iter().filter_map(|v| v.as_str()).collect();
                add(format!(
                    "{label}: kernel spd5118 unbound at {} (16-bit addressing refused)",
                    ids.join(", ")
                ));
            }
        }
        let mut quoted_page0 = false;
        if let Some(rows) = hub.get("stuck").and_then(|v| v.as_array()) {
            for row in rows {
                add(format!(
                    "{label}: MR11={} on {} ({})",
                    row.get("mr11_hex").and_then(|v| v.as_str()).unwrap_or(""),
                    row.get("sysfs").and_then(|v| v.as_str()).unwrap_or(""),
                    row.get("adapter").and_then(|v| v.as_str()).unwrap_or("i2c"),
                ));
                let head = row.get("spd_page0_head").and_then(|v| v.as_str()).unwrap_or("");
                if !head.is_empty() {
                    let mut spaced = Vec::new();
                    let take = head.len().min(32);
                    let mut i = 0;
                    while i < take {
                        spaced.push(head[i..(i + 2).min(take)].to_string());
                        i += 2;
                    }
                    add(format!(
                        "{label}: SPD page-0 window (not full EEPROM) first 16 bytes: `{}`",
                        spaced.join(" ")
                    ));
                    quoted_page0 = true;
                }
            }
        }
        if let Some(rows) = hub.get("hubs").and_then(|v| v.as_array()) {
            for row in rows {
                if row.get("stuck").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                let mr11 = row.get("mr11_hex").and_then(|v| v.as_str()).unwrap_or("");
                if mr11.is_empty() {
                    continue;
                }
                add(format!(
                    "{label}: MR11={} on {} ({})",
                    mr11,
                    row.get("sysfs").and_then(|v| v.as_str()).unwrap_or(""),
                    row.get("adapter").and_then(|v| v.as_str()).unwrap_or("i2c"),
                ));
            }
        }
        let hubs_empty = hub
            .get("hubs")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);
        let stuck_empty = hub
            .get("stuck")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if hubs_empty && stuck_empty {
            if let Some(attempts) = hub.get("attempts").and_then(|v| v.as_array()) {
                for row in attempts {
                    if row.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        continue;
                    }
                    let errno = row.get("errno_name").and_then(|v| v.as_str()).unwrap_or("");
                    if errno == "ENXIO" {
                        continue;
                    }
                    let err = if errno.is_empty() {
                        row.get("error").and_then(|v| v.as_str()).unwrap_or("failed")
                    } else {
                        errno
                    };
                    if err == "failed" {
                        continue;
                    }
                    add(format!(
                        "{label}: probe {} failed ({err})",
                        row.get("sysfs").and_then(|v| v.as_str()).unwrap_or("i2c"),
                    ));
                }
            }
        }
        let lines: Vec<_> = ev.dmesg_spd.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        for ln in lines.iter().take(3) {
            add(format!("{label} dmesg: `{ln}`"));
        }
        if !quoted_page0 {
            for page in &ev.spd_page0 {
                for ln in page.text.lines() {
                    if ln.starts_with("0000:") {
                        add(format!("{label} {}: `{ln}`", page.name));
                        break;
                    }
                }
            }
        }
    }
    for tr in transitions {
        for d in &tr.bad_dimms {
            if dimm_flags(d).iter().any(|f| f == "ghost_page0") {
                add(format!(
                    "ghost page-0 serial `{}` on {} (empty part)",
                    d.get("serial").map(String::as_str).unwrap_or(""),
                    d.get("locator").map(String::as_str).unwrap_or("")
                ));
            }
        }
    }
    if evidence.is_empty() {
        return "No SPD5118 hub probe evidence yet. Capture as root (or via the polkit snapshot helper) so `/dev/i2c-*` can be read, or look for `spd5118: Adapter does not support 16-bit register addresses` in dmesg.".into();
    }
    let mut lines = vec!["Evidence from this machine (corrupt captures kept even if identity later restored):".into()];
    lines.extend(evidence.into_iter().map(|item| format!("- {item}")));
    lines.join("\n")
}

pub fn current_state(state_dir: &Path, events: &[TimelineEvent], baseline: &Baseline) -> String {
    let (now, _) = spd_now_from_state(state_dir, events);
    let live_kb = memtotal_kb();
    let base_kb = value_as_i64(&baseline.memtotal_kb);
    let seen = if events.iter().any(|e| e.is_alert()) { "yes" } else { "no" };
    let mut lines = vec![format!("- **SPD now: {now}**")];
    if live_kb != 0 && base_kb != 0 && live_kb == base_kb {
        lines.push(format!(
            "- Firmware published RAM: **{live_kb} kB** ({} ; matches healthy baseline)",
            kb_to_gib(live_kb)
        ));
    } else {
        let mut live_line = format!(
            "- Firmware published RAM: **{live_kb} kB** ({})",
            kb_to_gib(live_kb)
        );
        if base_kb != 0 {
            live_line.push_str(&format!(
                "; last healthy baseline **{base_kb} kB** ({})",
                kb_to_gib(base_kb)
            ));
        }
        lines.push(live_line);
    }
    let sleep_total = events
        .iter()
        .filter(|e| e.event == "suspend-pre" || e.event == "hibernate-pre")
        .count();
    lines.push(format!(
        "- Suspends: kernel **{}** this boot; package recorded **{sleep_total}**",
        kernel_suspend_success()
    ));
    lines.push(format!("- Corruption seen in earlier captures: **{seen}**"));
    if now == "healthy" && seen == "yes" {
        let recover_lines = recover_status_lines(events, &now);
        if !recover_lines.iter().any(|l| l.contains("in-band MR11 clear")) {
            lines.push("- Identity looks restored since the last alert (AC power loss or `am5-spd-diag fix` + reboot).".into());
        }
        lines.extend(recover_lines);
    } else {
        lines.extend(recover_status_lines(events, &now));
    }
    let boot_kind = latest_boot_start_kind(events);
    if boot_kind == "unexpected_power_loss" {
        lines.push("- This boot followed an **unexpected power loss** (previous boot had no reboot/poweroff capture: crash, reset, or power interruption). That is not a clean ACPI shutdown and is not the same as pulling the AC cord: 5VSB/VDDSPD may stay up (DIMM RGB often stays lit).".into());
    } else if boot_kind == "shutdown_poweroff" {
        lines.push("- This boot followed a captured ACPI poweroff/shutdown.".into());
    } else if boot_kind == "warm_reboot" {
        lines.push("- This boot followed a captured warm reboot.".into());
    }
    lines.push(format!("- Timeline events: **{}**", events.len()));
    let notice = state_dir.join("NOTICE");
    if notice.is_file() && now == "corrupted" {
        if let Ok(text) = fs::read_to_string(&notice) {
            lines.push(format!("- Notice: {}", text.trim()));
        }
    }
    lines.join("\n")
}

pub fn fill_template(template: &str, mapping: &BTreeMap<String, String>) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\{\{([A-Z0-9_]+)\}\}").unwrap());
    re.replace_all(template, |caps: &regex::Captures| {
        mapping
            .get(&caps[1])
            .cloned()
            .unwrap_or_else(|| caps[0].to_string())
    })
    .into_owned()
}

fn systemctl_state(verb: &str, name: &str) -> String {
    let out = Command::new("systemctl")
        .args([verb, name])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else {
        return "unknown".into();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().unwrap_or("unknown").trim().to_string()
}

pub fn unit_line(name: &str) -> String {
    let enabled = systemctl_state("is-enabled", name);
    let active = systemctl_state("is-active", name);
    let mut note = String::new();
    if name.ends_with("-pre-sleep.service") || name.ends_with("-post-sleep.service") {
        if enabled == "enabled" && active == "inactive" {
            note = " (oneshot; idle until sleep)".into();
        }
    }
    format!("{name}: {active}, {enabled}{note}")
}

pub fn kernel_suspend_success() -> String {
    fs::read_to_string("/sys/power/suspend_stats/success")
        .map(|s| {
            let t = s.trim();
            if t.is_empty() {
                "unknown".into()
            } else {
                t.to_string()
            }
        })
        .unwrap_or_else(|_| "unknown".into())
}

pub fn build_context(_cfg: &Config, events: &[TimelineEvent], state_dir: &Path) -> Context {
    let boots = group_boots(events);
    let transitions = find_transitions(events);
    let dmi = pick_dmi(events);
    let last_alert = events.iter().rev().find(|e| e.is_alert()).cloned();
    let last_healthy = events.iter().rev().find(|e| !e.is_alert()).cloned();
    let latest = events.last().cloned();
    let baseline = load_baseline(state_dir);
    let (now, _) = spd_now_from_state(state_dir, events);
    let sleep_total: usize = boots.iter().map(|(_, v)| sleep_cycles(v)).sum();
    let alert_count = events.iter().filter(|e| e.is_alert()).count();
    let pattern = render_pattern(events, &boots, &transitions);
    Context {
        boots,
        transitions,
        dmi,
        last_alert,
        last_healthy,
        latest,
        baseline,
        sleep_total,
        alert_count,
        spd_now: now,
        pattern,
        state_dir: state_dir.to_path_buf(),
    }
}

fn report_title(dmi: &BTreeMap<String, String>, now: &str, alert_count: usize) -> String {
    let board = dmi.get("board_name").map(String::as_str).unwrap_or("AM5 board");
    let bios = dmi.get("bios_version").map(String::as_str).unwrap_or("unknown BIOS");
    if alert_count == 0 {
        format!("{board} BIOS {bios}: AM5 SPD identity monitor (no corruption recorded)")
    } else if now == "corrupted" {
        format!("{board} BIOS {bios}: DIMM identity lost after sleep + warm reboot")
    } else {
        format!("{board} BIOS {bios}: DIMM identity was corrupted after sleep + warm reboot (now restored)")
    }
}

fn dimm_expect_bullets(dimms: &[BTreeMap<String, String>]) -> String {
    if dimms.is_empty() {
        return "  - no DIMM snapshot".into();
    }
    dimms
        .iter()
        .map(|d| {
            let loc = d.get("locator").map(String::as_str).unwrap_or("?");
            let size = d.get("size").map(String::as_str).unwrap_or("?");
            let tw = d.get("total_width").map(String::as_str).unwrap_or("?");
            let dw = d.get("data_width").map(String::as_str).unwrap_or("?");
            let mfr = d.get("manufacturer").map(|s| if s.is_empty() { "Unknown" } else { s.as_str() }).unwrap_or("Unknown");
            let part = d.get("part").map(|s| if s.is_empty() { "empty" } else { s.as_str() }).unwrap_or("empty");
            let speed = d.get("speed").map(String::as_str).unwrap_or("");
            let serial = d.get("serial").map(String::as_str).unwrap_or("?");
            format!("  - **{loc}:** {size} · {tw}/{dw} · {mfr} {part} · {speed} · serial {serial}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expected_actual_section(
    now: &str,
    baseline: &Baseline,
    current_dimms: &[BTreeMap<String, String>],
    healthy_dimms: &[BTreeMap<String, String>],
    corrupt_dimms: &[BTreeMap<String, String>],
    last_alert: Option<&TimelineEvent>,
    live_kb: i64,
) -> String {
    let base_kb = value_as_i64(&baseline.memtotal_kb);
    let alert_kb = last_alert.map(|e| e.memtotal_kb_i64()).unwrap_or(0);
    let expected_dimms = if !healthy_dimms.is_empty() {
        healthy_dimms
    } else {
        &baseline.dimms
    };
    let actual_dimms = if now == "corrupted" {
        current_dimms
    } else if !corrupt_dimms.is_empty() {
        corrupt_dimms
    } else {
        current_dimms
    };
    let slot = slot_map_line(if expected_dimms.is_empty() { current_dimms } else { expected_dimms });
    format!(
        "- **Slot map:** {slot}\n- **Expected** firmware MemTotal {base_kb} kB ({}):\n{}\n- **Actual** firmware MemTotal {} kB ({}) (live now {live_kb} kB / {}):\n{}\n- **Impact:** firmware published placeholder DIMM identity and a smaller memory map. Linux is reflecting SMBIOS/e820 from UEFI, not dropping RAM in the MM layer.",
        kb_to_gib(base_kb),
        dimm_expect_bullets(expected_dimms),
        if alert_kb != 0 { alert_kb } else { live_kb },
        kb_to_gib(if alert_kb != 0 { alert_kb } else { live_kb }),
        kb_to_gib(live_kb),
        dimm_expect_bullets(actual_dimms),
    )
}

fn e820_display_line(ln: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^(?:\[[^\]]+\]\s*)+").unwrap());
    re.replace(ln.trim(), "").trim().to_string()
}

fn e820_lines(ev: Option<&TimelineEvent>) -> Vec<String> {
    let Some(ev) = ev else {
        return Vec::new();
    };
    let mut text = ev.e820.clone();
    if text.trim().is_empty() && !ev.dir.is_empty() {
        let path = Path::new(&ev.dir).join("dmesg-filtered.txt");
        if let Ok(raw) = fs::read_to_string(&path) {
            text = raw
                .lines()
                .filter(|ln| ln.contains("BIOS-e820"))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    text.lines()
        .filter(|ln| !ln.trim().is_empty() && !ln.starts_with('#'))
        .map(e820_display_line)
        .collect()
}

fn e820_high_end(lines: &[String]) -> String {
    let ram: Vec<_> = lines.iter().filter(|ln| ln.contains("System RAM")).collect();
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\[mem 0x[0-9a-f]+-(0x[0-9a-f]+)\]").unwrap());
    for ln in ram.iter().rev() {
        if let Some(c) = re.captures(ln) {
            return c.get(1).unwrap().as_str().to_ascii_lowercase();
        }
    }
    String::new()
}

pub fn e820_section(
    last_alert: Option<&TimelineEvent>,
    last_healthy: Option<&TimelineEvent>,
    baseline: &Baseline,
) -> String {
    let healthy_lines = e820_lines(last_healthy);
    let alert_lines = e820_lines(last_alert);
    if healthy_lines.is_empty() && alert_lines.is_empty() {
        return "_No e820 map captured. Privileged snapshot records the full BIOS-e820 table at boot/manual/alert._".into();
    }
    let mut blocks = vec![
        "Full firmware e820 table (all types, not only System RAM). Kernel dmesg timestamps are omitted.".into(),
        String::new(),
    ];
    if !healthy_lines.is_empty() && !alert_lines.is_empty() && healthy_lines != alert_lines {
        let healthy_end = e820_high_end(&healthy_lines);
        let alert_end = e820_high_end(&alert_lines);
        if !healthy_end.is_empty() && !alert_end.is_empty() && healthy_end != alert_end {
            blocks.push(format!(
                "System RAM high range differs: healthy ends at `{healthy_end}`, corrupt ends at `{alert_end}`."
            ));
            blocks.push(String::new());
        } else {
            blocks.push("Healthy and corrupt e820 tables differ (see full lists below).".into());
            blocks.push(String::new());
        }
    } else if !healthy_lines.is_empty() && !alert_lines.is_empty() {
        blocks.push("Healthy and corrupt e820 tables match. Identity corruption on this boot did not change the firmware memory map (typical until the next POST/warm reboot).".into());
        blocks.push(String::new());
    }
    let base_kb = value_as_i64(&baseline.memtotal_kb);
    if let Some(h) = last_healthy {
        blocks.push(format!(
            "Healthy `{}` MemTotal {} kB (baseline {base_kb} kB / {}):",
            h.ts,
            h.memtotal_kb_i64(),
            kb_to_gib(base_kb)
        ));
        push_e820_listing(&mut blocks, &healthy_lines);
    }
    if let Some(a) = last_alert {
        if last_healthy.is_some() {
            blocks.push(String::new());
        }
        blocks.push(format!(
            "Corrupt `{}` MemTotal {} kB ({}):",
            a.ts,
            a.memtotal_kb_i64(),
            kb_to_gib(a.memtotal_kb_i64())
        ));
        push_e820_listing(&mut blocks, &alert_lines);
    }
    blocks.join("\n")
}

fn push_e820_listing(blocks: &mut Vec<String>, lines: &[String]) {
    blocks.push(String::new());
    blocks.push("```".into());
    if lines.is_empty() {
        blocks.push("no e820 lines".into());
    } else {
        blocks.extend(lines.iter().cloned());
    }
    blocks.push("```".into());
}

fn attachment_checklist(
    last_alert: Option<&TimelineEvent>,
    latest: Option<&TimelineEvent>,
    state_dir: &Path,
) -> String {
    let ev = last_alert.or(latest);
    let directory = PathBuf::from(ev.map(|e| e.dir.as_str()).unwrap_or(""));
    let names = [
        "hub.json",
        "dmidecode-memory.txt",
        "dimm-summary.txt",
        "e820.txt",
        "e820-system-ram.txt",
        "dmesg-spd5118.txt",
        "dmesg-filtered.txt",
        "dmi-sysfs.txt",
        "system.json",
    ];
    let mut lines = vec![
        format!(
            "Capture logs: `{}` (timeline.jsonl, ALERTS.log, baseline.json, events/).",
            state_dir.display()
        ),
        "`am5-spd-diag package` builds a tarball of the same.".into(),
        String::new(),
        "Files in the last corrupt (or latest) event directory:".into(),
    ];
    if !directory.is_dir() {
        lines.push("- _event directory missing_".into());
        return lines.join("\n");
    }
    for name in names {
        let path = directory.join(name);
        let mark = if path.is_file() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            "present"
        } else {
            "missing"
        };
        lines.push(format!("- `{name}`: {mark}"));
    }
    let mut pages = Vec::new();
    if let Ok(rd) = fs::read_dir(&directory) {
        pages = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("spd-page0-") && n.ends_with(".txt"))
                    .unwrap_or(false)
            })
            .collect();
        pages.sort();
    }
    if pages.is_empty() {
        lines.push("- `spd-page0-*.txt`: missing".into());
    } else {
        for path in pages {
            lines.push(format!("- `{}`: present", path.file_name().unwrap().to_string_lossy()));
        }
    }
    lines.join("\n")
}

pub fn mapping_from_context(cfg: &Config, events: &[TimelineEvent], ctx: &Context) -> BTreeMap<String, String> {
    let live_kb = memtotal_kb();
    let now = ctx.spd_now.as_str();
    let recovs = recover_events(events);
    let this_boot_id = events
        .iter()
        .rev()
        .find(|e| e.event == "boot")
        .map(|e| e.boot_id.as_str())
        .unwrap_or("");
    let recover_pending = recovs.iter().any(|e| {
        recover_cleared(e) && (this_boot_id.is_empty() || e.boot_id == this_boot_id)
    });
    let summary = if ctx.alert_count == 0 {
        "Monitor is installed. No SPD identity corruption has been recorded yet. Use the system normally (sleep/wake, then reboot) and re-run `am5-spd-diag report`.".into()
    } else if now == "corrupted" {
        if recover_pending {
            "SPD identity is **currently corrupted**: firmware is still publishing placeholder DIMM fields. An in-band MR11 clear was applied this boot; a **warm reboot** is required before firmware re-reads SPD. Sleep/wake is not enough.".into()
        } else {
            "SPD identity is **currently corrupted**: firmware is publishing placeholder DIMM fields (Unknown/missing part and/or 8-bit width). Linux MemTotal is whatever firmware advertised, not a separate Linux bug. Warm reboot does not clear SPD5118 MR11 standby state; AC power loss does. Optional in-band fix: `am5-spd-diag fix` then reboot.".into()
        }
    } else if recover_before_latest_boot(events).is_some_and(recover_cleared) {
        "SPD identity looks **healthy now** after an in-band MR11 clear and warm reboot. See the last corrupt DIMM snapshot below.".into()
    } else {
        "SPD identity looks **healthy now**, but earlier captures recorded corruption. See the last corrupt DIMM snapshot below. Restore is AC power loss or `am5-spd-diag fix` + reboot.".into()
    };
    let healthy_dimms = if !ctx.baseline.dimms.is_empty() {
        ctx.baseline.dimms.clone()
    } else {
        pick_dimms(ctx.last_healthy.as_ref())
    };
    let current_dimms = pick_dimms(ctx.latest.as_ref().or(ctx.last_alert.as_ref()).or(ctx.last_healthy.as_ref()));
    let corrupt_dimms = pick_dimms(ctx.last_alert.as_ref());
    let system = collect_system_info();
    let attach = attachment_checklist(ctx.last_alert.as_ref(), ctx.latest.as_ref(), &ctx.state_dir);
    let repro = format!(
        "{}\n\nSuggested loop: known-healthy boot → N suspend/resume cycles → warm reboot → inspect `am5-spd-diag status`.",
        ctx.pattern
    );
    let title = report_title(&ctx.dmi, now, ctx.alert_count);
    BTreeMap::from([
        ("GENERATED_AT".into(), iso_now()),
        ("REPORT_TITLE".into(), title),
        ("SUMMARY".into(), summary),
        (
            "EXPECTED_ACTUAL".into(),
            expected_actual_section(
                now,
                &ctx.baseline,
                &current_dimms,
                &healthy_dimms,
                &corrupt_dimms,
                ctx.last_alert.as_ref(),
                live_kb,
            ),
        ),
        (
            "HARDWARE_TABLE".into(),
            hardware_table(cfg, &ctx.dmi, &live_cpu(), &live_os(), &live_kernel(), &ctx.baseline, &system),
        ),
        ("SYSTEM_CAPTURED".into(), captured_system_table(events, &system)),
        (
            "BIOS_VERSION".into(),
            first_nonempty(&[
                ctx.dmi.get("bios_version").map(String::as_str).unwrap_or(""),
                cfg.get("FALLBACK_BIOS"),
                "unknown",
            ]),
        ),
        (
            "CURRENT_STATE".into(),
            current_state(&ctx.state_dir, events, &ctx.baseline),
        ),
        ("SPD_NOW".into(), now.into()),
        ("PATTERN".into(), ctx.pattern.clone()),
        ("HUB_SECTION".into(), hub_section(events, &ctx.transitions)),
        (
            "E820_SECTION".into(),
            e820_section(ctx.last_alert.as_ref(), ctx.last_healthy.as_ref(), &ctx.baseline),
        ),
        (
            "DIMM_SECTION".into(),
            dimm_report_section(&current_dimms, &healthy_dimms, &corrupt_dimms),
        ),
        ("DIMM_TABLE_CURRENT".into(), dimm_table(&current_dimms)),
        ("DIMM_TABLE_HEALTHY".into(), dimm_table(&healthy_dimms)),
        ("DIMM_TABLE_CORRUPT".into(), dimm_table(&corrupt_dimms)),
        ("TRANSITIONS".into(), render_transitions(&ctx.transitions)),
        ("BOOT_TIMELINE".into(), render_boot_timeline(&ctx.boots)),
        ("ALERT_COUNT".into(), ctx.alert_count.to_string()),
        ("EVENT_COUNT".into(), events.len().to_string()),
        ("SLEEP_CYCLES".into(), ctx.sleep_total.to_string()),
        (
            "CORRUPTION_SEEN".into(),
            if ctx.alert_count > 0 { "yes" } else { "no" }.into(),
        ),
        (
            "MEMTOTAL_CURRENT".into(),
            format!("{live_kb} kB ({})", kb_to_gib(live_kb)),
        ),
        ("MEM_SLEEP".into(), mem_sleep()),
        (
            "OS_RELEASE".into(),
            if live_os().is_empty() { "unknown".into() } else { live_os() },
        ),
        ("KERNEL".into(), live_kernel()),
        ("ATTACHMENTS".into(), attach),
        ("SEQUENCE".into(), render_transitions(&ctx.transitions)),
        ("REPRO_STEPS".into(), repro),
        ("FORUM_URL".into(), FORUM_URL.into()),
    ])
}

fn strip_md_italics(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"_([^_\n]+)_").unwrap());
    re.replace_all(text, "$1").into_owned()
}

pub fn format_analyze(events: &[TimelineEvent], ctx: &Context) -> String {
    let mut out = String::new();
    writeln!(out, "SPD now: {}", ctx.spd_now).ok();
    writeln!(out).ok();
    writeln!(out, "## System").ok();
    writeln!(out).ok();
    let sys = ctx.latest.as_ref().and_then(|e| {
        if e.system.is_null() || e.system.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            None
        } else {
            Some(&e.system)
        }
    });
    writeln!(
        out,
        "{}",
        system_identity_table(&ctx.dmi, &live_cpu(), &live_kernel(), sys)
    )
    .ok();
    writeln!(out).ok();
    writeln!(out, "## History").ok();
    writeln!(out).ok();
    let history = vec![
        ("Events", events.len().to_string()),
        ("Alerts", ctx.alert_count.to_string()),
        ("Boots", ctx.boots.len().to_string()),
        ("Recorded suspends", ctx.sleep_total.to_string()),
        (
            "Kernel suspends this boot",
            format!(
                "{} (from /sys/power/suspend_stats/success)",
                kernel_suspend_success()
            ),
        ),
        (
            "Recorded by this package",
            format!("{} suspend-pre/hibernate-pre event(s)", ctx.sleep_total),
        ),
        (
            "In-band fix events",
            recover_events(events).len().to_string(),
        ),
    ];
    writeln!(out, "{}", md_kv_table(&history)).ok();
    let recover_lines = recover_status_lines(events, &ctx.spd_now);
    if !recover_lines.is_empty() {
        writeln!(out).ok();
        writeln!(out, "## In-band fix").ok();
        writeln!(out).ok();
        for line in recover_lines {
            writeln!(out, "{line}").ok();
        }
    }
    if ctx.sleep_total == 0 {
        writeln!(out).ok();
        writeln!(
            out,
            "No sleep captures in this log yet. Pre/post-sleep units are oneshot, so inactive between sleeps is normal."
        )
        .ok();
    }
    writeln!(out).ok();
    writeln!(out, "## Reproduction pattern").ok();
    writeln!(out).ok();
    writeln!(out, "{}", ctx.pattern).ok();
    writeln!(out).ok();
    writeln!(out, "## Boots").ok();
    writeln!(out).ok();
    writeln!(out, "{}", render_boot_timeline(&ctx.boots)).ok();
    if ctx.transitions.is_empty() {
        writeln!(out).ok();
        writeln!(out, "No corruption transitions yet.").ok();
    } else {
        writeln!(out).ok();
        writeln!(out, "## Corruption events").ok();
        writeln!(out).ok();
        for (i, tr) in ctx.transitions.iter().enumerate() {
            let ev = &tr.alert_event;
            let loc = if tr.bad_dimms.is_empty() {
                "?".into()
            } else {
                tr.bad_dimms
                    .iter()
                    .map(|d| d.get("locator").map(String::as_str).unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            writeln!(
                out,
                "  {}. {} {} boot={} sleeps_before={} dimms={loc} flags={}",
                i + 1,
                ev.ts,
                ev.event,
                tr.boot_kind,
                tr.sleep_count,
                ev.flags
            )
            .ok();
        }
    }
    writeln!(out).ok();
    writeln!(out, "## SPD5118 hub").ok();
    writeln!(out).ok();
    writeln!(out, "{}", strip_md_italics(&hub_section(events, &ctx.transitions))).ok();
    writeln!(out).ok();
    writeln!(out, "For a ticket: am5-spd-diag report").ok();
    out
}

fn dimm_status_lines(dimms: &[BTreeMap<String, String>], now: &str) -> Vec<String> {
    if now == "corrupted" {
        let lines = corrupt_dimm_lines(dimms);
        if lines.is_empty() {
            vec!["  (alert; DIMM fields missing from latest capture)".into()]
        } else {
            lines
        }
    } else if dimms.is_empty() {
        vec!["  No populated DIMM snapshot yet. Run: am5-spd-diag snapshot".into()]
    } else {
        dimms
            .iter()
            .map(|d| {
                format!(
                    "  {}: {} {} {} width {}/{}",
                    d.get("locator").map(String::as_str).unwrap_or("?"),
                    d.get("size").map(String::as_str).unwrap_or("?"),
                    d.get("manufacturer").map(|s| if s.is_empty() { "Unknown" } else { s }).unwrap_or("Unknown"),
                    d.get("part").map(|s| if s.is_empty() { "empty" } else { s }).unwrap_or("empty"),
                    d.get("total_width").map(String::as_str).unwrap_or("?"),
                    d.get("data_width").map(String::as_str).unwrap_or("?"),
                )
            })
            .collect()
    }
}

pub fn format_status(events: &[TimelineEvent], ctx: &Context) -> String {
    let now = ctx.spd_now.as_str();
    let live_kb = memtotal_kb();
    let base_kb = value_as_i64(&ctx.baseline.memtotal_kb);
    let dimms = {
        let latest = pick_dimms(ctx.latest.as_ref());
        if latest.is_empty() {
            ctx.baseline.dimms.clone()
        } else {
            latest
        }
    };
    let mut out = String::new();
    writeln!(out, "SPD now: {now}").ok();
    writeln!(out, "System: {}", system_oneliner(&ctx.dmi, &live_cpu(), &live_kernel(), None)).ok();
    for line in dimm_status_lines(&dimms, now) {
        writeln!(out, " · {}", line.trim()).ok();
    }
    let mut ram = format!("Firmware published RAM: {live_kb} kB ({})", kb_to_gib(live_kb));
    if base_kb != 0 {
        ram.push_str(&format!("; healthy baseline {base_kb} kB ({})", kb_to_gib(base_kb)));
    }
    writeln!(out, "{ram}").ok();
    writeln!(out, "Sleep policy: {}", mem_sleep()).ok();
    writeln!(
        out,
        "Log: {} events, {} alert(s), {} recorded suspend(s)",
        events.len(),
        ctx.alert_count,
        ctx.sleep_total
    )
    .ok();
    for line in recover_status_lines(events, now) {
        writeln!(out, "{}", line.trim_start_matches("- ")).ok();
    }
    writeln!(out).ok();
    writeln!(out, "Monitor").ok();
    writeln!(out, "  {}", unit_line("am5-spd-diag.service")).ok();
    writeln!(out, "  {}", unit_line("am5-spd-diag-pre-sleep.service")).ok();
    writeln!(out, "  {}", unit_line("am5-spd-diag-post-sleep.service")).ok();
    let hook = Path::new("/usr/lib/systemd/system-sleep/am5-spd-diag");
    writeln!(
        out,
        "  sleep hook: {}",
        if hook.is_file() { "installed" } else { "missing" }
    )
    .ok();
    let notice = ctx.state_dir.join("NOTICE");
    if notice.is_file() && now == "corrupted" {
        writeln!(out).ok();
        if let Ok(text) = fs::read_to_string(&notice) {
            writeln!(out, "{}", text.trim()).ok();
        }
    }
    writeln!(out).ok();
    writeln!(out, "For sleep/reboot history: am5-spd-diag analyze").ok();
    out
}

pub fn print_analyze(events: &[TimelineEvent], ctx: &Context) {
    print!("{}", format_analyze(events, ctx));
}

pub fn print_status(events: &[TimelineEvent], ctx: &Context) {
    print!("{}", format_status(events, ctx));
}

fn user_artifact_dir(subdir: &str) -> PathBuf {
    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("am5-spd-diag").join(subdir);
        }
    }
    let mut home = PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/".into()));
    if let Ok(sudo_user) = env::var("SUDO_USER") {
        if sudo_user != "root" {
            if let Some(dir) = home_for_user(&sudo_user) {
                home = dir;
            }
        }
    }
    home.join(".local/share/am5-spd-diag").join(subdir)
}

fn dir_is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".am5-spd-diag-write-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn resolve_writable_dir(preferred: &Path, fallback: &Path) -> Result<PathBuf, String> {
    if dir_is_writable(preferred) {
        return Ok(preferred.to_path_buf());
    }
    if fallback != preferred && dir_is_writable(fallback) {
        return Ok(fallback.to_path_buf());
    }
    Err(format!(
        "cannot write to {} or {}",
        preferred.display(),
        fallback.display()
    ))
}

fn staging_tempdir(prefix: &str) -> Result<tempfile::TempDir, String> {
    let mut candidates = Vec::new();
    if let Ok(dir) = env::var("TMPDIR") {
        if !dir.is_empty() {
            candidates.push(PathBuf::from(dir));
        }
    }
    if let Ok(xdg) = env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            candidates.push(PathBuf::from(xdg).join("am5-spd-diag"));
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.is_empty() && home != "/" {
            candidates.push(PathBuf::from(home).join(".cache/am5-spd-diag"));
        }
    }
    candidates.push(user_artifact_dir("tmp"));
    candidates.push(env::temp_dir());
    let mut seen = HashSet::new();
    let mut last = String::from("no candidate");
    for dir in candidates {
        if !seen.insert(dir.clone()) {
            continue;
        }
        if !dir_is_writable(&dir) {
            last = format!("not writable: {}", dir.display());
            continue;
        }
        match tempfile::Builder::new().prefix(prefix).tempdir_in(&dir) {
            Ok(tmp) => return Ok(tmp),
            Err(e) => last = format!("{}: {e}", dir.display()),
        }
    }
    Err(format!("tempdir: {last}"))
}

fn home_for_user(name: &str) -> Option<PathBuf> {
    let c = std::ffi::CString::new(name).ok()?;
    let pwd = unsafe { libc::getpwnam(c.as_ptr()) };
    if pwd.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr((*pwd).pw_dir) };
    Some(PathBuf::from(dir.to_string_lossy().as_ref()))
}

pub fn render_report(prefix: &Path, events: &[TimelineEvent], ctx: &Context, cfg: &Config) -> String {
    let share = env::var("AM5_SPD_DIAG_SHARE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| prefix.to_path_buf());
    let template_path = share.join("templates/ticket.md.tmpl");
    let template = fs::read_to_string(&template_path).unwrap_or_default();
    fill_template(&template, &mapping_from_context(cfg, events, ctx))
}

pub fn write_report(
    prefix: &Path,
    state_dir: &Path,
    cfg: &Config,
    events: &[TimelineEvent],
    ctx: &Context,
    out: Option<PathBuf>,
) -> PathBuf {
    let text = render_report(prefix, events, ctx, cfg);
    let mut out = out.unwrap_or_else(|| {
        let reports = env::var("AM5_SPD_DIAG_REPORT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(cfg.get("STATE_DIR")).join("reports"));
        let _ = fs::create_dir_all(&reports);
        reports.join(format!("report-{}.md", utc_stamp()))
    });
    if let Some(parent) = out.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&out, &text).is_err() {
        let fallback = user_artifact_dir("reports");
        let _ = fs::create_dir_all(&fallback);
        out = fallback.join(format!("report-{}.md", utc_stamp()));
        let _ = fs::write(&out, &text);
    }
    let _ = state_dir;
    out
}

fn chain_dirs(events: &[TimelineEvent], ctx: &Context, include_all: bool) -> Vec<PathBuf> {
    if include_all {
        return events
            .iter()
            .map(|ev| PathBuf::from(&ev.dir))
            .filter(|p| p.is_dir())
            .collect();
    }
    let mut wanted = HashSet::new();
    let mut boots_needed = HashSet::new();
    let boot_ids: Vec<_> = ctx.boots.iter().map(|(id, _)| id.clone()).collect();
    for tr in &ctx.transitions {
        for ev in &tr.chain {
            if !ev.dir.is_empty() {
                wanted.insert(ev.dir.clone());
            }
            if !ev.boot_id.is_empty() {
                boots_needed.insert(ev.boot_id.clone());
            }
        }
    }
    for ev in events {
        if ev.is_alert() {
            wanted.insert(ev.dir.clone());
            let bid = ev.boot_id.clone();
            boots_needed.insert(bid.clone());
            if let Some(idx) = boot_ids.iter().position(|id| id == &bid) {
                if idx > 0 {
                    boots_needed.insert(boot_ids[idx - 1].clone());
                }
            }
        }
    }
    for bid in &boots_needed {
        if let Some((_, evs)) = ctx.boots.iter().find(|(id, _)| id == bid) {
            for ev in evs {
                if !ev.dir.is_empty() {
                    wanted.insert(ev.dir.clone());
                }
            }
        }
    }
    if wanted.is_empty() {
        for ev in events.iter().rev().take(12) {
            if !ev.dir.is_empty() {
                wanted.insert(ev.dir.clone());
            }
        }
    }
    let mut dirs: Vec<_> = wanted
        .into_iter()
        .filter(|p| !p.is_empty() && Path::new(p).is_dir())
        .map(PathBuf::from)
        .collect();
    dirs.sort();
    dirs
}

pub fn make_package(
    prefix: &Path,
    state_dir: &Path,
    cfg: &Config,
    events: &[TimelineEvent],
    ctx: &Context,
    package_dir: &Path,
    include_all: bool,
) -> Result<PathBuf, String> {
    let package_dir = resolve_writable_dir(package_dir, &user_artifact_dir("packages"))?;
    let stamp = utc_stamp();
    let name = format!("am5-spd-diag-{stamp}");
    let tar_path = package_dir.join(format!("{name}.tar.gz"));
    let tmp = staging_tempdir("am5-spd-diag-pkg-")?;
    let root = tmp.path().join(&name);
    fs::create_dir(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let _ = write_report(prefix, state_dir, cfg, events, ctx, Some(root.join("report.md")));
    for fname in ["timeline.jsonl", "ALERTS.log", "baseline.json", "baseline.txt"] {
        let src = state_dir.join(fname);
        if src.is_file() {
            let _ = fs::copy(&src, root.join(fname));
        }
    }
    let events_out = root.join("events");
    let _ = fs::create_dir(&events_out);
    for d in chain_dirs(events, ctx, include_all) {
        let dest = events_out.join(d.file_name().unwrap_or_default());
        if dest.exists() {
            continue;
        }
        copy_dir_all(&d, &dest);
    }
    let manifest = json!({
        "generated": iso_now(),
        "include_all": include_all,
        "event_count": events.len(),
        "alert_count": ctx.alert_count,
        "spd_now": ctx.spd_now,
        "report": "report.md",
    });
    let _ = fs::write(
        root.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap_or_default()),
    );
    let tar_file = fs::File::create(&tar_path)
        .map_err(|e| format!("create {}: {e}", tar_path.display()))?;
    let enc = flate2::write::GzEncoder::new(tar_file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.follow_symlinks(false);
    tar.append_dir_all(&name, &root)
        .map_err(|e| format!("tar {}: {e}", tar_path.display()))?;
    tar.finish()
        .map_err(|e| format!("tar {}: {e}", tar_path.display()))?;
    Ok(tar_path)
}

fn copy_dir_all(src: &Path, dest: &Path) {
    let _ = fs::create_dir_all(dest);
    let Ok(rd) = fs::read_dir(src) else { return };
    for entry in rd.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to);
        } else {
            let _ = fs::copy(&from, &to);
        }
    }
}

pub fn print_inventory() {
    println!("{}", serde_json::to_string_pretty(&collect_system_info()).unwrap_or_else(|_| "{}".into()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::repo_root;
    use serde_json::json;
    use std::fs;
    use std::path::Path;

    fn test_tempdir() -> tempfile::TempDir {
        staging_tempdir("am5-test-").expect("tmpdir")
    }

    fn ev(event: &str, boot: &str, alert: bool, ts: &str) -> TimelineEvent {
        TimelineEvent {
            event: event.into(),
            boot_id: boot.into(),
            alert: json!(alert),
            ts: ts.into(),
            memtotal_kb: json!(if alert { 17800092 } else { 32000000 }),
            mem_sleep: "s2idle [deep]".into(),
            ..Default::default()
        }
    }

    fn corrupt_dimm() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("locator".into(), "DIMMB2".into()),
            ("size".into(), "2 GiB".into()),
            ("total_width".into(), "8 bits".into()),
            ("data_width".into(), "8 bits".into()),
            ("manufacturer".into(), "Unknown".into()),
            ("part".into(), "Unknown".into()),
            ("serial".into(), "00206200".into()),
        ])
    }

    fn stuck_hub() -> Value {
        json!({
            "dmesg_stuck": ["1-0053"],
            "stuck": [{
                "mr11_hex": "0x08",
                "sysfs": "1-0053",
                "adapter": "SMBus PIIX4 adapter",
                "spd_page0_head": "230c4d08",
            }]
        })
    }

    #[test]
    fn find_transitions_resume_and_dedupe() {
        let mut events = vec![
            ev("boot", "a", false, "t0"),
            ev("suspend-pre", "a", false, "t1"),
            ev("suspend-post", "a", true, "t2"),
            ev("boot", "a", true, "t3"),
        ];
        events[1].mem_sleep = "s2idle [deep]".into();
        events[1].meta.insert("mem_sleep".into(), "s2idle [deep]".into());
        events[2].flags = "unknown_part".into();
        events[2].dimms = vec![corrupt_dimm()];
        events[2].hub = stuck_hub();
        events[3].flags = "unknown_part".into();
        events[3].dimms = vec![corrupt_dimm()];
        events[3].hub = stuck_hub();
        let trans = find_transitions(&events);
        assert_eq!(trans.len(), 1);
        assert_eq!(trans[0].alert_event.event, "suspend-post");
        assert_eq!(trans[0].mem_sleep, "s2idle [deep]");
    }

    #[test]
    fn find_transitions_new_boot() {
        let mut events = vec![
            ev("boot", "a", false, "t0"),
            ev("reboot", "a", false, "t1"),
            ev("boot", "b", true, "t2"),
        ];
        events[2].boot_kind = "warm_reboot".into();
        events[2].flags = "unknown_part".into();
        events[2].dimms = vec![corrupt_dimm()];
        events[2].hub = stuck_hub();
        let trans = find_transitions(&events);
        assert_eq!(trans.len(), 1);
        assert_eq!(trans[0].boot_kind, "warm_reboot");
    }

    #[test]
    fn hub_section_uses_alert_when_latest_healthy() {
        let mut alert = ev("boot", "b", true, "t-alert");
        alert.dimms = vec![corrupt_dimm()];
        alert.hub = stuck_hub();
        alert.dmesg_spd = "spd5118 1-0053: Adapter does not support 16-bit register addresses".into();
        let mut latest = ev("boot", "c", false, "t-healthy");
        latest.hub = json!({"stuck": [], "dmesg_stuck": []});
        let trans = find_transitions(&[alert.clone()]);
        let text = hub_section(&[alert, latest], &trans);
        assert!(text.contains("0x08"));
        assert!(text.contains("1-0053"));
        assert!(text.contains("page-0"));
    }

    #[test]
    fn slot_map() {
        let dimms = vec![
            BTreeMap::from([("locator".into(), "DIMMA2".into()), ("size".into(), "16 GiB".into())]),
            BTreeMap::from([("locator".into(), "DIMMB2".into()), ("size".into(), "16 GiB".into())]),
        ];
        assert_eq!(slot_map_line(&dimms), "2×16 GiB in DIMMA2+DIMMB2");
    }

    #[test]
    fn e820_section_points_at_high_range() {
        let mut healthy = ev("boot", "a", false, "t-healthy");
        healthy.e820 = concat!(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n",
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n",
            "BIOS-e820: [mem 0x0000000100000000-0x000000085de7ffff] System RAM\n",
        )
        .into();
        let mut alert = ev("boot", "b", true, "t-alert");
        alert.e820 = concat!(
            "BIOS-e820: [mem 0x0000000000000000-0x000000000009ffff] System RAM\n",
            "BIOS-e820: [mem 0x000000000009fc00-0x000000000009ffff] reserved\n",
            "BIOS-e820: [mem 0x0000000100000000-0x00000004dde7ffff] System RAM\n",
        )
        .into();
        let mut baseline = Baseline::default();
        baseline.memtotal_kb = json!(32250768);
        let text = e820_section(Some(&alert), Some(&healthy), &baseline);
        assert!(text.contains("0x000000085de7ffff"));
        assert!(text.contains("0x00000004dde7ffff"));
        assert!(text.contains("System RAM high range differs"));
        assert!(text.contains("reserved"));
        assert!(text.contains("Full firmware e820 table"));
    }

    #[test]
    fn unexpected_power_loss() {
        let events = vec![
            ev("boot", "a", false, "t0"),
            ev("manual", "a", false, "t1"),
            ev("boot", "b", false, "t2"),
        ];
        assert_eq!(infer_boot_kind(&events, 2), "unexpected_power_loss");
        assert_eq!(boot_kind_from_previous_event("manual"), "unexpected_power_loss");
        assert_eq!(boot_kind_from_previous_event("reboot"), "warm_reboot");
        assert_eq!(boot_kind_from_previous_event("recover"), "warm_reboot");
        assert_eq!(boot_kind_from_previous_event("poweroff"), "shutdown_poweroff");
        let text = render_boot_timeline(&group_boots(&events));
        assert!(text.contains("unexpected_power_loss"));
        let mut baseline = Baseline::default();
        baseline.memtotal_kb = json!(32000000);
        let state = current_state(Path::new("/nonexistent"), &events, &baseline);
        assert!(state.contains("unexpected power loss"));
    }

    #[test]
    fn recover_event_is_not_a_corruption_transition() {
        let mut events = vec![
            ev("boot", "a", false, "t0"),
            ev("boot", "b", true, "t1"),
            ev("recover", "b", true, "t2"),
            ev("boot", "c", false, "t3"),
        ];
        events[1].flags = "unknown_part".into();
        events[2].flags = "unknown_part".into();
        events[2].recover = json!({"ok": true, "reason": "ok", "actions": [{"cleared": true}]});
        events[2].hub = json!({"stuck": [], "hubs": [{"mr11_hex": "0x00", "sysfs": "1-0053"}]});
        assert_eq!(find_transitions(&events).len(), 1);
        assert!(recover_cleared(&events[2]));
        let lines = recover_status_lines(&events, "healthy");
        assert!(lines.iter().any(|l| l.contains("in-band MR11 clear")));
        let pending = recover_status_lines(&events[..3], "corrupted");
        assert!(pending.iter().any(|l| l.contains("warm reboot")));
    }

    #[test]
    fn fixture_status_is_corrupted() {
        let state = repo_root().join("tests/fixture");
        let events = load_timeline(&state);
        assert!(!events.is_empty());
        let cfg = crate::config::load_config(&repo_root());
        let ctx = build_context(&cfg, &events, &state);
        assert_eq!(ctx.spd_now, "corrupted");
        let status = format_status(&events, &ctx);
        assert!(status.contains("SPD now: corrupted"));
        assert!(status.contains("Monitor"));
        assert!(status.contains("System:"));
        assert!(!status.contains("Reproduction pattern"));
        let analyze = format_analyze(&events, &ctx);
        assert!(analyze.contains("SPD now: corrupted"));
        assert!(analyze.contains("| Board |"));
        assert!(analyze.contains("Reproduction pattern"));
        assert!(analyze.contains("boot=warm_reboot"));
        assert!(analyze.contains("SPD5118 hub"));
        assert!(!analyze.contains("Monitor"));
    }

    #[test]
    fn load_timeline_remaps_dirs() {
        let fixture = repo_root().join("tests/fixture");
        let src = fixture.join("events/20260817T043030.0Z-boot");
        let tmp = test_tempdir();
        let state = tmp.path();
        fs::create_dir(state.join("events")).unwrap();
        copy_dir_all(&src, &state.join("events/20260817T043030.0Z-boot"));
        fs::write(
            state.join("timeline.jsonl"),
            r#"{"ts":"t","event":"boot","boot_id":"b","memtotal_kb":17800092,"alert":true,"dir":"/nonexistent/host/20260817T043030.0Z-boot","flags":"unknown_part"}
"#,
        )
        .unwrap();
        let events = load_timeline(state);
        assert_eq!(events.len(), 1);
        assert!(Path::new(&events[0].dir).starts_with(state));
        assert!(events[0].dir_exists);
        assert!(!events[0].dimms.is_empty());
        assert!(events[0].is_alert());
    }

    #[test]
    fn load_timeline_from_package_remaps_dirs() {
        let fixture = repo_root().join("tests/fixture");
        let src = fixture.join("events/20260817T043030.0Z-boot");
        let tmp = test_tempdir();
        let root = tmp.path();
        fs::create_dir(root.join("events")).unwrap();
        copy_dir_all(&src, &root.join("events/20260817T043030.0Z-boot"));
        fs::write(
            root.join("timeline.jsonl"),
            r#"{"ts":"t","event":"boot","boot_id":"b","memtotal_kb":17800092,"alert":true,"dir":"/nonexistent/host/20260817T043030.0Z-boot","flags":"unknown_part"}
"#,
        )
        .unwrap();
        let events = load_timeline_from_package(root);
        assert_eq!(events.len(), 1);
        assert!(Path::new(&events[0].dir).starts_with(root));
        assert!(events[0].dir_exists);
        assert!(!events[0].dimms.is_empty());
        assert!(events[0].is_alert());
    }

    #[test]
    fn package_tar_roundtrip() {
        let state = repo_root().join("tests/fixture");
        let events = load_timeline(&state);
        assert!(!events.is_empty());
        let cfg = crate::config::load_config(&repo_root());
        let ctx = build_context(&cfg, &events, &state);
        let tmp = test_tempdir();
        let tar = make_package(&repo_root(), &state, &cfg, &events, &ctx, tmp.path(), true)
            .expect("package");
        assert!(tar.is_file());
        let pkg = open_package(&tar).expect("extract");
        let loaded = load_timeline_from_package(&pkg.root);
        assert_eq!(loaded.len(), events.len());
        let last = loaded.last().unwrap();
        assert!(last.is_alert());
        assert!(last.dir_exists);
        assert!(!last.dimms.is_empty());
        assert!(Path::new(&last.dir).starts_with(&pkg.root));
        let as_dir = open_package(&pkg.root).expect("dir");
        assert_eq!(as_dir.root, pkg.root);
    }

    #[test]
    fn resolve_writable_dir_falls_back() {
        let tmp = test_tempdir();
        let not_dir = tmp.path().join("not-a-dir");
        fs::write(&not_dir, b"x").unwrap();
        let fallback = tmp.path().join("ok");
        let resolved = resolve_writable_dir(&not_dir, &fallback).expect("fallback");
        assert_eq!(resolved, fallback);
        assert!(fallback.is_dir());
    }
}
