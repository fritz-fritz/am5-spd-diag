//! Shared notification constants and session-launch helpers (no GTK).
//! The GTK results window lives in the `am5-spd-diag-notify` binary.

use crate::paths::{libexec_dir, share_dir};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const APP_ID: &str = "org.opensuse.am5spdDiag";
/// Visible FDO Notify app_name. GTK/D-Bus id and desktop-entry stay APP_ID.
pub const APP_NAME: &str = "Ghost DIMM";
pub const APP_ICON: &str = APP_ID;
/// Main image on the notice. The app logo comes from the desktop file.
pub const NOTIFY_IMAGE: &str = "dialog-warning";
pub const OBJECT_PATH: &str = "/org/opensuse/am5spdDiag";
pub const NOTIFY_ID: &str = "spd-corruption";
pub const DEFAULT_ACTION: &str = "app.status";
pub const ANALYZE_ACTION: &str = "app.analyze";
pub const REPORT_ACTION: &str = "app.report";
pub const ACTIONS: &[&str] = &["status", "analyze", "report", "probe"];
pub const NOTIFY_ACTIONS: &[&str] = &[
    "default", "Status", "analyze", "Analyze", "report", "Report",
];
pub const FDO_DEST: &str = "org.freedesktop.Notifications";
pub const FDO_PATH: &str = "/org/freedesktop/Notifications";
pub const FDO_IFACE: &str = "org.freedesktop.Notifications";
pub const GTK_NOTIFY_DEST: &str = "org.gtk.Notifications";
pub const CLOSE_GRACE_MS: u64 = 400;
pub const SESSION_ENV_KEYS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "HOME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "PATH",
    "QT_QPA_PLATFORM",
    "QT_QPA_PLATFORMTHEME",
];

pub fn notify_bin_path() -> PathBuf {
    let libexec = libexec_dir();
    let candidate = libexec.join("am5-spd-diag-notify");
    if candidate.is_file() {
        return candidate;
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let next = dir.join("am5-spd-diag-notify");
            if next.is_file() {
                return next;
            }
        }
    }
    PathBuf::from("/usr/libexec/am5-spd-diag/am5-spd-diag-notify")
}

pub fn normalize_action(name: &str) -> &'static str {
    let mut action = name.trim();
    if let Some(rest) = action.strip_prefix("app.") {
        action = rest;
    }
    if action.is_empty() || action == "default" {
        return "status";
    }
    match action {
        "status" => "status",
        "analyze" => "analyze",
        "report" => "report",
        "probe" => "probe",
        _ => "status",
    }
}

pub fn parse_systemd_env_file(text: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }
        let parsed = shlex_first(rest);
        env.insert(key.to_string(), parsed);
    }
    env
}

fn shlex_first(rest: &str) -> String {
    let rest = rest.trim();
    if rest.is_empty() {
        return String::new();
    }
    if (rest.starts_with('\'') && rest.ends_with('\''))
        || (rest.starts_with('"') && rest.ends_with('"'))
    {
        return rest[1..rest.len() - 1].to_string();
    }
    rest.split_whitespace().next().unwrap_or(rest).to_string()
}

pub fn probe_wayland_display(runtime: &str) -> Option<String> {
    let mut names: Vec<_> = std::fs::read_dir(runtime).ok()?.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for entry in names {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("wayland-") || name.ends_with(".lock") {
            continue;
        }
        if entry.path().exists() {
            return Some(name.into_owned());
        }
    }
    None
}

pub fn systemd_user_environment() -> BTreeMap<String, String> {
    let out = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output();
    let Ok(out) = out else {
        return BTreeMap::new();
    };
    if !out.status.success() || out.stdout.is_empty() {
        return BTreeMap::new();
    }
    parse_systemd_env_file(&String::from_utf8_lossy(&out.stdout))
}

pub fn ensure_session_env() {
    let imported = systemd_user_environment();
    for key in SESSION_ENV_KEYS {
        if env::var_os(key).map(|v| !v.is_empty()).unwrap_or(false) {
            continue;
        }
        if let Some(value) = imported.get(*key) {
            if !value.is_empty() {
                env::set_var(key, value);
            }
        }
    }
    if env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Ok(runtime) = env::var("XDG_RUNTIME_DIR") {
            if let Some(probed) = probe_wayland_display(&runtime) {
                env::set_var("WAYLAND_DISPLAY", probed);
            }
        }
    }
    if env::var_os("DISPLAY").is_none() && Path::new("/tmp/.X11-unix/X0").exists() {
        env::set_var("DISPLAY", ":0");
    }
}

pub fn systemd_user_run_argv(
    argv: &[String],
    env: &BTreeMap<String, String>,
    runner: Option<&str>,
) -> Option<Vec<String>> {
    let runner = runner.map(str::to_string).or_else(|| which("systemd-run"));
    let runner = runner?;
    if env
        .get("XDG_RUNTIME_DIR")
        .map(|s| s.is_empty())
        .unwrap_or(true)
        && std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_default()
            .is_empty()
    {
        // Match Python: require XDG_RUNTIME_DIR on the env map passed in.
        env.get("XDG_RUNTIME_DIR")?;
    }
    if env
        .get("XDG_RUNTIME_DIR")
        .map(String::as_str)
        .unwrap_or("")
        .is_empty()
    {
        return None;
    }
    let mut cmd = vec![
        runner,
        "--user".into(),
        "--collect".into(),
        "--quiet".into(),
    ];
    for key in [
        "XDG_ACTIVATION_TOKEN",
        "DESKTOP_STARTUP_ID",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
    ] {
        if let Some(value) = env.get(key) {
            if !value.is_empty() {
                cmd.push(format!("--setenv={key}={value}"));
            }
        }
    }
    cmd.push("--".into());
    cmd.extend(argv.iter().cloned());
    Some(cmd)
}

fn which(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn gtk_notification_body(title: &str, body: &str, persistent_hint: bool) -> serde_json::Value {
    let mut n = serde_json::json!({
        "title": title,
        "body": body,
        "icon": NOTIFY_IMAGE,
        "priority": "urgent",
        "default-action": DEFAULT_ACTION,
        "buttons": [
            {"label": "Analyze", "action": ANALYZE_ACTION},
            {"label": "Report", "action": REPORT_ACTION},
        ],
    });
    if persistent_hint {
        n["display-hint"] = serde_json::json!(["persistent"]);
    }
    n
}

pub fn ticket_template() -> PathBuf {
    share_dir().join("templates/ticket.md.tmpl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdo_app_name_is_human() {
        assert_eq!(APP_NAME, "Ghost DIMM");
        assert_eq!(APP_ID, "org.opensuse.am5spdDiag");
        assert_ne!(APP_NAME, APP_ID);
    }

    #[test]
    fn actions_normalize() {
        assert_eq!(normalize_action("probe"), "probe");
        assert_eq!(normalize_action("app.probe"), "probe");
        assert_eq!(normalize_action("app.status"), "status");
        assert_eq!(normalize_action("default"), "status");
        assert_eq!(normalize_action(""), "status");
    }

    #[test]
    fn parse_env_file() {
        let parsed = parse_systemd_env_file(
            "DISPLAY=:0\nWAYLAND_DISPLAY='wayland-0'\nPATH=/usr/bin\n# skip\n",
        );
        assert_eq!(parsed.get("DISPLAY").map(String::as_str), Some(":0"));
        assert_eq!(
            parsed.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("wayland-0")
        );
        assert_eq!(parsed.get("PATH").map(String::as_str), Some("/usr/bin"));
    }

    #[test]
    fn systemd_run_forwards_token() {
        let env = BTreeMap::from([
            ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ("XDG_ACTIVATION_TOKEN".into(), "tok".into()),
            ("DISPLAY".into(), ":0".into()),
            ("WAYLAND_DISPLAY".into(), "wayland-0".into()),
        ]);
        let argv = systemd_user_run_argv(
            &["/helper".into(), "analyze".into()],
            &env,
            Some("/usr/bin/systemd-run"),
        )
        .unwrap();
        assert_eq!(&argv[..3], ["/usr/bin/systemd-run", "--user", "--collect"]);
        assert!(argv
            .iter()
            .any(|s| s == "--setenv=XDG_ACTIVATION_TOKEN=tok"));
        assert!(argv
            .iter()
            .any(|s| s == "--setenv=WAYLAND_DISPLAY=wayland-0"));
        assert_eq!(&argv[argv.len() - 3..], ["--", "/helper", "analyze"]);
    }

    #[test]
    fn systemd_run_requires_runtime_dir() {
        assert!(systemd_user_run_argv(
            &["/helper".into(), "status".into()],
            &BTreeMap::new(),
            Some("/usr/bin/systemd-run"),
        )
        .is_none());
    }

    #[test]
    fn gtk_body_actions() {
        let body = gtk_notification_body("t", "b", false);
        assert_eq!(body["default-action"], DEFAULT_ACTION);
        assert_eq!(body["icon"], NOTIFY_IMAGE);
        assert_eq!(body["buttons"][0]["action"], ANALYZE_ACTION);
        assert_eq!(body["buttons"][1]["action"], REPORT_ACTION);
        assert!(body.get("display-hint").is_none());
        let portal = gtk_notification_body("t", "b", true);
        assert_eq!(portal["display-hint"], serde_json::json!(["persistent"]));
    }
}
