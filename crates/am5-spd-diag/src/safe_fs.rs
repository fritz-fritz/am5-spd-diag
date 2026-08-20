//! Privileged state writes that do not follow a final-component symlink.
//!
//! Capture runs as root via a passwordless polkit helper. Combined with a
//! world-writable state directory that was a symlink-swap primitive. Directories
//! are now 0755 root:root; these helpers are defense in depth (`O_NOFOLLOW`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const NOFOLLOW: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;

fn opts() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.custom_flags(NOFOLLOW);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

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
}
