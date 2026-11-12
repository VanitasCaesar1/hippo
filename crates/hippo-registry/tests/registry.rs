//! `hippo-registry` through its binary, on a temporary git repository that
//! holds a registry and a first-party pack. No test touches the network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hippo_core::packs::files::{pack_hash, read_dir};
use hippo_core::packs::registry::Settings;
use hippo_core::packs::review::{persona_hash, PackReview};
use hippo_core::records::Verdict;
use time::OffsetDateTime;

const PACK: &str = "gotchas";

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for item in fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let target = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_dir(&item.path(), &target);
        } else {
            fs::copy(item.path(), target).unwrap();
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn settings_text() -> String {
    format!(
        "protocol = \"HIPPO_V1\"\nrepo = \"https://github.com/VanitasCaesar1/hippo\"\n\
         first_party = [\"VanitasCaesar1\"]\nlicenses = [\"MIT\"]\n\
         personas = [\"{}\"]\nconfusable_ok = []\n",
        persona_hash()
    )
}

fn entry_text(name: &str, pack: &str, commit: &str) -> String {
    format!(
        "protocol = \"HIPPO_V1\"\nname = \"{name}\"\n\
         repo = \"https://github.com/VanitasCaesar1/hippo\"\nrepo_id = 1\nowner_id = 2\n\
         path = \"packs/{pack}\"\ncommit = \"{commit}\"\nmaintainers = [\"VanitasCaesar1\"]\n"
    )
}

/// A repository on `main` that holds the registry settings and `PACK`, with
/// `refs/remotes/origin/main` at its head. Returns its path and the commit
/// the pack is pinned at.
struct Repo {
    dir: tempfile::TempDir,
    commit: String,
}

impl Repo {
    fn new() -> Repo {
        Repo::with_packs(&[])
    }

    /// As `new`, with these first-party packs besides `PACK`.
    fn with_packs(more: &[&str]) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "--quiet", "-b", "main"]);
        fs::create_dir_all(root.join("registry/packs")).unwrap();
        fs::create_dir_all(root.join("registry/reviews")).unwrap();
        fs::write(root.join("registry/registry.toml"), settings_text()).unwrap();
        fs::write(root.join("registry/packs/.gitkeep"), "").unwrap();
        fs::write(root.join("registry/reviews/.gitkeep"), "").unwrap();
        fs::create_dir_all(root.join("registry/site")).unwrap();
        fs::copy(
            source_root().join("registry/site/registry.css"),
            root.join("registry/site/registry.css"),
        )
        .unwrap();
        for pack in std::iter::once(&PACK).chain(more) {
            copy_dir(
                &source_root().join("packs").join(pack),
                &root.join("packs").join(pack),
            );
        }
        let repo = Repo {
            commit: String::new(),
            dir,
        };
        repo.commit_all("packs");
        let commit = git(repo.root(), &["rev-parse", "HEAD"]);
        Repo { commit, ..repo }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn commit_all(&self, message: &str) {
        git(self.root(), &["add", "-A"]);
        git(self.root(), &["commit", "--quiet", "-m", message]);
        git(
            self.root(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
    }

    /// Replaces `from` with `to` in a file of `packs/<pack>/`, then commits
    /// and moves the pin to the new commit.
    fn edit_pack(&mut self, pack: &str, file: &str, from: &str, to: &str) {
        let path = self.root().join("packs").join(pack).join(file);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{from}");
        fs::write(&path, text.replace(from, to)).unwrap();
        self.commit_all("edit");
        self.commit = git(self.root(), &["rev-parse", "HEAD"]);
    }

    fn add_entry(&self, name: &str, pack: &str) {
        fs::write(
            self.root().join(format!("registry/packs/{name}.toml")),
            entry_text(name, pack, &self.commit),
        )
        .unwrap();
    }

    /// A review of `name` at the pack's content, written as the tool would.
    fn add_review(&self, id: &str, name: &str, verdict: Verdict) {
        let (files, _) = read_dir(&self.root().join("packs").join(name)).unwrap();
        let review = PackReview {
            id: id.to_owned(),
            pack: name.to_owned(),
            version: "1.0.0".to_owned(),
            source: "s".to_owned(),
            commit: Some(self.commit.clone()),
            repo_id: Some(1),
            pack_hash: pack_hash(&files),
            reviewer: "pack-reviewer".to_owned(),
            persona_hash: persona_hash(),
            invocation: "test".to_owned(),
            warnings: vec![],
            verdict,
            notes: "n".to_owned(),
            findings: vec![],
            at: OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap(),
        };
        review
            .save_new(&self.root().join(format!("registry/reviews/{id}.json")))
            .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hippo-registry"))
            .args(args)
            .arg("--repo-root")
            .arg(self.root())
            .output()
            .unwrap()
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn build_writes_the_index_of_a_listed_pack() {
    let repo = Repo::new();
    repo.add_entry(PACK, PACK);
    repo.add_review("01JCA5ZB3K9V7Q2M4N6P8R0T1W", PACK, Verdict::Confirm);
    repo.commit_all("entry");

    let out = repo.root().join("out");
    let output = repo.run(&["build", "--offline", "--out", out.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(text(&output).contains("1 listed: 1 first-party, 0 by other owners"));
    assert!(!text(&output).contains("unlisted"));

    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
    assert_eq!(index["protocol"], "HIPPO_V1");
    assert_eq!(index["generated"], "2026-11-06T21:20:00Z");
    assert_eq!(
        index["counts"],
        serde_json::json!({"listed": 1, "first_party": 1, "other_owners": 0})
    );
    let pack = &index["packs"][0];
    assert_eq!(pack["name"], PACK);
    assert_eq!(pack["owner"], "VanitasCaesar1");
    assert_eq!(pack["first_party"], true);
    assert_eq!(pack["kind"], "passes");
    assert_eq!(pack["commit"], repo.commit.as_str());
    assert_eq!(pack["review"]["id"], "01JCA5ZB3K9V7Q2M4N6P8R0T1W");
    assert_eq!(pack["review"]["verdict"], "confirm");
    assert_eq!(pack["passes"][0]["name"], PACK);
    let on_disk: Vec<String> = {
        let (files, _) = read_dir(&repo.root().join("packs").join(PACK)).unwrap();
        files.paths().map(str::to_owned).collect()
    };
    let listed: Vec<String> = pack["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(listed, on_disk);
    assert_eq!(pack["files"][0]["sha256"].as_str().unwrap().len(), 64);

    // The output depends only on the inputs.
    let again = repo.root().join("again");
    let output = repo.run(&["build", "--offline", "--out", again.to_str().unwrap()]);
    assert!(output.status.success());
    assert_eq!(
        fs::read(out.join("index.json")).unwrap(),
        fs::read(again.join("index.json")).unwrap()
    );

    let check = repo.run(&["check", "--offline"]);
    assert!(check.status.success(), "{check:?}");
    assert!(text(&check).contains(&format!("ok {PACK}")));
    assert!(text(&check).ends_with("1 listed: 1 first-party, 0 by other owners\n"));
}

#[test]
fn an_entry_without_a_review_is_unlisted_and_fails_check() {
    let repo = Repo::new();
    repo.add_entry(PACK, PACK);
    repo.commit_all("entry");

    let out = repo.root().join("out");
    let output = repo.run(&["build", "--offline", "--out", out.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output).contains(&format!("unlisted {PACK}: ")),
        "{}",
        text(&output)
    );
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
    assert_eq!(index["counts"]["listed"], 0);
    assert!(index["packs"].as_array().unwrap().is_empty());

    let check = repo.run(&["check", "--offline"]);
    assert_eq!(check.status.code(), Some(1), "{check:?}");
    assert!(text(&check).contains(&format!("error {PACK}: ")));
    assert!(text(&check).contains("0 listed: 0 first-party, 0 by other owners"));
}

#[test]
fn a_confusable_name_fails_check() {
    let repo = Repo::new();
    repo.add_entry(PACK, PACK);
    repo.add_review("01JCA5ZB3K9V7Q2M4N6P8R0T1W", PACK, Verdict::Confirm);
    repo.add_entry("g0tchas", PACK);
    repo.commit_all("entries");

    let check = repo.run(&["check", "--offline"]);
    assert_eq!(check.status.code(), Some(1), "{check:?}");
    let stdout = text(&check);
    assert!(
        stdout.contains("error gotchas: the name is confusable with g0tchas"),
        "{stdout}"
    );
    assert!(
        stdout.contains("error g0tchas: the name is confusable with gotchas"),
        "{stdout}"
    );
}

#[test]
fn an_empty_registry_passes() {
    let repo = Repo::new();
    let check = repo.run(&["check", "--offline"]);
    assert!(check.status.success(), "{check:?}");
    assert_eq!(text(&check), "0 listed: 0 first-party, 0 by other owners\n");
}

fn stub_command(verdict: &str) -> String {
    let script = source_root().join("fixtures/stub-model/model.sh");
    serde_json::json!([
        "env",
        "HIPPO_STUB_REVIEW=1",
        format!("HIPPO_STUB_VERDICT={verdict}"),
        script.to_string_lossy()
    ])
    .to_string()
}

fn reviews(repo: &Repo) -> Vec<PackReview> {
    let mut found = Vec::new();
    for item in fs::read_dir(repo.root().join("registry/reviews")).unwrap() {
        let path = item.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            found.push(PackReview::load(&path).unwrap());
        }
    }
    found
}

#[test]
fn review_saves_a_confirming_review_and_exits_0() {
    let repo = Repo::new();
    repo.add_entry(PACK, PACK);
    repo.commit_all("entry");
    let entry = repo.root().join(format!("registry/packs/{PACK}.toml"));
    let output = repo.run(&[
        "review",
        "--entry",
        entry.to_str().unwrap(),
        "--model-command",
        &stub_command("confirm"),
    ]);
    assert!(output.status.success(), "{output:?}");
    let saved = reviews(&repo);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].verdict, Verdict::Confirm);
    assert_eq!(saved[0].pack, PACK);
    assert_eq!(saved[0].repo_id, Some(1));
    assert_eq!(saved[0].persona_hash, persona_hash());
    assert!(text(&output).contains(&format!("{}.json", saved[0].id)));

    // The saved review lists the pack.
    let check = repo.run(&["check", "--offline"]);
    assert!(check.status.success(), "{check:?}");
}

#[test]
fn review_by_name_writes_a_reject_and_exits_1() {
    let repo = Repo::new();
    repo.add_entry(PACK, PACK);
    repo.commit_all("entry");
    let output = repo.run(&["review", PACK, "--model-command", &stub_command("reject")]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let saved = reviews(&repo);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].verdict, Verdict::Reject);
}

#[test]
fn the_repositorys_registry_lists_the_current_persona() {
    let text = fs::read_to_string(source_root().join("registry/registry.toml")).unwrap();
    let settings = Settings::parse(&text).unwrap();
    assert!(settings.personas.contains(&persona_hash()), "{settings:?}");
}

// ---- the site, and theme packs ----

const SCRIPT: &str = "<script>alert(1)</script>";
const GOTCHAS_DESCRIPTION: &str = "Appends surprising behaviour that cost a session time and is not an error to a gotchas or pitfalls note.";

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Every page below `out`, by its path relative to `out`.
fn pages(out: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for dir in [out.to_path_buf(), out.join("packs")] {
        for item in fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            if path.extension().is_some_and(|e| e == "html") {
                let relative = path
                    .strip_prefix(out)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                found.push((relative, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

/// A registry that lists `gotchas` (with a script in its description) and
/// the theme pack `paper`, built into `out`.
fn site_repo() -> (Repo, PathBuf) {
    let mut repo = Repo::with_packs(&["paper"]);
    repo.edit_pack(PACK, "pack.toml", GOTCHAS_DESCRIPTION, SCRIPT);
    for name in [PACK, "paper"] {
        repo.add_entry(name, name);
    }
    repo.add_review("01JCA5ZB3K9V7Q2M4N6P8R0T1W", PACK, Verdict::Confirm);
    repo.add_review("01JCA5ZB3K9V7Q2M4N6P8R0T1X", "paper", Verdict::Confirm);
    repo.commit_all("entries");
    let out = repo.root().join("out");
    let landing = source_root().join("docs/site/landing.html");
    let output = repo.run(&[
        "build",
        "--offline",
        "--out",
        out.to_str().unwrap(),
        "--landing",
        landing.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output).contains("2 listed: 2 first-party, 0 by other owners"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("unlisted"), "{}", text(&output));
    (repo, out)
}

#[test]
fn build_writes_the_site_and_a_pack_page_holds_every_file() {
    let (repo, out) = site_repo();
    for file in [
        "index.html",
        "packs.html",
        "themes.html",
        "packs/gotchas.html",
        "packs/paper.html",
        "hippo.css",
        "registry.css",
        "icon.svg",
        "index.json",
    ] {
        assert!(out.join(file).is_file(), "{file}");
    }
    assert_eq!(
        fs::read_to_string(out.join("hippo.css")).unwrap(),
        format!("{}{}", hippo_theme::BASE_CSS, hippo_theme::AUTO_CSS)
    );
    assert_eq!(
        fs::read_to_string(out.join("registry.css")).unwrap(),
        fs::read_to_string(source_root().join("registry/site/registry.css")).unwrap()
    );

    let index = fs::read_to_string(out.join("packs.html")).unwrap();
    assert!(index.starts_with("<!doctype html>\n<html lang=\"en\">"));
    assert!(index.contains("<body class=\"hp-page hp-page-registry-index\">"));
    assert!(index.contains("<title>Packs | hippo</title>"));
    assert!(
        index.contains("2 packs: 2 first-party, 0 by other owners"),
        "{index}"
    );
    assert!(index.contains("<svg"), "the mark is inlined");
    assert!(index.contains("class=\"hp-mark\""));
    assert!(index.contains("generated 2026-11-06T21:20:00Z"));
    assert!(index.contains("href=\"packs/gotchas.html\""));
    assert!(index.contains("href=\"packs/paper.html\""));
    assert!(index.contains("(github.com/VanitasCaesar1/hippo)"));
    assert!(index.contains("hippo add gotchas --registry &lt;path to index.json&gt;"));
    assert!(index.contains("fixture test"));
    assert!(index.contains(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src &#39;none&#39;; style-src &#39;self&#39;; img-src &#39;self&#39; data:\">"
    ));
    // The submit link is the registry README of the settings' repository.
    assert!(index
        .contains("href=\"https://github.com/VanitasCaesar1/hippo/blob/main/registry/README.md\""));

    // The pack page shows every file in full, as text, with a link to the
    // pinned directory on GitHub.
    let page = fs::read_to_string(out.join("packs/gotchas.html")).unwrap();
    assert!(page.contains("<body class=\"hp-page hp-page-registry-pack\">"));
    assert!(page.contains("href=\"../packs.html\""));
    assert!(
        page.contains("href=\"../index.html\""),
        "the home link is the landing page"
    );
    assert!(page.contains("href=\"../docs/index.html\""));
    assert!(page.contains(&format!(
        "href=\"https://github.com/VanitasCaesar1/hippo/tree/{}/packs/gotchas\"",
        repo.commit
    )));
    let (files, _) = read_dir(&repo.root().join("packs").join(PACK)).unwrap();
    let mut shown = 0;
    for (path, bytes) in files.iter() {
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            page.contains(&html_escape(&body)),
            "{path} is not in the pack page"
        );
        assert!(page.contains(&format!("<h2>{path} ({} bytes)</h2>", bytes.len())));
        shown += 1;
    }
    assert!(shown > 5);
    // README first, then the licence, the manifest and the passes.
    let at = |needle: &str| page.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("<h2>README.md (") < at("<h2>LICENSE ("));
    assert!(at("<h2>LICENSE (") < at("<h2>pack.toml ("));
    assert!(at("<h2>pack.toml (") < at("<h2>passes/gotchas.toml ("));
    assert!(at("<h2>passes/gotchas.toml (") < at("<h2>tests/"));
    // What the pass reads, one line per input, and the review.
    assert!(page.contains("sessions: session records with their task, notes touched and handoff"));
    assert!(page.contains("turns: transcript turns that mention the note, already redacted"));
    assert!(page.contains("review 01JCA5ZB3K9V7Q2M4N6P8R0T1W: confirm, 2026-11-06"));
}

/// The landing page: the fragment in the page
/// frame, its links relative, and no `index.html` without `--landing`.
#[test]
fn the_landing_page_is_the_index_and_links_inside_the_site() {
    let (repo, out) = site_repo();
    let landing = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(landing.contains("<title>One brain for all your coding agents | hippo</title>"));
    assert!(landing.contains("One brain for all your coding agents.</b>"));
    assert!(landing.contains("src=\"assets/card.png\""));
    for href in [
        "packs.html",
        "docs/index.html",
        "docs/install.html",
        "https://github.com/VanitasCaesar1/hippo",
    ] {
        assert!(landing.contains(&format!("href=\"{href}\"")), "{href}");
    }
    assert!(landing.contains("aria-current=\"page\""));

    let bare = repo.root().join("bare");
    let output = repo.run(&["build", "--offline", "--out", bare.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(bare.join("packs.html").is_file());
    assert!(!bare.join("index.html").exists());
}

#[test]
fn no_page_runs_script_or_reaches_another_host() {
    let (_repo, out) = site_repo();
    let all = pages(&out);
    assert_eq!(all.len(), 5);
    for (path, html) in &all {
        let lower = html.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "{path}");
        assert!(!lower.contains("javascript:"), "{path}");
        assert!(
            !lower.contains(" onload=") && !lower.contains(" onclick="),
            "{path}"
        );
        assert!(
            !lower.contains("<style") && !lower.contains(" style=\""),
            "{path}"
        );
        assert!(
            lower.contains("http-equiv=\"content-security-policy\""),
            "{path}"
        );
        let mut links = 0;
        for attribute in ["href=\"", "src=\""] {
            for part in html.split(attribute).skip(1) {
                let value = part.split('"').next().unwrap();
                links += 1;
                let relative = !value.contains(':') && !value.starts_with("//");
                assert!(
                    relative || value.starts_with("https://github.com/"),
                    "{path}: {value}"
                );
            }
        }
        assert!(links > 5, "{path}: {links}");
    }
}

#[test]
fn a_description_with_a_script_renders_escaped() {
    let (_repo, out) = site_repo();
    let page = fs::read_to_string(out.join("packs/gotchas.html")).unwrap();
    assert!(!page.contains(SCRIPT), "the script is raw in the page");
    assert!(!page.contains("<script>"));
    assert!(
        page.contains("<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"),
        "{page}"
    );
    // The manifest shown as a file is escaped too.
    assert!(page.contains("description = &quot;&lt;script&gt;alert(1)&lt;/script&gt;&quot;"));
    // The index keeps the text as data.
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
    assert!(index["packs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["description"] == SCRIPT));
}

#[test]
fn a_theme_pack_is_listed_on_the_themes_page_with_its_files_and_versions() {
    let (repo, out) = site_repo();
    let themes = fs::read_to_string(out.join("themes.html")).unwrap();
    assert!(themes.contains("<body class=\"hp-page hp-page-registry-themes\">"));
    assert!(themes.contains("1 theme pack"));
    assert!(themes.contains("href=\"packs/paper.html\""));
    assert!(themes.contains("hp-item-theme"));
    assert!(!themes.contains("packs/gotchas.html"));
    let index = fs::read_to_string(out.join("packs.html")).unwrap();
    assert!(index.contains("href=\"themes.html\""));
    assert!(index.contains("href=\"packs/paper.html\""));
    assert!(index.contains("theme pack"));

    let page = fs::read_to_string(out.join("packs/paper.html")).unwrap();
    let css = fs::read(repo.root().join("packs/paper/theme.css")).unwrap();
    let json = fs::read(repo.root().join("packs/paper/theme.json")).unwrap();
    assert!(page.contains(&format!(
        "<li>theme web: theme.css, contract 1, {} bytes</li>",
        css.len()
    )));
    assert!(page.contains(&format!(
        "<li>theme desktop: theme.json, format 1, {} bytes</li>",
        json.len()
    )));
    assert!(page.contains("<h2>theme.css ("));
    assert!(page.contains("<h2>theme.json ("));
    assert!(!page.contains("<h2>Pass "));
    assert!(page.contains("web and desktop theme"));

    // The index carries the theme fields.
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
    let paper = index["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "paper")
        .unwrap();
    assert_eq!(paper["kind"], "theme");
    assert_eq!(paper["theme_contract"], 1);
    assert_eq!(paper["desktop_theme_format"], 1);
    assert_eq!(paper["passes"], serde_json::json!([]));
    assert!(paper["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "theme.css" && f["bytes"] == css.len()));

    // A theme pack needs no test case.
    let check = repo.run(&["check", "--offline"]);
    assert!(check.status.success(), "{check:?}");
    assert!(text(&check).contains("ok paper"), "{}", text(&check));
}

#[test]
fn a_theme_pack_needs_a_licence_and_obeys_the_theme_rules() {
    let mut repo = Repo::with_packs(&["paper"]);
    repo.edit_pack(
        "paper",
        "theme.css",
        "--hp-text: #2b2620;",
        "--hp-text: #f4eddc;",
    );
    fs::remove_file(repo.root().join("packs/paper/LICENSE")).unwrap();
    repo.commit_all("no licence");
    repo.commit = git(repo.root(), &["rev-parse", "HEAD"]);
    repo.add_entry("paper", "paper");
    repo.commit_all("entry");
    let check = repo.run(&["check", "--offline"]);
    assert_eq!(check.status.code(), Some(1), "{check:?}");
    let stdout = text(&check);
    assert!(
        stdout.contains("error paper: LICENSE: missing (LICENSE or LICENSE.md)"),
        "{stdout}"
    );
    assert!(stdout.contains("error paper: theme.css:"), "{stdout}");
    assert!(!stdout.contains("fixture test"), "{stdout}");
}

#[test]
fn the_pages_use_only_contract_classes_and_properties() {
    let (_repo, out) = site_repo();
    for (path, html) in pages(&out) {
        let body_pages: Vec<String> = hippo_theme::PAGES
            .iter()
            .map(|name| format!("hp-page-{name}"))
            .collect();
        let mut used = 0;
        for part in html.split("class=\"").skip(1) {
            for class in part.split('"').next().unwrap().split_whitespace() {
                used += 1;
                assert!(
                    hippo_theme::CLASSES.contains(&class) || body_pages.iter().any(|p| p == class),
                    "{path}: {class} is not a class of the contract"
                );
            }
        }
        // The landing page is mostly prose: its frame alone has 15 or more.
        let least = if path == "index.html" { 15 } else { 20 };
        assert!(used > least, "{path}: {used}");
        // The body class names the page.
        if path == "index.html" {
            assert!(
                html.contains("<body class=\"hp-page hp-page-about\">"),
                "{path}"
            );
        } else {
            assert!(
                html.contains("hp-page-registry-"),
                "{path} has no registry page class"
            );
        }
    }

    // registry.css: the contract's custom properties only, and no colour.
    let css = fs::read_to_string(source_root().join("registry/site/registry.css")).unwrap();
    let mut properties = 0;
    for part in css.split("--").skip(1) {
        let name: String = part
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let property = format!("--{name}");
        properties += 1;
        assert!(
            hippo_theme::PROPERTIES.contains(&property.as_str()),
            "{property} is not a property of the contract"
        );
    }
    assert!(properties > 3);
    assert!(!css.contains("url("));
    assert!(!css.contains("@import"));
    assert!(!css.contains("!important"));
    for (index, _) in css.match_indices('#') {
        let digits = css[index + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        assert!(digits < 3, "a colour literal in registry.css at {index}");
    }
    for forbidden in ["font-family: ", "rgb(", "hsl("] {
        for line in css.lines().filter(|l| l.contains(forbidden)) {
            assert!(
                line.contains("var(--hp-"),
                "registry.css sets its own value: {line}"
            );
        }
    }
}
