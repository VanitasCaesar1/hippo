//! `hippo schedule install|uninstall|status`: the two launchd jobs.
//!
//! `dev.hippo.ingest` runs `hippo ingest` every `ingest_interval_minutes`
//! and once at load, at background priority; `dev.hippo.dream` runs `hippo
//! dream` at `dream_at`. Both run the absolute path of this `hippo` binary
//! with the installing shell's `PATH` and `HOME`, and `HIPPO_HOME` when it
//! is set, and write their output to `$HIPPO_HOME/logs/<label>.log`.
//! `install` writes `~/Library/LaunchAgents/<label>.plist` and reloads each
//! job with `launchctl bootout` (its failure ignored) and `launchctl
//! bootstrap`; `uninstall` boots both out and removes the files; `status`
//! prints `launchctl print`'s exit status for each. `--job ingest|dream`
//! makes `install` and `uninstall` touch that job only. Off macOS nothing is written: `install`
//! prints a crontab block and a systemd user timer pair for the same jobs,
//! and `status` and `uninstall` print the commands that check and remove
//! them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode, ExitStatus, Stdio};

use anyhow::{anyhow, bail, Context as _, Result};
use hippo_core::config::Config;
use hippo_core::resume::quote;

/// The ingest job's label.
const INGEST: &str = "dev.hippo.ingest";
/// The dreaming job's label.
const DREAM: &str = "dev.hippo.dream";

#[derive(clap::Subcommand)]
pub enum Command {
    /// Write both jobs (or one, with --job) to ~/Library/LaunchAgents and load them; elsewhere print cron and systemd lines
    Install {
        /// Only this job; the other is left as it is
        #[arg(long, value_enum)]
        job: Option<JobName>,
    },
    /// Unload both jobs (or one, with --job) and remove their files
    Uninstall {
        /// Only this job; the other is left as it is
        #[arg(long, value_enum)]
        job: Option<JobName>,
    },
    /// Print launchctl print's exit status for each job
    Status,
}

/// One of the two jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum JobName {
    Ingest,
    Dream,
}

impl JobName {
    fn label(self) -> &'static str {
        match self {
            JobName::Ingest => INGEST,
            JobName::Dream => DREAM,
        }
    }
}

/// The labels `--job` picks: both without it.
fn labels(job: Option<JobName>) -> Vec<&'static str> {
    match job {
        Some(j) => vec![j.label()],
        None => vec![INGEST, DREAM],
    }
}

pub fn run(command: Command) -> Result<ExitCode> {
    if !cfg!(target_os = "macos") {
        return elsewhere(command);
    }
    match command {
        Command::Install { job } => install(job),
        Command::Uninstall { job } => uninstall(job),
        Command::Status => status(),
    }
}

fn install(only: Option<JobName>) -> Result<ExitCode> {
    let config = Config::load()?;
    let exe = std::env::current_exe().context("cannot find the running hippo binary")?;
    let agents = launch_agents()?;
    let logs = config.home.join("logs");
    for dir in [&agents, &logs] {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let uid = uid()?;
    let [ingest, dream] = jobs(&config, &exe, &environment()?)?;
    // Without a notes folder there is nothing to dream over: the ingest job
    // alone keeps the history read.
    let chosen = match (only, config.workspace.is_some()) {
        (Some(JobName::Ingest), _) => vec![ingest],
        (Some(JobName::Dream), true) => vec![dream],
        (Some(JobName::Dream), false) => bail!("no notes folder yet: dreaming needs one"),
        (None, true) => vec![ingest, dream],
        (None, false) => {
            println!("no notes folder yet: installing the ingest job only; run hippo schedule install again after choosing one");
            vec![ingest]
        }
    };
    for job in chosen {
        let path = agents.join(format!("{}.plist", job.label));
        fs::write(&path, job.plist())
            .with_context(|| format!("cannot write {}", path.display()))?;
        // A job loaded from an earlier file is unloaded first; when none
        // was loaded this fails, which is fine.
        launchctl(&["bootout", &format!("gui/{uid}/{}", job.label)], true)?;
        let status = launchctl(&["bootstrap", &format!("gui/{uid}"), &utf8(&path)?], false)?;
        if !status.success() {
            bail!("launchctl bootstrap {} failed: {status}", path.display());
        }
        println!("installed {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn uninstall(only: Option<JobName>) -> Result<ExitCode> {
    let agents = launch_agents()?;
    let uid = uid()?;
    for label in labels(only) {
        launchctl(&["bootout", &format!("gui/{uid}/{label}")], true)?;
        let path = agents.join(format!("{label}.plist"));
        match fs::remove_file(&path) {
            Ok(()) => println!("removed {}", path.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                println!("{} was not installed", path.display());
            }
            Err(e) => return Err(anyhow!("cannot remove {}: {e}", path.display())),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn status() -> Result<ExitCode> {
    let uid = uid()?;
    for label in [INGEST, DREAM] {
        let status = launchctl(&["print", &format!("gui/{uid}/{label}")], true)?;
        let code = status
            .code()
            .map_or_else(|| "none".to_owned(), |code| code.to_string());
        println!("{label}: launchctl print exit status {code}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Off macOS: hippo cannot install a cron job or a systemd timer for the
/// person, so it prints both, ready to copy.
fn elsewhere(command: Command) -> Result<ExitCode> {
    match command {
        Command::Install { job } => {
            let config = Config::load()?;
            let exe = std::env::current_exe().context("cannot find the running hippo binary")?;
            let jobs = jobs(&config, &exe, &environment()?)?;
            let chosen: Vec<&Job> = jobs
                .iter()
                .filter(|j| match job {
                    Some(only) => j.label == only.label(),
                    None => j.label == INGEST || config.workspace.is_some(),
                })
                .collect();
            if job.is_none() && config.workspace.is_none() {
                println!("no notes folder yet: only the ingest job; dreaming needs one\n");
            }
            println!(
                "hippo does not install jobs on this system; it prints them. Use one of the two.\n"
            );
            print!("{}", cron_text(&chosen));
            println!();
            print!("{}", systemd_text(&chosen));
            println!("logs: {}", config.home.join("logs").display());
        }
        Command::Status => {
            println!("hippo cannot see cron or systemd here. Check them with:");
            println!("  crontab -l | grep hippo");
            println!("  systemctl --user list-timers 'hippo-*'");
        }
        Command::Uninstall { .. } => {
            println!("hippo installed nothing here. To remove what you added:");
            println!("  crontab -e   (delete the lines that run hippo)");
            println!("  systemctl --user disable --now hippo-ingest.timer hippo-dream.timer");
            println!(
                "  rm ~/.config/systemd/user/hippo-ingest.* ~/.config/systemd/user/hippo-dream.*"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The cron schedule of a job: `*/10 * * * *`, `0 */2 * * *`, `5 4 * * *`.
fn cron_when(when: When) -> String {
    match when {
        When::Every { seconds } => {
            let minutes = (seconds / 60).max(1);
            if minutes < 60 {
                format!("*/{minutes} * * * *")
            } else {
                let hours = (minutes / 60).min(23);
                format!("0 */{hours} * * *")
            }
        }
        When::At { hour, minute } => format!("{minute} {hour} * * *"),
    }
}

/// A crontab block: one line per job, the environment inline, output
/// appended to the job's log. `%` is special to cron, so it is escaped.
fn cron_text(jobs: &[&Job]) -> String {
    let mut out = String::from("# cron: add these lines with  crontab -e\n");
    for job in jobs {
        let env: Vec<String> = job
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={}", quote(v)))
            .collect();
        let args: Vec<String> = job.arguments.iter().map(|a| quote(a)).collect();
        let line = format!(
            "{} {} {} >> {} 2>&1",
            cron_when(job.when),
            env.join(" "),
            args.join(" "),
            quote(&job.log)
        );
        out.push_str(&line.replace('%', "\\%"));
        out.push('\n');
    }
    out
}

/// A value for a systemd unit file: in double quotes, `\` and `"`
/// escaped, `%` doubled.
fn unit_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// A systemd user service and timer per job, and the commands that load
/// them.
fn systemd_text(jobs: &[&Job]) -> String {
    let mut out = String::from("# systemd: write these files, then run the commands at the end\n");
    let mut timers = Vec::new();
    for job in jobs {
        let name = job.label.replace("dev.hippo.", "hippo-");
        let what = job.arguments.last().map_or("", String::as_str);
        out.push_str(&format!("\n# ~/.config/systemd/user/{name}.service\n"));
        out.push_str(&format!(
            "[Unit]\nDescription=hippo {what}\n\n[Service]\nType=oneshot\n"
        ));
        for (k, v) in &job.environment {
            out.push_str(&format!(
                "Environment={}\n",
                unit_quote(&format!("{k}={v}"))
            ));
        }
        let args: Vec<String> = job.arguments.iter().map(|a| unit_quote(a)).collect();
        out.push_str(&format!("ExecStart={}\n", args.join(" ")));
        out.push_str(&format!(
            "StandardOutput=append:{log}\nStandardError=append:{log}\n",
            log = job.log.replace('%', "%%")
        ));
        let timer = match job.when {
            When::Every { seconds } => {
                out.push_str("Nice=10\n");
                let minutes = (seconds / 60).max(1);
                format!("OnBootSec=1min\nOnUnitActiveSec={minutes}min\n")
            }
            When::At { hour, minute } => {
                format!("OnCalendar=*-*-* {hour:02}:{minute:02}:00\nPersistent=true\n")
            }
        };
        out.push_str(&format!(
            "\n# ~/.config/systemd/user/{name}.timer\n[Unit]\nDescription=hippo {what} timer\n\n[Timer]\n{timer}\n[Install]\nWantedBy=timers.target\n"
        ));
        timers.push(format!("{name}.timer"));
    }
    out.push_str(&format!(
        "\nsystemctl --user daemon-reload\nsystemctl --user enable --now {}\n",
        timers.join(" ")
    ));
    out
}

/// One job's property list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    label: &'static str,
    /// The argv: this binary's absolute path and the subcommand.
    arguments: Vec<String>,
    when: When,
    /// `EnvironmentVariables`, in order.
    environment: Vec<(String, String)>,
    /// Both `StandardOutPath` and `StandardErrorPath`.
    log: String,
}

/// When launchd runs a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// Every so many seconds and once at load, at background priority.
    Every { seconds: u64 },
    /// Daily at a time on the 24-hour clock.
    At { hour: u8, minute: u8 },
}

/// The ingest job and the dreaming job for `config`, running `exe` with
/// `environment`.
fn jobs(config: &Config, exe: &Path, environment: &[(String, String)]) -> Result<[Job; 2]> {
    let exe = utf8(exe)?;
    let log = |label: &str| utf8(&config.home.join("logs").join(format!("{label}.log")));
    let (hour, minute) = config
        .dream_at
        .split_once(':')
        .and_then(|(hour, minute)| Some((hour.parse().ok()?, minute.parse().ok()?)))
        .ok_or_else(|| anyhow!("dream_at is not HH:MM: {}", config.dream_at))?;
    Ok([
        Job {
            label: INGEST,
            arguments: vec![exe.clone(), "ingest".to_owned()],
            when: When::Every {
                seconds: config.ingest_interval_minutes.saturating_mul(60),
            },
            environment: environment.to_vec(),
            log: log(INGEST)?,
        },
        Job {
            label: DREAM,
            arguments: vec![exe, "dream".to_owned()],
            when: When::At { hour, minute },
            environment: environment.to_vec(),
            log: log(DREAM)?,
        },
    ])
}

impl Job {
    /// The job as an XML property list.
    fn plist(&self) -> String {
        let mut out = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n",
        );
        let key = |out: &mut String, name: &str| out.push_str(&format!("\t<key>{name}</key>\n"));
        key(&mut out, "Label");
        out.push_str(&format!("\t<string>{}</string>\n", xml(self.label)));
        key(&mut out, "ProgramArguments");
        out.push_str("\t<array>\n");
        for argument in &self.arguments {
            out.push_str(&format!("\t\t<string>{}</string>\n", xml(argument)));
        }
        out.push_str("\t</array>\n");
        match self.when {
            When::Every { seconds } => {
                key(&mut out, "StartInterval");
                out.push_str(&format!("\t<integer>{seconds}</integer>\n"));
                key(&mut out, "RunAtLoad");
                out.push_str("\t<true/>\n");
                key(&mut out, "Nice");
                out.push_str("\t<integer>10</integer>\n");
                key(&mut out, "ProcessType");
                out.push_str("\t<string>Background</string>\n");
            }
            When::At { hour, minute } => {
                key(&mut out, "StartCalendarInterval");
                out.push_str(&format!(
                    "\t<dict>\n\t\t<key>Hour</key>\n\t\t<integer>{hour}</integer>\n\
                     \t\t<key>Minute</key>\n\t\t<integer>{minute}</integer>\n\t</dict>\n"
                ));
            }
        }
        key(&mut out, "EnvironmentVariables");
        out.push_str("\t<dict>\n");
        for (name, value) in &self.environment {
            out.push_str(&format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                xml(name),
                xml(value)
            ));
        }
        out.push_str("\t</dict>\n");
        for name in ["StandardOutPath", "StandardErrorPath"] {
            key(&mut out, name);
            out.push_str(&format!("\t<string>{}</string>\n", xml(&self.log)));
        }
        out.push_str("</dict>\n</plist>\n");
        out
    }
}

/// The installing shell's `PATH` and `HOME`, and `HIPPO_HOME` and
/// `CLAUDE_CONFIG_DIR` when they are set and not empty. launchd starts a job
/// with almost no environment; without `CLAUDE_CONFIG_DIR` the dreaming
/// job's `claude` would use the default profile, which may not be signed in.
fn environment() -> Result<Vec<(String, String)>> {
    environment_from(|name| std::env::var(name).ok())
}

fn environment_from(var: impl Fn(&str) -> Option<String>) -> Result<Vec<(String, String)>> {
    let mut environment = Vec::new();
    for name in ["PATH", "HOME"] {
        let value = var(name).with_context(|| format!("{name} is not set"))?;
        environment.push((name.to_owned(), value));
    }
    for name in ["HIPPO_HOME", "CLAUDE_CONFIG_DIR"] {
        if let Some(value) = var(name).filter(|value| !value.is_empty()) {
            environment.push((name.to_owned(), value));
        }
    }
    Ok(environment)
}

/// `~/Library/LaunchAgents`.
fn launch_agents() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library").join("LaunchAgents"))
}

/// The user's id, from `id -u`.
fn uid() -> Result<u32> {
    let out = Process::new("id")
        .arg("-u")
        .output()
        .context("cannot run id -u")?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .context("id -u printed no user id")
}

/// Runs `launchctl` with `args`, its output discarded when `quiet`.
fn launchctl(args: &[&str], quiet: bool) -> Result<ExitStatus> {
    let mut command = Process::new("launchctl");
    command.args(args);
    if quiet {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    command.status().context("cannot run launchctl")
}

fn utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("not a UTF-8 path: {}", path.display()))
}

/// `text` with the five XML special characters escaped.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "workspace = \"/w\"\ningest_interval_minutes = 15\ndream_at = \"04:05\"\n",
        )
        .unwrap();
        let mut config = Config::load_in(home.path()).unwrap();
        config.home = PathBuf::from("/Users/me/.hippo");
        config
    }

    fn both() -> [Job; 2] {
        let environment = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), "/Users/me".to_owned()),
        ];
        jobs(
            &config(),
            Path::new("/Users/me/.cargo/bin/hippo"),
            &environment,
        )
        .unwrap()
    }

    #[test]
    fn the_environment_carries_the_claude_profile_when_set() {
        let set = |name: &str| match name {
            "PATH" => Some("/bin".to_owned()),
            "HOME" => Some("/Users/me".to_owned()),
            "CLAUDE_CONFIG_DIR" => Some("/Users/me/.claude-work".to_owned()),
            "HIPPO_HOME" => Some(String::new()),
            _ => None,
        };
        let names: Vec<_> = environment_from(set)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["PATH", "HOME", "CLAUDE_CONFIG_DIR"]);
        assert!(environment_from(|_| None).is_err());
    }

    #[test]
    fn hippo_home_and_the_binary_path_are_in_the_ingest_plist() {
        // Hippo.app reads `ProgramArguments[0]` and
        // `EnvironmentVariables.HIPPO_HOME` of this file, so both are an
        // interface.
        let set = |name: &str| match name {
            "PATH" => Some("/bin".to_owned()),
            "HOME" => Some("/Users/me".to_owned()),
            "HIPPO_HOME" => Some("/Users/me/hippo-home".to_owned()),
            _ => None,
        };
        let environment = environment_from(set).unwrap();
        let [ingest, _] = jobs(
            &config(),
            Path::new("/Users/me/.cargo/bin/hippo"),
            &environment,
        )
        .unwrap();
        let plist = ingest.plist();
        assert!(
            plist.contains(
                "\t<key>ProgramArguments</key>\n\t<array>\n\
                 \t\t<string>/Users/me/.cargo/bin/hippo</string>\n"
            ),
            "{plist}"
        );
        assert!(
            plist.contains(
                "\t<key>EnvironmentVariables</key>\n\t<dict>\n\
                 \t\t<key>PATH</key>\n\t\t<string>/bin</string>\n\
                 \t\t<key>HOME</key>\n\t\t<string>/Users/me</string>\n\
                 \t\t<key>HIPPO_HOME</key>\n\t\t<string>/Users/me/hippo-home</string>\n"
            ),
            "{plist}"
        );
    }

    #[test]
    fn the_ingest_job_runs_every_interval_at_load_and_in_the_background() {
        let [ingest, _] = both();
        assert_eq!(
            ingest.plist(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>dev.hippo.ingest</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n\
             \t\t<string>/Users/me/.cargo/bin/hippo</string>\n\t\t<string>ingest</string>\n\
             \t</array>\n\
             \t<key>StartInterval</key>\n\t<integer>900</integer>\n\
             \t<key>RunAtLoad</key>\n\t<true/>\n\
             \t<key>Nice</key>\n\t<integer>10</integer>\n\
             \t<key>ProcessType</key>\n\t<string>Background</string>\n\
             \t<key>EnvironmentVariables</key>\n\t<dict>\n\
             \t\t<key>PATH</key>\n\t\t<string>/usr/bin:/bin</string>\n\
             \t\t<key>HOME</key>\n\t\t<string>/Users/me</string>\n\
             \t</dict>\n\
             \t<key>StandardOutPath</key>\n\t<string>/Users/me/.hippo/logs/dev.hippo.ingest.log</string>\n\
             \t<key>StandardErrorPath</key>\n\t<string>/Users/me/.hippo/logs/dev.hippo.ingest.log</string>\n\
             </dict>\n</plist>\n"
        );
    }

    #[test]
    fn the_dream_job_runs_daily_at_dream_at() {
        let [_, dream] = both();
        assert_eq!(dream.arguments[1], "dream");
        assert_eq!(dream.when, When::At { hour: 4, minute: 5 });
        let plist = dream.plist();
        assert!(plist.contains(
            "\t<key>StartCalendarInterval</key>\n\t<dict>\n\t\t<key>Hour</key>\n\t\t<integer>4</integer>\n\
             \t\t<key>Minute</key>\n\t\t<integer>5</integer>\n\t</dict>\n"
        ));
        assert!(!plist.contains("RunAtLoad") && !plist.contains("StartInterval</key>"));
        assert!(plist.contains("logs/dev.hippo.dream.log"));
    }

    #[test]
    fn job_picks_one_label_and_none_picks_both() {
        assert_eq!(labels(None), [INGEST, DREAM]);
        assert_eq!(labels(Some(JobName::Ingest)), [INGEST]);
        assert_eq!(labels(Some(JobName::Dream)), [DREAM]);
    }

    #[test]
    fn values_are_escaped_for_xml() {
        assert_eq!(
            xml("a&b <c> \"d\" 'e'"),
            "a&amp;b &lt;c&gt; &quot;d&quot; &apos;e&apos;"
        );
        let environment = vec![("PATH".to_owned(), "/x&y".to_owned())];
        let [ingest, _] = jobs(&config(), Path::new("/tmp/hip<po"), &environment).unwrap();
        let plist = ingest.plist();
        assert!(plist.contains("<string>/tmp/hip&lt;po</string>"));
        assert!(plist.contains("<string>/x&amp;y</string>"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_both_property_lists() {
        let dir = tempfile::tempdir().unwrap();
        for job in both() {
            let path = dir.path().join(format!("{}.plist", job.label));
            fs::write(&path, job.plist()).unwrap();
            let out = Process::new("plutil")
                .arg("-lint")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }

    #[test]
    fn cron_lines_run_the_same_jobs_with_their_environment_and_logs() {
        let [ingest, dream] = both();
        let text = cron_text(&[&ingest, &dream]);
        assert!(
            text.contains(
                "*/15 * * * * PATH=/usr/bin:/bin HOME=/Users/me /Users/me/.cargo/bin/hippo ingest >> /Users/me/.hippo/logs/dev.hippo.ingest.log 2>&1\n"
            ),
            "{text}"
        );
        assert!(text.contains("\n5 4 * * * PATH="), "{text}");
        assert_eq!(cron_when(When::Every { seconds: 7200 }), "0 */2 * * *");
        // cron reads % as a line break; a value with spaces is quoted.
        let mut odd = ingest.clone();
        odd.environment = vec![("HOME".into(), "/home/a b%c".into())];
        assert!(cron_text(&[&odd]).contains("HOME='/home/a b\\%c'"));
    }

    #[test]
    fn systemd_units_run_the_same_jobs() {
        let [ingest, dream] = both();
        let text = systemd_text(&[&ingest, &dream]);
        for line in [
            "# ~/.config/systemd/user/hippo-ingest.service",
            "Environment=\"PATH=/usr/bin:/bin\"",
            "ExecStart=\"/Users/me/.cargo/bin/hippo\" \"ingest\"",
            "StandardOutput=append:/Users/me/.hippo/logs/dev.hippo.ingest.log",
            "OnUnitActiveSec=15min",
            "OnCalendar=*-*-* 04:05:00",
            "systemctl --user enable --now hippo-ingest.timer hippo-dream.timer",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_macos_status_and_uninstall_write_nothing() {
        for command in [
            Command::Uninstall {
                job: Some(JobName::Dream),
            },
            Command::Status,
        ] {
            assert!(run(command).is_ok());
        }
    }
}
