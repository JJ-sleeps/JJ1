//! Source links: check that a quote really appears on a page, and build a
//! link that opens the page with that passage highlighted.
//!
//! The link is a URL text fragment (`#:~:text=...`), which Chrome, Edge,
//! Safari and Firefox scroll to and highlight. Nothing is ever linked unless
//! the words were found on the page, so an agent cannot "cite" something the
//! page does not say.

use url::Url;

/// Lower-case, straight quotes, markdown punctuation removed, whitespace
/// collapsed. Applied to both the page text and the quote before matching.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Drop the URL part of markdown links: [label](https://...)
        if c == ']' && chars.peek() == Some(&'(') {
            for d in chars.by_ref() {
                if d == ')' {
                    break;
                }
            }
            continue;
        }
        let c = match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' | '`' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            '*' | '_' | '#' | '>' | '|' | '[' | ']' => ' ',
            c => c,
        };
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.extend(c.to_lowercase());
            space = false;
        }
    }
    out.trim_end().to_string()
}

/// Does `quote` appear in `text` (after normalizing both)? Very short quotes
/// (under three words) are not accepted: they prove nothing.
pub fn verify(text: &str, quote: &str) -> bool {
    let q = normalize(quote.trim_matches(|c: char| c == '"' || c.is_whitespace()));
    q.split(' ').count() >= 3 && normalize(text).contains(&q)
}

/// Percent-encode for a text fragment: everything but unreserved characters,
/// and always `-`, `,` and `&`, which the fragment syntax uses.
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A link to `page` that highlights `quote`. Long quotes use the
/// `start,end` form so small differences in the middle don't break it.
pub fn fragment_url(page: &Url, quote: &str) -> String {
    let words: Vec<&str> = quote
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '*' | '_' | '`' | '#' | '>' | '|' | '"' | '[' | ']')))
        .filter(|w| !w.is_empty())
        .collect();
    let frag = if words.len() > 10 {
        format!("{},{}", enc(&words[..5].join(" ")), enc(&words[words.len() - 5..].join(" ")))
    } else {
        enc(&words.join(" "))
    };
    let mut u = page.clone();
    u.set_fragment(None);
    format!("{u}#:~:text={frag}")
}

/// Verify `quote` against `text` and return its source link if it is there.
pub fn cite(page: &Url, text: &str, quote: &str) -> Option<String> {
    verify(text, quote).then(|| fragment_url(page, quote))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_quotes_do_not_get_in_the_way() {
        let text = "## Pricing\nThe **Pro plan** costs [$20 a month](https://x.test/p) for \u{201C}teams\u{201D}.";
        assert!(verify(text, "the Pro plan costs $20 a month for \"teams\""));
        assert!(!verify(text, "the Pro plan costs $25 a month"));
        assert!(!verify(text, "Pro plan"), "too short to prove anything");
    }

    #[test]
    fn fragment_links_are_encoded_and_shortened() {
        let u = Url::parse("https://ex.test/a#old").unwrap();
        assert_eq!(fragment_url(&u, "costs $20, per-seat"), "https://ex.test/a#:~:text=costs%20%2420%2C%20per%2Dseat");
        let long = "one two three four five six seven eight nine ten eleven twelve";
        assert_eq!(fragment_url(&u, long), "https://ex.test/a#:~:text=one%20two%20three%20four%20five,eight%20nine%20ten%20eleven%20twelve");
    }
}
