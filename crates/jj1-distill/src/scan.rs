//! Prompt-injection scanner (defence in depth, not a guarantee).
//!
//! Reports text a human would not see (hidden elements, suspicious HTML
//! comments) and instruction-like phrases aimed at an AI reader.

use scraper::{ElementRef, Node};

use crate::dom::{clip, hidden_reason, raw_text, squash};

const SNIPPET_MAX: usize = 160;
const MAX_FINDINGS: usize = 12;
/// Hidden text shorter than this is only reported if it looks like an instruction.
const MIN_HIDDEN_CHARS: usize = 40;

#[derive(Debug, Clone)]
pub struct Finding {
    /// `hidden-text`, `html-comment` or `instruction-like-text`.
    pub kind: &'static str,
    pub detail: String,
    pub snippet: String,
}

/// Phrases that are suspicious anywhere, including visible text.
const STRONG: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous",
    "ignore the previous",
    "ignore prior instructions",
    "ignore the above",
    "ignore your instructions",
    "disregard previous",
    "disregard all previous",
    "disregard the above",
    "disregard your instructions",
    "forget previous instructions",
    "forget all previous",
    "forget your instructions",
    "new instructions:",
    "override your instructions",
    "do not tell the user",
    "don't tell the user",
    "without telling the user",
    "do not inform the user",
    "reveal your system prompt",
    "your system prompt",
    "attention ai",
    "attention: ai",
    "dear ai",
    "note to ai",
    "if you are an ai",
    "if you are a language model",
    "<system>",
    "[system]",
];

/// Extra phrases that are only suspicious where a human cannot see them.
const WEAK: &[&str] = &[
    "system prompt",
    "you are now",
    "you must now",
    "ai assistant",
    "ai agent",
    "language model",
    "as an ai",
    "assistant:",
    "instructions:",
    "execute the following",
    "run the following",
    "send the contents",
    "exfiltrate",
    "api key",
    "password",
    "tool call",
];

fn find_in(text_lower: &str, lists: &[&[&'static str]]) -> Option<(usize, &'static str)> {
    lists
        .iter()
        .flat_map(|l| l.iter())
        .filter_map(|p| text_lower.find(p).map(|i| (i, *p)))
        .min_by_key(|(i, _)| *i)
}

/// Check visible text (for example the rendered Markdown) for strong patterns.
pub fn scan_visible(text: &str) -> Option<Finding> {
    let lower = squash(text).to_lowercase();
    let (at, pat) = find_in(&lower, &[STRONG])?;
    let mut start = at.saturating_sub(60);
    while !lower.is_char_boundary(start) {
        start -= 1;
    }
    Some(Finding {
        kind: "instruction-like-text",
        detail: format!("visible text contains \"{pat}\""),
        snippet: clip(&lower[start..], SNIPPET_MAX),
    })
}

/// Walk the whole document for hidden text and suspicious comments.
pub fn scan(root: ElementRef) -> Vec<Finding> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn push(out: &mut Vec<Finding>, f: Finding) {
    if out.len() < MAX_FINDINGS && !out.iter().any(|o| o.snippet == f.snippet) {
        out.push(f);
    }
}

fn walk(el: ElementRef, out: &mut Vec<Finding>) {
    for child in el.children() {
        match child.value() {
            Node::Comment(c) => {
                let s: &str = &c.comment;
                let text = squash(s);
                if let Some((_, pat)) = find_in(&text.to_lowercase(), &[STRONG, WEAK]) {
                    push(out, Finding {
                        kind: "html-comment",
                        detail: format!("comment contains \"{pat}\""),
                        snippet: clip(&text, SNIPPET_MAX),
                    });
                }
            }
            Node::Element(e) => {
                if matches!(e.name(), "script" | "style" | "svg" | "template") {
                    continue;
                }
                let Some(c) = ElementRef::wrap(child) else { continue };
                if let Some(reason) = hidden_reason(c) {
                    let text = raw_text(c);
                    if text.is_empty() {
                        continue;
                    }
                    let hit = find_in(&text.to_lowercase(), &[STRONG, WEAK]);
                    // aria-hidden, utility classes and closed dialogs are routine
                    // (icons, screen-reader labels); report those only when the
                    // text looks like an instruction. Style tricks are rarer and
                    // are reported once they hide a real sentence.
                    let routine = matches!(reason, "aria-hidden" | "hidden by class" | "closed dialog" | "hidden attribute" | "clipped"
                            | "display:none (computed)" | "visibility:hidden (computed)");
                    if hit.is_some() || (!routine && text.chars().count() >= MIN_HIDDEN_CHARS) {
                        let detail = match hit {
                            Some((_, pat)) => format!("{reason}; contains \"{pat}\""),
                            None => reason.to_string(),
                        };
                        push(out, Finding { kind: "hidden-text", detail, snippet: clip(&text, SNIPPET_MAX) });
                    }
                    continue;
                }
                walk(c, out);
            }
            _ => {}
        }
    }
}
