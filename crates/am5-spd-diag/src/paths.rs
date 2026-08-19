use std::env;
use std::path::{Path, PathBuf};

pub fn exe_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("am5-spd-diag"))
}

pub fn detect_share_libexec() -> (PathBuf, PathBuf) {
    if let (Ok(share), Ok(libexec)) = (env::var("AM5_SPD_DIAG_SHARE"), env::var("AM5_SPD_DIAG_LIBEXEC")) {
        return (PathBuf::from(share), PathBuf::from(libexec));
    }
    let exe = exe_path();
    let exe_s = exe.to_string_lossy();
    if exe_s == "/usr/bin/am5-spd-diag"
        || exe_s == "/usr/local/bin/am5-spd-diag"
        || is_installed_pkexec(&exe_s)
    {
        if exe_s.starts_with("/usr/local/") {
            return (
                PathBuf::from("/usr/local/share/am5-spd-diag"),
                PathBuf::from("/usr/local/libexec/am5-spd-diag"),
            );
        }
        return (
            PathBuf::from("/usr/share/am5-spd-diag"),
            PathBuf::from("/usr/libexec/am5-spd-diag"),
        );
    }
    if let Some(dir) = exe.parent() {
        if dir.join("../templates").is_dir() || dir.join("../config/default.conf").is_file() {
            let root = dir.join("..");
            return (root.clone(), dir.to_path_buf());
        }
        if dir.join("templates").is_dir() {
            return (dir.to_path_buf(), dir.join("libexec"));
        }
        // cargo target dir: crates/am5-spd-diag -> repo root
        if let Some(root) = find_repo_root(dir) {
            return (root.clone(), root.join("libexec"));
        }
    }
    (
        PathBuf::from("/usr/share/am5-spd-diag"),
        PathBuf::from("/usr/libexec/am5-spd-diag"),
    )
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    for anc in start.ancestors() {
        if anc.join("templates/ticket.md.tmpl").is_file() && anc.join("config/default.conf").is_file() {
            return Some(anc.to_path_buf());
        }
    }
    None
}

pub fn share_dir() -> PathBuf {
    env::var("AM5_SPD_DIAG_SHARE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| detect_share_libexec().0)
}

pub fn libexec_dir() -> PathBuf {
    env::var("AM5_SPD_DIAG_LIBEXEC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| detect_share_libexec().1)
}

pub fn is_snapshot_helper() -> bool {
    helper_kind() == Some(HelperKind::Snapshot)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    Snapshot,
    Probe,
    Recover,
}

pub fn helper_kind_from_argv0(argv0: &str) -> Option<HelperKind> {
    match Path::new(argv0).file_name().and_then(|n| n.to_str()) {
        Some("pkexec-snapshot") => Some(HelperKind::Snapshot),
        Some("pkexec-probe") => Some(HelperKind::Probe),
        Some("pkexec-recover") => Some(HelperKind::Recover),
        _ => None,
    }
}

pub fn helper_kind() -> Option<HelperKind> {
    env::args()
        .next()
        .and_then(|argv0| helper_kind_from_argv0(&argv0))
}

fn is_installed_pkexec(exe_s: &str) -> bool {
    let name = Path::new(exe_s).file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(name, "pkexec-snapshot" | "pkexec-probe" | "pkexec-recover")
        && (exe_s.starts_with("/usr/libexec/") || exe_s.starts_with("/usr/local/libexec/"))
}

pub fn pkexec_helper_path(kind: HelperKind) -> PathBuf {
    let name = match kind {
        HelperKind::Snapshot => "pkexec-snapshot",
        HelperKind::Probe => "pkexec-probe",
        HelperKind::Recover => "pkexec-recover",
    };
    let installed = PathBuf::from(format!("/usr/libexec/am5-spd-diag/{name}"));
    if installed.is_file() {
        return installed;
    }
    let local = libexec_dir().join(name);
    if local.is_file() {
        return local;
    }
    installed
}

pub fn run_pkexec_helper(kind: HelperKind) -> Result<std::process::Output, String> {
    let helper = pkexec_helper_path(kind);
    if !helper.is_file() {
        return Err(format!(
            "polkit helper missing ({})",
            helper.display()
        ));
    }
    std::process::Command::new("pkexec")
        .arg(&helper)
        .output()
        .map_err(|e| format!("pkexec {}: {e}", helper.display()))
}

pub fn pin_helper_paths() {
    env::remove_var("AM5_SPD_DIAG_STATE_DIR");
    env::remove_var("AM5_SPD_DIAG_LIBEXEC");
    env::remove_var("AM5_SPD_DIAG_SHARE");
    env::remove_var("AM5_SPD_DIAG_PREFIX");
    env::remove_var("PYTHONPATH");
    env::remove_var("PYTHONHOME");
    env::remove_var("PYTHONUSERBASE");
    env::remove_var("LD_PRELOAD");
    env::remove_var("LD_LIBRARY_PATH");
    let exe = exe_path();
    let here = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let here_s = here.to_string_lossy();
    match here_s.as_ref() {
        "/usr/libexec/am5-spd-diag" => {
            env::set_var("AM5_SPD_DIAG_SHARE", "/usr/share/am5-spd-diag");
            env::set_var("AM5_SPD_DIAG_LIBEXEC", "/usr/libexec/am5-spd-diag");
        }
        "/usr/local/libexec/am5-spd-diag" => {
            env::set_var("AM5_SPD_DIAG_SHARE", "/usr/local/share/am5-spd-diag");
            env::set_var("AM5_SPD_DIAG_LIBEXEC", "/usr/local/libexec/am5-spd-diag");
        }
        _ => {
            if let Some(root) = here.parent() {
                env::set_var("AM5_SPD_DIAG_SHARE", root);
            }
            env::set_var("AM5_SPD_DIAG_LIBEXEC", &here);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_kind_uses_invocation_name_not_target_path() {
        assert_eq!(
            helper_kind_from_argv0("pkexec-snapshot"),
            Some(HelperKind::Snapshot)
        );
        assert_eq!(
            helper_kind_from_argv0("/usr/libexec/am5-spd-diag/pkexec-probe"),
            Some(HelperKind::Probe)
        );
        assert_eq!(
            helper_kind_from_argv0("/usr/libexec/am5-spd-diag/pkexec-recover"),
            Some(HelperKind::Recover)
        );
        assert_eq!(helper_kind_from_argv0("am5-spd-diag"), None);
        assert_eq!(helper_kind_from_argv0("/usr/bin/am5-spd-diag"), None);
        assert_eq!(helper_kind_from_argv0("snapshot"), None);
    }
}
