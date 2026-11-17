//! DOM -> compact Markdown for reading.

use scraper::{ElementRef, Node, Selector};
use url::Url;

use crate::dom::{hidden_reason, SKIP_TAGS};

#[derive(Clone, Copy, Debug)]
pub struct MdOpts {
    /// Emit `[text](url)` for links. Off by default: links are in the Action Map.
    pub links: bool,
    /// Drop `<nav>`, `<footer>`, `<aside>` and navigation roles.
    pub skip_boilerplate: bool,
}

pub fn render(root: ElementRef, base: &Url, opts: MdOpts) -> String {
    let mut w = Writer { out: String::new(), base, opts, lists: Vec::new(), pre: false };
    w.element_or_children(root);
    w.finish()
}

struct Writer<'a> {
    out: String,
    base: &'a Url,
    opts: MdOpts,
    /// One entry per open list: `Some(n)` = ordered, next number n.
    lists: Vec<Option<u32>>,
    pre: bool,
}

impl<'a> Writer<'a> {
    fn finish(self) -> String {
        let mut res = String::with_capacity(self.out.len());
        let mut blank = 0;
        for line in self.out.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                blank += 1;
                if blank > 1 {
                    continue;
                }
            } else {
                blank = 0;
            }
            res.push_str(line);
            res.push('\n');
        }
        res.trim().to_string()
    }

    fn trim_trailing_spaces(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
    }

    fn newline(&mut self) {
        self.trim_trailing_spaces();
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn blank(&mut self) {
        self.trim_trailing_spaces();
        if self.out.is_empty() {
            return;
        }
        while !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn text(&mut self, t: &str) {
        if self.pre {
            self.out.push_str(t);
            return;
        }
        let lead = t.starts_with(char::is_whitespace);
        let trail = t.ends_with(char::is_whitespace);
        let at_line_start = self.out.is_empty() || self.out.ends_with('\n');
        let after_space = self.out.ends_with(' ');
        let mut words = t.split_whitespace();
        let Some(first) = words.next() else {
            if lead && !at_line_start && !after_space {
                self.out.push(' ');
            }
            return;
        };
        if lead && !at_line_start && !after_space {
            self.out.push(' ');
        }
        self.out.push_str(first);
        for word in words {
            self.out.push(' ');
            self.out.push_str(word);
        }
        if trail {
            self.out.push(' ');
        }
    }

    fn element_or_children(&mut self, el: ElementRef) {
        // The root itself (body / main / article) is just a container.
        self.children(el);
    }

    fn children(&mut self, el: ElementRef) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => {
                    let s: &str = &t.text;
                    self.text(s);
                }
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        self.element(c);
                    }
                }
                _ => {}
            }
        }
    }

    fn wrapped(&mut self, el: ElementRef, mark: &str) {
        if self.pre {
            self.children(el);
            return;
        }
        let start = self.out.len();
        self.out.push_str(mark);
        let inner = self.out.len();
        self.children(el);
        self.trim_trailing_spaces();
        if self.out.len() <= inner {
            self.out.truncate(start);
        } else {
            self.out.push_str(mark);
        }
    }

    fn element(&mut self, el: ElementRef) {
        let e = el.value();
        let name = e.name();
        if SKIP_TAGS.contains(&name) || hidden_reason(el).is_some() {
            return;
        }
        if self.opts.skip_boilerplate {
            let role = e.attr("role").unwrap_or("");
            if matches!(name, "nav" | "footer" | "aside")
                || matches!(role, "navigation" | "contentinfo" | "complementary" | "search")
            {
                return;
            }
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = (name.as_bytes()[1] - b'0') as usize;
                self.blank();
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
                self.children(el);
                self.blank();
            }
            "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "aside"
            | "form" | "fieldset" | "figure" | "figcaption" | "address" | "details" | "summary"
            | "dl" | "dt" | "dd" | "body" | "html" | "center" => {
                self.blank();
                self.children(el);
                self.blank();
            }
            "br" => {
                self.trim_trailing_spaces();
                self.out.push('\n');
            }
            "hr" => {
                self.blank();
                self.out.push_str("---");
                self.blank();
            }
            "ul" | "ol" => {
                if self.lists.is_empty() {
                    self.blank();
                } else {
                    self.newline();
                }
                self.lists.push(if name == "ol" { Some(1) } else { None });
                self.children(el);
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                } else {
                    self.newline();
                }
            }
            "li" => {
                self.newline();
                let depth = self.lists.len().max(1);
                self.out.push_str(&"  ".repeat(depth - 1));
                match self.lists.last_mut() {
                    Some(Some(n)) => {
                        self.out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => self.out.push_str("- "),
                }
                self.children(el);
                self.newline();
            }
            "a" => self.link(el),
            "strong" | "b" => self.wrapped(el, "**"),
            "em" | "i" => self.wrapped(el, "*"),
            "code" | "kbd" | "samp" => self.wrapped(el, "`"),
            "pre" => {
                self.blank();
                self.out.push_str("```\n");
                self.pre = true;
                self.children(el);
                self.pre = false;
                if !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                self.out.push_str("```");
                self.blank();
            }
            "blockquote" => {
                let mut sub = Writer { out: String::new(), base: self.base, opts: self.opts, lists: Vec::new(), pre: false };
                sub.children(el);
                let quoted = sub.finish();
                self.blank();
                for line in quoted.lines() {
                    self.out.push_str("> ");
                    self.out.push_str(line);
                    self.out.push('\n');
                }
                self.blank();
            }
            "table" => self.table(el),
            "tr" => {
                self.newline();
                self.children(el);
                self.newline();
            }
            "td" | "th" => {
                self.children(el);
                self.text(" ");
            }
            // Form controls live in the Action Map, not in the reading text.
            "input" | "button" | "select" | "textarea" | "option" | "label" | "img" | "title" => {}
            _ => self.children(el),
        }
    }

    fn link(&mut self, el: ElementRef) {
        let href = el.value().attr("href").unwrap_or("").trim();
        if !self.opts.links || self.pre || href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
            self.children(el);
            return;
        }
        let start = self.out.len();
        self.out.push('[');
        let inner = self.out.len();
        self.children(el);
        self.trim_trailing_spaces();
        if self.out.len() <= inner {
            self.out.truncate(start);
            return;
        }
        let target = self.base.join(href).map(|u| u.to_string()).unwrap_or_else(|_| href.to_string());
        self.out.push_str("](");
        self.out.push_str(&target);
        self.out.push(')');
    }

    fn table(&mut self, el: ElementRef) {
        let nested = Selector::parse("table").unwrap();
        if el.select(&nested).next().is_some() {
            // Layout table: treat rows as plain blocks.
            self.blank();
            self.children(el);
            self.blank();
            return;
        }
        let rows = Selector::parse("tr").unwrap();
        self.blank();
        let mut first = true;
        for tr in el.select(&rows) {
            let mut cells: Vec<String> = Vec::new();
            for child in tr.children() {
                let Some(cell) = ElementRef::wrap(child) else { continue };
                if !matches!(cell.value().name(), "td" | "th") || hidden_reason(cell).is_some() {
                    continue;
                }
                let mut sub = Writer { out: String::new(), base: self.base, opts: self.opts, lists: Vec::new(), pre: false };
                sub.children(cell);
                cells.push(sub.finish().replace('\n', " ").replace('|', "\\|"));
            }
            if cells.iter().all(|c| c.is_empty()) {
                continue;
            }
            self.out.push_str("| ");
            self.out.push_str(&cells.join(" | "));
            self.out.push_str(" |\n");
            if first {
                self.out.push_str(&format!("|{}\n", "---|".repeat(cells.len())));
                first = false;
            }
        }
        self.blank();
    }
}
