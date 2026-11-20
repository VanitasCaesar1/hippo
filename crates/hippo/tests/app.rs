//! `hippo app install|status|uninstall` against a synthetic Hippo.app
//! release in a temporary folder, served as `file://`, installed into a
//! scratch Applications folder with a scratch HOME. macOS only, like the
//! command. Nothing outside the temporary folder is touched.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Release {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Release {
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn apps(&self) -> PathBuf {
        self.root.join("home/Applications")
    }
    fn dist(&self) -> PathBuf {
        self.root.join("dist")
    }
}

/// A release folder holding `Hippo-<asset>-macos.tar.gz` whose bundle says
/// `version`, and its `.sha256`.
fn release(asset: &str, version: &str) -> Release {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let build = root.join("build");
    let bundle = build.join("Hippo.app/Contents");
    fs::create_dir_all(bundle.join("MacOS")).unwrap();
    fs::write(
        bundle.join("Info.plist"),
        format!("<plist><dict>\n<key>CFBundleExecutable</key>\n<string>hippo-app</string>\n<key>CFBundleShortVersionString</key>\n<string>{version}</string>\n</dict></plist>\n"),
    )
    .unwrap();
    fs::write(bundle.join("MacOS/hippo-app"), "#!/bin/sh\nexit 0\n").unwrap();
    let dist = root.join("dist");
    fs::create_dir_all(&dist).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    let name = format!("Hippo-{asset}-macos.tar.gz");
    let ok = Command::new("tar")
        .arg("-czf")
        .arg(dist.join(&name))
        .arg("-C")
        .arg(&build)
        .arg("Hippo.app")
        .status()
        .unwrap();
    assert!(ok.success());
    let sum = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&name)
        .current_dir(&dist)
        .output()
        .unwrap();
    fs::write(dist.join(format!("{name}.sha256")), sum.stdout).unwrap();
    Release { _dir: dir, root }
}

fn hippo(r: &Release, args: &[&str]) -> Output {
    hippo_with(r, args, &format!("file://{}", r.dist().display()), &[])
}

fn hippo_with(r: &Release, args: &[&str], base: &str, env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hippo"))
        .args(args)
        .env_clear()
        .env("HOME", r.home())
        .env("HIPPO_HOME", r.home().join(".hippo"))
        .env("PATH", "/usr/bin:/bin")
        .env("HIPPO_BASE_URL", base)
        .envs(env.iter().copied())
        .current_dir(r.home())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn record(r: &Release) -> Value {
    serde_json::from_str(&fs::read_to_string(r.home().join(".hippo/install.json")).unwrap())
        .unwrap()
}

fn has_quarantine(path: &Path) -> bool {
    Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(path)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn install_checks_records_and_uninstalls() {
    let r = release("0.9.9", "0.9.9");
    let out = hippo(&r, &["app", "install", "--version", "0.9.9"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    // Not on a terminal: no bar, the size and time once it is done.
    assert!(said.contains("Downloaded 0.0 MB in"), "{said}");
    assert!(
        said.contains("Checked Hippo-0.9.9-macos.tar.gz against its published sha256"),
        "{said}"
    );
    let app = r.apps().join("Hippo.app");
    assert!(app.join("Contents/MacOS/hippo-app").is_file());
    assert!(!has_quarantine(&app), "curl and tar set no quarantine");
    // Only Hippo.app is left in the folder: the staging folder is gone.
    let left: Vec<_> = fs::read_dir(r.apps())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["Hippo.app"]);
    let rec = record(&r);
    assert_eq!(rec["app"]["version"], "0.9.9");
    assert_eq!(rec["app"]["path"], app.display().to_string());
    assert_eq!(rec["app"]["asset"], "Hippo-0.9.9-macos.tar.gz");
    assert_eq!(rec["app"]["binary_sha256"].as_str().unwrap().len(), 64);

    let status = hippo(&r, &["app", "status"]);
    assert_eq!(status.status.code(), Some(0), "{}", text(&status));
    assert!(
        text(&status).contains("Hippo.app 0.9.9 at"),
        "{}",
        text(&status)
    );

    // A changed binary is caught.
    fs::write(
        app.join("Contents/MacOS/hippo-app"),
        "#!/bin/sh\necho changed\n",
    )
    .unwrap();
    let status = hippo(&r, &["app", "status"]);
    assert_eq!(status.status.code(), Some(1), "{}", text(&status));
    assert!(
        text(&status).contains("is not the Hippo.app 0.9.9"),
        "{}",
        text(&status)
    );

    // Installing again replaces it.
    let again = hippo(&r, &["app", "install", "--version", "0.9.9"]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again));
    assert_eq!(hippo(&r, &["app", "status"]).status.code(), Some(0));

    let gone = hippo(&r, &["app", "uninstall"]);
    assert_eq!(gone.status.code(), Some(0), "{}", text(&gone));
    assert!(!app.exists());
    assert!(!r.home().join(".hippo/install.json").exists());
}

#[test]
fn a_missing_release_says_how_to_retry() {
    let r = release("0.9.9", "0.9.9");
    let out = hippo(&r, &["app", "install", "--version", "0.9.7"]);
    let said = text(&out);
    assert_ne!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("the file is not there"), "{said}");
    assert!(
        said.contains(
            "Nothing was installed; check the connection and run hippo app install again to retry"
        ),
        "{said}"
    );
    assert!(!r.apps().join("Hippo.app").exists());
    assert_eq!(fs::read_dir(r.apps()).unwrap().count(), 0);
}

/// A server that accepts the connection and never answers: the download
/// stops after the stall limit (2 s here, 30 s by default) with a message.
#[test]
fn a_stalled_download_times_out() {
    let r = release("0.9.9", "0.9.9");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for c in listener.incoming().flatten() {
            held.push(c);
        }
    });
    let started = std::time::Instant::now();
    let out = hippo_with(
        &r,
        &["app", "install", "--version", "0.9.9"],
        &format!("http://127.0.0.1:{port}"),
        &[("HIPPO_APP_DOWNLOAD_TIMEOUTS", "2,2")],
    );
    let said = text(&out);
    assert_ne!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("the download timed out"), "{said}");
    assert!(said.contains("no data for 2 s"), "{said}");
    assert!(
        said.contains("run hippo app install again to retry"),
        "{said}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(!r.apps().join("Hippo.app").exists());
}

#[test]
fn a_tampered_download_is_refused() {
    let r = release("0.9.9", "0.9.9");
    let sum = r.dist().join("Hippo-0.9.9-macos.tar.gz.sha256");
    fs::write(
        &sum,
        format!("{}  Hippo-0.9.9-macos.tar.gz\n", "0".repeat(64)),
    )
    .unwrap();
    let out = hippo(&r, &["app", "install", "--version", "0.9.9"]);
    let said = text(&out);
    assert_ne!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("does not match"), "{said}");
    assert!(!r.apps().join("Hippo.app").exists());
    assert!(!r.home().join(".hippo/install.json").exists());
    assert_eq!(
        fs::read_dir(r.apps()).unwrap().count(),
        0,
        "no staging folder is left"
    );
}

#[test]
fn another_version_inside_is_refused() {
    let r = release("0.9.8", "0.9.9");
    let out = hippo(&r, &["app", "install", "--version", "0.9.8"]);
    let said = text(&out);
    assert_ne!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("holds Hippo.app 0.9.9, not 0.9.8"), "{said}");
    assert!(!r.apps().join("Hippo.app").exists());
}

#[test]
fn uninstall_with_nothing_recorded_removes_nothing() {
    let r = release("0.9.9", "0.9.9");
    let mine = r.apps().join("Hippo.app");
    fs::create_dir_all(&mine).unwrap();
    let out = hippo(&r, &["app", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(mine.exists(), "an app hippo did not install is left alone");
}
