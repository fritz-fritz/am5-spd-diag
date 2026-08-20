use std::env;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub fn exe_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("am5-spd-diag"))
}

pub fn detect_share_libexec() -> (PathBuf, PathBuf) {
    if let (Ok(share), Ok(libexec)) = (
        env::var("AM5_SPD_DIAG_SHARE"),
        env::var("AM5_SPD_DIAG_LIBEXEC"),
    ) {
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
        if anc.join("templates/ticket.md.tmpl").is_file()
            && anc.join("config/default.conf").is_file()
        {
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
    let name = Path::new(exe_s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
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
        return Err(format!("polkit helper missing ({})", helper.display()));
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
    let here = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
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

/// Packaged capture tree. Privileged purge only removes this path, via tmpfiles.
pub const SYSTEM_STATE_DIR: &str = "/var/log/am5-spd-diag";

pub fn home_for_user(name: &str) -> Option<PathBuf> {
    Some(passwd_uid_home(name)?.1)
}

fn uid_for_user(name: &str) -> Option<u32> {
    Some(passwd_uid_home(name)?.0)
}

fn passwd_uid_home(name: &str) -> Option<(u32, PathBuf)> {
    let c = std::ffi::CString::new(name).ok()?;
    let pwd = unsafe { libc::getpwnam(c.as_ptr()) };
    if pwd.is_null() {
        return None;
    }
    let uid = unsafe { (*pwd).pw_uid };
    let dir = unsafe { std::ffi::CStr::from_ptr((*pwd).pw_dir) };
    Some((uid, PathBuf::from(dir.to_string_lossy().as_ref())))
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

/// XDG user data directory for reports/packages (`$XDG_DATA_HOME/am5-spd-diag`).
///
/// When running as root, ignore `XDG_DATA_HOME` / `HOME` (they survive `sudo -E`)
/// and use `SUDO_USER`'s passwd home instead.
pub fn user_data_dir() -> PathBuf {
    if euid() == 0 {
        if let Ok(sudo_user) = env::var("SUDO_USER") {
            if sudo_user != "root" {
                if let Some(home) = home_for_user(&sudo_user) {
                    return home.join(".local/share/am5-spd-diag");
                }
            }
        }
        return PathBuf::from("/root/.local/share/am5-spd-diag");
    }
    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            return xdg.join("am5-spd-diag");
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/".into());
    if home.is_empty() || home == "/" {
        return PathBuf::from(".local/share/am5-spd-diag");
    }
    PathBuf::from(home).join(".local/share/am5-spd-diag")
}

/// True if `path` is the default per-user data dir under `home`.
pub fn is_default_user_data_dir(path: &Path, home: &Path) -> bool {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    resolved == home.join(".local/share/am5-spd-diag")
}

fn is_real_dir_owned_by(path: &Path, uid: u32) -> bool {
    let Ok(meta) = path.symlink_metadata() else {
        return false;
    };
    meta.file_type().is_dir() && meta.uid() == uid
}

/// User-owned XDG data dirs that unprivileged (or `SUDO_USER`) purge may delete.
pub fn user_purge_targets() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if euid() == 0 {
        if let Ok(sudo_user) = env::var("SUDO_USER") {
            if sudo_user != "root" {
                if let Some(home) = home_for_user(&sudo_user) {
                    let dir = home.join(".local/share/am5-spd-diag");
                    let uid = uid_for_user(&sudo_user).unwrap_or(u32::MAX);
                    if is_real_dir_owned_by(&dir, uid) && is_default_user_data_dir(&dir, &home) {
                        out.push(dir);
                    }
                }
            }
        }
        return out;
    }
    let dir = user_data_dir();
    if dir.file_name().and_then(|n| n.to_str()) != Some("am5-spd-diag") {
        return out;
    }
    let resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    if resolved == Path::new(SYSTEM_STATE_DIR) {
        return out;
    }
    if is_real_dir_owned_by(&dir, euid()) {
        out.push(resolved);
    }
    out
}

pub fn tmpfiles_purge_conf() -> PathBuf {
    let installed = share_dir().join("tmpfiles-purge.conf");
    if installed.is_file() {
        return installed;
    }
    share_dir().join("systemd/am5-spd-diag-purge.conf")
}

pub fn tmpfiles_create_conf() -> PathBuf {
    for path in [
        "/usr/lib/tmpfiles.d/am5-spd-diag.conf",
        "/usr/local/lib/tmpfiles.d/am5-spd-diag.conf",
    ] {
        let path = PathBuf::from(path);
        if path.is_file() {
            return path;
        }
    }
    share_dir().join("systemd/am5-spd-diag.tmpfiles.conf")
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

    #[test]
    fn default_user_data_dir_is_under_home_share() {
        let home = Path::new("/home/user");
        assert!(is_default_user_data_dir(
            Path::new("/home/user/.local/share/am5-spd-diag"),
            home
        ));
        assert!(!is_default_user_data_dir(
            Path::new("/media/usb/.local/share/am5-spd-diag"),
            home
        ));
        assert!(!is_default_user_data_dir(
            Path::new("/home/user/.local/share/am5-spd-diag-evil"),
            home
        ));
        assert!(!is_default_user_data_dir(Path::new("/etc/ssh"), home));
        assert!(!is_default_user_data_dir(
            Path::new("/var/log/am5-spd-diag"),
            home
        ));
        assert!(!is_default_user_data_dir(
            Path::new("/etc/am5-spd-diag"),
            home
        ));
        assert!(!is_default_user_data_dir(Path::new("am5-spd-diag"), home));
    }
}
