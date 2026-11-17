//! DOM -> interactive elements ("actions") and the forms they belong to.

use std::collections::{HashMap, HashSet};

use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::dom::{clip, hidden_reason, squash, text_of, SKIP_TAGS};

const LABEL_MAX: usize = 80;

#[derive(Debug, Clone)]
pub struct Opt {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// One interactive element, before an ID is assigned.
#[derive(Debug, Clone, Default)]
pub struct RawAction {
    /// `link`, `submit`, `checkbox`, `radio`, `select`, `textarea`, `input[<type>]`.
    pub kind: String,
    pub label: String,
    pub name: Option<String>,
    /// Absolute URL, links only.
    pub href: Option<String>,
    pub value: Option<String>,
    pub checked: Option<bool>,
    pub disabled: bool,
    pub options: Vec<Opt>,
    /// Index into the forms list.
    pub form: Option<usize>,
    /// Inside `<main>` / `<article>` / `role=main`.
    pub in_main: bool,
    /// Tag number on the live page (WebView tier only).
    pub node: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Hidden,
    Text,
    Check,
    Select,
    Submit,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    /// Value from the HTML (for hidden fields this is what gets sent).
    pub value: String,
    /// Index into the actions list for fields the agent can change.
    pub action: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Form {
    /// Absolute target URL.
    pub action: String,
    /// `get` or `post`.
    pub method: String,
    pub enctype: String,
    pub fields: Vec<Field>,
}

pub fn extract(doc: &Html, body: ElementRef, base: &Url) -> (Vec<RawAction>, Vec<Form>) {
    let mut labels_for = HashMap::new();
    let sel = Selector::parse("label[for]").unwrap();
    for label in doc.select(&sel) {
        if let Some(target) = label.value().attr("for") {
            labels_for.insert(target.to_string(), text_of(label));
        }
    }
    let mut x = Extractor {
        base,
        labels_for,
        actions: Vec::new(),
        forms: Vec::new(),
        cur_form: None,
        cur_label: None,
        in_main: false,
        seen_links: HashSet::new(),
    };
    x.walk(body);
    (x.actions, x.forms)
}

struct Extractor<'a> {
    base: &'a Url,
    labels_for: HashMap<String, String>,
    actions: Vec<RawAction>,
    forms: Vec<Form>,
    cur_form: Option<usize>,
    cur_label: Option<String>,
    in_main: bool,
    seen_links: HashSet<(String, String)>,
}

/// The `data-jj1` tag the WebView tier put on a live element.
fn node_of(el: ElementRef) -> Option<u32> {
    el.value().attr("data-jj1").and_then(|n| n.parse().ok())
}

/// Live value/checked state from the WebView tier, falling back to the HTML.
fn live_value(el: ElementRef, html: &str) -> String {
    el.value().attr("data-jj1-v").unwrap_or(html).to_string()
}

fn live_checked(el: ElementRef) -> bool {
    match el.value().attr("data-jj1-c") {
        Some(c) => c == "1",
        None => el.value().attr("checked").is_some(),
    }
}

fn attr(el: ElementRef, name: &str) -> Option<String> {
    el.value().attr(name).map(squash).filter(|s| !s.is_empty())
}

impl<'a> Extractor<'a> {
    fn walk(&mut self, el: ElementRef) {
        for child in el.children() {
            if let Some(c) = ElementRef::wrap(child) {
                self.element(c);
            }
        }
    }

    fn element(&mut self, el: ElementRef) {
        let e = el.value();
        let name = e.name();
        if SKIP_TAGS.contains(&name) || hidden_reason(el).is_some() {
            return;
        }
        match name {
            "form" => {
                let action = e.attr("action").unwrap_or("").trim();
                let target = if action.is_empty() {
                    self.base.to_string()
                } else {
                    self.base.join(action).map(|u| u.to_string()).unwrap_or_else(|_| self.base.to_string())
                };
                let method = match e.attr("method").map(|m| m.trim().to_ascii_lowercase()) {
                    Some(m) if m == "post" => "post".to_string(),
                    _ => "get".to_string(),
                };
                let enctype = e.attr("enctype").unwrap_or("").trim().to_ascii_lowercase();
                self.forms.push(Form { action: target, method, enctype, fields: Vec::new() });
                let prev = self.cur_form.replace(self.forms.len() - 1);
                self.walk(el);
                self.cur_form = prev;
            }
            "label" => {
                let prev = self.cur_label.replace(text_of(el));
                self.walk(el);
                self.cur_label = prev;
            }
            "main" | "article" => {
                let prev = std::mem::replace(&mut self.in_main, true);
                self.walk(el);
                self.in_main = prev;
            }
            "a" => {
                if e.attr("href").is_some() {
                    self.link(el);
                } else {
                    self.live_button(el);
                    self.walk(el);
                }
            }
            "button" => self.button(el),
            "input" => self.input(el),
            "select" => self.select(el),
            "textarea" => {
                let value = live_value(el, &el.text().collect::<String>());
                self.field_action("textarea", el, Some(value), None, Vec::new(), FieldKind::Text);
            }
            _ => {
                if e.attr("role") == Some("main") {
                    let prev = std::mem::replace(&mut self.in_main, true);
                    self.walk(el);
                    self.in_main = prev;
                } else {
                    // Clickable non-form elements (role=button, onclick, ...)
                    // exist only on a live page.
                    self.live_button(el);
                    self.walk(el);
                }
            }
        }
    }

    /// A JavaScript-driven control on a live page. Does nothing in the static
    /// tier, where elements carry no `data-jj1` tag.
    fn live_button(&mut self, el: ElementRef) {
        let Some(node) = node_of(el) else { return };
        let mut label = text_of(el);
        if label.is_empty() {
            label = attr(el, "aria-label")
                .or_else(|| attr(el, "title"))
                .or_else(|| attr(el, "value"))
                .unwrap_or_default();
        }
        self.actions.push(RawAction {
            kind: "button".into(),
            label: clip(&label, LABEL_MAX),
            disabled: el.value().attr("disabled").is_some(),
            form: self.cur_form,
            in_main: self.in_main,
            node: Some(node),
            ..Default::default()
        });
    }

    fn link(&mut self, el: ElementRef) {
        let href = el.value().attr("href").unwrap_or("").trim();
        if href.is_empty() || href.starts_with('#') {
            return self.live_button(el);
        }
        let Ok(mut url) = self.base.join(href) else { return };
        if !matches!(url.scheme(), "http" | "https") {
            return self.live_button(el);
        }
        url.set_fragment(None);
        let mut label = text_of(el);
        if label.is_empty() {
            label = attr(el, "aria-label").or_else(|| attr(el, "title")).unwrap_or_default();
        }
        if label.is_empty() {
            let img = Selector::parse("img[alt]").unwrap();
            if let Some(i) = el.select(&img).next() {
                label = squash(i.value().attr("alt").unwrap_or(""));
            }
        }
        let label = clip(&label, LABEL_MAX);
        let href = url.to_string();
        if !self.seen_links.insert((href.clone(), label.clone())) {
            return;
        }
        self.actions.push(RawAction {
            kind: "link".into(),
            label,
            href: Some(href),
            in_main: self.in_main,
            node: node_of(el),
            ..Default::default()
        });
    }

    fn button(&mut self, el: ElementRef) {
        let e = el.value();
        let ty = e.attr("type").unwrap_or("submit").to_ascii_lowercase();
        // Only submit buttons do anything without JavaScript.
        if ty != "submit" || self.cur_form.is_none() {
            return self.live_button(el);
        }
        let mut label = text_of(el);
        if label.is_empty() {
            label = attr(el, "aria-label")
                .or_else(|| attr(el, "title"))
                .or_else(|| attr(el, "value"))
                .unwrap_or_else(|| "Submit".into());
        }
        let value = e.attr("value").unwrap_or("").to_string();
        self.push_field(
            RawAction {
                kind: "submit".into(),
                label: clip(&label, LABEL_MAX),
                name: attr(el, "name"),
                disabled: e.attr("disabled").is_some(),
                node: node_of(el),
                ..Default::default()
            },
            FieldKind::Submit,
            value,
        );
    }

    fn input(&mut self, el: ElementRef) {
        let e = el.value();
        let ty = e.attr("type").unwrap_or("text").trim().to_ascii_lowercase();
        let html_value = e.attr("value").unwrap_or("").to_string();
        let value = if matches!(ty.as_str(), "hidden" | "submit" | "image" | "checkbox" | "radio" | "button") {
            html_value
        } else {
            live_value(el, &html_value)
        };
        match ty.as_str() {
            "hidden" => {
                if let (Some(form), Some(name)) = (self.cur_form, e.attr("name")) {
                    self.forms[form].fields.push(Field {
                        name: name.to_string(),
                        kind: FieldKind::Hidden,
                        value,
                        action: None,
                    });
                }
            }
            "submit" | "image" => {
                if self.cur_form.is_none() {
                    return self.live_button(el);
                }
                let label = attr(el, "value")
                    .or_else(|| attr(el, "alt"))
                    .or_else(|| attr(el, "aria-label"))
                    .unwrap_or_else(|| "Submit".into());
                self.push_field(
                    RawAction {
                        kind: "submit".into(),
                        label: clip(&label, LABEL_MAX),
                        name: attr(el, "name"),
                        disabled: e.attr("disabled").is_some(),
                        node: node_of(el),
                        ..Default::default()
                    },
                    FieldKind::Submit,
                    value,
                );
            }
            // No effect without JavaScript, or not supported in Tier 0.
            "button" => self.live_button(el),
            "reset" | "file" => {}
            "checkbox" | "radio" => {
                let checked = live_checked(el);
                let v = if value.is_empty() { "on".to_string() } else { value };
                self.field_action(&ty, el, Some(v), Some(checked), Vec::new(), FieldKind::Check);
            }
            _ => {
                let kind = format!("input[{ty}]");
                self.field_action(&kind, el, Some(value), None, Vec::new(), FieldKind::Text);
            }
        }
    }

    fn select(&mut self, el: ElementRef) {
        let sel = Selector::parse("option").unwrap();
        let options: Vec<Opt> = el
            .select(&sel)
            .map(|o| {
                let label = squash(&o.text().collect::<String>());
                Opt {
                    value: o.value().attr("value").map(str::to_string).unwrap_or_else(|| label.clone()),
                    label,
                    selected: o.value().attr("selected").is_some(),
                }
            })
            .collect();
        let value = match el.value().attr("data-jj1-v") {
            Some(live) => Some(live.to_string()),
            None => options.iter().find(|o| o.selected).or_else(|| options.first()).map(|o| o.value.clone()),
        };
        self.field_action("select", el, value, None, options, FieldKind::Select);
    }

    fn control_label(&self, el: ElementRef) -> String {
        let e = el.value();
        let label = attr(el, "aria-label")
            .or_else(|| e.attr("id").and_then(|id| self.labels_for.get(id)).cloned().filter(|s| !s.is_empty()))
            .or_else(|| self.cur_label.clone().filter(|s| !s.is_empty()))
            .or_else(|| attr(el, "placeholder"))
            .or_else(|| attr(el, "title"))
            .unwrap_or_default();
        clip(&label, LABEL_MAX)
    }

    fn field_action(
        &mut self,
        kind: &str,
        el: ElementRef,
        value: Option<String>,
        checked: Option<bool>,
        options: Vec<Opt>,
        field_kind: FieldKind,
    ) {
        let e = el.value();
        let html_value = value.clone().unwrap_or_default();
        self.push_field(
            RawAction {
                kind: kind.to_string(),
                label: self.control_label(el),
                name: attr(el, "name"),
                value,
                checked,
                disabled: e.attr("disabled").is_some(),
                options,
                node: node_of(el),
                ..Default::default()
            },
            field_kind,
            html_value,
        );
    }

    fn push_field(&mut self, mut action: RawAction, kind: FieldKind, value: String) {
        action.form = self.cur_form;
        action.in_main = self.in_main;
        let index = self.actions.len();
        if let Some(form) = self.cur_form {
            self.forms[form].fields.push(Field {
                name: action.name.clone().unwrap_or_default(),
                kind,
                value,
                action: Some(index),
            });
        }
        self.actions.push(action);
    }
}
