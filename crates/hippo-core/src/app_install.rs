//! Hippo.app installed by `hippo app install`: its record under the `app`
//! key of `$HIPPO_HOME/install.json` (the file `install.sh` writes for the
//! CLI, whose other keys are kept), and `hippo doctor`'s `app` row.
//! Design: docs/designs/app-download.md.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doctor::{install_record_path, Check, Status};
use crate::error::Result;
use crate::util::{hex, sha256, write_atomic};

/// The installed app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRecord {
    pub version: String,
    /// The `Hippo.app` folder.
    pub path: PathBuf,
    /// The release asset it came from, and that asset's checked SHA-256.
    pub asset: String,
    pub asset_sha256: String,
    /// SHA-256 of `Contents/MacOS/hippo-app` when it was installed.
    pub binary_sha256: String,
}

impl AppRecord {
    /// The bundle's executable.
    pub fn binary(&self) -> PathBuf {
        binary_in(&self.path)
    }
}

/// `<bundle>/Contents/MacOS/hippo-app`.
pub fn binary_in(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS/hippo-app")
}

/// SHA-256 of a file, lowercase hex.
pub fn file_sha256(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| hex(&sha256(&b)))
}

/// SHA-256 of bytes, lowercase hex.
pub fn bytes_sha256(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

fn read_json(home: &Path) -> serde_json::Map<String, serde_json::Value> {
    fs::read_to_string(install_record_path(home))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_json(home: &Path, map: serde_json::Map<String, serde_json::Value>) -> Result<()> {
    fs::create_dir_all(home)?;
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .expect("a JSON object serializes");
    text.push('\n');
    write_atomic(&install_record_path(home), text.as_bytes())
}

/// The recorded app, if any.
pub fn read(home: &Path) -> Option<AppRecord> {
    serde_json::from_value(read_json(home).get("app")?.clone()).ok()
}

/// Records `app`, keeping every other key of `install.json`.
pub fn record(home: &Path, app: &AppRecord) -> Result<()> {
    let mut map = read_json(home);
    map.insert(
        "app".into(),
        serde_json::to_value(app).expect("the record serializes"),
    );
    write_json(home, map)
}

/// Removes the `app` key; removes `install.json` when nothing else is in it.
pub fn forget(home: &Path) -> Result<()> {
    let mut map = read_json(home);
    if map.remove("app").is_none() {
        return Ok(());
    }
    if map.is_empty() {
        let _ = fs::remove_file(install_record_path(home));
        return Ok(());
    }
    write_json(home, map)
}

/// `hippo doctor`'s `app` row: `None` when no app is recorded.
pub fn check(home: &Path) -> Option<Check> {
    let app = read(home)?;
    let row = |status, detail: String| Check {
        name: "app",
        status,
        detail,
    };
    Some(match file_sha256(&app.binary()) {
        None => row(
            Status::Warn,
            format!(
                "Hippo.app {} was installed at {}, which is gone; hippo app install puts it back, hippo app uninstall forgets it",
                app.version,
                app.path.display()
            ),
        ),
        Some(have) if have != app.binary_sha256 => row(
            Status::Problem,
            format!(
                "{} is not the Hippo.app {} that hippo app install checked: its binary's sha256 is {have}, recorded {}. If you rebuilt it yourself, run hippo app install again; otherwise treat this machine as compromised",
                app.path.display(),
                app.version,
                app.binary_sha256
            ),
        ),
        Some(_) => row(
            Status::Ok,
            format!(
                "Hippo.app {} at {}, its sha256 as recorded in {}",
                app.version,
                app.path.display(),
                install_record_path(home).display()
            ),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(dir: &Path) -> AppRecord {
        let bundle = dir.join("Hippo.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::write(binary_in(&bundle), b"app binary").unwrap();
        AppRecord {
            version: "0.7.1".into(),
            path: bundle.clone(),
            asset: "Hippo-0.7.1-macos.tar.gz".into(),
            asset_sha256: "ab".repeat(32),
            binary_sha256: file_sha256(&binary_in(&bundle)).unwrap(),
        }
    }

    #[test]
    fn the_record_keeps_the_cli_keys() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".hippo");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            install_record_path(&home),
            "{\"version\": \"0.7.1\", \"binary\": \"/x/hippo\", \"sha256\": \"cd\"}\n",
        )
        .unwrap();
        let a = app(dir.path());
        record(&home, &a).unwrap();
        assert_eq!(read(&home), Some(a));
        let cli = crate::doctor::install_record(&home).unwrap();
        assert_eq!(cli.sha256, "cd");
        forget(&home).unwrap();
        assert_eq!(read(&home), None);
        assert!(crate::doctor::install_record(&home).is_some());
    }

    #[test]
    fn doctor_catches_a_changed_app() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".hippo");
        assert!(check(&home).is_none());
        let a = app(dir.path());
        record(&home, &a).unwrap();
        assert_eq!(check(&home).unwrap().status, Status::Ok);
        fs::write(a.binary(), b"something else").unwrap();
        assert_eq!(check(&home).unwrap().status, Status::Problem);
        fs::remove_dir_all(&a.path).unwrap();
        assert_eq!(check(&home).unwrap().status, Status::Warn);
        forget(&home).unwrap();
        assert!(!install_record_path(&home).exists());
    }
}
