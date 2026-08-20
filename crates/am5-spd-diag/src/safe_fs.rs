//! Privileged state writes that do not follow a final-component symlink.
//!
//! Capture runs as root via a passwordless polkit helper. Combined with a
//! world-writable state directory that was a symlink-swap primitive. Directories
//! are now 0755 root:root; these helpers are defense in depth (`O_NOFOLLOW`).
//!
//! File and directory creation still honors umask, so Snapshot/Recover/capture
//! must call [`set_privileged_umask`] (`022`) before creating state. Combined
//! with explicit 0644/0755, that yields the documented world-readable layout
//! even if the helper inherited umask 000 or 077.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

const NOFOLLOW: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Documented capture-tree file mode (`0666 & !022`).
pub const FILE_MODE: u32 = 0o644;
/// Documented capture-tree directory mode (`0777 & !022`).
pub const DIR_MODE: u32 = 0o755;
/// Known umask so privileged creates are world-readable and not world-writable.
pub const PRIVILEGED_UMASK: libc::mode_t = 0o022;

/// Pin umask to [`PRIVILEGED_UMASK`] at the privileged-helper / capture boundary.
pub fn set_privileged_umask() {
    // SAFETY: umask(2) only changes this process's file-creation mask.
    unsafe {
        libc::umask(PRIVILEGED_UMASK);
    }
}

fn opts() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.custom_flags(NOFOLLOW);
    opts.mode(FILE_MODE);
    opts
}

pub fn write_nofollow(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let mut f = opts()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path.as_ref())?;
    f.write_all(contents.as_ref())?;
    Ok(())
}

pub fn open_append_nofollow(path: impl AsRef<Path>) -> io::Result<File> {
    opts().create(true).append(true).open(path.as_ref())
}

pub fn create_nofollow(path: impl AsRef<Path>) -> io::Result<File> {
    opts()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path.as_ref())
}

pub fn copy_nofollow(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> io::Result<u64> {
    let data = std::fs::read(src.as_ref())?;
    let n = data.len() as u64;
    write_nofollow(dest, data)?;
    Ok(n)
}

/// Create `path` as a real 0755 directory.
///
/// If a symlink, regular file, or other non-directory is already there (legacy
/// 2775 `root:users` tree), remove that object first. Does not follow a final
/// component symlink.
pub fn ensure_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => {
            fs::remove_file(path).or_else(|_| fs::remove_dir(path))?;
            mkdir_0755(path)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => mkdir_0755(path),
        Err(e) => Err(e),
    }
}

fn mkdir_0755(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::Command;

    fn tmp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("am5-safe-fs-")
            .tempdir()
            .expect("tmpdir")
    }

    #[test]
    fn write_refuses_symlink() {
        let dir = tmp();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::write(&target, b"orig").unwrap();
        symlink(&target, &link).unwrap();
        let err = write_nofollow(&link, b"pwn").unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
        assert_eq!(std::fs::read(&target).unwrap(), b"orig");
    }

    #[test]
    fn write_creates_regular_file() {
        let dir = tmp();
        let path = dir.path().join("SPD_NOW");
        write_nofollow(&path, b"healthy\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"healthy\n");
    }

    #[test]
    fn ensure_dir_replaces_legacy_events_and_latest_symlinks() {
        let dir = tmp();
        let state = dir.path();
        let evil = dir.path().join("evil");
        fs::create_dir(&evil).unwrap();
        fs::write(evil.join("pwn"), b"x").unwrap();
        symlink(&evil, state.join("events")).unwrap();
        symlink(&evil, state.join("latest")).unwrap();

        ensure_dir(state.join("events")).unwrap();
        ensure_dir(state.join("latest")).unwrap();

        let events_meta = fs::symlink_metadata(state.join("events")).unwrap();
        let latest_meta = fs::symlink_metadata(state.join("latest")).unwrap();
        assert!(events_meta.file_type().is_dir());
        assert!(!events_meta.file_type().is_symlink());
        assert!(latest_meta.file_type().is_dir());
        assert!(!latest_meta.file_type().is_symlink());
        assert!(evil.join("pwn").is_file());
        assert!(!state.join("events").join("pwn").exists());
    }

    #[test]
    fn umask_000_child() {
        let Ok(root) = std::env::var("AM5_TEST_UMASK_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        // SAFETY: isolated test child; umask only affects this process.
        unsafe {
            libc::umask(0o000);
        }
        set_privileged_umask();
        ensure_dir(root.join("events")).unwrap();
        write_nofollow(root.join("SPD_NOW"), b"healthy\n").unwrap();
    }

    #[test]
    fn state_creation_with_umask_000_is_0644_0755() {
        if std::env::var_os("AM5_TEST_UMASK_ROOT").is_some() {
            return;
        }
        let dir = tmp();
        let root = dir.path();
        let exe = std::env::current_exe().expect("test exe");
        let status = Command::new(&exe)
            .args(["--exact", "safe_fs::tests::umask_000_child"])
            .env("AM5_TEST_UMASK_ROOT", root)
            .status()
            .expect("spawn umask child");
        assert!(status.success(), "umask child failed: {status:?}");
        let file_mode = fs::metadata(root.join("SPD_NOW"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = fs::metadata(root.join("events"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, FILE_MODE);
        assert_eq!(dir_mode, DIR_MODE);
    }
}
