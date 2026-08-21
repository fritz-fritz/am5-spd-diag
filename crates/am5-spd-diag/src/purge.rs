//! Wipe and recreate `/var/log/am5-spd-diag` via systemd-tmpfiles.

use crate::paths::{tmpfiles_create_conf, tmpfiles_purge_conf, SYSTEM_STATE_DIR};
use crate::safe_fs::ensure_dir;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

/// Overlayfs and some OBS build roots ETXTBSY a binary that was just written.
fn tmpfiles_status(bin: &Path, flag: &str, conf: &Path) -> Result<ExitStatus, String> {
    let mut delay = Duration::from_millis(5);
    let mut last = None;
    for _ in 0..32 {
        match Command::new(bin).arg(flag).arg(conf).status() {
            Ok(status) => return Ok(status),
            Err(e) if e.raw_os_error() == Some(26) => {
                last = Some(e);
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("systemd-tmpfiles {flag}: {e}")),
        }
    }
    Err(format!(
        "systemd-tmpfiles {flag}: {}",
        last.expect("ETXTBSY retry")
    ))
}

/// How to invoke systemd-tmpfiles for a system-log purge.
pub struct PurgeSystem {
    pub tmpfiles_bin: PathBuf,
    pub remove_conf: PathBuf,
    pub create_conf: PathBuf,
}

impl PurgeSystem {
    pub fn installed() -> Self {
        Self {
            tmpfiles_bin: PathBuf::from("systemd-tmpfiles"),
            remove_conf: tmpfiles_purge_conf(),
            create_conf: tmpfiles_create_conf(),
        }
    }
}

/// Recreate the secured system tree. On `--create` failure, return `Err` so
/// callers skip deleting user XDG copies.
pub fn purge_system_state() -> Result<(), String> {
    purge_system_state_with(&PurgeSystem::installed())
}

pub fn purge_system_state_with(plan: &PurgeSystem) -> Result<(), String> {
    if plan.remove_conf.is_file() {
        let status = tmpfiles_status(&plan.tmpfiles_bin, "--remove", &plan.remove_conf)?;
        if !status.success() {
            return Err(format!(
                "systemd-tmpfiles --remove {} failed",
                plan.remove_conf.display()
            ));
        }
    } else {
        let path = Path::new(SYSTEM_STATE_DIR);
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if resolved != path {
            return Err(format!(
                "refusing to purge {}: not {}",
                resolved.display(),
                SYSTEM_STATE_DIR
            ));
        }
        let _ = std::fs::remove_dir_all(path);
    }
    if plan.create_conf.is_file() {
        let status = tmpfiles_status(&plan.tmpfiles_bin, "--create", &plan.create_conf)?;
        if !status.success() {
            return Err(format!(
                "systemd-tmpfiles --create {} failed",
                plan.create_conf.display()
            ));
        }
    } else {
        ensure_dir(SYSTEM_STATE_DIR).map_err(|e| format!("recreate {SYSTEM_STATE_DIR}: {e}"))?;
    }
    Ok(())
}

/// Wipe the system tree, then user XDG copies. User data is kept if system
/// recreate fails.
pub fn purge_then_user(plan: &PurgeSystem, user_targets: &[PathBuf]) -> Result<(), String> {
    purge_system_state_with(plan)?;
    for path in user_targets {
        let _ = std::fs::remove_dir_all(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn purge_create_failure_returns_err_and_keeps_user_data() {
        let tmp = tempfile::Builder::new()
            .prefix("am5-purge-")
            .tempdir()
            .unwrap();
        let bin =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/tmpfiles-create-fail.sh");
        let mut exec = fs::metadata(&bin).unwrap().permissions();
        exec.set_mode(0o755);
        fs::set_permissions(&bin, exec).unwrap();
        let remove_conf = tmp.path().join("remove.conf");
        let create_conf = tmp.path().join("create.conf");
        fs::write(&remove_conf, "R /var/log/am5-spd-diag\n").unwrap();
        fs::write(&create_conf, "d /var/log/am5-spd-diag 0755 root root -\n").unwrap();

        let user = tmp.path().join("user-data");
        fs::create_dir(&user).unwrap();
        let keep = user.join("report.md");
        fs::write(&keep, b"keep me\n").unwrap();

        let plan = PurgeSystem {
            tmpfiles_bin: bin,
            remove_conf,
            create_conf,
        };
        let err = purge_then_user(&plan, std::slice::from_ref(&user)).unwrap_err();
        assert!(
            err.contains("systemd-tmpfiles --create"),
            "unexpected error: {err}"
        );
        assert_eq!(fs::read(&keep).unwrap(), b"keep me\n");
    }

    #[test]
    fn purge_create_success_removes_user_data() {
        let tmp = tempfile::Builder::new()
            .prefix("am5-purge-ok-")
            .tempdir()
            .unwrap();
        let bin = PathBuf::from("/bin/true");
        let remove_conf = tmp.path().join("remove.conf");
        let create_conf = tmp.path().join("create.conf");
        fs::write(&remove_conf, "R /var/log/am5-spd-diag\n").unwrap();
        fs::write(&create_conf, "d /var/log/am5-spd-diag 0755 root root -\n").unwrap();

        let user = tmp.path().join("user-data");
        fs::create_dir(&user).unwrap();
        fs::write(user.join("report.md"), b"gone\n").unwrap();

        let plan = PurgeSystem {
            tmpfiles_bin: bin,
            remove_conf,
            create_conf,
        };
        purge_then_user(&plan, std::slice::from_ref(&user)).unwrap();
        assert!(!user.exists());
    }
}
