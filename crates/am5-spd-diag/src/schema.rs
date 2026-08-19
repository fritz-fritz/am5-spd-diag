//! Frozen on-disk contracts for capture, timeline.jsonl, hub.json, and baseline.json.
//!
//! Extra keys are ignored on read; writers keep the original field set so
//! existing captures and fixtures still parse.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FORUM_URL: &str = concat!(
    "https://forum-en.msi.com/index.php?threads/",
    "ddr5-module-detected-as-2gb-ghost-dimm-after-s3-sleep-on-am5-root-cause-found.419787/"
);

/// Timeline events written by capture (one JSON object per record).
pub const TIMELINE_EVENTS: &[&str] = &[
    "boot",
    "suspend-pre",
    "suspend-post",
    "hibernate-pre",
    "hibernate-post",
    "reboot",
    "poweroff",
    "shutdown",
    "manual",
    "recover",
];

/// `boot_kind` values capture may store on a boot event.
pub const BOOT_KINDS: &[&str] = &[
    "warm_reboot",
    "shutdown_poweroff",
    "unexpected_power_loss",
    "same_boot",
    "unknown",
    "",
];

/// Corruption flags that may appear in `flags` (comma-separated) and ALERT.flags.
pub const FLAG_NAMES: &[&str] = &[
    "unknown_part",
    "dimm_8bit_width",
    "ghost_page0",
    "hub_mr11_stuck",
];

/// Files written into each event directory.
pub const EVENT_DIR_FILES: &[&str] = &[
    "meta.txt",
    "dimm-summary.txt",
    "dmidecode-memory.txt",
    "dmi-sysfs.txt",
    "hub.json",
    "system.json",
    "e820.txt",
    "e820-system-ram.txt",
    "dmesg-spd5118.txt",
    "ALERT.flags",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineEvent {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub boot_id: String,
    #[serde(default)]
    pub memtotal_kb: Value,
    #[serde(default)]
    pub mem_sleep: String,
    #[serde(default)]
    pub sleep_type: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default)]
    pub alert: Value,
    #[serde(default, rename = "dir")]
    pub dir: String,
    #[serde(default)]
    pub boot_kind: String,
    #[serde(default)]
    pub suspend_success: String,
    #[serde(default)]
    pub hub_stuck: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
    #[serde(skip)]
    pub dimms: Vec<BTreeMap<String, String>>,
    #[serde(skip)]
    pub meta: BTreeMap<String, String>,
    #[serde(skip)]
    pub dmi: BTreeMap<String, String>,
    #[serde(skip)]
    pub hub: Value,
    #[serde(skip)]
    pub recover: Value,
    #[serde(skip)]
    pub system: Value,
    #[serde(skip)]
    pub e820: String,
    #[serde(skip)]
    pub dmesg_spd: String,
    #[serde(skip)]
    pub spd_page0: Vec<Page0File>,
    #[serde(skip)]
    pub dir_exists: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Page0File {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubProbe {
    #[serde(default)]
    pub dmesg: Vec<String>,
    #[serde(default)]
    pub dmesg_stuck: Vec<String>,
    #[serde(default)]
    pub adapters: Vec<Value>,
    #[serde(default)]
    pub hubs: Vec<Value>,
    #[serde(default)]
    pub stuck: Vec<Value>,
    #[serde(default)]
    pub attempts: Vec<Value>,
    #[serde(default)]
    pub method: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Baseline {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub memtotal_kb: Value,
    #[serde(default)]
    pub cpu: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub dmi: BTreeMap<String, String>,
    #[serde(default)]
    pub dimms: Vec<BTreeMap<String, String>>,
    #[serde(default)]
    pub hubs: Vec<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TimelineEvent {
    pub fn is_alert(&self) -> bool {
        match &self.alert {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_i64() == Some(1),
            Value::String(s) => {
                let t = s.to_ascii_lowercase();
                t == "true" || t == "1" || t == "yes"
            }
            _ => false,
        }
    }

    pub fn memtotal_kb_i64(&self) -> i64 {
        value_as_i64(&self.memtotal_kb)
    }

    pub fn dir_path(&self) -> PathBuf {
        PathBuf::from(&self.dir)
    }
}

pub fn value_as_i64(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)).unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

pub fn value_as_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Yield JSON objects from JSONL, including multiple objects jammed on one line.
pub fn iter_json_objects(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut idx = 0;
    let n = text.len();
    while idx < n {
        while idx < n && text.as_bytes()[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= n {
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(&text[idx..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(obj)) => {
                let consumed = stream.byte_offset();
                if obj.is_object() {
                    out.push(obj);
                }
                idx += consumed.max(1);
            }
            _ => match text[idx + 1..].find('{') {
                Some(rel) => idx = idx + 1 + rel,
                None => break,
            },
        }
    }
    out
}

pub fn load_json_object(path: &Path) -> Value {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Value::Object(Default::default());
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => Value::Object(Default::default()),
    }
}

pub fn event_from_value(value: Value) -> Option<TimelineEvent> {
    serde_json::from_value(value).ok()
}

#[cfg(test)]
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn fixture_timeline_has_frozen_fields() {
        let path = repo_root().join("tests/fixture/timeline.jsonl");
        let text = fs::read_to_string(&path).expect("timeline.jsonl");
        let objs = iter_json_objects(&text);
        assert_eq!(objs.len(), 7);
        let last: TimelineEvent = serde_json::from_value(objs[6].clone()).unwrap();
        assert_eq!(last.event, "boot");
        assert!(last.is_alert());
        assert_eq!(last.boot_kind, "warm_reboot");
        assert_eq!(last.hub_stuck, "yes");
        assert!(last.flags.contains("unknown_part"));
        assert!(last.flags.contains("ghost_page0"));
        assert_eq!(last.memtotal_kb_i64(), 17800092);
        for key in [
            "ts",
            "event",
            "boot_id",
            "memtotal_kb",
            "mem_sleep",
            "sleep_type",
            "flags",
            "alert",
            "dir",
            "boot_kind",
            "hub_stuck",
        ] {
            assert!(objs[0].get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn fixture_hub_json_contract() {
        let path = repo_root().join(
            "tests/fixture/events/20260817T043030.0Z-boot/hub.json",
        );
        let hub: HubProbe = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(hub.dmesg_stuck, vec!["1-0053".to_string()]);
        assert_eq!(hub.stuck.len(), 1);
        let row = &hub.stuck[0];
        assert_eq!(row["sysfs"], "1-0053");
        assert_eq!(row["mr11_hex"], "0x08");
        assert_eq!(row["stuck"], true);
        assert!(row.get("spd_page0_head").is_some());
    }

    #[test]
    fn fixture_baseline_contract() {
        let path = repo_root().join("tests/fixture/baseline.json");
        let base: Baseline = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value_as_i64(&base.memtotal_kb), 32000000);
        assert_eq!(base.dmi.get("bios_version").map(String::as_str), Some("2.A52"));
        assert_eq!(base.dimms.len(), 2);
        assert_eq!(base.dimms[0]["part"], "CMH32GX5M2M6000Z36");
    }

    #[test]
    fn iter_json_objects_concatenated() {
        let text = r#"{"ts":"a","event":"manual"}{"ts":"b","event":"boot"}
{"ts":"c","event":"manual"}
"#;
        let objs = iter_json_objects(text);
        let events: Vec<&str> = objs
            .iter()
            .map(|o| o.get("event").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(events, ["manual", "boot", "manual"]);
    }
}
