//! The registry's static site: `packs.html`, `themes.html`, one page per
//! pack, and, given one, the landing page as
//! `index.html`. Plain HTML in the web theme contract's
//! `hp-` classes, no script and no request to another host. Every string
//! from a pack, an entry or a review goes through [`escape`]; links are
//! relative, or GitHub links built from validated fields with each path
//! segment percent-encoded.

use std::fmt::Write as _;

use hippocamp_core::packs::manifest::Kind;
use hippocamp_core::packs::registry::{Index, IndexPack, Settings};
use hippocamp_core::packs::validate::{filter_words, theme_lines};
use time::format_description::well_known::Rfc3339;

use crate::check::Listed;

/// The content security policy of every page, also set as a meta tag.
const CSP: &str = "default-src 'none'; style-src 'self'; img-src 'self' data:";

/// The text of `text` with `&`, `<`, `>`, `"` and `'` written as entities.
/// The one function every interpolated string passes through.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// One path segment, percent-encoded: everything but the unreserved
/// characters `A-Z a-z 0-9 - . _ ~`, byte by byte in UTF-8.
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// `/`-separated `path` with each segment percent-encoded.
fn encode_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// `https://github.com/<owner>/<repo>/tree/<commit>/<path>` for a pack,
/// from the entry's repository URL (`https://github.com/<owner>/<repo>`).
pub fn tree_url(repo: &str, commit: &str, path: &str) -> String {
    let name = repo.strip_prefix("https://github.com/").unwrap_or(repo);
    let mut url = format!("https://github.com/{}", encode_path(name));
    let _ = write!(url, "/tree/{}", encode_segment(commit));
    let path = encode_path(path);
    if !path.is_empty() {
        url.push('/');
        url.push_str(&path);
    }
    url
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Current {
    Home,
    Packs,
    Themes,
}

/// Where a page sits: its title, body class, which navigation link is
/// current, and the prefix that reaches the site's root from it.
struct Frame<'a> {
    title: &'a str,
    body_class: &'a str,
    current: Current,
    prefix: &'a str,
}

/// The `<pre>`-ready words for each input a pass may read.
fn input_words(input: &str) -> &'static str {
    match input {
        "sessions" => "session records with their task, notes touched and handoff",
        "notes" => "the notes it works on, in full",
        "turns" => "transcript turns that mention the note, already redacted",
        "human_confirmed" => "text a human restored with `hippocamp revert`",
        _ => "an input hippocamp does not describe here",
    }
}

fn page(frame: &Frame, settings: &Settings, generated: &str, main: &str) -> String {
    let p = frame.prefix;
    let current = |which: Current| {
        if frame.current == which {
            " class=\"hp-nav-current\" aria-current=\"page\""
        } else {
            ""
        }
    };
    let submit = format!("{}/blob/main/registry/README.md", settings.repo);
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n\
         <title>{title} | hippocamp</title>\n\
         <link rel=\"icon\" type=\"image/svg+xml\" href=\"{p}icon.svg\">\n\
         <link rel=\"stylesheet\" href=\"{p}hippocamp.css\">\n\
         <link rel=\"stylesheet\" href=\"{p}registry.css\">\n</head>\n\
         <body class=\"hp-page {body}\">\n\
         <a class=\"hp-skip\" href=\"#main\">Skip to the content</a>\n\
         <div class=\"hp-content\">\n\
         <header class=\"hp-topbar\">\
         <a class=\"hp-home\" href=\"{p}index.html\"{home}>{mark}<span class=\"hp-name\">hippocamp</span></a>\
         <nav class=\"hp-nav\" aria-label=\"Site\">\
         <a href=\"{p}docs/index.html\">docs</a><span class=\"hp-sep\"> | </span>\
         <a href=\"{p}packs.html\"{packs}>packs</a><span class=\"hp-sep\"> | </span>\
         <a href=\"{p}themes.html\"{themes}>themes</a><span class=\"hp-sep\"> | </span>\
         <a href=\"{submit}\">submit</a><span class=\"hp-sep\"> | </span>\
         <a href=\"{repo}\">github</a></nav></header>\n\
         <main class=\"hp-main\" id=\"main\">\n{main}</main>\n\
         <footer class=\"hp-footer\">{footer}</footer>\n\
         </div>\n</body>\n</html>\n",
        csp = escape(CSP),
        title = escape(frame.title),
        body = escape(frame.body_class),
        mark = hippocamp_theme::mark_svg("hp-mark"),
        home = if frame.current == Current::Home {
            " aria-current=\"page\""
        } else {
            ""
        },
        packs = current(Current::Packs),
        themes = current(Current::Themes),
        repo = escape(&settings.repo),
        footer = if frame.current == Current::Home {
            format!(
                "hippocamp | <a href=\"{}\">source</a> | MIT licence",
                escape(&settings.repo)
            )
        } else {
            format!(
                "<a href=\"{p}index.json\">index.json</a> | generated {}",
                escape(generated),
                p = escape(p)
            )
        },
        submit = escape(&submit),
        p = escape(p),
    );
    out
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The install command, as text.
fn install_command(name: &str) -> String {
    format!("hippocamp add {name} --registry <path to index.json>")
}

/// A pack's meta line: authors, version, kind, the evidence the registry
/// has, what its passes read, and the install command.
fn meta(pack: &IndexPack, listed: &Listed) -> String {
    let mut parts = Vec::new();
    let authors: Vec<String> = pack.authors.iter().map(|a| escape(a)).collect();
    parts.push(format!(
        "<span class=\"hp-user\">by {}</span>",
        authors.join(", ")
    ));
    parts.push(format!("v{}", escape(&pack.version)));
    let theme = listed.manifest.kind == Kind::Theme;
    parts.push(if theme { "theme pack" } else { "pass pack" }.to_owned());
    let mut evidence = format!("reviewed {}", listed.review.at.date());
    if !theme {
        let tests = listed.validation.cases.len();
        let _ = write!(
            evidence,
            ", {}",
            plural(tests, "fixture test", "fixture tests")
        );
    }
    parts.push(format!(
        "<span class=\"hp-evidence\">{}</span>",
        escape(&evidence)
    ));
    if theme {
        let mut kinds = Vec::new();
        if pack.theme_contract.is_some() {
            kinds.push("web");
        }
        if pack.desktop_theme_format.is_some() {
            kinds.push("desktop");
        }
        parts.push(format!("{} theme", kinds.join(" and ")));
    } else {
        let many = listed.validation.passes.len() > 1;
        for (name, pass) in &listed.validation.passes {
            let reads: Vec<&str> = pass.reads.iter().map(|r| r.as_str()).collect();
            let mut words = format!("reads {}; {}", reads.join(", "), filter_words(pass));
            if many {
                words = format!("{name}: {words}");
            }
            parts.push(escape(&words));
        }
    }
    parts.push(format!(
        "<span class=\"hp-install\">{}</span>",
        escape(&install_command(&pack.name))
    ));
    parts.join(" | ")
}

/// One row: rank, an empty vote column, the name linked to its page with
/// its source in parentheses, and the meta line.
fn row(rank: Option<usize>, pack: &IndexPack, listed: &Listed, prefix: &str) -> String {
    let kind = if listed.manifest.kind == Kind::Theme {
        "hp-item-theme"
    } else {
        "hp-item-pack"
    };
    let domain = pack
        .repo
        .strip_prefix("https://")
        .unwrap_or(pack.repo.as_str());
    format!(
        "<li class=\"hp-item {kind}\"><span class=\"hp-rank\">{rank}</span>\
         <span class=\"hp-vote\"></span>\
         <span class=\"hp-title\"><a class=\"hp-title-link\" href=\"{prefix}packs/{href}.html\">{name}</a> \
         <span class=\"hp-domain\">({domain})</span></span>\
         <div class=\"hp-meta\">{meta}</div></li>\n",
        rank = rank.map_or_else(String::new, |n| format!("{n}.")),
        prefix = escape(prefix),
        href = escape(&encode_segment(&pack.name)),
        name = escape(&pack.name),
        domain = escape(domain),
        meta = meta(pack, listed),
    )
}

const INTRO: &str = "A pack is text hippocamp copies into a workspace and never runs. \
Every listed version passed the checks and a fresh-context review shown on its page.";

fn list_page(
    frame: &Frame,
    settings: &Settings,
    generated: &str,
    heading: &str,
    counts: &str,
    rows: &[(&IndexPack, &Listed)],
    nothing: &str,
) -> String {
    let mut main = format!(
        "<h1 class=\"hp-sr\">{}</h1>\n<p class=\"hp-notice\">{}</p>\n<p class=\"hp-notice\">{}</p>\n",
        escape(heading),
        escape(INTRO),
        escape(counts),
    );
    if rows.is_empty() {
        let _ = writeln!(main, "<p class=\"hp-notice\">{}</p>", escape(nothing));
    } else {
        main.push_str("<ol class=\"hp-items\">\n");
        for (n, (pack, listed)) in rows.iter().enumerate() {
            main.push_str(&row(Some(n + 1), pack, listed, frame.prefix));
        }
        main.push_str("</ol>\n");
    }
    page(frame, settings, generated, &main)
}

/// A file's place in a pack page: README, licence, manifest, then the
/// directories in the order of the design.
fn file_order(path: &str) -> (usize, &str) {
    let rank = match path {
        "README.md" => 0,
        "LICENSE" | "LICENSE.md" => 1,
        "pack.toml" => 2,
        p if p.starts_with("passes/") => 3,
        p if p.starts_with("prompts/") => 4,
        p if p.starts_with("personas/") => 5,
        p if p.starts_with("templates/") => 6,
        "theme.css" => 7,
        "theme.json" => 8,
        p if p.starts_with("tests/") => 9,
        _ => 10,
    };
    (rank, path)
}

fn pack_page(settings: &Settings, generated: &str, pack: &IndexPack, listed: &Listed) -> String {
    let theme = listed.manifest.kind == Kind::Theme;
    let frame = Frame {
        title: &pack.name,
        body_class: "hp-page-registry-pack",
        current: if theme {
            Current::Themes
        } else {
            Current::Packs
        },
        prefix: "../",
    };
    let mut main = format!(
        "<h1 class=\"hp-sr\">{}</h1>\n<ol class=\"hp-items\">\n{}</ol>\n<article class=\"hp-post\">\n",
        escape(&pack.name),
        row(None, pack, listed, "../"),
    );

    let list = |items: &[String]| escape(&items.join(", "));
    let _ = write!(
        main,
        "<section class=\"hp-pack-box\">\n<h2>About</h2>\n<p>{description}</p>\n\
         <p>maintainers: <span class=\"hp-user\">{maintainers}</span></p>\n\
         <p>owner: <span class=\"hp-user\">{owner}</span>{first}</p>\n\
         <p>licence: {license}</p>\n</section>\n",
        description = escape(&pack.description),
        maintainers = list(&pack.maintainers),
        owner = escape(&pack.owner),
        first = if pack.first_party {
            " (first-party)"
        } else {
            ""
        },
        license = escape(&pack.license),
    );

    let shown = pack
        .repo
        .strip_prefix("https://")
        .unwrap_or(pack.repo.as_str());
    let _ = write!(
        main,
        "<section class=\"hp-pack-box\">\n<h2>Source</h2>\n\
         <p>repository: {repo}</p>\n<p>directory: {path}</p>\n<p>commit: {commit}</p>\n\
         <p><a href=\"{href}\">{repo}//{path}@{commit}</a></p>\n\
         <p>pack hash: {hash}</p>\n</section>\n",
        repo = escape(shown),
        path = escape(&pack.path),
        commit = escape(&pack.commit),
        href = escape(&tree_url(&pack.repo, &pack.commit, &pack.path)),
        hash = escape(&pack.pack_hash),
    );

    let _ = write!(
        main,
        "<section class=\"hp-install-box\">\n<h2>Install</h2>\n<pre class=\"hp-code\">{}</pre>\n</section>\n",
        escape(&install_command(&pack.name)),
    );

    if theme {
        main.push_str(
            "<section class=\"hp-pack-box\">\n<h2>Theme</h2>\n<ul class=\"hp-passes\">\n",
        );
        for line in theme_lines(&listed.files, &listed.manifest) {
            let _ = writeln!(main, "<li>{}</li>", escape(&line));
        }
        main.push_str("</ul>\n</section>\n");
    }
    for (name, pass) in &listed.validation.passes {
        let _ = write!(
            main,
            "<section class=\"hp-pack-box\">\n<h2>Pass {}</h2>\n<ul class=\"hp-passes\">\n",
            escape(name)
        );
        for input in &pass.reads {
            let _ = writeln!(
                main,
                "<li>{}: {}</li>",
                escape(input.as_str()),
                escape(input_words(input.as_str()))
            );
        }
        let calls = if pass.per_note {
            "one call per note"
        } else {
            "one call for all notes"
        };
        let _ = writeln!(
            main,
            "<li>{}</li>\n<li>{}</li>\n</ul>\n</section>",
            escape(calls),
            escape(&filter_words(pass))
        );
    }

    let review = &listed.review;
    let verdict = match review.verdict {
        hippocamp_core::records::Verdict::Confirm => "confirm",
        hippocamp_core::records::Verdict::Reject => "reject",
        hippocamp_core::records::Verdict::Revert => "revert",
    };
    let _ = write!(
        main,
        "<section class=\"hp-pack-box\">\n<h2>Review</h2>\n\
         <p>review {id}: {verdict}, {date}</p>\n<p>reviewer: {reviewer}</p>\n<p>{notes}</p>\n",
        id = escape(&review.id),
        verdict = escape(verdict),
        date = review.at.date(),
        reviewer = escape(&review.reviewer),
        notes = escape(&review.notes),
    );
    if !review.findings.is_empty() || !review.warnings.is_empty() {
        main.push_str("<ul class=\"hp-passes\">\n");
        for finding in &review.findings {
            let _ = writeln!(main, "<li>finding: {}</li>", escape(finding));
        }
        for warning in &review.warnings {
            let _ = writeln!(main, "<li>warning: {}</li>", escape(warning));
        }
        main.push_str("</ul>\n");
    }
    main.push_str("</section>\n");

    let mut files: Vec<(&str, &[u8])> = listed.files.iter().collect();
    files.sort_by(|a, b| file_order(a.0).cmp(&file_order(b.0)));
    for (path, bytes) in files {
        let _ = write!(
            main,
            "<section class=\"hp-pack-box\">\n<h2>{} ({} bytes)</h2>\n<pre class=\"hp-code\">{}</pre>\n</section>\n",
            escape(path),
            bytes.len(),
            escape(&String::from_utf8_lossy(bytes)),
        );
    }
    main.push_str("</article>\n");
    page(&frame, settings, generated, &main)
}

/// Every page of the site: its path below the output directory and its
/// HTML. `listed` holds each listed pack; `index` its index, whose packs
/// are in name order.
pub fn pages(
    settings: &Settings,
    index: &Index,
    listed: &[&Listed],
    landing: Option<&str>,
) -> Vec<(String, String)> {
    let generated = index
        .generated
        .format(&Rfc3339)
        .unwrap_or_else(|_| index.generated.to_string());
    let rows: Vec<(&IndexPack, &Listed)> = index
        .packs
        .iter()
        .filter_map(|pack| {
            listed
                .iter()
                .find(|l| l.entry.name == pack.name)
                .map(|l| (pack, *l))
        })
        .collect();
    let themes: Vec<(&IndexPack, &Listed)> = rows
        .iter()
        .filter(|(_, l)| l.manifest.kind == Kind::Theme)
        .copied()
        .collect();

    let counts = format!(
        "{}: {} first-party, {} by other owners",
        plural(rows.len(), "pack", "packs"),
        index.counts.first_party,
        index.counts.other_owners
    );
    let mut out = vec![(
        "packs.html".to_owned(),
        list_page(
            &Frame {
                title: "Packs",
                body_class: "hp-page-registry-index",
                current: Current::Packs,
                prefix: "",
            },
            settings,
            &generated,
            "Packs",
            &counts,
            &rows,
            "Nothing is listed yet.",
        ),
    )];
    out.push((
        "themes.html".to_owned(),
        list_page(
            &Frame {
                title: "Themes",
                body_class: "hp-page-registry-themes",
                current: Current::Themes,
                prefix: "",
            },
            settings,
            &generated,
            "Themes",
            &plural(themes.len(), "theme pack", "theme packs"),
            &themes,
            "No theme is listed yet.",
        ),
    ));
    if let Some(main) = landing {
        out.push((
            "index.html".to_owned(),
            page(
                &Frame {
                    title: "One brain for all your coding agents",
                    body_class: "hp-page-about",
                    current: Current::Home,
                    prefix: "",
                },
                settings,
                &generated,
                main,
            ),
        ));
    }
    for (pack, listed) in rows {
        out.push((
            format!("packs/{}.html", encode_segment(&pack.name)),
            pack_page(settings, &generated, pack, listed),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_covers_the_five_characters_and_nothing_else() {
        assert_eq!(
            escape("<script>alert(\"x\" & 'y')</script>"),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#39;y&#39;)&lt;/script&gt;"
        );
        assert_eq!(escape("plain text, é, ü"), "plain text, é, ü");
        assert_eq!(escape("&amp;"), "&amp;amp;");
    }

    #[test]
    fn segments_are_percent_encoded_byte_by_byte() {
        assert_eq!(
            encode_segment("glossary-terms_1.0~x"),
            "glossary-terms_1.0~x"
        );
        assert_eq!(encode_segment("a b"), "a%20b");
        assert_eq!(encode_segment("a/b?c#d%"), "a%2Fb%3Fc%23d%25");
        assert_eq!(encode_segment("é"), "%C3%A9");
        assert_eq!(encode_segment("\"<>"), "%22%3C%3E");
    }

    #[test]
    fn a_tree_link_has_each_path_segment_encoded() {
        assert_eq!(
            tree_url(
                "https://github.com/alice/hippocamp-packs",
                "3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e",
                "packs/My Pack"
            ),
            "https://github.com/alice/hippocamp-packs/tree/3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e/packs/My%20Pack"
        );
        assert_eq!(
            tree_url("https://github.com/a/b", "abc", ""),
            "https://github.com/a/b/tree/abc"
        );
        // A segment cannot add a query, a fragment or another directory.
        let url = tree_url("https://github.com/a/b", "abc", "x?y#z/..%2f");
        assert_eq!(url, "https://github.com/a/b/tree/abc/x%3Fy%23z/..%252f");
    }

    #[test]
    fn files_are_shown_in_the_designs_order() {
        let mut paths = vec![
            "tests/a/case.toml",
            "theme.json",
            "templates/t.md",
            "personas/p.md",
            "LICENSE",
            "passes/p.toml",
            "pack.toml",
            "theme.css",
            "prompts/p.md",
            "README.md",
        ];
        paths.sort_by_key(|p| file_order(p));
        assert_eq!(
            paths,
            [
                "README.md",
                "LICENSE",
                "pack.toml",
                "passes/p.toml",
                "prompts/p.md",
                "personas/p.md",
                "templates/t.md",
                "theme.css",
                "theme.json",
                "tests/a/case.toml",
            ]
        );
    }

    #[test]
    fn counts_agree_in_number() {
        assert_eq!(plural(1, "pack", "packs"), "1 pack");
        assert_eq!(plural(0, "pack", "packs"), "0 packs");
        assert_eq!(plural(26, "pack", "packs"), "26 packs");
    }
}
