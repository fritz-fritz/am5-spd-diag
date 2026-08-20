use crate::i2c::{probe_hubs, uid_from_bus_path, STUCK_MR11};
use crate::notify::{notify_bin_path, APP_ID};
use crate::safe_fs::write_nofollow;
use crate::schema::FORUM_URL;
use serde_json::Value;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn notify_user_argv(bus_path: &str, title: &str, body: &str) -> Vec<String> {
    let uid = uid_from_bus_path(bus_path);
    let helper = notify_bin_path();
    let mut argv = vec![
        "env".into(),
        format!("XDG_RUNTIME_DIR=/run/user/{}", uid.unwrap_or(0)),
        format!("DBUS_SESSION_BUS_ADDRESS=unix:path={bus_path}"),
        helper.display().to_string(),
        "--notify".into(),
        title.into(),
        body.into(),
    ];
    if unsafe { libc::geteuid() } == 0 && uid.filter(|&u| u != 0).is_some() {
        if let Some(uid) = uid {
            if let Some(name) = username_for_uid(uid) {
                let mut wrapped = vec!["runuser".into(), "-u".into(), name, "--".into()];
                wrapped.append(&mut argv);
                return wrapped;
            }
        }
    }
    argv
}

fn username_for_uid(uid: u32) -> Option<String> {
    let pwd = unsafe { libc::getpwuid(uid) };
    if pwd.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*pwd).pw_name) };
    Some(name.to_string_lossy().into_owned())
}

pub fn notify_desktop(title: &str, body: &str) {
    let helper = notify_bin_path();
    if !helper.is_file() {
        return;
    }
    let Ok(entries) = fs::read_dir("/run/user") else {
        return;
    };
    for entry in entries.flatten() {
        let bus = entry.path().join("bus");
        if !bus.exists() {
            continue;
        }
        let bus_path = bus.display().to_string();
        let uid = uid_from_bus_path(&bus_path);
        if uid.is_none() || uid == Some(0) {
            continue;
        }
        let argv = notify_user_argv(&bus_path, title, body);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let _ = cmd.spawn();
    }
}

pub fn notify_all(message: &str) {
    let _ = Command::new("logger")
        .args(["-p", "user.alert", "-t", "am5-spd-diag", message])
        .status();
    let _ = Command::new("wall")
        .args(["-n", message])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    notify_desktop("SPD corruption detected", message);
}

pub fn write_baseline(path: &Path, payload: &Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".into());
    let _ = write_nofollow(path, format!("{text}\n"));
    let txt = path.with_extension("txt");
    let mut lines = vec![
        format!(
            "captured={}",
            payload.get("ts").and_then(|v| v.as_str()).unwrap_or("")
        ),
        format!(
            "memtotal_kb={}",
            payload
                .get("memtotal_kb")
                .map(|v| match v {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                })
                .unwrap_or_default()
        ),
        format!(
            "cpu={}",
            payload.get("cpu").and_then(|v| v.as_str()).unwrap_or("")
        ),
        format!(
            "board={}",
            payload
                .pointer("/dmi/board_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
        format!(
            "bios={}",
            payload
                .pointer("/dmi/bios_version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
    ];
    if let Some(dimms) = payload.get("dimms").and_then(|v| v.as_array()) {
        for dimm in dimms {
            lines.push(format!(
                "{} {} {} {}",
                dimm.get("locator").and_then(|v| v.as_str()).unwrap_or("?"),
                dimm.get("size").and_then(|v| v.as_str()).unwrap_or("?"),
                dimm.get("manufacturer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?"),
                dimm.get("part").and_then(|v| v.as_str()).unwrap_or("?"),
            ));
        }
    }
    let _ = write_nofollow(txt, format!("{}\n", lines.join("\n")));
}

pub fn print_probe(json_out: bool) {
    let probe = probe_hubs();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&probe).unwrap_or_else(|_| "{}".into())
        );
        return;
    }
    println!("{}", format_probe_human(&probe));
}

pub fn format_probe_human(probe: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(stuck) = probe["dmesg_stuck"].as_array() {
        if !stuck.is_empty() {
            let ids: Vec<_> = stuck.iter().filter_map(|v| v.as_str()).collect();
            lines.push(format!("dmesg stuck: {}", ids.join(", ")));
        }
    }
    let hubs = probe["hubs"].as_array().cloned().unwrap_or_default();
    let adapters = probe["adapters"].as_array().cloned().unwrap_or_default();
    let attempts = probe["attempts"].as_array().cloned().unwrap_or_default();
    let smbus: Vec<_> = adapters
        .iter()
        .filter(|a| a["smbus"].as_bool() == Some(true))
        .collect();
    if adapters.is_empty() && attempts.is_empty() && hubs.is_empty() {
        if let Some(stuck) = probe["stuck"].as_array() {
            if stuck.is_empty() {
                lines.push("No /dev/i2c-* devices (need the i2c-dev module).".into());
                return lines.join("\n");
            }
        } else {
            lines.push("No /dev/i2c-* devices (need the i2c-dev module).".into());
            return lines.join("\n");
        }
    }
    if adapters.is_empty() && !hubs.is_empty() {
        for hub in &hubs {
            lines.push(format_hub_line(hub));
        }
        if let Some(stuck) = probe["stuck"].as_array() {
            for hub in stuck {
                let line = format_hub_line(hub);
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
        return lines.join("\n");
    }
    if adapters.is_empty() {
        if let Some(stuck) = probe["stuck"].as_array() {
            for hub in stuck {
                lines.push(format_hub_line(hub));
            }
        }
        return lines.join("\n");
    }
    if smbus.is_empty() {
        lines.push("No host SMBus adapter matched (PIIX4 / I801 / FCH / AMD SMBus).".into());
        for adapter in &adapters {
            lines.push(format!(
                "  bus {} {}",
                adapter["bus"],
                adapter["name"].as_str().unwrap_or("?")
            ));
        }
        return lines.join("\n");
    }

    let need_root = attempts
        .iter()
        .any(|a| matches!(a["errno_name"].as_str(), Some("EACCES") | Some("EPERM")));
    if hubs.is_empty() && attempts.is_empty() {
        lines.push("No spd5118 hub devices in sysfs on host SMBus adapters.".into());
        for adapter in &smbus {
            lines.push(format!(
                "  bus {} {}",
                adapter["bus"],
                adapter["name"].as_str().unwrap_or("?")
            ));
        }
        return lines.join("\n");
    }
    if hubs.is_empty() {
        if need_root {
            lines
                .push("spd5118 hubs found, but /dev/i2c-* could not be opened (need root).".into());
        } else {
            lines.push("spd5118 hubs found, but MR11 could not be read.".into());
        }
    }

    if !attempts.is_empty() {
        let probed: std::collections::BTreeSet<_> =
            attempts.iter().filter_map(|a| a["bus"].as_i64()).collect();
        for adapter in smbus
            .iter()
            .filter(|a| probed.contains(&a["bus"].as_i64().unwrap_or(-1)))
        {
            lines.push(format!(
                "bus {} {}",
                adapter["bus"],
                adapter["name"].as_str().unwrap_or("?")
            ));
            for attempt in attempts
                .iter()
                .filter(|a| a["bus"].as_i64() == adapter["bus"].as_i64())
            {
                lines.push(format_attempt_line(attempt));
            }
        }
        let skipped: Vec<_> = smbus
            .iter()
            .filter(|a| !probed.contains(&a["bus"].as_i64().unwrap_or(-1)))
            .map(|a| format!("bus {}", a["bus"]))
            .collect();
        if !skipped.is_empty() {
            lines.push(format!(
                "SMBus adapters not probed (no spd5118 devices): {}",
                skipped.join(", ")
            ));
        }
        return lines.join("\n");
    }

    for hub in hubs {
        lines.push(format_hub_line(&hub));
    }
    lines.join("\n")
}

fn format_attempt_line(attempt: &Value) -> String {
    let addr = attempt["addr_hex"].as_str().unwrap_or("?");
    let sysfs = attempt["sysfs"].as_str().unwrap_or("?");
    let mut extra = String::new();
    if let Some(driver) = attempt["driver"].as_str() {
        extra.push_str(&format!(" driver={driver}"));
    }
    if attempt["forced"].as_bool() == Some(true) {
        extra.push_str(" (forced)");
    }
    if attempt["ok"].as_bool() == Some(true) {
        let mr11 = attempt["mr11"].as_u64().unwrap_or(u64::MAX);
        let state = if mr11 == u64::from(STUCK_MR11) {
            "STUCK MR11=0x08".into()
        } else {
            format!("MR11={}", attempt["mr11_hex"].as_str().unwrap_or("?"))
        };
        return format!("  {addr} ({sysfs}) {state}{extra}");
    }
    let err = attempt["errno_name"]
        .as_str()
        .map(str::to_string)
        .or_else(|| attempt["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| "failed".into());
    let hint = match attempt["errno_name"].as_str() {
        Some("EACCES") | Some("EPERM") => " (need root to open /dev/i2c-*)",
        Some("EBUSY") => " (kernel driver holds the address)",
        Some("ENXIO") => " (no ACK)",
        Some("EIO") => " (transfer failed)",
        _ => "",
    };
    format!("  {addr} ({sysfs}) {err}{hint}{extra}")
}

fn format_hub_line(hub: &Value) -> String {
    let state = if hub["stuck"].as_bool() == Some(true) {
        "STUCK MR11=0x08".into()
    } else {
        format!("MR11={}", hub["mr11_hex"].as_str().unwrap_or("?"))
    };
    format!(
        "bus {} {} ({}) {state}",
        hub["bus"],
        hub["addr_hex"].as_str().unwrap_or("?"),
        hub["sysfs"].as_str().unwrap_or("?"),
    )
}

pub fn recover_run() -> (i32, serde_json::Value) {
    let result = crate::capture::recover_and_record();
    let rc = if result["ok"].as_bool() == Some(true) {
        0
    } else {
        1
    };
    (rc, result)
}

pub fn recover_warn() {
    println!("WARNING: experimental in-band fix for a stuck SPD5118 hub (MR11).");
    println!("This does not rewrite EEPROM. BIOS may show 'Devices Changed' and retrain.");
    println!("Source: {FORUM_URL}");
    println!("This tool will NOT reboot the machine. A warm reboot is required after a successful clear.");
    println!();
}

pub fn confirm_fix() -> bool {
    eprint!("Type YES to probe and clear stuck hubs: ");
    let mut reply = String::new();
    let _ = std::io::stdin().read_line(&mut reply);
    reply.trim() == "YES"
}

pub fn print_recover_result(result: &serde_json::Value) -> i32 {
    let slim = serde_json::json!({
        "ok": result["ok"],
        "reason": result["reason"],
        "actions": result["actions"],
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&slim).unwrap_or_default()
    );
    if result["ok"].as_bool() != Some(true) {
        if result["reason"].as_str() == Some("no_stuck_hub") {
            println!("No hub with MR11=0x08 was found. SMBIOS Unknown/missing part can still be present.");
        }
        return if result["ok"].as_bool() == Some(true) {
            0
        } else {
            1
        };
    }
    println!("MR11 cleared. Warm reboot now so firmware re-reads the real SPD.");
    0
}

pub fn recover_interactive(yes: bool) -> i32 {
    recover_warn();
    if !yes && !confirm_fix() {
        println!("Aborted.");
        return 2;
    }
    let (rc, result) = recover_run();
    let _ = print_recover_result(&result);
    rc
}

pub fn forum_url() -> &'static str {
    FORUM_URL
}

pub fn app_id() -> &'static str {
    APP_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_user_argv_uses_session_bus() {
        let argv = notify_user_argv("/run/user/1000/bus", "title", "body");
        assert!(argv
            .iter()
            .any(|s| s == "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"));
        assert!(argv.iter().any(|s| s == "XDG_RUNTIME_DIR=/run/user/1000"));
        assert!(argv.iter().any(|s| s == "--notify"));
        assert_eq!(&argv[argv.len() - 2..], ["title", "body"]);
        if unsafe { libc::geteuid() } == 0 {
            assert_eq!(argv[0], "runuser");
            assert_eq!(argv[1], "-u");
        } else {
            assert_eq!(argv[0], "env");
        }
    }

    #[test]
    fn format_probe_prints_mr11_only_for_spd_hubs() {
        let probe = serde_json::json!({
            "dmesg_stuck": ["1-0053"],
            "adapters": [
                {"bus": 1, "name": "SMBus PIIX4 adapter port 0 at 0b00", "smbus": true, "dev": "/dev/i2c-1"},
                {"bus": 2, "name": "SMBus PIIX4 adapter port 2 at 0b00", "smbus": true, "dev": "/dev/i2c-2"},
                {"bus": 3, "name": "SMBus PIIX4 adapter port 1 at 0b20", "smbus": true, "dev": "/dev/i2c-3"}
            ],
            "hubs": [
                {"bus": 1, "addr_hex": "0x51", "sysfs": "1-0051", "mr11_hex": "0x00", "stuck": false}
            ],
            "attempts": [
                {"bus": 1, "addr_hex": "0x51", "sysfs": "1-0051", "ok": true, "mr11": 0, "mr11_hex": "0x00", "forced": true, "driver": "spd5118"},
                {"bus": 1, "addr_hex": "0x53", "sysfs": "1-0053", "ok": true, "mr11": 8, "mr11_hex": "0x08", "forced": true, "driver": "spd5118"}
            ]
        });
        let text = format_probe_human(&probe);
        assert!(text.contains("dmesg stuck: 1-0053"));
        assert!(text.contains("bus 1 SMBus PIIX4 adapter port 0 at 0b00"));
        assert!(text.contains("0x51 (1-0051) MR11=0x00 driver=spd5118 (forced)"));
        assert!(text.contains("0x53 (1-0053) STUCK MR11=0x08 driver=spd5118 (forced)"));
        assert!(text.contains("SMBus adapters not probed (no spd5118 devices): bus 2, bus 3"));
        assert!(!text.contains("0x50"));
        assert!(!text.contains("ENXIO"));
        assert!(!text.contains("need root to open"));
    }

    #[test]
    fn format_probe_explains_permission_denied() {
        let probe = serde_json::json!({
            "adapters": [
                {"bus": 1, "name": "SMBus PIIX4 adapter port 0 at 0b00", "smbus": true, "dev": "/dev/i2c-1"}
            ],
            "hubs": [],
            "attempts": [
                {"bus": 1, "addr_hex": "0x51", "sysfs": "1-0051", "ok": false, "errno_name": "EACCES", "error": "Permission denied (os error 13)"}
            ]
        });
        let text = format_probe_human(&probe);
        assert!(text.contains("need root"));
        assert!(text.contains("0x51 (1-0051) EACCES"));
    }

    #[test]
    fn format_probe_lists_adapters_when_no_spd_clients() {
        let probe = serde_json::json!({
            "adapters": [
                {"bus": 2, "name": "SMBus PIIX4 adapter port 2 at 0b00", "smbus": true, "dev": "/dev/i2c-2"}
            ],
            "hubs": [],
            "attempts": []
        });
        let text = format_probe_human(&probe);
        assert!(text.contains("No spd5118 hub devices in sysfs"));
        assert!(text.contains("bus 2 SMBus PIIX4 adapter port 2 at 0b00"));
        assert!(!text.contains("ENXIO"));
    }
}
