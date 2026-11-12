//! Dreaming step 3: the rendered prompt.
//!
//! Each `{{name}}` placeholder becomes that input's text between a
//! `<<<UNTRUSTED name>>>` line and a `<<<END name>>>` line, in one pass, so
//! input text that looks like a placeholder stays as it is. Over the pass's
//! budget, turns are dropped from the lowest-ranked end, each drop noted as
//! `omitted: turns (<bytes>)`; still over, the pass fails with the size.
//! The prompt ends with [`CLOSING_INSTRUCTION`], which is added only to a
//! prompt file that does not already end with it.

use std::io;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::error::{Error, Result};
use crate::records::Input;

/// The sentence every rendered prompt ends with (Dreaming step 3).
pub const CLOSING_INSTRUCTION: &str = "Respond with only a JSON array of objects `{note, kind, old, new, evidence: [{session, ordinal}], reason}` as your final message; an empty array means no proposals.";

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([^{}]*)\}\}").expect("the placeholder pattern is valid"));

/// A rendered prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// The size of each turn dropped to fit the budget, in drop order.
    pub dropped: Vec<usize>,
}

impl Rendered {
    /// One `omitted: turns (<bytes>)` line per dropped turn.
    pub fn omitted_lines(&self) -> Vec<String> {
        self.dropped
            .iter()
            .map(|bytes| format!("omitted: turns ({bytes})"))
            .collect()
    }
}

/// Renders `template` with `inputs`, the text of each input but `turns`,
/// and `turns`, one entry per turn with the best-ranked first, within
/// `budget` bytes. Turns count only when the template has `{{turns}}`.
pub fn render(
    template: &str,
    inputs: &[(Input, String)],
    turns: &[String],
    budget: usize,
) -> Result<Rendered> {
    let template = with_closing_instruction(template);
    let has_turns = PLACEHOLDER
        .captures_iter(&template)
        .any(|captures| captures[1].trim() == Input::Turns.as_str());
    let turns = if has_turns { turns } else { &[] };
    let mut kept = turns.len();
    let mut dropped = Vec::new();
    loop {
        let text = substitute(&template, inputs, &turns[..kept])?;
        if text.len() <= budget {
            return Ok(Rendered { text, dropped });
        }
        if kept == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("prompt over budget: {} > {budget} bytes", text.len()),
            )));
        }
        // Drop, from the lowest-ranked end, at least as many bytes as the
        // prompt is over; render again in case `{{turns}}` occurs twice.
        let mut excess = text.len() - budget;
        while kept > 0 && excess > 0 {
            kept -= 1;
            let size = turns[kept].len();
            dropped.push(size);
            excess = excess.saturating_sub(size + 1);
        }
    }
}

/// The size of `template` rendered with `inputs` and no turns: the
/// smallest prompt these inputs can make, since only turns are dropped.
pub fn size_without_turns(template: &str, inputs: &[(Input, String)]) -> Result<usize> {
    Ok(substitute(&with_closing_instruction(template), inputs, &[])?.len())
}

/// `template`, ending with [`CLOSING_INSTRUCTION`] on its own paragraph
/// unless it already does.
fn with_closing_instruction(template: &str) -> String {
    let trimmed = template.trim_end();
    if trimmed.ends_with(CLOSING_INSTRUCTION) {
        template.to_owned()
    } else {
        format!("{trimmed}\n\n{CLOSING_INSTRUCTION}\n")
    }
}

/// One pass over `template`, each placeholder replaced by its delimited
/// block. A placeholder for an input that was not given is an error.
fn substitute(template: &str, inputs: &[(Input, String)], turns: &[String]) -> Result<String> {
    let mut missing = None;
    let text = PLACEHOLDER.replace_all(template, |captures: &Captures| {
        let name = captures[1].trim();
        let content = if name == Input::Turns.as_str() {
            Some(turns.join("\n"))
        } else {
            inputs
                .iter()
                .find(|(input, _)| input.as_str() == name)
                .map(|(_, text)| text.clone())
        };
        match content {
            Some(content) => block(name, &content),
            None => {
                missing.get_or_insert_with(|| name.to_owned());
                String::new()
            }
        }
    });
    match missing {
        Some(name) => Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the prompt has {{{{{name}}}}}, which this pass was not given"),
        ))),
        None => Ok(text.into_owned()),
    }
}

/// `content` between the delimiter lines of input `name`.
fn block(name: &str, content: &str) -> String {
    let content = content.strip_suffix('\n').unwrap_or(content);
    if content.is_empty() {
        format!("<<<UNTRUSTED {name}>>>\n<<<END {name}>>>")
    } else {
        format!("<<<UNTRUSTED {name}>>>\n{content}\n<<<END {name}>>>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(pairs: &[(Input, &str)]) -> Vec<(Input, String)> {
        pairs
            .iter()
            .map(|(input, text)| (*input, (*text).to_owned()))
            .collect()
    }

    #[test]
    fn placeholders_become_delimited_blocks_in_one_pass() {
        let template =
            format!("# Pass\n\n{{{{notes}}}}\n\n{{{{ turns }}}}\n\n{CLOSING_INSTRUCTION}\n");
        let rendered = render(
            &template,
            &inputs(&[(Input::Notes, "{\"note\":\"n.md\",\"text\":\"{{turns}}\"}")]),
            &["turn one".to_owned(), "turn two".to_owned()],
            10_000,
        )
        .unwrap();
        assert_eq!(
            rendered.text,
            format!(
                "# Pass\n\n<<<UNTRUSTED notes>>>\n{{\"note\":\"n.md\",\"text\":\"{{{{turns}}}}\"}}\n<<<END notes>>>\n\n\
                 <<<UNTRUSTED turns>>>\nturn one\nturn two\n<<<END turns>>>\n\n{CLOSING_INSTRUCTION}\n"
            )
        );
        assert!(rendered.dropped.is_empty());
    }

    #[test]
    fn the_closing_instruction_is_added_only_when_missing() {
        let ends = format!("Check.\n\n{CLOSING_INSTRUCTION}\n");
        let text = render(&ends, &[], &[], 10_000).unwrap().text;
        assert_eq!(text, ends);
        assert_eq!(text.matches(CLOSING_INSTRUCTION).count(), 1);
        let bare = render("Check.\n", &[], &[], 10_000).unwrap().text;
        assert_eq!(bare, format!("Check.\n\n{CLOSING_INSTRUCTION}\n"));
    }

    #[test]
    fn an_empty_input_is_an_empty_block() {
        let text = render(
            "{{human_confirmed}}",
            &inputs(&[(Input::HumanConfirmed, "")]),
            &[],
            10_000,
        )
        .unwrap()
        .text;
        assert!(text.starts_with("<<<UNTRUSTED human_confirmed>>>\n<<<END human_confirmed>>>\n"));
    }

    #[test]
    fn over_budget_drops_the_lowest_ranked_turns_first() {
        let template = format!("{{{{turns}}}}\n\n{CLOSING_INSTRUCTION}\n");
        let turns: Vec<String> = ["a".repeat(100), "b".repeat(100), "c".repeat(100)].into();
        let full = render(&template, &[], &turns, 10_000).unwrap();
        // Room for two turns, not three.
        let budget = full.text.len() - 50;
        let rendered = render(&template, &[], &turns, budget).unwrap();
        assert_eq!(rendered.dropped, [100]);
        assert_eq!(rendered.omitted_lines(), ["omitted: turns (100)"]);
        assert!(rendered.text.contains(&turns[1]) && !rendered.text.contains(&turns[2]));
        assert!(rendered.text.len() <= budget);
    }

    #[test]
    fn still_over_budget_without_turns_fails_with_the_size() {
        let err = render("x".repeat(500).as_str(), &[], &["t".to_owned()], 100).unwrap_err();
        assert!(err.to_string().contains("prompt over budget"), "{err}");
    }

    #[test]
    fn a_placeholder_without_its_input_is_an_error() {
        let err = render("{{sessions}}", &[], &[], 10_000).unwrap_err();
        assert!(err.to_string().contains("{{sessions}}"), "{err}");
    }
}
