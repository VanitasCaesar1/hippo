//! `/about`: the rules, in plain language, from the same constants the code
//! enforces.

use maud::{html, Markup};

use crate::policy;
use crate::web::Ctx;

fn minutes(secs: u64) -> String {
    // `is_multiple_of` needs Rust 1.90; the workspace's minimum is 1.89.
    if secs / 60 * 60 == secs && secs >= 120 {
        format!("{} minutes", secs / 60)
    } else if secs == 60 {
        "minute".to_string()
    } else {
        format!("{secs} seconds")
    }
}

fn per(limit: policy::Limit) -> String {
    let secs = limit.window_secs;
    let window = if secs % 86_400 == 0 {
        "24 hours".to_string()
    } else if secs % 3_600 == 0 {
        if secs == 3_600 {
            "hour".to_string()
        } else {
            format!("{} hours", secs / 3_600)
        }
    } else {
        format!("{secs} seconds")
    };
    format!("{} per {}", limit.count, window)
}

pub async fn about(ctx: Ctx) -> axum::response::Response {
    use axum::response::IntoResponse;
    let contact = ctx.state.config.contact.clone();
    let measuring_open = !ctx.state.config.eval_sets.is_empty();
    ctx.page("about", "About")
        .body(body(contact.as_deref(), measuring_open))
        .into_response()
}

fn body(contact: Option<&str>, measuring_open: bool) -> Markup {
    html! {
        section {
            h2 { "What this is" }
            p {
                "hippocamp feed is a list of packs and discussions about hippocamp, a memory engine for AI "
                "coding agents. Packs that measurably help rank higher. Every pack the registry lists "
                "gets one post. The pages use no JavaScript."
            }
        }
        section {
            h2 { "Rules" }
            ul {
                li { "Sign in with GitHub. The feed keeps no GitHub token." }
                li {
                    "A new account (under " (policy::ESTABLISHED_FEED_AGE_DAYS) " days old, or a GitHub account under "
                    (policy::ESTABLISHED_GITHUB_AGE_DAYS) " days old) may post "
                    (per(policy::POSTS_NEW)) " discussion, may not post links, and may not submit measurements. "
                    "Its votes are stored and count once it is established."
                }
                li { "Be civil. Spam, abuse and malicious packs are removed, and every moderator action is on the public moderation log." }
                li { "Three reports from established members hide an item until a moderator looks at it." }
                li { "Authors may edit an item for " (policy::EDIT_WINDOW_HOURS) " hours after posting it." }
            }
        }
        section {
            h2 { "Limits" }
            table class="hp-table" {
                thead { tr { th { "Action" } th { "Established" } th { "New" } } }
                tbody {
                    tr { td { "Discussion posts" } td { (per(policy::POSTS_ESTABLISHED)) } td { (per(policy::POSTS_NEW)) } }
                    tr {
                        td { "Comments" }
                        td { (per(policy::COMMENTS_ESTABLISHED_HOUR)) " and " (per(policy::COMMENTS_ESTABLISHED_DAY)) }
                        td { (per(policy::COMMENTS_NEW)) }
                    }
                    tr { td { "Votes" } td { (per(policy::VOTES)) } td { (per(policy::VOTES)) } }
                    tr { td { "Reports" } td { (per(policy::REPORTS_ESTABLISHED)) } td { (per(policy::REPORTS_NEW)) } }
                    tr { td { "Measurements" } td { (per(policy::MEASUREMENTS_ESTABLISHED)) } td { "not allowed" } }
                }
            }
            p {
                "Each address may make " (policy::ALL_QUOTA.count) " requests per "
                (minutes(policy::ALL_QUOTA.per.as_secs())) ", " (policy::API_QUOTA.count)
                " of them to the API, and start sign-in " (policy::SIGN_IN_QUOTA.count) " times per "
                (minutes(policy::SIGN_IN_QUOTA.per.as_secs())) "."
            }
        }
        section {
            h2 { "Ranking" }
            p {
                "The front page lists posts from the last " (policy::FRONT_WINDOW_DAYS) " days by "
                code { "(V + " (policy::RANK_EVIDENCE_WEIGHT) " x E) / (h + " (policy::RANK_OFFSET_HOURS) ")^" (policy::RANK_GRAVITY) }
                ", where V is the post's counted votes, E is the pack's evidence (0 for a discussion or "
                "a pack without evidence) and h is its age in hours. The gravity and the offset follow "
                "Hacker News's published ranking function. Lists show " (policy::PAGE_SIZE) " entries a page, up to page "
                (policy::PAGE_MAX) "."
            }
        }
        section {
            h2 { "Evidence" }
            p {
                "A measurement is one member's entry of the counts from eval's dreaming suite, run at least "
                (policy::MEASURE_RUNS_MIN) " times without a pack and as many times with it, on one eval set. "
                "A pack's evidence is the median benefit of at least " (policy::EVIDENCE_MIN_MEASUREMENTS)
                " counted measurements, shrunk toward zero when there are few: E = median x n / (n + "
                (policy::EVIDENCE_SHRINK) "). Measurements by a pack's own maintainers are shown and never counted."
            }
            p {
                "Evidence is noisy. An eval set has only a handful of planted errors, so a single run moves the "
                "rate in large steps, and members measure with their own models. Read it as a hint, not a verdict."
            }
            @if !measuring_open {
                p { "Measuring is not open yet, so every pack ranks on votes alone." }
            }
        }
        section {
            h2 { "Licence policy" }
            p {
                "The feed lists a pack only if its licence is one of: "
                (policy::LICENCES.join(", "))
                ". Copy and own means users edit their copy and commit it to their own repositories, which "
                "permissive and public-domain terms allow."
            }
        }
        section {
            h2 { "Content licence" }
            p {
                "Posts and comments are the members' own text, published under "
                a href="https://creativecommons.org/licenses/by/4.0/" rel="nofollow noopener noreferrer" { "CC BY 4.0" } "."
            }
        }
        section {
            h2 { "Privacy" }
            table class="hp-table" {
                thead { tr { th { "Data" } th { "Why" } th { "Kept until" } } }
                tbody {
                    tr { td { "GitHub numeric id" } td { "identity across renames" } td { "account deletion; for a banned or restricted account, until the sanction ends" } }
                    tr { td { "GitHub login" } td { "display; matching pack maintainers" } td { "account deletion" } }
                    tr { td { "GitHub account creation date" } td { "standing" } td { "account deletion" } }
                    tr { td { "Session token hash, CSRF token, times" } td { "being signed in" } td { "sign-out or expiry (" (policy::SESSION_DAYS) " days)" } }
                    tr { td { "Sign-in state and PKCE verifier" } td { "signing in" } td { "use, or " (policy::OAUTH_STATE_MINUTES) " minutes" } }
                    tr { td { "Posts and comments" } td { "the content" } td { "the author's deletion or account deletion" } }
                    tr { td { "Votes" } td { "counting" } td { "withdrawal or account deletion" } }
                    tr { td { "Hidden posts" } td { "the member's own lists" } td { "un-hiding or account deletion" } }
                    tr { td { "Theme choice and personal CSS" } td { "the member's own pages" } td { "the member's change, or account deletion" } }
                    tr { td { "Measurements" } td { "evidence" } td { "replacement or account deletion" } }
                    tr { td { "Reports filed" } td { "moderation" } td { "the note and snapshot " (policy::REPORT_TEXT_DAYS) " days after resolution" } }
                    tr { td { "Audit log" } td { "accountability" } td { "kept; ids, reason codes and counts only" } }
                    tr { td { "Client IP addresses" } td { "rate limits" } td { "never written; held in memory only" } }
                }
            }
            p {
                "Not collected: email addresses, names, avatars, OAuth tokens, analytics. Pages request nothing "
                "from other origins. The only cookies are the session and the sign-in state. Members can export "
                "and delete their data from settings."
            }
        }
        section {
            h2 { "Backups" }
            p {
                "The database is backed up nightly and copies are deleted after " (policy::BACKUP_KEEP_DAYS)
                " days, so deleted data leaves every copy within " (policy::BACKUP_KEEP_DAYS) " days."
            }
        }
        section {
            h2 { "Contact" }
            p {
                @if let Some(contact) = contact { (contact) }
                @else { "No contact address is configured on this server." }
            }
        }
    }
}
