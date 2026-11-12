//! Ask: has any agent already solved this?
//! The sources come from `hippo_core::ask` in the app, at once; the
//! written answer from the installed CLI (`hippo ask <question> --json`), so
//! the one model call never runs inside the app.

use gpui::{
    Context, FocusHandle, Focusable, FontWeight, HighlightStyle, Role, StyledText, Window, div,
    prelude::*,
};
use hippo_core::ask::{Answer, Asked, Source};

use super::Shell;
use super::kit::{self, Kind};
use super::theme::Look;
use crate::cli;
use crate::state::{Section, Tab};

/// How long the written answer may take.
const ANSWER_LIMIT: std::time::Duration = std::time::Duration::from_secs(180);

/// The written answer as the CLI returned it: the answer, or why it was
/// dropped.
pub type Written = (Option<Answer>, Option<String>);

/// The Ask view's state.
pub struct AskState {
    pub question: String,
    /// The sources, or why they could not be read.
    pub result: Option<Result<Asked, String>>,
    pub searching: bool,
    /// The written answer: what the CLI returned, or why it failed.
    pub answer: Option<Result<Written, String>>,
    pub answering: bool,
    /// The person asked for sources only (⌘↩).
    pub sources_only: bool,
    /// The selected source, from 0.
    pub selected: usize,
    /// Each question's number: a late result of an earlier one is dropped.
    pub seq: u64,
    /// Focus on the sources, where ↑ ↓ and Return work.
    pub focus: FocusHandle,
}

impl AskState {
    pub fn new(focus: FocusHandle) -> AskState {
        AskState {
            question: String::new(),
            result: None,
            searching: false,
            answer: None,
            answering: false,
            sources_only: false,
            selected: 0,
            seq: 0,
            focus,
        }
    }

    fn sources(&self) -> &[Source] {
        match &self.result {
            Some(Ok(a)) => &a.sources,
            _ => &[],
        }
    }
}

/// `Sun 9 Nov 21:58` in local time.
fn when(rfc3339: &str) -> String {
    time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
        .map(|t| {
            let local = t.to_offset(super::status::local_offset());
            format!(
                "{} {}",
                crate::state::edition_title(local.date()),
                kit::hm(t)
            )
        })
        .unwrap_or_default()
}

/// What the answer's inline marks become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Strong,
    Code,
    Cite,
}

/// A paragraph of the answer as plain text and the ranges to mark: `**bold**`
/// and `` `code` `` lose their markers, citations `[n]` keep theirs, and a
/// line starting `- ` or `* ` becomes a bullet.
pub fn inline(text: &str) -> (String, Vec<(std::ops::Range<usize>, Mark)>) {
    let mut out = String::new();
    let mut marks = Vec::new();
    for (k, line) in text.lines().enumerate() {
        if k > 0 {
            out.push('\n');
        }
        let line = match line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            Some(rest) => {
                out.push_str("• ");
                rest
            }
            None => line,
        };
        let mut rest = line;
        let mut strong_from: Option<usize> = None;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("**") {
                match strong_from.take() {
                    Some(from) => marks.push((from..out.len(), Mark::Strong)),
                    None => strong_from = Some(out.len()),
                }
                rest = after;
                continue;
            }
            if let Some((after, end)) = rest
                .strip_prefix('`')
                .and_then(|after| after.find('`').map(|end| (after, end)))
            {
                let from = out.len();
                out.push_str(&after[..end]);
                marks.push((from..out.len(), Mark::Code));
                rest = &after[end + 1..];
                continue;
            }
            let cite_end = rest.starts_with('[').then(|| rest.find(']')).flatten();
            if let Some(end) = cite_end.filter(|&end| {
                let inner = &rest[1..end];
                !inner.is_empty()
                    && inner
                        .chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, ',' | ' ' | '-' | '–'))
            }) {
                let from = out.len();
                out.push_str(&rest[..=end]);
                marks.push((from..out.len(), Mark::Cite));
                rest = &rest[end + 1..];
                continue;
            }
            let ch = rest.chars().next().expect("not empty");
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
        if let Some(from) = strong_from {
            marks.push((from..out.len(), Mark::Strong));
        }
    }
    (out, marks)
}

/// A paragraph of the answer, its marks drawn: citations in the accent
/// colour, bold in the text colour, code on the code background.
fn cited(text: &str, look: &Look) -> StyledText {
    let (plain, marks) = inline(text);
    let highlights = marks
        .into_iter()
        .map(|(r, m)| {
            (
                r,
                match m {
                    Mark::Cite => HighlightStyle {
                        color: Some(look.c.text_accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        background_color: Some(look.c.text_accent.opacity(0.12)),
                        ..Default::default()
                    },
                    Mark::Strong => HighlightStyle {
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                    Mark::Code => HighlightStyle {
                        background_color: Some(look.c.selected),
                        ..Default::default()
                    },
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(plain).with_highlights(highlights)
}

impl Shell {
    /// Runs the Ask bar's question: the sources now, the written answer
    /// after, unless `sources_only` or the setting is off.
    pub(crate) fn submit_ask(
        &mut self,
        sources_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let question = self.ask_field.read(cx).text().trim().to_owned();
        if question.is_empty() {
            return;
        }
        self.inbox.proposed = None;
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let Some(cfg) = loaded.setup.config().cloned() else {
            return;
        };
        let ws = loaded.setup.workspace().cloned();
        self.recent_questions = crate::prefs::remember(&self.recent_questions, &question);
        let recent = self.recent_questions.clone();
        let _ = crate::prefs::update(self.themes.prefs_path(), |p| p.recent_questions = recent);
        self.ask.seq += 1;
        let seq = self.ask.seq;
        self.ask.question = question.clone();
        self.ask.result = None;
        self.ask.answer = None;
        self.ask.searching = true;
        self.ask.answering = false;
        self.ask.sources_only = sources_only;
        self.ask.selected = 0;
        if self.state.tabs.section() != Section::Home {
            self.state.tabs.select(Section::Home);
        }
        self.state.tabs.open(Tab::Ask);
        let want_answer = !sources_only && self.ask_answers;
        let installation = self.installation.clone();
        cx.spawn(async move |this, cx| {
            let (cfg2, ws2, q2) = (cfg.clone(), ws.clone(), question.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    hippo_core::ask::run(&cfg2, ws2.as_ref(), &q2, hippo_core::ask::DEFAULT_LIMIT)
                        .map_err(|e| e.to_string())
                })
                .await;
            let go_on = this
                .update(cx, |this, cx| {
                    if this.ask.seq != seq {
                        return false;
                    }
                    let has = result.as_ref().is_ok_and(|a| !a.sources.is_empty());
                    this.ask.searching = false;
                    this.ask.result = Some(result);
                    this.ask.answering = want_answer && has;
                    cx.notify();
                    want_answer && has
                })
                .unwrap_or(false);
            if !go_on {
                return;
            }
            let cwd = ws.as_ref().map_or_else(crate::flow::job_cwd, |w| w.root.clone());
            let answer = cx
                .background_executor()
                .spawn(async move {
                    let args = vec!["ask".to_owned(), question, "--json".to_owned()];
                    let out = cli::run(
                        &installation.binary,
                        &installation.home,
                        &cwd,
                        &args,
                        cli::Limits {
                            run: ANSWER_LIMIT,
                            grace: std::time::Duration::from_secs(2),
                        },
                    )
                    .map_err(|e| format!("cannot run hippo: {e}"))?;
                    if out.timed_out {
                        return Err("the answer took longer than 3 minutes".to_owned());
                    }
                    if !out.success() {
                        let err = out.stderr.trim();
                        return Err(if err.is_empty() {
                            "hippo ask failed".to_owned()
                        } else {
                            err.lines().last().unwrap_or(err).to_owned()
                        });
                    }
                    let asked: Asked = serde_json::from_str(out.stdout.trim()).map_err(|e| {
                        format!("hippo ask printed something else: {e}; is the installed hippo older than the app?")
                    })?;
                    Ok((asked.answer, asked.answer_dropped))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.ask.seq != seq {
                    return;
                }
                this.ask.answering = false;
                this.ask.answer = Some(answer);
                cx.notify();
            })
            .ok();
        })
        .detach();
        window.focus(&self.ask.focus, cx);
        cx.notify();
    }

    /// ⌘K: Home, with the question field focused.
    pub(crate) fn focus_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.tabs.section() != Section::Home {
            self.state.tabs.select(Section::Home);
        }
        if !matches!(self.state.tabs.active_tab(), Some(Tab::Home | Tab::Ask)) {
            self.state.tabs.select(Section::Home);
        }
        let focus = self.ask_field.focus_handle(cx);
        window.focus(&focus, cx);
    }

    pub(crate) fn step_source(&mut self, step: isize) {
        let n = self.ask.sources().len();
        if n == 0 {
            return;
        }
        let at = self.ask.selected as isize + step;
        self.ask.selected = at.clamp(0, n as isize - 1) as usize;
    }

    /// Opens a source: its session, or its note.
    pub(crate) fn open_source(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.ask.sources().get(i).cloned() else {
            return;
        };
        let tab = match (&s.id, &s.path) {
            (Some(id), _) => Tab::Session {
                id: id.clone(),
                ordinal: s.ordinal,
            },
            (None, Some(p)) => Tab::Note(p.clone()),
            _ => return,
        };
        self.state.tabs.open(tab);
        self.after_state_change(window, cx);
    }

    /// The selected source's resume command, for ⌘C.
    pub(crate) fn selected_resume(&self) -> Option<String> {
        self.ask.sources().get(self.ask.selected)?.resume.clone()
    }

    pub(super) fn render_ask_tab(
        &mut self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Narrow: the resume command leaves its button (⌘C still copies it).
        let narrow = f32::from(window.viewport_size().width) < super::rail::NARROW;
        let a = &self.ask;
        let sources: Vec<Source> = a.sources().to_vec();
        let meta = match &a.result {
            Some(Ok(asked)) => {
                let sessions = sources.iter().filter(|s| s.kind == "session").count();
                let notes = sources.len() - sessions;
                let mut profiles: Vec<&str> = sources
                    .iter()
                    .filter_map(|s| s.profile.as_deref())
                    .collect();
                profiles.sort_unstable();
                profiles.dedup();
                let found = if sources.is_empty() {
                    "Nothing matched".to_owned()
                } else {
                    let mut parts = vec![format!(
                        "{} {}",
                        sessions,
                        if sessions == 1 { "session" } else { "sessions" }
                    )];
                    if !profiles.is_empty() {
                        parts.push(format!(
                            "{} {}",
                            profiles.len(),
                            if profiles.len() == 1 {
                                "profile"
                            } else {
                                "profiles"
                            }
                        ));
                    }
                    if notes > 0 {
                        parts.push(format!(
                            "{notes} {}",
                            if notes == 1 { "note" } else { "notes" }
                        ));
                    }
                    parts.join(", ")
                };
                format!(
                    "{found} · searched {} sessions and {} notes in {:.1} s for {}",
                    asked.searched.sessions,
                    asked.searched.notes,
                    asked.searched.ms as f64 / 1000.0,
                    asked.terms.join(" ")
                )
            }
            Some(Err(e)) => format!("The search failed: {e}"),
            None => "Searching every session and note…".to_owned(),
        };
        let answer_block: gpui::AnyElement = if a.searching {
            div().into_any_element()
        } else if a.answering {
            div()
                .text_color(look.c.text_muted)
                .child("Writing an answer from these sources, one model call… (⌘↩ asks again with sources only)")
                .into_any_element()
        } else {
            match &a.answer {
                Some(Ok((Some(ans), _))) => self.render_answer(&ans.text, look),
                Some(Ok((None, why))) => div()
                    .text_color(look.c.text_muted)
                    .child(format!(
                        "No written answer: {}. The sources below are what the search found.",
                        why.as_deref().unwrap_or("none came back")
                    ))
                    .into_any_element(),
                Some(Err(e)) => div()
                    .text_color(look.c.warning)
                    .child(format!("No written answer: {e}. The sources stand alone."))
                    .into_any_element(),
                None if matches!(a.result, Some(Ok(_))) && !sources.is_empty() => div()
                    .text_color(kit::dim(look))
                    .child(if a.sources_only {
                        "Sources only, as asked."
                    } else {
                        "Written answers are off (Manage). The sources stand alone."
                    })
                    .into_any_element(),
                None => div().into_any_element(),
            }
        };
        let can_propose =
            matches!(&a.answer, Some(Ok((Some(_), _)))) && !a.answering && !self.inbox.proposing;
        let proposal_line = if self.inbox.proposing {
            Some(div().text_color(look.c.text_muted).child(
                "Drafting from this answer, then a fresh reviewer checks it: two model calls…",
            ))
        } else {
            self.inbox
                .proposed
                .as_ref()
                .map(|r| {
                    match r {
                        Ok(text) => div()
                            .id("proposed")
                            .flex()
                            .gap(look.px(10.))
                            .cursor_pointer()
                            .text_color(if text.contains("rejected") {
                                look.c.warning
                            } else {
                                kit::ok(look)
                            })
                            .child(proposed_words(text))
                            .when(!text.contains("rejected"), |d| {
                                d.child(div().text_color(look.c.text_accent).child("Open"))
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_section(crate::state::Section::Inbox, window, cx)
                            }))
                            .into_any_element(),
                        Err(e) => div()
                            .text_color(look.c.error)
                            .child(e.clone())
                            .into_any_element(),
                    }
                    .into_any_element()
                })
                .map(|e| div().child(e))
        };
        let focused = a.focus.clone();
        let selected = a.selected;
        let rows: Vec<gpui::AnyElement> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| self.render_source(i, s, i == selected, narrow, look, cx))
            .collect();
        let page = div()
            .flex()
            .flex_col()
            .gap(look.px(14.))
            .w_full()
            .max_w(look.px(1180.))
            .mx_auto()
            .px(look.px(24.))
            .pt(look.px(18.))
            .pb(look.px(28.))
            .text_size(look.text(13.))
            .child(self.render_ask_bar(false, look, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(kit::dim(look))
                            .child(meta),
                    )
                    .child(
                        kit::button(
                            look,
                            "save-note",
                            "Save as note",
                            if can_propose { Kind::Plain } else { Kind::Disabled },
                        )
                        .tooltip(super::tooltip::Tooltip::text(if can_propose {
                            "Drafts a new note from this answer; a fresh reviewer checks it, then it waits in the Inbox for you to keep"
                        } else {
                            "Needs a written answer that cites its sources"
                        }))
                        .when(can_propose, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.propose_from_ask(true, cx)))
                        }),
                    )
                    .child(
                        kit::button(
                            look,
                            "make-rule",
                            "Make it a rule",
                            if can_propose { Kind::Primary } else { Kind::Disabled },
                        )
                        .tooltip(super::tooltip::Tooltip::text(if can_propose {
                            "Drafts a rule for your agents from this answer; a fresh reviewer checks it, then it waits in the Inbox for you to keep"
                        } else {
                            "Needs a written answer that cites its sources"
                        }))
                        .when(can_propose, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.propose_from_ask(false, cx)))
                        }),
                    ),
            )
            .children(proposal_line)
            .child(answer_block)
            .when(!sources.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(10.))
                        .pt(look.px(6.))
                        .child(kit::label(look, "Sources", look.c.text_muted))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(kit::dim(look))
                                .child("↑↓ move · ↩ open · ⌘C copy resume command"),
                        ),
                )
                .child(
                    div()
                        .id("ask-sources")
                        .key_context("AskSources")
                        .track_focus(&focused)
                        .flex()
                        .flex_col()
                        .gap(look.px(8.))
                        .children(rows),
                )
            });
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("ask")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .child(page),
            )
            .children(super::scrollbar::vertical(&self.page_scroll, look))
            .into_any_element()
    }

    /// The answer: paragraphs with their citations, code blocks in the
    /// monospace font.
    fn render_answer(&self, text: &str, look: &Look) -> gpui::AnyElement {
        let mut blocks = div().flex().flex_col().gap(look.px(10.));
        let mut in_code = false;
        let mut code: Vec<&str> = Vec::new();
        let mut para: Vec<&str> = Vec::new();
        let flush_para = |para: &mut Vec<&str>, blocks: gpui::Div| -> gpui::Div {
            if para.is_empty() {
                return blocks;
            }
            let text = para.join("\n");
            para.clear();
            blocks.child(
                div()
                    .text_size(look.text(15.))
                    .line_height(gpui::relative(1.55))
                    .text_color(look.c.text)
                    .child(cited(&text, look)),
            )
        };
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                if in_code {
                    blocks = blocks.child(
                        div()
                            .p(look.px(12.))
                            .rounded(look.px(8.))
                            .border(look.fixed(1.))
                            .border_color(look.c.border_variant)
                            .bg(look.c.window)
                            .font_family("Menlo")
                            .text_size(look.text(12.5))
                            .overflow_hidden()
                            .children(
                                code.drain(..)
                                    .map(|l| div().whitespace_nowrap().child(l.to_owned())),
                            ),
                    );
                    in_code = false;
                } else {
                    blocks = flush_para(&mut para, blocks);
                    in_code = true;
                }
                continue;
            }
            if in_code {
                code.push(line);
            } else if line.trim().is_empty() {
                blocks = flush_para(&mut para, blocks);
            } else {
                para.push(line);
            }
        }
        if in_code && !code.is_empty() {
            blocks = blocks.child(
                div()
                    .font_family("Menlo")
                    .text_size(look.text(12.5))
                    .children(code.drain(..).map(|l| div().child(l.to_owned()))),
            );
        }
        blocks = flush_para(&mut para, blocks);
        blocks.into_any_element()
    }

    fn render_source(
        &self,
        i: usize,
        s: &Source,
        selected: bool,
        narrow: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let note = s.kind == "note";
        let mut facts: Vec<String> = Vec::new();
        if note {
            facts.push("note".into());
            facts.extend(s.path.as_ref().map(|p| p.display().to_string()));
        } else {
            facts.extend(s.profile.as_deref().map(kit::profile_name));
            facts.extend(s.project.clone());
            facts.extend(s.time.as_deref().map(when));
        }
        let resume = s.resume.clone().filter(|_| !narrow);
        let copied = resume.is_some() && self.copied.as_deref() == resume.as_deref();
        let open_label = "Open";
        div()
            .id(("source", i))
            .role(Role::ListItem)
            .aria_label(s.title.clone())
            .aria_selected(selected)
            .flex()
            .items_center()
            .gap(look.px(14.))
            .px(look.px(16.))
            .py(look.px(11.))
            .rounded(look.px(9.))
            .border(look.fixed(1.))
            .border_color(if selected {
                look.c.text_accent.opacity(0.6)
            } else {
                look.c.border_variant
            })
            .bg(look.c.elevated.opacity(0.55))
            .cursor_pointer()
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                this.ask.selected = i;
                if e.click_count() >= 2 {
                    this.open_source(i, window, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .w(look.px(18.))
                    .text_color(look.c.text_accent)
                    .font_weight(FontWeight::MEDIUM)
                    .child(s.n.to_string()),
            )
            .child(if note {
                div()
                    .flex_shrink_0()
                    .size(look.px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(look.px(7.))
                    .border(look.fixed(1.5))
                    .border_color(look.c.text_accent.opacity(0.6))
                    .bg(look.c.text_accent.opacity(0.1))
                    .child(kit::icon(look, "note", 15., look.c.text_accent))
            } else {
                kit::avatar(look, s.profile.as_deref(), None)
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(look.px(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(s.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(gpui::relative(0.6))
                                    .truncate()
                                    .text_color(kit::dim(look))
                                    .child(format!("· {}", facts.join(" · "))),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(look.text(12.5))
                            .text_color(kit::dim(look))
                            .italic()
                            .child(s.why.clone()),
                    ),
            )
            .child(
                kit::button(look, ("source-open", i), open_label, Kind::Plain).on_click(
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_source(i, window, cx);
                    }),
                ),
            )
            .children(resume.map(|cmd| {
                // Every resume button reads Copy resume; the command is in
                // its tooltip.
                let label = if copied { "Copied" } else { "Copy resume" };
                kit::button(look, ("source-resume", i), label, Kind::Plain)
                    .tooltip(super::tooltip::Tooltip::text(format!("Copies: {cmd}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(cmd.clone()));
                        this.copied = Some(cmd.clone());
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}

/// What `hippo lessons propose` printed, as the banner says it:
/// `In the Inbox for review: <title>`, never the lesson's raw id.
fn proposed_words(text: &str) -> String {
    let text = text.trim();
    // A leading `<ULID>: ` is the record's id.
    let text = match text.split_once(": ") {
        Some((id, rest)) if id.len() == 26 && id.chars().all(|c| c.is_ascii_alphanumeric()) => rest,
        _ => text,
    };
    if let Some(rest) = text.strip_prefix("Sent to the Inbox for you to keep: ") {
        // `<title> (<id>)`
        let title = match rest.rsplit_once(" (") {
            Some((t, id)) if id.ends_with(')') => t,
            _ => rest,
        };
        return format!("In the Inbox for review: {title}");
    }
    if let Some(rest) = text.strip_prefix("in the Inbox for review: ") {
        return format!("In the Inbox for review: {rest}");
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_banner_names_the_title_not_the_id() {
        assert_eq!(
            proposed_words(
                "Sent to the Inbox for you to keep: Run tests first (01M70W29EWG854XF0D8201C2T9)"
            ),
            "In the Inbox for review: Run tests first"
        );
        assert_eq!(
            proposed_words("01M70W29EWG854XF0D8201C2T9: in the Inbox for review: Run tests first"),
            "In the Inbox for review: Run tests first"
        );
        assert_eq!(
            proposed_words("The reviewer rejected it"),
            "The reviewer rejected it"
        );
    }

    #[test]
    fn marks_lose_their_markers_and_citations_keep_theirs() {
        let (text, marks) = inline("- **Why:** use `linter` [2].");
        // Byte ranges: the bullet is three bytes.
        assert_eq!(text, "• Why: use linter [2].");
        assert_eq!(
            marks,
            vec![
                (4..8, Mark::Strong),
                (13..19, Mark::Code),
                (20..23, Mark::Cite)
            ]
        );
        let (text, marks) = inline("a [x] b [1, 3]");
        assert_eq!(text, "a [x] b [1, 3]");
        assert_eq!(marks, vec![(8..14, Mark::Cite)]);
    }

    #[test]
    fn times_read_as_day_and_clock() {
        assert!(when("2026-11-09T16:28:00Z").contains(':'));
        assert_eq!(when("not a time"), "");
    }
}
