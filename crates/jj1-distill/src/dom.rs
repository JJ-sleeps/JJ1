//! Shared DOM helpers: what counts as hidden, and visible-text extraction.

use scraper::{ElementRef, Node};

/// Elements whose content is never page text.
pub const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "head", "iframe", "canvas", "object",
    "embed", "audio", "video", "picture", "map",
];

const HIDDEN_CLASSES: &[&str] = &[
    "hidden", "d-none", "sr-only", "visually-hidden", "invisible", "screen-reader-text",
];

const BLOCK_TAGS: &[&str] = &[
    "p", "div", "li", "td", "th", "tr", "br", "h1", "h2", "h3", "h4", "h5", "h6", "section",
    "article", "header", "footer", "nav", "aside", "ul", "ol", "table", "blockquote", "pre",
    "form", "dt", "dd", "figure", "figcaption", "main", "option",
];

pub fn is_block(name: &str) -> bool {
    BLOCK_TAGS.contains(&name)
}

/// Collapse all whitespace runs to single spaces and trim.
pub fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cut to at most `max` characters, adding an ellipsis when shortened.
pub fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Why this element is invisible to a human, if it is.
///
/// Tier 0 has no CSS engine, so this sees attributes, inline styles and a few
/// well-known utility classes. Stylesheet rules are not evaluated.
pub fn hidden_reason(el: ElementRef) -> Option<&'static str> {
    let e = el.value();
    // Set by the WebView tier from the element's computed style.
    if let Some(why) = e.attr("data-jj1-h") {
        return Some(match why {
            // Menus, dialogs and tabs are routinely hidden this way on live
            // pages, so these two get their own name (see scan.rs).
            "display:none" => "display:none (computed)",
            "visibility:hidden" => "visibility:hidden (computed)",
            "opacity:0" => "opacity:0",
            "zero font-size" => "zero font-size",
            "off-screen positioning" => "off-screen positioning",
            "clipped" => "clipped",
            _ => "hidden (computed style)",
        });
    }
    if e.attr("hidden").is_some() {
        return Some("hidden attribute");
    }
    if e.attr("aria-hidden").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return Some("aria-hidden");
    }
    if e.name() == "dialog" && e.attr("open").is_none() {
        return Some("closed dialog");
    }
    if let Some(class) = e.attr("class") {
        if class.split_whitespace().any(|c| HIDDEN_CLASSES.contains(&c)) {
            return Some("hidden by class");
        }
    }
    e.attr("style").and_then(style_hidden)
}

fn style_hidden(style: &str) -> Option<&'static str> {
    let mut color: Option<String> = None;
    let mut has_background = false;
    for decl in style.split(';') {
        let Some((prop, val)) = decl.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        let val = val.to_ascii_lowercase().replace("!important", "");
        let val = val.trim();
        match prop.as_str() {
            "display" if val == "none" => return Some("display:none"),
            "visibility" if val == "hidden" || val == "collapse" => return Some("visibility:hidden"),
            "font-size" if is_zero(val) => return Some("zero font-size"),
            "opacity" if is_zero(val) => return Some("opacity:0"),
            "width" | "height" | "max-width" | "max-height" if is_zero(val) => return Some("zero size"),
            "left" | "top" | "text-indent" | "margin-left" | "margin-top" if far_negative(val) => {
                return Some("off-screen positioning")
            }
            "color" => color = Some(val.replace(' ', "")),
            "background" | "background-color" => has_background = true,
            _ => {}
        }
    }
    if !has_background {
        if let Some(c) = color {
            if matches!(c.as_str(), "white" | "#fff" | "#ffffff" | "rgb(255,255,255)" | "transparent")
                || c.starts_with("rgba(") && c.ends_with(",0)")
            {
                return Some("white or transparent text");
            }
        }
    }
    None
}

fn leading_number(v: &str) -> Option<f32> {
    let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.parse().ok()
}

fn is_zero(v: &str) -> bool {
    leading_number(v) == Some(0.0)
}

fn far_negative(v: &str) -> bool {
    v.strip_prefix('-').and_then(leading_number).is_some_and(|n| n >= 999.0)
}

/// Text a human would see inside `el`, whitespace-collapsed.
pub fn text_of(el: ElementRef) -> String {
    let mut out = String::new();
    collect_visible(el, &mut out);
    squash(&out)
}

fn collect_visible(el: ElementRef, out: &mut String) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let s: &str = &t.text;
                out.push_str(s);
            }
            Node::Element(e) => {
                if SKIP_TAGS.contains(&e.name()) {
                    continue;
                }
                let Some(c) = ElementRef::wrap(child) else { continue };
                if hidden_reason(c).is_some() {
                    continue;
                }
                let block = is_block(e.name());
                if block {
                    out.push(' ');
                }
                collect_visible(c, out);
                if block {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
}

/// All text inside `el`, including text a human would not see.
pub fn raw_text(el: ElementRef) -> String {
    squash(&el.text().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_style_detection() {
        assert_eq!(style_hidden("display: none"), Some("display:none"));
        assert_eq!(style_hidden("font-size:0px"), Some("zero font-size"));
        assert_eq!(style_hidden("font-size:0.9em"), None);
        assert_eq!(style_hidden("position:absolute; left:-9999px"), Some("off-screen positioning"));
        assert_eq!(style_hidden("color:#fff"), Some("white or transparent text"));
        assert_eq!(style_hidden("color:#fff;background:#000"), None);
        assert_eq!(style_hidden("opacity:0.5"), None);
        assert_eq!(style_hidden("margin-left:-4px"), None);
    }
}
