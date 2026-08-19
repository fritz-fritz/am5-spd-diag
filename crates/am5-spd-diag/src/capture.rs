use crate::analyze::{collect_system_info, iso_now, mem_sleep, memtotal_kb};
use crate::config::{load_config, Config};
use crate::dimm::{format_dimm_summary, summary_flags as dimm_summary_flags};
use crate::hub::{notify_all, write_baseline};
use crate::i2c::{probe_hubs, recover_stuck};
use crate::paths::share_dir;
use crate::schema::iter_json_objects;
use crate::smbios::{collect_memory_dump, collect_system_dump, parse_memory_devices, write_spd_page0_files};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn capture_main(args: &[String]) -> i32 {
    let event = args.first().map(String::as_str).unwrap_or("manual");
    let sleep_type = args.get(1).cloned().unwrap_or_default();
    match event {
        "pre" | "post" | "boot" | "shutdown" | "reboot" | "poweroff"
        | "suspend-pre" | "suspend-post" | "hibernate-pre" | "hibernate-post" | "manual"
        | "recover" => {}
        _ => {
            eprintln!("am5-spd-diag capture: unknown event: {event}");
            return 0;
        }
    }
    let cfg = load_config(&share_dir());
    if std::env::var("AM5_SPD_DIAG_IN_TIMEOUT").is_err() {
        if let Ok(status) = Command::new("timeout")
            .args([
                "--preserve-status",
                &cfg.capture_timeout_sec().to_string(),
            ])
            .env("AM5_SPD_DIAG_IN_TIMEOUT", "1")
            .arg(std::env::current_exe().unwrap_or_else(|_| "am5-spd-diag".into()))
            .arg("capture")
            .arg(event)
            .arg(&sleep_type)
            .status()
        {
            let _ = status;
            return 0;
        }
    }
    if let Err(err) = capture_event(event, &sleep_type, &cfg) {
        eprintln!("am5-spd-diag capture: capture failed for event={event} (ignored): {err}");
    }
    0
}

/// Clear stuck MR11 hubs, then capture a `recover` timeline event (hub.json + recover.json).
pub fn recover_and_record() -> Value {
    let result = recover_stuck(None);
    let cfg = load_config(&share_dir());
    let state_dir = cfg.state_dir();
    let _ = fs::create_dir_all(&state_dir);
    let pending = state_dir.join("pending-recover.json");
    let _ = fs::write(
        &pending,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".into())
        ),
    );
    if let Err(err) = capture_event("recover", "", &cfg) {
        eprintln!("am5-spd-diag capture: recover event failed (ignored): {err}");
        let _ = fs::remove_file(&pending);
    }
    result
}

fn utc_now() -> String {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    chrono::DateTime::from_timestamp(t.as_secs() as i64, t.subsec_nanos())
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap())
        .format("%Y%m%dT%H%M%S%.fZ")
        .to_string()
}

fn boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn suspend_success() -> String {
    fs::read_to_string("/sys/power/suspend_stats/success")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn infer_sleep_type(sleep_type: &str) -> String {
    if !sleep_type.is_empty() {
        return sleep_type.to_string();
    }
    if let Ok(out) = Command::new("systemctl").arg("list-jobs").output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("hibernate.target") && text.contains("start") {
            return "hibernate".into();
        }
        if text.contains("hybrid-sleep.target") && text.contains("start") {
            return "hybrid-sleep".into();
        }
        if text.contains("suspend-then-hibernate.target") && text.contains("start") {
            return "suspend-then-hibernate".into();
        }
    }
    sleep_type.to_string()
}

fn recover_cleared_payload(payload: &Value) -> bool {
    payload.get("ok").and_then(|v| v.as_bool()) == Some(true)
}

/// Verified in-band clear still leaves SMBIOS ghosted until reboot.
/// Do not page the user as if the fix failed.
fn skip_corruption_broadcast(event: &str, recover: &Value) -> bool {
    event == "recover" && recover_cleared_payload(recover)
}

fn detect_shutdown_mode() -> String {
    let path = Path::new("/run/systemd/shutdown/scheduled");
    if path.is_file() {
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                if let Some(mode) = line.strip_prefix("MODE=") {
                    return mode.to_string();
                }
            }
        }
    }
    if let Ok(out) = Command::new("systemctl").arg("list-jobs").output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("reboot.target") && text.contains("start") {
            return "reboot".into();
        }
        if text.contains("poweroff.target") && text.contains("start")
            || text.contains("halt.target") && text.contains("start")
        {
            return "poweroff".into();
        }
    }
    "shutdown".into()
}

fn normalize_event(event: &str, sleep_type: &str) -> String {
    match event {
        "shutdown" => match detect_shutdown_mode().as_str() {
            "reboot" => "reboot".into(),
            "poweroff" | "halt" => "poweroff".into(),
            _ => "shutdown".into(),
        },
        "pre" => {
            if matches!(
                sleep_type,
                "hibernate" | "hybrid-sleep" | "suspend-then-hibernate"
            ) {
                "hibernate-pre".into()
            } else {
                "suspend-pre".into()
            }
        }
        "post" => {
            if matches!(
                sleep_type,
                "hibernate" | "hybrid-sleep" | "suspend-then-hibernate"
            ) {
                "hibernate-post".into()
            } else {
                "suspend-post".into()
            }
        }
        other => other.to_string(),
    }
}

fn classify_boot_kind(timeline: &Path, current_boot: &str) -> String {
    let Ok(text) = fs::read_to_string(timeline) else {
        return "unknown".into();
    };
    let objs = iter_json_objects(&text);
    if objs.is_empty() {
        return "unknown".into();
    }
    let last_boot = objs
        .last()
        .and_then(|o| o.get("boot_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if last_boot == current_boot {
        return "same_boot".into();
    }
    for ev in objs.iter().rev() {
        let bid = ev.get("boot_id").and_then(|v| v.as_str()).unwrap_or("");
        if bid != current_boot {
            let prev = ev.get("event").and_then(|v| v.as_str()).unwrap_or("");
            return match prev {
                "reboot" => "warm_reboot".into(),
                "poweroff" | "shutdown" | "halt" => "shutdown_poweroff".into(),
                "" => "unknown".into(),
                _ => "unexpected_power_loss".into(),
            };
        }
    }
    "unknown".into()
}

fn append_timeline(state_dir: &Path, rec: &Value) -> std::io::Result<()> {
    fs::create_dir_all(state_dir)?;
    let lock_path = state_dir.join("timeline.lock");
    let lock = OpenOptions::new().create(true).append(true).open(&lock_path)?;
    let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let timeline = state_dir.join("timeline.jsonl");
    let mut f = OpenOptions::new().create(true).append(true).open(&timeline)?;
    writeln!(f, "{}", serde_json::to_string(rec).unwrap_or_else(|_| "{}".into()))?;
    Ok(())
}

fn prune_old_events(events_dir: &Path, timeline: &Path, keep_days: u64) {
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(keep_days.saturating_mul(86400)))
        .unwrap_or(UNIX_EPOCH);
    if let Ok(rd) = fs::read_dir(events_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(meta) = path.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            if mtime < cutoff {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
    let Ok(text) = fs::read_to_string(timeline) else {
        return;
    };
    let mut kept = Vec::new();
    for obj in iter_json_objects(&text) {
        let dir = obj.get("dir").and_then(|v| v.as_str()).unwrap_or("");
        if !dir.is_empty() && !Path::new(dir).is_dir() {
            continue;
        }
        kept.push(serde_json::to_string(&obj).unwrap_or_default());
    }
    let tmp = timeline.with_extension("jsonl.tmp");
    let body = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    };
    if fs::write(&tmp, body).is_ok() {
        let _ = fs::rename(tmp, timeline);
    }
}

fn write_system_files(dir: &Path) {
    let mut dmi = String::new();
    for f in [
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
    ] {
        let val = fs::read_to_string(format!("/sys/class/dmi/id/{f}")).unwrap_or_default();
        dmi.push_str(&format!("{f}={val}"));
        if !val.ends_with('\n') {
            dmi.push('\n');
        }
    }
    let _ = fs::write(dir.join("dmi-sysfs.txt"), dmi);
    if Path::new("/etc/os-release").is_file() {
        let _ = fs::copy("/etc/os-release", dir.join("os-release.txt"));
    }
    if let Ok(out) = Command::new("uname").arg("-a").output() {
        let _ = fs::write(dir.join("uname.txt"), out.stdout);
    }
    let boot = if Path::new("/sys/firmware/efi").is_dir() {
        "UEFI"
    } else {
        "legacy"
    };
    let arch = crate::analyze::live_kernel_info()
        .get("machine")
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    let _ = fs::write(dir.join("firmware.txt"), format!("boot_mode={boot}\narch={arch}\n"));
    if let Ok(text) = fs::read_to_string("/proc/cpuinfo") {
        let mut n = 0;
        let mut lines = Vec::new();
        for line in text.lines() {
            if line.starts_with("processor") {
                if n > 0 {
                    break;
                }
                n += 1;
            }
            if line.starts_with("processor")
                || line.starts_with("vendor_id")
                || line.starts_with("cpu family")
                || line.starts_with("model")
                || line.starts_with("model name")
                || line.starts_with("stepping")
                || line.starts_with("microcode")
            {
                if line.contains(':') {
                    lines.push(line.to_string());
                }
            }
        }
        let _ = fs::write(dir.join("cpuinfo-head.txt"), format!("{}\n", lines.join("\n")));
    }
    let (sys, _) = collect_system_dump(None, true);
    let _ = fs::write(dir.join("dmidecode-system.txt"), sys);
    let inv = collect_system_info();
    let _ = fs::write(
        dir.join("system.json"),
        format!("{}\n", serde_json::to_string_pretty(&inv).unwrap_or_else(|_| "{}".into())),
    );
}

fn write_healthy_baseline(state_dir: &Path, event_dir: &Path, mem_kb: i64) {
    let dmi = crate::analyze::read_kv_file(&event_dir.join("dmi-sysfs.txt"));
    let dimms = crate::dimm::parse_dimm_summary(
        &fs::read_to_string(event_dir.join("dimm-summary.txt")).unwrap_or_default(),
    );
    let hub = crate::schema::load_json_object(&event_dir.join("hub.json"));
    let cpu = crate::analyze::live_cpu();
    let os_name = crate::analyze::live_os();
    let ts = crate::analyze::read_kv_file(&event_dir.join("meta.txt"))
        .get("ts")
        .cloned()
        .unwrap_or_default();
    let payload = json!({
        "ts": ts,
        "event_dir": event_dir.display().to_string(),
        "memtotal_kb": mem_kb,
        "cpu": cpu,
        "os": os_name,
        "kernel": crate::analyze::live_kernel(),
        "dmi": dmi,
        "dimms": dimms,
        "hubs": hub.get("hubs").cloned().unwrap_or(json!([])),
    });
    write_baseline(&state_dir.join("baseline.json"), &payload);
    let _ = dmi;
}

fn capture_event(event_in: &str, sleep_type: &str, cfg: &Config) -> std::io::Result<PathBuf> {
    let sleep_type = infer_sleep_type(sleep_type);
    let event = normalize_event(event_in, &sleep_type);
    let state_dir = cfg.state_dir();
    let events_dir = state_dir.join("events");
    let latest = state_dir.join("latest");
    fs::create_dir_all(&events_dir)?;
    fs::create_dir_all(&latest)?;
    let stamp = utc_now();
    let dir = events_dir.join(format!("{stamp}-{event}"));
    fs::create_dir_all(&dir)?;
    let mem_kb = memtotal_kb();
    let sleep_state = mem_sleep();
    let susp = suspend_success();
    let mut boot_kind = "unknown".to_string();
    if event == "boot" {
        boot_kind = classify_boot_kind(&state_dir.join("timeline.jsonl"), &boot_id());
    }
    let cmdline = fs::read_to_string("/proc/cmdline")
        .map(|s| s.replace('\n', ""))
        .unwrap_or_default();
    let mut meta = format!(
        "ts={}\nevent={event}\nsleep_type={sleep_type}\nboot_id={}\nuname={}\nmemtotal_kb={mem_kb}\nmem_sleep={sleep_state}\nsuspend_success={susp}\nboot_kind={boot_kind}\ncmdline={cmdline}\n",
        iso_now(),
        boot_id(),
        crate::analyze::live_kernel(),
    );
    let _ = fs::write(dir.join("meta.txt"), &meta);
    if let Ok(text) = fs::read_to_string("/proc/meminfo") {
        let head: Vec<_> = text
            .lines()
            .filter(|l| l.starts_with("MemTotal:") || l.starts_with("MemFree:") || l.starts_with("MemAvailable:"))
            .collect();
        let _ = fs::write(dir.join("meminfo-head.txt"), format!("{}\n", head.join("\n")));
    }
    if let Ok(out) = Command::new("free").arg("-h").output() {
        let _ = fs::write(dir.join("free.txt"), out.stdout);
    }
    let _ = fs::copy("/sys/power/mem_sleep", dir.join("mem_sleep.txt"));
    write_system_files(&dir);
    let (raw, _) = collect_memory_dump(None, true);
    let _ = fs::write(dir.join("dmidecode-memory.txt"), if raw.ends_with('\n') { raw.clone() } else { format!("{raw}\n") });
    let summary = format_dimm_summary(&parse_memory_devices(raw.as_bytes()));
    let _ = fs::write(dir.join("dimm-summary.txt"), &summary);
    let probe = probe_hubs();
    let _ = fs::write(
        dir.join("hub.json"),
        format!("{}\n", serde_json::to_string_pretty(&probe).unwrap_or_else(|_| "{\"hubs\":[],\"stuck\":[],\"dmesg\":[]}".into())),
    );
    let mut recover_payload = json!({});
    if event == "recover" {
        let pending = state_dir.join("pending-recover.json");
        if pending.is_file() {
            let _ = fs::copy(&pending, dir.join("recover.json"));
            let _ = fs::remove_file(&pending);
        }
        recover_payload = crate::schema::load_json_object(&dir.join("recover.json"));
    }
    let hush_fix_notice = skip_corruption_broadcast(&event, &recover_payload);
    write_spd_page0_files(&dir, &probe);
    if let Ok(out) = Command::new("dmesg").output() {
        let lines: Vec<_> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|ln| ln.to_ascii_lowercase().contains("spd5118"))
            .rev()
            .take(50)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|s| s.to_string())
            .collect();
        let _ = fs::write(dir.join("dmesg-spd5118.txt"), format!("{}\n", lines.join("\n")));
    }
    let mut hub_stuck = "no";
    if probe
        .get("stuck")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
        || probe
            .get("dmesg_stuck")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    {
        hub_stuck = "yes";
    }
    let mut flags = dimm_summary_flags(&summary).join(",");
    if flags.is_empty() && hub_stuck == "yes" {
        flags = "hub_mr11_stuck".into();
    } else if !flags.is_empty() && hub_stuck == "yes" && !format!(",{flags},").contains(",hub_mr11_stuck,") {
        flags = format!("{flags},hub_mr11_stuck");
    }
    let mut alert = false;
    if !flags.is_empty() {
        alert = true;
        let _ = fs::write(dir.join("ALERT.flags"), format!("{flags}\n"));
        let _ = File::create(state_dir.join("CORRUPTION_SEEN"));
        let _ = fs::write(state_dir.join("SPD_NOW"), "corrupt\n");
        if hush_fix_notice {
            let msg = format!(
                "Hub MR11 cleared this boot. Warm reboot so firmware re-reads SPD (identity stays stale until reboot; flags={flags})."
            );
            let _ = fs::write(state_dir.join("NOTICE"), &msg);
        } else {
            let mut alerts = OpenOptions::new()
                .create(true)
                .append(true)
                .open(state_dir.join("ALERTS.log"))?;
            writeln!(
                alerts,
                "{} ALERT event={event} flags={flags} memtotal_kb={mem_kb} boot_kind={boot_kind} dir={}",
                iso_now(),
                dir.display()
            )?;
            let _ = alerts.write_all(summary.as_bytes());
            writeln!(alerts)?;
            let msg = format!(
                "SPD corruption is current (flags={flags}; firmware published {mem_kb} kB). Click for status, or Analyze / Report."
            );
            let _ = fs::write(state_dir.join("NOTICE"), &msg);
            notify_all(&msg);
        }
    } else {
        let _ = fs::write(state_dir.join("SPD_NOW"), "healthy\n");
        let _ = fs::remove_file(state_dir.join("NOTICE"));
        write_healthy_baseline(&state_dir, &dir, mem_kb);
    }
    meta.push_str(&format!("alert={}\nflags={flags}\nhub_stuck={hub_stuck}\n", if alert { "true" } else { "false" }));
    let _ = fs::write(dir.join("meta.txt"), &meta);
    if matches!(event.as_str(), "boot" | "manual" | "reboot" | "poweroff" | "shutdown" | "recover") {
        write_dmesg_filtered(&dir);
    }
    if alert || event == "boot" || event == "manual" || event == "recover" {
        if dir.join("e820.txt").metadata().map(|m| m.len() == 0).unwrap_or(true) {
            write_e820(&dir);
        }
        write_e820_ram(&dir);
    }
    let _ = symlink_force(&dir, &latest.join(&event));
    let _ = symlink_force(&dir, &latest.join("any"));
    let rec = json!({
        "ts": iso_now(),
        "event": event,
        "boot_id": boot_id(),
        "memtotal_kb": mem_kb,
        "mem_sleep": sleep_state,
        "sleep_type": sleep_type,
        "flags": flags,
        "alert": alert,
        "dir": dir.display().to_string(),
        "boot_kind": boot_kind,
        "suspend_success": susp,
        "hub_stuck": hub_stuck,
    });
    append_timeline(&state_dir, &rec)?;
    prune_old_events(&events_dir, &state_dir.join("timeline.jsonl"), cfg.keep_days());
    if event != "recover" {
        println!("{}", dir.display());
    }
    Ok(dir)
}

fn write_dmesg_filtered(dir: &Path) {
    if let Ok(out) = Command::new("dmesg").output() {
        let text = String::from_utf8_lossy(&out.stdout);
        let filtered: Vec<_> = text
            .lines()
            .filter(|ln| {
                ln.contains("BIOS-e820")
                    || ln.contains("Memory: ")
                    || ln.contains("PM: suspend")
                    || ln.contains("suspend entry")
                    || ln.contains("suspend exit")
            })
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let _ = fs::write(dir.join("dmesg-filtered.txt"), format!("{}\n", filtered.join("\n")));
        let e820: Vec<_> = text.lines().filter(|ln| ln.contains("BIOS-e820:")).collect();
        let _ = fs::write(dir.join("e820.txt"), format!("{}\n", e820.join("\n")));
    }
}

fn write_e820(dir: &Path) {
    if let Ok(out) = Command::new("dmesg").output() {
        let e820: Vec<_> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|ln| ln.contains("BIOS-e820:"))
            .map(|s| s.to_string())
            .collect();
        let _ = fs::write(dir.join("e820.txt"), format!("{}\n", e820.join("\n")));
    }
}

fn write_e820_ram(dir: &Path) {
    let e820 = fs::read_to_string(dir.join("e820.txt")).unwrap_or_default();
    let ram: Vec<_> = e820.lines().filter(|ln| ln.contains("System RAM")).collect();
    if !ram.is_empty() {
        let _ = fs::write(dir.join("e820-system-ram.txt"), format!("{}\n", ram.join("\n")));
        return;
    }
    if let Ok(out) = Command::new("dmesg").output() {
        let ram: Vec<_> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|ln| ln.contains("BIOS-e820:") && ln.contains("System RAM"))
            .map(|s| s.to_string())
            .collect();
        let _ = fs::write(dir.join("e820-system-ram.txt"), format!("{}\n", ram.join("\n")));
    }
}

fn symlink_force(target: &Path, link: &Path) -> std::io::Result<()> {
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verified_fix_does_not_broadcast_corruption() {
        let cleared = json!({"ok": true, "reason": "ok", "actions": [{"cleared": true}]});
        assert!(skip_corruption_broadcast("recover", &cleared));
        let failed = json!({"ok": false, "reason": "no_stuck_hub"});
        assert!(!skip_corruption_broadcast("recover", &failed));
        assert!(!skip_corruption_broadcast("boot", &cleared));
        assert!(!skip_corruption_broadcast("manual", &cleared));
    }

    #[test]
    fn explicit_hibernate_sleep_type_is_not_suspend() {
        assert_eq!(normalize_event("pre", "hibernate"), "hibernate-pre");
        assert_eq!(normalize_event("post", "hibernate"), "hibernate-post");
        assert_eq!(normalize_event("pre", "suspend"), "suspend-pre");
        assert_eq!(normalize_event("pre", ""), "suspend-pre");
    }

    #[test]
    fn infer_sleep_type_keeps_explicit_value() {
        assert_eq!(infer_sleep_type("hibernate"), "hibernate");
        assert_eq!(infer_sleep_type("suspend"), "suspend");
    }
}

