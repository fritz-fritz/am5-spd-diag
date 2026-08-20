use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub values: BTreeMap<String, String>,
}

impl Config {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn state_dir(&self) -> PathBuf {
        PathBuf::from(self.get("STATE_DIR"))
    }

    pub fn keep_days(&self) -> u64 {
        self.get("KEEP_DAYS").parse().unwrap_or(60)
    }

    pub fn capture_timeout_sec(&self) -> u64 {
        self.get("CAPTURE_TIMEOUT_SEC").parse().unwrap_or(20)
    }
}

pub fn parse_conf(path: &Path) -> BTreeMap<String, String> {
    let mut cfg = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return cfg;
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        cfg.insert(
            key.trim().to_string(),
            val.trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string(),
        );
    }
    cfg
}

pub fn load_config(prefix: &Path) -> Config {
    let mut values = BTreeMap::from([
        (
            "STATE_DIR".into(),
            env::var("AM5_SPD_DIAG_STATE_DIR").unwrap_or_else(|_| "/var/log/am5-spd-diag".into()),
        ),
        ("FALLBACK_BOARD".into(), "unknown AM5 board".into()),
        ("FALLBACK_CPU".into(), "AMD Ryzen AM5".into()),
        ("FALLBACK_MEMORY".into(), "DDR5 UDIMM kit".into()),
        ("FALLBACK_BIOS".into(), "unknown".into()),
        ("KEEP_DAYS".into(), "60".into()),
        ("CAPTURE_TIMEOUT_SEC".into(), "20".into()),
    ]);
    let share = env::var("AM5_SPD_DIAG_SHARE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| prefix.to_path_buf());
    for path in [
        share.join("config/default.conf"),
        PathBuf::from("/etc/am5-spd-diag.conf"),
    ] {
        values.extend(parse_conf(&path));
    }
    if let Ok(state) = env::var("AM5_SPD_DIAG_STATE_DIR") {
        values.insert("STATE_DIR".into(), state);
    }
    Config { values }
}
