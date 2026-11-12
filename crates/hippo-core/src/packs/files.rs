//! A pack as files in memory: reading a directory, and the path, layout,
//! text and size rules and the pack hash.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use walkdir::WalkDir;

pub use super::MAX_FILE_BYTES;
use crate::error::{Error, Result};
use crate::util::{hex, sha256};

/// Files outside `tests/`, in bytes in all.
pub const MAX_OUTSIDE_TESTS_BYTES: usize = 131_072;
/// The whole pack, in bytes.
pub const MAX_PACK_BYTES: usize = 524_288;
/// Files in a pack.
pub const MAX_FILES: usize = 100;
/// Components in a path.
pub const MAX_DEPTH: usize = 6;
/// Bytes in a path.
pub const MAX_PATH_BYTES: usize = 200;

/// The files of a pack: path (`/`-separated, relative to the pack) to bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackFiles(BTreeMap<String, Vec<u8>>);

impl PackFiles {
    /// The bytes of `path`, if the pack has it.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.0.get(path).map(Vec::as_slice)
    }

    /// Every path, in byte order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Every path with its bytes, in byte order of path.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.0.iter().map(|(p, b)| (p.as_str(), b.as_slice()))
    }

    /// The number of files.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there is no file.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The bytes of all files together.
    pub fn total_bytes(&self) -> usize {
        self.0.values().map(Vec::len).sum()
    }

    pub(crate) fn insert(&mut self, path: String, bytes: Vec<u8>) {
        self.0.insert(path, bytes);
    }
}

/// `<path>: <why>`, escaped: a directory entry's name is outside text, and
/// `read_dir` names it before any path rule has looked at it.
fn refused(path: &str, why: &str) -> Error {
    Error::Pack(super::escape(&format!("{path}: {why}")))
}

/// `O_NOFOLLOW` for the targets where its value is known; 0 elsewhere, where
/// the `symlink_metadata` checks alone stand.
#[cfg(unix)]
const O_NOFOLLOW: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    0x0100
} else if cfg!(all(
    target_os = "linux",
    any(target_arch = "x86", target_arch = "x86_64")
)) {
    0o400000
} else if cfg!(all(
    target_os = "linux",
    any(target_arch = "aarch64", target_arch = "arm")
)) {
    0o100000
} else {
    0
};

/// Opens `path` for reading, refusing a symbolic link at the last
/// component where the target allows it.
fn open_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NOFOLLOW);
    }
    options.open(path)
}

/// Whether the opened file has more than one name: a hard link to a file
/// outside the pack would be read as if it were the pack's own. Not
/// detectable off Unix, where this is `false`.
#[cfg(unix)]
fn has_other_links(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
fn has_other_links(_meta: &fs::Metadata) -> bool {
    false
}

/// The most `skipped` paths [`read_dir`] lists; the rest are counted.
const MAX_SKIPPED: usize = 100;

/// Reads the pack in `dir`, and returns it with the paths it skipped.
///
/// Before any other check it skips every entry whose name starts with `.`
/// (not descending into it) and an `origin.json` at the root, decided from
/// the raw name before the UTF-8 check. It refuses a symbolic link and
/// anything that is neither a regular file nor a directory, and a file with
/// more than one hard link. A file is
/// opened without following links, and at most [`MAX_FILE_BYTES`] + 1 bytes
/// of it are read. It stops with an error at the first file past
/// [`MAX_FILES`] files, and at the first file that takes the running total
/// over [`MAX_PACK_BYTES`], so a huge directory is never read whole. After
/// 100 skipped paths the list ends with one line `… and <n> more`. A `dir`
/// that is not a directory is an error.
pub fn read_dir(dir: &Path) -> Result<(PackFiles, Vec<String>)> {
    let root = fs::symlink_metadata(dir).map_err(|e| Error::io_at(dir, e))?;
    if !root.is_dir() && !root.file_type().is_symlink() {
        return Err(Error::Pack(format!("{}: not a directory", dir.display())));
    }
    let mut files = PackFiles::default();
    let mut skipped = Vec::new();
    let mut more = 0usize;
    let mut total = 0usize;
    let mut walk = WalkDir::new(dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    while let Some(entry) = walk.next() {
        let entry = entry.map_err(|e| Error::Pack(format!("{}: {e}", dir.display())))?;
        if entry.depth() == 0 {
            if entry.file_type().is_symlink() {
                return Err(refused(
                    &entry.path().display().to_string(),
                    "a symbolic link; packs may not contain links",
                ));
            }
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(dir)
            .expect("an entry is below the directory");
        let name = entry.file_name().as_encoded_bytes();
        if name.starts_with(b".") || (entry.depth() == 1 && name == b"origin.json") {
            if entry.file_type().is_dir() {
                walk.skip_current_dir();
            }
            if skipped.len() < MAX_SKIPPED {
                skipped.push(relative.to_string_lossy().replace('\\', "/"));
            } else {
                more += 1;
            }
            continue;
        }
        let mut parts = Vec::new();
        for component in relative.components() {
            let Some(part) = component.as_os_str().to_str() else {
                let lossy = relative.to_string_lossy();
                return Err(refused(&lossy, "not UTF-8"));
            };
            parts.push(part);
        }
        let path = parts.join("/");
        let kind = entry.file_type();
        if kind.is_symlink() {
            return Err(refused(
                &path,
                "a symbolic link; packs may not contain links",
            ));
        }
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(refused(&path, "not a regular file or directory"));
        }
        if files.len() >= MAX_FILES {
            return Err(Error::Pack(format!("pack: more than {MAX_FILES} files")));
        }
        let mut file = open_no_follow(entry.path()).map_err(|e| Error::io_at(entry.path(), e))?;
        let meta = file
            .metadata()
            .map_err(|e| Error::Pack(format!("{path}: {e}")))?;
        if !meta.is_file() {
            return Err(refused(&path, "not a regular file or directory"));
        }
        if has_other_links(&meta) {
            return Err(refused(&path, "a hard link; packs may not contain links"));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io_at(entry.path(), e))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            let size = meta.len().max(bytes.len() as u64);
            return Err(refused(
                &path,
                &format!("{size} bytes, over the {MAX_FILE_BYTES}-byte limit"),
            ));
        }
        total += bytes.len();
        if total > MAX_PACK_BYTES {
            return Err(Error::Pack(format!(
                "pack: {total} bytes, over the {MAX_PACK_BYTES}-byte limit"
            )));
        }
        files.insert(path, bytes);
    }
    if more > 0 {
        skipped.push(format!("… and {more} more"));
    }
    Ok((files, skipped))
}

/// The path rules: one to six components of at most 200 bytes in all, each
/// matching `^[A-Za-z0-9][A-Za-z0-9 ._-]{0,99}$` and not ending in a space
/// or a dot. The error is `<path>: breaks the path rules (<why>)`.
pub fn check_path(path: &str) -> std::result::Result<(), String> {
    let breaks = |why: &str| format!("{path}: breaks the path rules ({why})");
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > MAX_DEPTH {
        return Err(breaks("more than 6 components"));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(breaks("over 200 bytes"));
    }
    for component in components {
        let bytes = component.as_bytes();
        let allowed = (1..=100).contains(&bytes.len())
            && bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'))
            && !component.ends_with([' ', '.']);
        if !allowed {
            return Err(breaks(&format!("component \"{component}\" is not allowed")));
        }
    }
    Ok(())
}

/// Whether `s` matches `^[a-z0-9]+(-[a-z0-9]+)*$`.
pub(crate) fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether `path` is a file the layout table allows.
fn allowed(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let stem_of = |name: &str, ext: &str| name.strip_suffix(ext).map(str::to_owned);
    let slug_file = |name: &str, ext: &str| stem_of(name, ext).is_some_and(|s| is_slug(&s));
    let root = matches!(
        path,
        "pack.toml" | "README.md" | "LICENSE" | "LICENSE.md" | "origin.json"
    );
    if root {
        return true;
    }
    match parts.as_slice() {
        ["passes", name] => slug_file(name, ".toml"),
        ["prompts" | "personas", name] => slug_file(name, ".md"),
        ["templates", .., last] => last.ends_with(".md"),
        ["tests", case, "case.toml"] => is_slug(case),
        ["tests", case, "notes", .., last] => is_slug(case) && last.ends_with(".md"),
        ["tests", case, "transcripts", name] => is_slug(case) && name.ends_with(".jsonl"),
        _ => false,
    }
}

/// The pack layout table, and case-insensitive equality of paths and of
/// directory prefixes. One message per problem.
pub fn check_layout(files: &PackFiles) -> Vec<String> {
    let mut errors = Vec::new();
    for path in files.paths() {
        if !allowed(path) {
            errors.push(format!("unexpected file {path}"));
        }
    }
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut reported = BTreeSet::new();
    for path in files.paths() {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let key = prefix.to_ascii_lowercase();
            match seen.get(&key) {
                Some(first) if *first != prefix => {
                    if reported.insert((first.clone(), prefix.clone())) {
                        errors.push(format!("{first} and {prefix} differ only in case"));
                    }
                }
                Some(_) => {}
                None => {
                    seen.insert(key, prefix.clone());
                }
            }
        }
    }
    errors
}

/// The code points a human cannot see but a model reads: sorted, disjoint
/// ranges. Unassigned code points are not listed.
const DENIED: &[(u32, u32)] = &[
    (0x0000, 0x0008),
    (0x000B, 0x000C),
    (0x000E, 0x001F),
    (0x007F, 0x009F),
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x2028, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xE000, 0xF8FF),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFFB),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
    (0xF0000, 0xFFFFD),
    (0x10_0000, 0x10_FFFD),
];

/// Whether `c` is one of the denied code points of the table above. The
/// desktop app escapes the same set when it shows text from hippo's files.
pub fn is_denied(c: char) -> bool {
    let c = u32::from(c);
    DENIED
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The text rules: UTF-8, and none of the denied code points. The error is
/// `<path>: not UTF-8` or `<path>:<line>:<column>: U+<hex>`, line and
/// column 1-based, the column in code points.
pub fn check_text(path: &str, bytes: &[u8]) -> std::result::Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{path}: not UTF-8"))?;
    let (mut line, mut column) = (1, 1);
    for c in text.chars() {
        if is_denied(c) {
            return Err(format!("{path}:{line}:{column}: U+{:04X}", u32::from(c)));
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Ok(())
}

/// The size rules.
pub fn check_sizes(files: &PackFiles) -> Vec<String> {
    let mut errors = Vec::new();
    let mut outside = 0;
    for (path, bytes) in files.iter() {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            errors.push(format!(
                "{path}: {} bytes, over the {MAX_FILE_BYTES}-byte limit",
                bytes.len()
            ));
        }
        if !path.starts_with("tests/") {
            outside += bytes.len();
        }
    }
    if outside > MAX_OUTSIDE_TESTS_BYTES {
        errors.push(format!(
            "files outside tests/: {outside} bytes, over the {MAX_OUTSIDE_TESTS_BYTES}-byte limit"
        ));
    }
    if files.total_bytes() > MAX_PACK_BYTES {
        errors.push(format!(
            "pack: {} bytes, over the {MAX_PACK_BYTES}-byte limit",
            files.total_bytes()
        ));
    }
    if files.len() > MAX_FILES {
        errors.push(format!(
            "pack: {} files, over the {MAX_FILES}-file limit",
            files.len()
        ));
    }
    errors
}

/// The pack hash: the lowercase hex SHA-256 of one line `<path> NUL <hex
/// SHA-256 of the file> LF` per file, in byte order of path, `origin.json`
/// left out.
pub fn pack_hash(files: &PackFiles) -> String {
    let mut text = Vec::new();
    for (path, bytes) in files.iter().filter(|(path, _)| *path != "origin.json") {
        text.extend_from_slice(path.as_bytes());
        text.push(0);
        text.extend_from_slice(hex(&sha256(bytes)).as_bytes());
        text.push(b'\n');
    }
    hex(&sha256(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(paths: &[&str]) -> PackFiles {
        let mut files = PackFiles::default();
        for path in paths {
            files.insert((*path).to_owned(), b"x".to_vec());
        }
        files
    }

    #[test]
    fn path_rules() {
        assert!(check_path("passes/a.toml").is_ok());
        assert!(check_path("templates/My Note 2.md").is_ok());
        for bad in [
            "../x", "/x", "a\\b", ".hidden", "a/./b", "con.", "a /b", "a:b",
        ] {
            let err = check_path(bad).unwrap_err();
            assert!(
                err.starts_with(&format!("{bad}: breaks the path rules (component")),
                "{err}"
            );
        }
        assert_eq!(
            check_path("../x").unwrap_err(),
            "../x: breaks the path rules (component \"..\" is not allowed)"
        );
        assert!(check_path("a/b/c/d/e/f").is_ok());
        assert_eq!(
            check_path("a/b/c/d/e/f/g").unwrap_err(),
            "a/b/c/d/e/f/g: breaks the path rules (more than 6 components)"
        );
        let long = format!("{}/{}", "a".repeat(99), "b".repeat(100));
        assert!(check_path(&long).is_ok());
        let long = format!("{}/{}/c", "a".repeat(99), "b".repeat(100));
        assert!(check_path(&long).unwrap_err().ends_with("(over 200 bytes)"));
        let wide = "a".repeat(101);
        assert!(check_path(&wide).unwrap_err().contains("is not allowed"));
        assert!(check_path("a//b").is_err());
    }

    #[test]
    fn layout_table() {
        let ok = pack(&[
            "pack.toml",
            "README.md",
            "LICENSE",
            "passes/a-b.toml",
            "prompts/a-b.md",
            "personas/a-b.md",
            "templates/Deep/Note.md",
            "tests/one/case.toml",
            "tests/one/notes/x/Y.md",
            "tests/one/transcripts/t.jsonl",
        ]);
        assert!(check_layout(&ok).is_empty());
        for bad in [
            "extra.txt",
            "passes/A.toml",
            "passes/sub/a.toml",
            "prompts/a.txt",
            "templates/a.txt",
            "tests/One/case.toml",
            "tests/one/other.toml",
            "tests/one/transcripts/t.json",
            "theme.css",
        ] {
            assert_eq!(
                check_layout(&pack(&[bad])),
                [format!("unexpected file {bad}")],
                "{bad}"
            );
        }
    }

    #[test]
    fn case_insensitive_paths() {
        assert_eq!(
            check_layout(&pack(&["templates/A.md", "templates/a.md"])),
            ["templates/A.md and templates/a.md differ only in case"]
        );
        let errors = check_layout(&pack(&[
            "tests/one/notes/Foo/a.md",
            "tests/one/notes/foo/b.md",
        ]));
        assert_eq!(
            errors,
            ["tests/one/notes/Foo and tests/one/notes/foo differ only in case"]
        );
    }

    #[test]
    fn denied_code_points_at_each_end_of_each_range() {
        for &(lo, hi) in DENIED {
            for cp in [lo, hi] {
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                let err = check_text("a.md", format!("ab{c}").as_bytes()).unwrap_err();
                assert_eq!(err, format!("a.md:1:3: U+{cp:04X}"));
            }
            // The code point after the range, when it is not another range.
            let after = char::from_u32(hi + 1);
            if let Some(c) = after.filter(|c| !is_denied(*c)) {
                assert!(
                    check_text("a.md", c.to_string().as_bytes()).is_ok(),
                    "{hi:X}"
                );
            }
        }
        // The explicitly listed boundaries.
        for cp in [
            0x09_u32,
            0x0A,
            0x0D,
            0x20,
            0xA0,
            0x2027,
            0x202F,
            0x205F,
            0x2070,
            0xE0FFF + 1,
        ] {
            let c = char::from_u32(cp).unwrap();
            assert!(
                check_text("a.md", c.to_string().as_bytes()).is_ok(),
                "{cp:X}"
            );
        }
        assert_eq!(
            check_text("prompts/a.md", "x\ny\n\u{202E}".as_bytes()).unwrap_err(),
            "prompts/a.md:3:1: U+202E"
        );
        assert_eq!(
            check_text("prompts/a.md", "a\nbc\u{1F}".as_bytes()).unwrap_err(),
            "prompts/a.md:2:3: U+001F"
        );
        assert_eq!(check_text("a", &[0xff, 0xfe]).unwrap_err(), "a: not UTF-8");
    }

    #[test]
    fn allowed_text() {
        assert!(check_text("a.md", "é — → ünï\r\nsecond line\ttab\n".as_bytes()).is_ok());
        assert!(check_text("a.md", "😀 日本語".as_bytes()).is_ok());
    }

    #[test]
    fn size_limits_at_their_boundaries() {
        let one = |path: &str, n: usize| {
            let mut files = PackFiles::default();
            files.insert(path.to_owned(), vec![b'a'; n]);
            files
        };
        assert!(check_sizes(&one("tests/a/case.toml", 65_536)).is_empty());
        assert_eq!(
            check_sizes(&one("tests/a/case.toml", 65_537)),
            ["tests/a/case.toml: 65537 bytes, over the 65536-byte limit"]
        );

        let mut files = PackFiles::default();
        files.insert("a.md".into(), vec![b'a'; 65_536]);
        files.insert("b.md".into(), vec![b'a'; 65_536]);
        assert!(check_sizes(&files).is_empty());
        files.insert("c.md".into(), vec![b'a'; 1]);
        assert_eq!(
            check_sizes(&files),
            ["files outside tests/: 131073 bytes, over the 131072-byte limit"]
        );

        // Under tests/ the outside limit does not apply, the pack limit does.
        let mut files = PackFiles::default();
        for n in 0..8 {
            files.insert(format!("tests/a/notes/{n}.md"), vec![b'a'; 65_536]);
        }
        assert!(check_sizes(&files).is_empty());
        files.insert("tests/a/notes/x.md".into(), vec![b'a'; 1]);
        assert_eq!(
            check_sizes(&files),
            ["pack: 524289 bytes, over the 524288-byte limit"]
        );

        let many = |n: usize| {
            let mut files = PackFiles::default();
            for i in 0..n {
                files.insert(format!("templates/{i}.md"), b"a".to_vec());
            }
            files
        };
        assert!(check_sizes(&many(100)).is_empty());
        assert_eq!(
            check_sizes(&many(101)),
            ["pack: 101 files, over the 100-file limit"]
        );
    }

    #[test]
    fn pack_hash_follows_the_formula() {
        use sha2::{Digest, Sha256};
        let mut files = PackFiles::default();
        files.insert("pack.toml".into(), b"one".to_vec());
        files.insert("README.md".into(), b"two".to_vec());
        files.insert("origin.json".into(), b"{}".to_vec());
        let line = |path: &str, bytes: &[u8]| {
            let digest: String = Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("{path}\0{digest}\n")
        };
        let text = format!("{}{}", line("README.md", b"two"), line("pack.toml", b"one"));
        let want: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(pack_hash(&files), want);
    }

    #[test]
    fn read_dir_skips_hidden_entries_and_the_root_origin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("passes")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("pack.toml"), "a").unwrap();
        fs::write(root.join("passes/a.toml"), "b").unwrap();
        fs::write(root.join(".DS_Store"), "x").unwrap();
        fs::write(root.join(".git/objects/o"), "x").unwrap();
        fs::write(root.join("origin.json"), "{}").unwrap();
        fs::write(root.join("sub/origin.json"), "{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", root.join(".hidden-link")).unwrap();
        let (files, skipped) = read_dir(root).unwrap();
        assert_eq!(
            files.paths().collect::<Vec<_>>(),
            ["pack.toml", "passes/a.toml", "sub/origin.json"]
        );
        let mut want = vec![".DS_Store", ".git", "origin.json"];
        if cfg!(unix) {
            want.push(".hidden-link");
        }
        want.sort_unstable();
        let mut got = skipped.clone();
        got.sort();
        assert_eq!(got, want);
        assert_eq!(files.get("pack.toml"), Some(&b"a"[..]));
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_refuses_visible_links_and_special_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "a").unwrap();
        symlink("/etc/hosts", dir.path().join("b.md")).unwrap();
        let err = read_dir(dir.path()).unwrap_err();
        assert!(matches!(err, Error::Pack(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "b.md: a symbolic link; packs may not contain links"
        );

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("d")).unwrap();
        symlink(dir.path().join("d"), dir.path().join("e")).unwrap();
        let err = read_dir(dir.path()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "e: a symbolic link; packs may not contain links"
        );

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("f");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            let err = read_dir(dir.path()).unwrap_err();
            assert_eq!(err.to_string(), "f: not a regular file or directory");
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_refuses_a_hard_link_and_escapes_the_name_it_refuses() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "private").unwrap();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("templates")).unwrap();
        fs::hard_link(&secret, dir.path().join("templates/x.md")).unwrap();
        let err = read_dir(dir.path()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "templates/x.md: a hard link; packs may not contain links"
        );

        // A refused entry whose name holds a terminal escape is named in
        // escaped form.
        let dir = tempfile::tempdir().unwrap();
        let name = "\u{1b}]0;pwned\u{7}";
        std::os::unix::fs::symlink("/etc/hosts", dir.path().join(name)).unwrap();
        let err = read_dir(dir.path()).unwrap_err().to_string();
        assert!(!err.contains('\u{1b}') && !err.contains('\u{7}'), "{err}");
        assert!(err.contains("\\u{1b}]0;pwned\\u{7}"), "{err}");
    }

    #[test]
    fn read_dir_checks_size_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("big.md"), vec![b'a'; 65_537]).unwrap();
        let err = read_dir(dir.path()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "big.md: 65537 bytes, over the 65536-byte limit"
        );
    }

    #[test]
    fn read_dir_stops_at_too_many_files_or_bytes() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..=MAX_FILES {
            fs::write(dir.path().join(format!("f{n:03}.md")), "a").unwrap();
        }
        let err = read_dir(dir.path()).unwrap_err();
        assert!(matches!(err, Error::Pack(_)), "{err}");
        assert_eq!(err.to_string(), "pack: more than 100 files");
        // Exactly 100 files is fine.
        fs::remove_file(dir.path().join("f100.md")).unwrap();
        assert_eq!(read_dir(dir.path()).unwrap().0.len(), 100);

        let dir = tempfile::tempdir().unwrap();
        for n in 0..9 {
            fs::write(dir.path().join(format!("f{n}.md")), vec![b'a'; 65_536]).unwrap();
        }
        let err = read_dir(dir.path()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "pack: 589824 bytes, over the 524288-byte limit"
        );
    }

    #[test]
    fn read_dir_accepts_65536_bytes_and_refuses_65537() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), vec![b'a'; 65_536]).unwrap();
        assert_eq!(
            read_dir(dir.path()).unwrap().0.get("a.md").unwrap().len(),
            65_536
        );
        fs::write(dir.path().join("a.md"), vec![b'a'; 65_537]).unwrap();
        assert_eq!(
            read_dir(dir.path()).unwrap_err().to_string(),
            "a.md: 65537 bytes, over the 65536-byte limit"
        );
    }

    #[test]
    fn read_dir_caps_the_skipped_list() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pack.toml"), "a").unwrap();
        for n in 0..105 {
            fs::write(dir.path().join(format!(".h{n:03}")), "x").unwrap();
        }
        let (files, skipped) = read_dir(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(skipped.len(), 101);
        assert_eq!(skipped[100], "… and 5 more");
    }

    #[test]
    fn read_dir_needs_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        let err = read_dir(&file).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("{}: not a directory", file.display())
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_skips_hidden_names_that_are_not_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "a").unwrap();
        let hidden = std::ffi::OsStr::from_bytes(b".\xff");
        if fs::write(dir.path().join(hidden), "x").is_err() {
            return; // the file system refuses non-UTF-8 names
        }
        let (files, skipped) = read_dir(dir.path()).unwrap();
        assert_eq!(files.paths().collect::<Vec<_>>(), ["a.md"]);
        assert_eq!(skipped.len(), 1);
        let visible = std::ffi::OsStr::from_bytes(b"b\xff.md");
        fs::write(dir.path().join(visible), "x").unwrap();
        assert!(read_dir(dir.path())
            .unwrap_err()
            .to_string()
            .ends_with("not UTF-8"));
    }

    #[test]
    fn a_path_of_201_bytes_is_refused_and_200_accepted() {
        let at = |last: usize| {
            [66, 66, last]
                .iter()
                .map(|&len| "a".repeat(len))
                .collect::<Vec<_>>()
                .join("/")
        };
        assert_eq!(at(66).len(), 200);
        assert!(check_path(&at(66)).is_ok());
        assert_eq!(at(67).len(), 201);
        assert!(check_path(&at(67))
            .unwrap_err()
            .ends_with("(over 200 bytes)"));
    }

    #[test]
    fn code_points_just_outside_three_denied_ranges_are_allowed() {
        for (below, lo, hi, above) in [
            (0x2027_u32, 0x2028_u32, 0x202E_u32, 0x202F_u32),
            (0x007E, 0x007F, 0x009F, 0x00A0),
            (0x1D172, 0x1D173, 0x1D17A, 0x1D17B),
            (0xD7FF, 0xE000, 0xF8FF, 0xF900),
        ] {
            for cp in [below, above] {
                let c = char::from_u32(cp).unwrap();
                assert!(
                    check_text("a.md", c.to_string().as_bytes()).is_ok(),
                    "{cp:X}"
                );
            }
            for cp in [lo, hi] {
                let c = char::from_u32(cp).unwrap();
                assert!(
                    check_text("a.md", c.to_string().as_bytes()).is_err(),
                    "{cp:X}"
                );
            }
        }
    }
}
