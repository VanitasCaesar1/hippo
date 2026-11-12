//! Configuration from environment variables.
//!
//! Each subcommand reads only the variables it uses. A value that breaks a
//! rule stops the command at start, and the error names the variable.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use url::Url;

use crate::policy;

/// The subcommand being configured; it decides which variables are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Serve,
    Migrate,
    Backup,
    SyncRegistry,
    Activity,
    Account,
    MockOauth,
    SeedDemo,
}

/// A configuration error: the variable at fault and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{variable}: {message}")]
pub struct ConfigError {
    pub variable: &'static str,
    pub message: String,
}

fn err(variable: &'static str, message: impl Into<String>) -> ConfigError {
    ConfigError {
        variable,
        message: message.into(),
    }
}

/// Where the registry index is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryIndex {
    Url(String),
    Path(String),
}

impl RegistryIndex {
    /// The text the install command uses: the URL or the file path.
    pub fn as_str(&self) -> &str {
        match self {
            RegistryIndex::Url(s) | RegistryIndex::Path(s) => s,
        }
    }
}

/// An eval set the operator allows measurements against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalSet {
    pub id: String,
    pub planted: u32,
    pub kept: u32,
}

/// The OAuth app's credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubCredentials {
    pub client_id: String,
    pub client_secret: String,
}

/// The configuration. Fields a command does not read hold their defaults.
#[derive(Debug, Clone)]
pub struct Config {
    pub dev: bool,
    /// The public origin without a trailing slash; set for `serve`.
    pub base_url: Option<String>,
    pub bind: SocketAddr,
    pub db: Option<PathBuf>,
    pub github: Option<GithubCredentials>,
    pub oauth_authorize_url: String,
    pub oauth_token_url: String,
    pub oauth_api_url: String,
    pub admins: Vec<i64>,
    pub registry_index: Option<RegistryIndex>,
    pub raw_url: String,
    pub registry_sync_minutes: u32,
    pub eval_sets: Vec<EvalSet>,
    pub trust_proxy: bool,
    pub contact: Option<String>,
}

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const DEFAULT_TOKEN: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_API: &str = "https://api.github.com";
const DEFAULT_RAW: &str = "https://raw.githubusercontent.com";

/// The variables a command reads, so tests and docs agree with the code.
fn reads(command: Command) -> &'static [&'static str] {
    match command {
        Command::Migrate | Command::Backup | Command::Activity | Command::Account => &["FEED_DB"],
        Command::SyncRegistry => &["FEED_DB", "FEED_REGISTRY_INDEX", "FEED_RAW_URL", "FEED_DEV"],
        Command::MockOauth => &["FEED_DEV"],
        Command::SeedDemo => &["FEED_DB", "FEED_DEV"],
        Command::Serve => &[
            "FEED_BASE_URL",
            "FEED_BIND",
            "FEED_DB",
            "FEED_GITHUB_CLIENT_ID",
            "FEED_GITHUB_CLIENT_SECRET",
            "FEED_OAUTH_AUTHORIZE_URL",
            "FEED_OAUTH_TOKEN_URL",
            "FEED_OAUTH_API_URL",
            "FEED_ADMINS",
            "FEED_REGISTRY_INDEX",
            "FEED_RAW_URL",
            "FEED_REGISTRY_SYNC_MINUTES",
            "FEED_EVAL_SETS",
            "FEED_TRUST_PROXY",
            "FEED_CONTACT",
            "FEED_DEV",
        ],
    }
}

impl Config {
    /// Reads the process environment.
    pub fn from_env(command: Command) -> Result<Config, ConfigError> {
        let vars: HashMap<String, String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("FEED_"))
            .collect();
        Config::from_vars(command, &vars)
    }

    /// Reads the given variables; for tests.
    pub fn from_vars(
        command: Command,
        vars: &HashMap<String, String>,
    ) -> Result<Config, ConfigError> {
        let read = reads(command);
        let reading = |name: &str| read.contains(&name);
        // A variable the command does not read is ignored; one set to the
        // empty string counts as unset.
        let get = |name: &'static str| -> Option<&str> {
            if !reading(name) {
                return None;
            }
            vars.get(name).map(String::as_str).filter(|v| !v.is_empty())
        };

        let dev = match get("FEED_DEV") {
            None => false,
            Some(v) => parse_flag("FEED_DEV", v)?,
        };

        let db = get("FEED_DB").map(PathBuf::from);
        if reading("FEED_DB") && db.is_none() {
            return Err(err("FEED_DB", "is required"));
        }

        let bind = match get("FEED_BIND") {
            None => DEFAULT_BIND
                .parse()
                .map_err(|_| err("FEED_BIND", "the default is not a socket address"))?,
            Some(v) => v
                .parse::<SocketAddr>()
                .map_err(|_| err("FEED_BIND", format!("{v:?} is not a socket address")))?,
        };

        let base_url = match get("FEED_BASE_URL") {
            None if command == Command::Serve => {
                return Err(err("FEED_BASE_URL", "is required"));
            }
            None => None,
            Some(v) => Some(parse_origin("FEED_BASE_URL", v)?),
        };

        let client_id = get("FEED_GITHUB_CLIENT_ID");
        let client_secret = get("FEED_GITHUB_CLIENT_SECRET");
        let github = match (client_id, client_secret) {
            (None, None) => None,
            (Some(id), Some(secret)) => Some(GithubCredentials {
                client_id: id.to_string(),
                client_secret: secret.to_string(),
            }),
            (Some(_), None) => {
                return Err(err(
                    "FEED_GITHUB_CLIENT_SECRET",
                    "must be set when FEED_GITHUB_CLIENT_ID is set",
                ))
            }
            (None, Some(_)) => {
                return Err(err(
                    "FEED_GITHUB_CLIENT_ID",
                    "must be set when FEED_GITHUB_CLIENT_SECRET is set",
                ))
            }
        };

        let oauth_authorize_url = url_or_default(
            "FEED_OAUTH_AUTHORIZE_URL",
            get("FEED_OAUTH_AUTHORIZE_URL"),
            DEFAULT_AUTHORIZE,
        )?;
        let oauth_token_url = url_or_default(
            "FEED_OAUTH_TOKEN_URL",
            get("FEED_OAUTH_TOKEN_URL"),
            DEFAULT_TOKEN,
        )?;
        let oauth_api_url =
            url_or_default("FEED_OAUTH_API_URL", get("FEED_OAUTH_API_URL"), DEFAULT_API)?;
        let raw_url = match get("FEED_RAW_URL") {
            Some(v) => parse_origin("FEED_RAW_URL", v)?,
            None if reading("FEED_RAW_URL") => parse_origin("FEED_RAW_URL", DEFAULT_RAW)?,
            None => DEFAULT_RAW.to_string(),
        };

        let admins = match get("FEED_ADMINS") {
            None => Vec::new(),
            Some(v) => parse_admins(v)?,
        };

        let registry_index = match get("FEED_REGISTRY_INDEX") {
            None => None,
            Some(v) => Some(parse_registry_index(v, dev)?),
        };

        let registry_sync_minutes = match get("FEED_REGISTRY_SYNC_MINUTES") {
            None => 15,
            Some(v) => match v.parse::<u32>() {
                Ok(n) if n <= 1440 => n,
                _ => {
                    return Err(err(
                        "FEED_REGISTRY_SYNC_MINUTES",
                        format!("{v:?} is not an integer from 0 to 1440"),
                    ))
                }
            },
        };

        let eval_sets = match get("FEED_EVAL_SETS") {
            None => Vec::new(),
            Some(v) => parse_eval_sets(v)?,
        };

        let trust_proxy = match get("FEED_TRUST_PROXY") {
            None => false,
            Some(v) => parse_flag("FEED_TRUST_PROXY", v)?,
        };

        let contact = match get("FEED_CONTACT") {
            None => None,
            Some(v) => {
                let n = v.chars().count();
                if !(1..=policy::CONTACT_MAX).contains(&n) {
                    return Err(err("FEED_CONTACT", "must be 1 to 200 characters"));
                }
                if v.chars().any(is_control) {
                    return Err(err("FEED_CONTACT", "must not contain control characters"));
                }
                Some(v.to_string())
            }
        };

        let config = Config {
            dev,
            base_url,
            bind,
            db,
            github,
            oauth_authorize_url,
            oauth_token_url,
            oauth_api_url,
            admins,
            registry_index,
            raw_url,
            registry_sync_minutes,
            eval_sets,
            trust_proxy,
            contact,
        };
        config.check_dev_guard(command)?;
        Ok(config)
    }

    /// The dev-mode guard. With `FEED_DEV=1` the base URL is `http` on
    /// `127.0.0.1` or `localhost` and the bind address is loopback; without
    /// it every URL variable is `https`.
    fn check_dev_guard(&self, command: Command) -> Result<(), ConfigError> {
        let read = reads(command);
        let reading = |name: &str| read.contains(&name);
        if self.dev {
            if let Some(base) = &self.base_url {
                let url = Url::parse(base).map_err(|e| err("FEED_BASE_URL", e.to_string()))?;
                let local = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
                if url.scheme() != "http" || !local {
                    return Err(err(
                        "FEED_BASE_URL",
                        "with FEED_DEV=1 must be http on 127.0.0.1 or localhost",
                    ));
                }
            }
            if reading("FEED_BIND") && !self.bind.ip().is_loopback() {
                return Err(err(
                    "FEED_BIND",
                    "with FEED_DEV=1 must be a loopback address",
                ));
            }
            return Ok(());
        }
        let https_only: [(&'static str, Option<&str>); 6] = [
            ("FEED_BASE_URL", self.base_url.as_deref()),
            ("FEED_OAUTH_AUTHORIZE_URL", Some(&self.oauth_authorize_url)),
            ("FEED_OAUTH_TOKEN_URL", Some(&self.oauth_token_url)),
            ("FEED_OAUTH_API_URL", Some(&self.oauth_api_url)),
            ("FEED_RAW_URL", Some(&self.raw_url)),
            (
                "FEED_REGISTRY_INDEX",
                match &self.registry_index {
                    Some(RegistryIndex::Url(u)) => Some(u.as_str()),
                    _ => None,
                },
            ),
        ];
        for (name, value) in https_only {
            if let Some(v) = value {
                if reading(name) && !v.starts_with("https://") {
                    return Err(err(name, "must be an https URL unless FEED_DEV=1"));
                }
            }
        }
        Ok(())
    }
}

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}')
}

fn parse_flag(name: &'static str, v: &str) -> Result<bool, ConfigError> {
    match v {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(err(name, format!("{v:?} must be 0 or 1"))),
    }
}

/// A URL with a host and no credentials, query or fragment.
fn parse_plain_url(name: &'static str, v: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(v).map_err(|e| err(name, format!("{v:?} is not a URL: {e}")))?;
    if url.host_str().is_none() || !matches!(url.scheme(), "http" | "https") {
        return Err(err(
            name,
            format!("{v:?} must be an http or https URL with a host"),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(err(name, "must not contain a user name or password"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(err(name, "must not contain a query or a fragment"));
    }
    Ok(url)
}

/// An origin: a plain URL with no path other than `/`, stored without a
/// trailing slash.
fn parse_origin(name: &'static str, v: &str) -> Result<String, ConfigError> {
    let url = parse_plain_url(name, v)?;
    if url.path() != "/" {
        return Err(err(name, "must not have a path"));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// A URL with a host; the value is kept as written.
fn url_or_default(
    name: &'static str,
    value: Option<&str>,
    default: &str,
) -> Result<String, ConfigError> {
    let text = value.unwrap_or(default);
    let url = Url::parse(text).map_err(|e| err(name, format!("{text:?} is not a URL: {e}")))?;
    if url.host_str().is_none() || !matches!(url.scheme(), "http" | "https") {
        return Err(err(
            name,
            format!("{text:?} must be an http or https URL with a host"),
        ));
    }
    Ok(text.to_string())
}

fn parse_admins(v: &str) -> Result<Vec<i64>, ConfigError> {
    let mut out = Vec::new();
    for part in v.split(',') {
        match part.trim().parse::<i64>() {
            Ok(n) if n > 0 => out.push(n),
            _ => {
                return Err(err(
                    "FEED_ADMINS",
                    format!("{:?} is not a positive integer", part.trim()),
                ))
            }
        }
    }
    Ok(out)
}

fn parse_registry_index(v: &str, dev: bool) -> Result<RegistryIndex, ConfigError> {
    const NAME: &str = "FEED_REGISTRY_INDEX";
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~' | '/' | ':'))
    {
        return Err(err(
            NAME,
            "may use only the characters A-Z a-z 0-9 . - _ ~ / :",
        ));
    }
    if v.starts_with("https://") || v.starts_with("http://") {
        let url = parse_plain_url(NAME, v)?;
        if url.scheme() == "http" && !dev {
            return Err(err(NAME, "must be an https URL unless FEED_DEV=1"));
        }
        Ok(RegistryIndex::Url(v.to_string()))
    } else {
        Ok(RegistryIndex::Path(v.to_string()))
    }
}

fn parse_eval_sets(v: &str) -> Result<Vec<EvalSet>, ConfigError> {
    const NAME: &str = "FEED_EVAL_SETS";
    let mut out: Vec<EvalSet> = Vec::new();
    for entry in v.split(',') {
        let entry = entry.trim();
        let parts: Vec<&str> = entry.split(':').collect();
        let [id, planted, kept] = parts[..] else {
            return Err(err(NAME, format!("{entry:?} is not <id>:<planted>:<kept>")));
        };
        let id_ok = id.len() == 12 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !id_ok {
            return Err(err(
                NAME,
                format!("{id:?} is not a set id of 12 lower-case hex digits"),
            ));
        }
        let count = |text: &str, what: &str| -> Result<u32, ConfigError> {
            match text.parse::<u32>() {
                Ok(n) if n >= policy::EVAL_SET_MIN_COUNT => Ok(n),
                _ => Err(err(
                    NAME,
                    format!("{what} {text:?} is not an integer of at least 5"),
                )),
            }
        };
        let set = EvalSet {
            id: id.to_string(),
            planted: count(planted, "planted errors")?,
            kept: count(kept, "kept claims")?,
        };
        if out.iter().any(|s| s.id == set.id) {
            return Err(err(NAME, format!("set id {id} appears twice")));
        }
        out.push(set);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn serve(extra: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let mut pairs = vec![
            ("FEED_BASE_URL", "https://feed.example"),
            ("FEED_DB", "/tmp/feed.sqlite"),
        ];
        pairs.extend_from_slice(extra);
        Config::from_vars(Command::Serve, &vars(&pairs))
    }

    fn refused(extra: &[(&str, &str)], variable: &str) {
        let e = serve(extra).expect_err("should be refused");
        assert_eq!(e.variable, variable, "{e}");
        assert!(e.to_string().starts_with(variable), "{e}");
    }

    #[test]
    fn minimal_serve_config_uses_defaults() {
        let c = serve(&[]).expect("valid");
        assert_eq!(c.base_url.as_deref(), Some("https://feed.example"));
        assert_eq!(c.bind, "127.0.0.1:8080".parse().expect("addr"));
        assert_eq!(c.registry_sync_minutes, 15);
        assert!(c.admins.is_empty() && c.eval_sets.is_empty());
        assert_eq!(c.raw_url, "https://raw.githubusercontent.com");
        assert!(!c.dev && !c.trust_proxy);
    }

    #[test]
    fn base_url_rules() {
        let c = serve(&[("FEED_BASE_URL", "https://feed.example:8443/")]).expect("valid");
        assert_eq!(c.base_url.as_deref(), Some("https://feed.example:8443"));
        for bad in [
            "feed.example",
            "https://user:pw@feed.example",
            "https://feed.example/?a=1",
            "https://feed.example/#x",
            "https://feed.example/path",
            "ftp://feed.example",
        ] {
            refused(&[("FEED_BASE_URL", bad)], "FEED_BASE_URL");
        }
        let e = Config::from_vars(Command::Serve, &vars(&[("FEED_DB", "x")])).expect_err("missing");
        assert_eq!(e.variable, "FEED_BASE_URL");
    }

    #[test]
    fn bind_and_db_rules() {
        refused(&[("FEED_BIND", "localhost")], "FEED_BIND");
        refused(&[("FEED_BIND", "8080")], "FEED_BIND");
        let e = Config::from_vars(Command::Migrate, &vars(&[])).expect_err("db required");
        assert_eq!(e.variable, "FEED_DB");
    }

    #[test]
    fn github_credentials_come_in_pairs() {
        refused(
            &[("FEED_GITHUB_CLIENT_ID", "abc")],
            "FEED_GITHUB_CLIENT_SECRET",
        );
        refused(
            &[("FEED_GITHUB_CLIENT_SECRET", "abc")],
            "FEED_GITHUB_CLIENT_ID",
        );
        let c = serve(&[
            ("FEED_GITHUB_CLIENT_ID", "abc"),
            ("FEED_GITHUB_CLIENT_SECRET", "def"),
        ])
        .expect("valid");
        assert!(c.github.is_some());
    }

    #[test]
    fn oauth_and_raw_urls_need_a_host() {
        refused(
            &[("FEED_OAUTH_AUTHORIZE_URL", "not a url")],
            "FEED_OAUTH_AUTHORIZE_URL",
        );
        refused(
            &[("FEED_OAUTH_TOKEN_URL", "https://")],
            "FEED_OAUTH_TOKEN_URL",
        );
        refused(&[("FEED_OAUTH_API_URL", "file:///x")], "FEED_OAUTH_API_URL");
        refused(
            &[("FEED_RAW_URL", "https://raw.example/path")],
            "FEED_RAW_URL",
        );
        let c = serve(&[("FEED_RAW_URL", "https://raw.example/")]).expect("valid");
        assert_eq!(c.raw_url, "https://raw.example");
    }

    #[test]
    fn admins_are_positive_integers() {
        let c = serve(&[("FEED_ADMINS", "1001, 42")]).expect("valid");
        assert_eq!(c.admins, vec![1001, 42]);
        refused(&[("FEED_ADMINS", "1001,abc")], "FEED_ADMINS");
        refused(&[("FEED_ADMINS", "0")], "FEED_ADMINS");
        refused(&[("FEED_ADMINS", "-3")], "FEED_ADMINS");
        refused(&[("FEED_ADMINS", "1,,2")], "FEED_ADMINS");
    }

    #[test]
    fn registry_index_rules() {
        let c = serve(&[("FEED_REGISTRY_INDEX", "https://r.example/index.json")]).expect("url");
        assert_eq!(
            c.registry_index,
            Some(RegistryIndex::Url("https://r.example/index.json".into()))
        );
        let c = serve(&[("FEED_REGISTRY_INDEX", "/srv/index.json")]).expect("path");
        assert_eq!(
            c.registry_index,
            Some(RegistryIndex::Path("/srv/index.json".into()))
        );
        for bad in [
            "http://r.example/index.json",
            "https://r.example/index.json?x=1",
            "https://r.example/index.json#x",
            "https://u:p@r.example/index.json",
            "/srv/my index.json",
            "/srv/index;rm.json",
            "https://r.example/a b.json",
        ] {
            refused(&[("FEED_REGISTRY_INDEX", bad)], "FEED_REGISTRY_INDEX");
        }
    }

    #[test]
    fn sync_minutes_range() {
        assert_eq!(
            serve(&[("FEED_REGISTRY_SYNC_MINUTES", "0")])
                .expect("zero")
                .registry_sync_minutes,
            0
        );
        assert!(serve(&[("FEED_REGISTRY_SYNC_MINUTES", "1440")]).is_ok());
        refused(
            &[("FEED_REGISTRY_SYNC_MINUTES", "1441")],
            "FEED_REGISTRY_SYNC_MINUTES",
        );
        refused(
            &[("FEED_REGISTRY_SYNC_MINUTES", "-1")],
            "FEED_REGISTRY_SYNC_MINUTES",
        );
        refused(
            &[("FEED_REGISTRY_SYNC_MINUTES", "soon")],
            "FEED_REGISTRY_SYNC_MINUTES",
        );
    }

    #[test]
    fn eval_set_rules() {
        let c = serve(&[("FEED_EVAL_SETS", "3f2a9c01b7d4:6:8,000000000000:5:5")]).expect("valid");
        assert_eq!(c.eval_sets.len(), 2);
        assert_eq!(c.eval_sets[0].planted, 6);
        for bad in [
            "3f2a9c01b7d4:6",
            "3F2A9C01B7D4:6:8",
            "3f2a9c01b7:6:8",
            "3f2a9c01b7d4:4:8",
            "3f2a9c01b7d4:6:4",
            "3f2a9c01b7d4:six:8",
            "3f2a9c01b7d4:6:8,3f2a9c01b7d4:7:9",
            "3f2a9c01b7d4:6:8,",
        ] {
            refused(&[("FEED_EVAL_SETS", bad)], "FEED_EVAL_SETS");
        }
    }

    #[test]
    fn flags_and_contact() {
        refused(&[("FEED_TRUST_PROXY", "yes")], "FEED_TRUST_PROXY");
        refused(&[("FEED_TRUST_PROXY", "2")], "FEED_TRUST_PROXY");
        refused(&[("FEED_DEV", "true")], "FEED_DEV");
        assert!(serve(&[("FEED_TRUST_PROXY", "1")]).expect("on").trust_proxy);
        refused(&[("FEED_CONTACT", "a\nb")], "FEED_CONTACT");
        refused(&[("FEED_CONTACT", &"x".repeat(201))], "FEED_CONTACT");
        assert!(serve(&[("FEED_CONTACT", &"x".repeat(200))]).is_ok());
    }

    #[test]
    fn empty_value_counts_as_unset() {
        let c = serve(&[
            ("FEED_BIND", ""),
            ("FEED_ADMINS", ""),
            ("FEED_TRUST_PROXY", ""),
            ("FEED_REGISTRY_INDEX", ""),
            ("FEED_REGISTRY_SYNC_MINUTES", ""),
            ("FEED_GITHUB_CLIENT_ID", ""),
            ("FEED_CONTACT", ""),
            ("FEED_RAW_URL", ""),
        ])
        .expect("valid");
        assert_eq!(c.bind.port(), 8080);
        assert!(c.admins.is_empty() && c.registry_index.is_none() && c.contact.is_none());
        assert_eq!(c.registry_sync_minutes, 15);
        // An empty required variable is missing.
        let e = Config::from_vars(Command::Migrate, &vars(&[("FEED_DB", "")])).expect_err("db");
        assert_eq!(e.variable, "FEED_DB");
    }

    #[test]
    fn dev_mode_guard_both_ways() {
        let dev = [
            ("FEED_DEV", "1"),
            ("FEED_BASE_URL", "http://127.0.0.1:9000"),
            ("FEED_BIND", "127.0.0.1:9000"),
        ];
        let c = serve(&dev).expect("dev ok");
        assert!(c.dev);
        assert!(serve(&[
            ("FEED_DEV", "1"),
            ("FEED_BASE_URL", "http://localhost:9000"),
            ("FEED_BIND", "[::1]:9000"),
        ])
        .is_ok());
        // Dev mode refuses https, other hosts and non-loopback binds.
        refused(&[("FEED_DEV", "1")], "FEED_BASE_URL");
        refused(
            &[
                ("FEED_DEV", "1"),
                ("FEED_BASE_URL", "http://feed.example"),
                ("FEED_BIND", "127.0.0.1:9000"),
            ],
            "FEED_BASE_URL",
        );
        refused(
            &[
                ("FEED_DEV", "1"),
                ("FEED_BASE_URL", "http://127.0.0.1:9000"),
                ("FEED_BIND", "0.0.0.0:9000"),
            ],
            "FEED_BIND",
        );
        // Without dev mode every URL must be https.
        refused(&[("FEED_BASE_URL", "http://feed.example")], "FEED_BASE_URL");
        refused(
            &[(
                "FEED_OAUTH_AUTHORIZE_URL",
                "http://127.0.0.1:9100/authorize",
            )],
            "FEED_OAUTH_AUTHORIZE_URL",
        );
        refused(&[("FEED_RAW_URL", "http://127.0.0.1:9200")], "FEED_RAW_URL");
        // In dev mode the other URLs may use http.
        assert!(serve(&[
            ("FEED_DEV", "1"),
            ("FEED_BASE_URL", "http://127.0.0.1:9000"),
            ("FEED_BIND", "127.0.0.1:9000"),
            (
                "FEED_OAUTH_AUTHORIZE_URL",
                "http://127.0.0.1:9100/authorize"
            ),
            ("FEED_RAW_URL", "http://127.0.0.1:9200"),
            ("FEED_REGISTRY_INDEX", "http://127.0.0.1:9200/index.json"),
        ])
        .is_ok());
    }

    #[test]
    fn a_command_reads_only_its_variables() {
        // A bad value in a variable the command does not read is ignored.
        let v = vars(&[
            ("FEED_DB", "x.sqlite"),
            ("FEED_BIND", "junk"),
            ("FEED_ADMINS", "no"),
        ]);
        assert!(Config::from_vars(Command::Migrate, &v).is_ok());
        assert!(Config::from_vars(Command::Backup, &v).is_ok());
        let e = Config::from_vars(Command::Serve, &v).expect_err("serve reads them");
        assert_eq!(e.variable, "FEED_BIND");
        // mock-oauth reads only FEED_DEV.
        assert!(Config::from_vars(Command::MockOauth, &vars(&[("FEED_DEV", "1")])).is_ok());
        // sync-registry reads the index and raw URL.
        let v = vars(&[("FEED_DB", "x"), ("FEED_REGISTRY_INDEX", "bad index")]);
        let e = Config::from_vars(Command::SyncRegistry, &v).expect_err("index");
        assert_eq!(e.variable, "FEED_REGISTRY_INDEX");
    }
}
