//! The install box's command and `hippocamp://add` link for a pack of passes.
//!
//! Both carry only identifiers the entry rules constrain: the pack name admits no shell
//! syntax and no leading `-`, and the version and hash use unreserved
//! characters, so nothing is quoted or encoded.

use crate::config::{Config, RegistryIndex};
use crate::registry::{Kind, PackRow};

/// The file the URL form of the command downloads the index to.
const INDEX_FILE: &str = "hippocamp-index.json";

/// What a visible post of a listed pack of passes offers to install it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLinks {
    /// The command to copy.
    pub command: String,
    /// `hippocamp://add?pack=<name>&version=<version>&sha256=<pack_hash>`.
    pub deep_link: String,
}

/// The install command and deep link, or `None` unless the post is visible
/// and the pack is a listed pack of passes (so never for a theme pack or a
/// delisted pack), and `FEED_REGISTRY_INDEX` is set.
///
/// With the index a URL the command downloads it first; with a path it points
/// `--registry` at the path.
pub fn install_links(post_visible: bool, pack: &PackRow, config: &Config) -> Option<InstallLinks> {
    if !post_visible || !pack.listed || pack.kind != Kind::Passes {
        return None;
    }
    let index = config.registry_index.as_ref()?;
    let name = &pack.name;
    let command = match index {
        RegistryIndex::Url(url) => {
            format!(
                "curl -fsSLo {INDEX_FILE} {url} && hippocamp add {name} --registry {INDEX_FILE}"
            )
        }
        RegistryIndex::Path(path) => format!("hippocamp add {name} --registry {path}"),
    };
    let deep_link = format!(
        "hippocamp://add?pack={name}&version={}&sha256={}",
        pack.version, pack.pack_hash
    );
    Some(InstallLinks { command, deep_link })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Command;
    use crate::registry::Pass;
    use std::collections::HashMap;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn config(index: Option<&str>) -> Config {
        let mut vars = HashMap::new();
        vars.insert("FEED_DB".to_owned(), "/tmp/feed.sqlite".to_owned());
        if let Some(index) = index {
            vars.insert("FEED_REGISTRY_INDEX".to_owned(), index.to_owned());
        }
        Config::from_vars(Command::SyncRegistry, &vars).expect("a valid configuration")
    }

    fn pack() -> PackRow {
        PackRow {
            name: "glossary-terms".into(),
            kind: Kind::Passes,
            version: "1.2.3".into(),
            description: "d".into(),
            authors: vec![],
            owner: "ada".into(),
            owner_id: 1001,
            maintainers: vec!["ada".into()],
            first_party: false,
            license: "MIT".into(),
            repo: "https://github.com/ada/hippocamp-packs".into(),
            repo_id: 1,
            path: "glossary-terms".into(),
            commit_id: "a".repeat(40),
            pack_hash: HASH.into(),
            passes: vec![Pass {
                name: "p".into(),
                reads: vec![],
            }],
            theme_contract: None,
            theme_sha256: None,
            review_id: "r".into(),
            review_at: "2026-11-08T11:00:00.000000Z".into(),
            listed: true,
            synced_at: "2026-11-08T11:00:00.000000Z".into(),
        }
    }

    #[test]
    fn a_url_index_is_downloaded_first() {
        let links = install_links(true, &pack(), &config(Some("https://r.example/index.json")))
            .expect("links");
        assert_eq!(
            links.command,
            "curl -fsSLo hippocamp-index.json https://r.example/index.json && hippocamp add glossary-terms --registry hippocamp-index.json"
        );
        assert_eq!(
            links.deep_link,
            format!("hippocamp://add?pack=glossary-terms&version=1.2.3&sha256={HASH}")
        );
    }

    #[test]
    fn a_file_index_is_named_directly() {
        let links = install_links(true, &pack(), &config(Some("/srv/hippocamp/index.json")))
            .expect("links");
        assert_eq!(
            links.command,
            "hippocamp add glossary-terms --registry /srv/hippocamp/index.json"
        );
    }

    #[test]
    fn a_relative_file_index_is_named_as_given() {
        let links =
            install_links(true, &pack(), &config(Some("fixtures/index.json"))).expect("links");
        assert_eq!(
            links.command,
            "hippocamp add glossary-terms --registry fixtures/index.json"
        );
    }

    #[test]
    fn no_index_means_no_command() {
        assert_eq!(install_links(true, &pack(), &config(None)), None);
    }

    #[test]
    fn a_post_that_is_not_visible_has_none() {
        assert_eq!(
            install_links(false, &pack(), &config(Some("/srv/index.json"))),
            None
        );
    }

    #[test]
    fn a_delisted_pack_has_none() {
        let mut p = pack();
        p.listed = false;
        assert_eq!(
            install_links(true, &p, &config(Some("/srv/index.json"))),
            None
        );
    }

    #[test]
    fn a_theme_pack_has_none() {
        let mut p = pack();
        p.kind = Kind::Theme;
        p.passes.clear();
        assert_eq!(
            install_links(true, &p, &config(Some("/srv/index.json"))),
            None
        );
    }
}
