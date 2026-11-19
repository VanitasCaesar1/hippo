//! Permissions under `$HIPPO_HOME`: every directory 0700 and every file
//! 0600, the index included (`docs/designs/common.md`, rule 4).
//!
//! Three parts keep that true. [`private_umask`] makes everything a hippo
//! process creates private from the start; the CLI and the app call it
//! first. [`tighten_home`] closes `$HIPPO_HOME` itself when an older hippo
//! left it open, so nothing in it is reachable by another account even
//! before the files are fixed. [`loose`] lists what is still group- or
//! world-accessible, for `doctor`, and [`fix`] tightens it for `hippo
//! doctor --fix-permissions`. Off Unix all four do nothing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The mode hippo gives a directory it creates.
pub const DIR_MODE: u32 = 0o700;
/// The mode hippo gives a file it creates.
pub const FILE_MODE: u32 = 0o600;

/// Sets this process's umask to `077`, so files and directories it
/// creates, and those its children create unless they set their own, are
/// private to the person. Returns the umask it replaced.
pub fn private_umask() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: umask has no preconditions and cannot fail.
        let old = unsafe { libc::umask(0o077) };
        #[allow(clippy::unnecessary_cast)]
        return old as u32;
    }
    #[cfg(not(unix))]
    0
}

/// The permission bits of `path` (not following a link), or `None` when
/// it cannot be read or off Unix.
pub fn mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::symlink_metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Creates `dir` and its missing parents, then makes `dir` itself 0700.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    set_mode(dir, DIR_MODE).map_err(|e| Error::io_at(dir, e))
}

/// When `home` exists and another account can enter or read it, makes it
/// 0700 and returns the mode it had, so the caller can say so once. A
/// missing home, or one that is already private, returns `None`.
pub fn tighten_home(home: &Path) -> Option<u32> {
    let old = mode(home)?;
    let is_dir = fs::symlink_metadata(home).is_ok_and(|m| m.is_dir());
    if !is_dir || old & 0o077 == 0 {
        return None;
    }
    set_mode(home, old & !0o077 | 0o700).ok()?;
    Some(old)
}

/// One path under `$HIPPO_HOME` that another account could read, enter or
/// write, with its mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loose {
    pub path: PathBuf,
    pub mode: u32,
    pub is_dir: bool,
}

/// Every file and directory under `home`, `home` included, whose mode
/// gives the group or others any access. Links are neither followed nor
/// reported: their own mode means nothing.
pub fn loose(home: &Path) -> Vec<Loose> {
    let mut out = Vec::new();
    if !home.exists() {
        return out;
    }
    for entry in walkdir::WalkDir::new(home)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let ft = entry.file_type();
        if ft.is_symlink() {
            continue;
        }
        let Some(mode) = mode(entry.path()) else {
            continue;
        };
        if mode & 0o077 != 0 {
            out.push(Loose {
                path: entry.path().to_path_buf(),
                mode,
                is_dir: ft.is_dir(),
            });
        }
    }
    out
}

/// Takes the group's and others' bits off every path [`loose`] lists, and
/// gives the owner at least read and write (and search, for a directory).
/// Returns how many it changed; the first failure stops it.
pub fn fix(home: &Path) -> Result<usize> {
    let found = loose(home);
    for item in &found {
        let base = if item.is_dir { DIR_MODE } else { FILE_MODE };
        set_mode(&item.path, item.mode & 0o700 | base).map_err(|e| Error::io_at(&item.path, e))?;
    }
    Ok(found.len())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn loose_lists_group_and_world_access_and_fix_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        set_mode(&home, 0o755).unwrap();
        fs::write(home.join("index.sqlite"), "x").unwrap();
        set_mode(&home.join("index.sqlite"), 0o644).unwrap();
        fs::write(home.join("ok.json"), "x").unwrap();
        set_mode(&home.join("ok.json"), 0o600).unwrap();
        fs::create_dir(home.join("lessons")).unwrap();
        set_mode(&home.join("lessons"), 0o750).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", home.join("link")).unwrap();

        let found = loose(&home);
        let names: Vec<_> = found
            .iter()
            .map(|l| l.path.strip_prefix(&home).unwrap().display().to_string())
            .collect();
        assert_eq!(found.len(), 3, "{names:?}");
        assert!(names.contains(&"index.sqlite".into()));
        assert!(names.contains(&"lessons".into()));

        assert_eq!(fix(&home).unwrap(), 3);
        assert!(loose(&home).is_empty());
        assert_eq!(mode(&home), Some(0o700));
        assert_eq!(mode(&home.join("index.sqlite")), Some(0o600));
        assert_eq!(mode(&home.join("lessons")), Some(0o700));
    }

    #[test]
    fn tighten_home_closes_only_an_open_home_and_says_what_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        set_mode(&home, 0o755).unwrap();
        assert_eq!(tighten_home(&home), Some(0o755));
        assert_eq!(mode(&home), Some(0o700));
        assert_eq!(tighten_home(&home), None);
        assert_eq!(tighten_home(&dir.path().join("missing")), None);
    }

    #[test]
    fn a_private_dir_is_0700_whatever_the_umask() {
        let dir = tempfile::tempdir().unwrap();
        let made = dir.path().join("a").join("b");
        create_private_dir(&made).unwrap();
        assert_eq!(mode(&made), Some(0o700));
    }
}
