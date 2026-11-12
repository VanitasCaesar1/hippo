//! Helpers the modules share: SHA-256 in the lowercase hex records use,
//! atomic replacement of a file, and creation that never overwrites.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Lowercase hex, the form hashes take in records.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Copies the files under `from` to `to`, creating directories. Links are
/// not followed and not copied.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(io::Error::from)?;
        let relative = entry.path().strip_prefix(from).expect("under its root");
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|e| Error::io_at(&target, e))?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &target).map_err(|e| Error::io_at(&target, e))?;
        }
    }
    Ok(())
}

/// Replaces `path` with `bytes` atomically. A hidden temporary file beside
/// it, `.<name>.tmp` (for a record, `.<id>.json.tmp`), is written and
/// synced, takes the old file's permissions, and is renamed over `path`. A
/// reader sees the old bytes or the new ones, never a mix.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(temp) = temp_beside(path) else {
        let err = io::Error::new(io::ErrorKind::InvalidInput, "not a file path");
        return Err(Error::io_at(path, err));
    };
    let permissions = fs::metadata(path).ok().map(|m| m.permissions());
    let written = File::create(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, path));
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(Error::io_at(path, err));
    }
    Ok(())
}

/// Creates `path` holding `bytes` and never replaces an existing file: the
/// name is claimed with `create_new`, then filled by [`write_atomic`].
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| Error::io_at(path, e))?;
    write_atomic(path, bytes).inspect_err(|_| {
        let _ = fs::remove_file(path);
    })
}

/// `.<name>.tmp` beside `path`; `None` when `path` has no file name.
fn temp_beside(path: &Path) -> Option<PathBuf> {
    let mut name = OsString::from(".");
    name.push(path.file_name()?);
    name.push(".tmp");
    Some(path.with_file_name(name))
}

/// The visible `*.json` files in `dir`, sorted; none when `dir` is missing.
/// Hidden files, such as a write's temporary file, are passed over.
pub(crate) fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io_at(dir, err)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(dir, e))?;
        let path = entry.path();
        let hidden = entry.file_name().as_encoded_bytes().starts_with(b".");
        let is_json = path.extension().is_some_and(|ext| ext == "json");
        if !hidden && is_json && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Makes the tests that spawn processes and the tests that take the ingest
/// lock run one at a time. While the kernel spawns a child, the child holds
/// a copy of every open file of the test process until it execs, the ingest
/// lock among them, so an ingest that drops its lock and takes it again
/// can find it still held. A real `hippocamp` process never spawns while it
/// holds that lock.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restores the default action for `SIGPIPE` on Unix, so a command whose
/// output goes into a closed pipe (`hippocamp dream | head`) ends quietly,
/// as other command-line tools do, instead of panicking in `println!`.
/// Does nothing elsewhere.
pub fn default_sigpipe() {
    #[cfg(unix)]
    // SAFETY: called once at the start of `main`, before any other thread
    // exists; setting a signal's disposition to its default is sound.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_in_lowercase_hex() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn write_atomic_replaces_and_keeps_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        fs::write(&path, "old\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        write_atomic(&path, b"new\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert!(!temp_beside(&path).unwrap().exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn write_new_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.md");
        write_new(&path, b"first\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
        let err = write_new(&path, b"second\n").unwrap_err();
        assert!(
            matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::AlreadyExists),
            "{err}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"first\n");
        assert!(!temp_beside(&path).unwrap().exists());
    }
}
