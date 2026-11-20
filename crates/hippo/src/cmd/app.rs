//! `hippo app install|uninstall|status`: Hippo.app from the GitHub release
//! of this hippo's version, ad-hoc signed and not notarised, downloaded
//! with `curl` (which sets no quarantine attribute), checked against the
//! published sha256, unpacked into `~/Applications` with no `sudo`, and
//! recorded in `$HIPPO_HOME/install.json`. hippo never runs `xattr`.
//! Design: docs/designs/app-download.md.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode, Stdio};

use anyhow::{anyhow, bail, Context as _, Result};
use hippo_core::app_install::{self, AppRecord};
use hippo_core::config;

const REPO: &str = "https://github.com/VanitasCaesar1/hippo";

#[derive(clap::Subcommand)]
pub enum Command {
    /// Download Hippo.app for this hippo's version, check its sha256, put it in ~/Applications
    Install(InstallArgs),
    /// Remove the Hippo.app that hippo app install put there, and its record
    Uninstall,
    /// The installed app's version and path, and whether it is still the one that was checked
    Status,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// The release to install [default: this hippo's version]
    #[arg(long)]
    version: Option<String>,
    /// The folder to put Hippo.app in [default: ~/Applications]
    #[arg(long)]
    dir: Option<PathBuf>,
}

pub fn run(command: Command) -> Result<ExitCode> {
    if !cfg!(target_os = "macos") {
        bail!("Hippo.app is for macOS; on this system use hippo's commands, or hippo view for the app's figures");
    }
    let home = config::home();
    match command {
        Command::Install(args) => install(&home, args),
        Command::Uninstall => uninstall(&home),
        Command::Status => status(&home),
    }
}

/// The asset's file name.
fn asset(version: &str) -> String {
    format!("Hippo-{version}-macos.tar.gz")
}

/// How long curl may take to connect, and how long a download may go with
/// no data, in seconds. `HIPPO_APP_DOWNLOAD_TIMEOUTS=<connect>,<stall>`
/// changes them (the tests use it).
const CONNECT_SECS: u64 = 15;
const STALL_SECS: u64 = 30;

fn timeouts() -> (u64, u64) {
    std::env::var("HIPPO_APP_DOWNLOAD_TIMEOUTS")
        .ok()
        .and_then(|v| {
            let (c, s) = v.split_once(',')?;
            Some((c.trim().parse().ok()?, s.trim().parse().ok()?))
        })
        .unwrap_or((CONNECT_SECS, STALL_SECS))
}

/// What a curl exit code means, in words.
fn curl_failure(code: Option<i32>, connect: u64, stall: u64) -> String {
    match code {
        Some(28) => format!(
            "the download timed out (no connection within {connect} s, or no data for {stall} s)"
        ),
        Some(6) => "the host's name could not be resolved".to_owned(),
        Some(7) => "the connection was refused".to_owned(),
        Some(22) => {
            "the server answered with an error (is there a release of this version?)".to_owned()
        }
        Some(35 | 60) => "the secure connection failed".to_owned(),
        Some(37) => "the file is not there".to_owned(),
        Some(c) => format!("curl stopped with exit code {c}"),
        None => "curl was stopped".to_owned(),
    }
}

/// Download `url` to `file` with curl: a progress bar on a terminal when
/// `progress`, a 15-second connect timeout and a 30-second stall limit.
fn fetch(url: &str, file: &Path, progress: bool) -> Result<()> {
    use std::io::IsTerminal as _;
    let (connect, stall) = timeouts();
    let bar = progress && std::io::stderr().is_terminal();
    let mut cmd = Process::new("curl");
    cmd.args(["-fL", "--proto", "=https,http,file"])
        .args(["--connect-timeout", &connect.to_string()])
        // Under 1 byte a second for `stall` seconds: a stalled download.
        .args(["--speed-limit", "1", "--speed-time", &stall.to_string()])
        .arg(if bar { "--progress-bar" } else { "-sS" })
        .arg(url)
        .arg("-o")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let started = std::time::Instant::now();
    let (status, stderr) = if bar {
        let status = cmd
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| anyhow!("cannot run curl to download {url}: {e}"))?;
        (status, String::new())
    } else {
        let out = cmd
            .output()
            .map_err(|e| anyhow!("cannot run curl to download {url}: {e}"))?;
        (
            out.status,
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        )
    };
    if !status.success() {
        let why = curl_failure(status.code(), connect, stall);
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!(" ({stderr})")
        };
        bail!(
            "could not download {url}: {why}{detail}. Nothing was installed; check the connection and run hippo app install again to retry"
        );
    }
    if progress && !bar {
        let size = fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        println!(
            "Downloaded {:.1} MB in {:.0} s",
            size as f64 / 1_000_000.0,
            started.elapsed().as_secs_f64().max(1.0)
        );
    }
    Ok(())
}

/// The `CFBundleShortVersionString` of an Info.plist's text.
fn plist_version(text: &str) -> Option<String> {
    let after = text.split("<key>CFBundleShortVersionString</key>").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].trim().to_owned())
}

fn install(home: &Path, args: InstallArgs) -> Result<ExitCode> {
    let version = args
        .version
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let base = std::env::var("HIPPO_BASE_URL")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| format!("{REPO}/releases/download/v{version}"));
    let base = base.trim_end_matches('/');
    let dir = match args.dir {
        Some(d) => d,
        None => config::user_home()
            .ok_or_else(|| anyhow!("cannot find your home folder"))?
            .join("Applications"),
    };
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let name = asset(&version);
    // Staged beside the destination, so the final move is a rename.
    let stage = dir.join(format!(".hippo-app-install-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).with_context(|| format!("cannot create {}", stage.display()))?;
    let result = install_staged(home, &dir, &stage, base, &version, &name);
    let _ = fs::remove_dir_all(&stage);
    result
}

fn install_staged(
    home: &Path,
    dir: &Path,
    stage: &Path,
    base: &str,
    version: &str,
    name: &str,
) -> Result<ExitCode> {
    println!("Downloading Hippo.app {version} from {base}/{name}");
    let tarball = stage.join(name);
    let sumfile = stage.join(format!("{name}.sha256"));
    fetch(&format!("{base}/{name}"), &tarball, true)?;
    fetch(&format!("{base}/{name}.sha256"), &sumfile, false)?;
    let want = fs::read_to_string(&sumfile)?
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let have = app_install::bytes_sha256(&fs::read(&tarball)?);
    if want.len() != 64 || want != have {
        bail!("the checksum of {name} does not match (published {want}, downloaded {have}); nothing was installed");
    }
    println!("Checked {name} against its published sha256");

    let unpack = stage.join("unpacked");
    fs::create_dir_all(&unpack)?;
    let out = Process::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&unpack)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow!("cannot run tar: {e}"))?;
    if !out.status.success() {
        bail!(
            "could not unpack {name}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let entries: Vec<String> = fs::read_dir(&unpack)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if entries != ["Hippo.app"] {
        bail!(
            "{name} should hold one Hippo.app folder, and holds {entries:?}; nothing was installed"
        );
    }
    let bundle = unpack.join("Hippo.app");
    let binary = app_install::binary_in(&bundle);
    if !binary.is_file() {
        bail!("{name} holds no Contents/MacOS/hippo-app; nothing was installed");
    }
    let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap_or_default();
    match plist_version(&plist) {
        Some(v) if v == version => {}
        other => bail!(
            "{name} holds Hippo.app {}, not {version}; nothing was installed",
            other.unwrap_or_else(|| "with no version".into())
        ),
    }
    let binary_sha256 = app_install::file_sha256(&binary)
        .ok_or_else(|| anyhow!("cannot read {}", binary.display()))?;

    let dest = dir.join("Hippo.app");
    let old = stage.join("Hippo.app.old");
    if dest.exists() {
        fs::rename(&dest, &old)
            .with_context(|| format!("cannot move the old {} aside", dest.display()))?;
    }
    if let Err(e) = fs::rename(&bundle, &dest) {
        if old.exists() {
            let _ = fs::rename(&old, &dest);
        }
        return Err(anyhow!(e).context(format!("cannot move Hippo.app to {}", dest.display())));
    }
    app_install::record(
        home,
        &AppRecord {
            version: version.to_owned(),
            path: dest.clone(),
            asset: name.to_owned(),
            asset_sha256: have,
            binary_sha256,
        },
    )?;
    println!("Installed Hippo.app {version} to {}", dest.display());
    println!(
        "Recorded its sha256 in {}",
        hippo_core::doctor::install_record_path(home).display()
    );
    if running(&dest) {
        println!("Hippo is open: quit it and open it again to use {version}.");
    } else {
        println!(
            "Open it from Spotlight or Launchpad, or: open {}",
            shell_path(&dest)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn shell_path(p: &Path) -> String {
    let s = p.display().to_string();
    if s.contains(' ') {
        format!("'{s}'")
    } else {
        s
    }
}

/// Whether the bundle at `dest` is running (`pgrep -f` on its executable).
fn running(dest: &Path) -> bool {
    Process::new("pgrep")
        .arg("-f")
        .arg(app_install::binary_in(dest))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn uninstall(home: &Path) -> Result<ExitCode> {
    let Some(app) = app_install::read(home) else {
        println!("No Hippo.app is recorded in install.json: hippo app install did not put one here, so there is nothing to remove.");
        return Ok(ExitCode::SUCCESS);
    };
    if app.path.file_name().and_then(|n| n.to_str()) != Some("Hippo.app") {
        bail!(
            "install.json records {} for the app, which is not a Hippo.app folder; remove it yourself if you want it gone",
            app.path.display()
        );
    }
    if app.path.exists() {
        if running(&app.path) {
            println!("Hippo is open: it keeps running until you quit it.");
        }
        fs::remove_dir_all(&app.path)
            .with_context(|| format!("cannot remove {}", app.path.display()))?;
        println!("Removed {}", app.path.display());
    } else {
        println!("{} was already gone", app.path.display());
    }
    app_install::forget(home)?;
    println!(
        "Forgot it in {}",
        hippo_core::doctor::install_record_path(home).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn status(home: &Path) -> Result<ExitCode> {
    let Some(check) = app_install::check(home) else {
        println!(
            "Hippo.app was not installed with hippo app install. hippo app install downloads it."
        );
        return Ok(ExitCode::SUCCESS);
    };
    println!("{}", check.detail);
    Ok(if check.status == hippo_core::doctor::Status::Ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_version_is_read() {
        let plist = "<key>CFBundleShortVersionString</key>\n    <string>0.7.1</string>";
        assert_eq!(plist_version(plist).as_deref(), Some("0.7.1"));
        assert_eq!(plist_version("<dict></dict>"), None);
    }
}
