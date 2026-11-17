//! jj1-view: the Tier 2 worker. One process = one live page in the system
//! WebView, with no visible window.
//!
//! `jj1` starts this on demand and drives it with JSON lines:
//!
//!   -> {"op":"load","url":"...","allow":[],"deny":[],"blocked":[],"timeout_ms":30000}
//!   -> {"op":"act","node":7,"action":"click","value":""}
//!   -> {"op":"dump"}
//!   -> {"op":"quit"}
//!   <- {"ok":true,"url":"...","html":"...","elapsed_ms":1234,"timed_out":false,"blocked":[]}
//!   <- {"ok":false,"error":"..."}
//!
//! Before every dump the page is annotated (see TAG_JS): interactive elements
//! get `data-jj1="N"` so the core can act on them by number, and elements a
//! human cannot see get `data-jj1-h="reason"` from their *computed* style.
//!
//! Stdout carries the protocol; diagnostics go to stderr.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jj1_engine_view::{install_request_filter, ResourceKind};
use jj1_net::{host_matches, points_to_private_network};
use serde_json::{json, Value};
use url::Url;
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder};

const TICK: Duration = Duration::from_millis(150);
/// The DOM counts as settled after this many identical samples in a row.
const STABLE_TICKS: u32 = 3;
/// Never dump sooner than this after an action, so handlers get to run.
const MIN_SETTLE: Duration = Duration::from_millis(350);
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Grace period for the final tag/dump round-trips after a timeout.
const GRACE: Duration = Duration::from_secs(5);

/// "<readyState>:<DOM size>:<requests in flight>"
const PROBE_JS: &str = "document.readyState+':'+document.documentElement.outerHTML.length+':'+(window.__jj1p||0)";
/// With requests still in flight, an unchanged DOM is accepted after this
/// many ticks anyway (long-polling pages never go idle).
const STABLE_TICKS_BUSY: u32 = 30;

/// Runs before the page's own scripts: counts fetch/XHR requests in flight so
/// the worker can wait for the network to go quiet before reading the page.
const INIT_JS: &str = r#"(function(){
  if (window.__jj1p !== undefined) return;
  window.__jj1p = 0;
  function done() { window.__jj1p = Math.max(0, window.__jj1p - 1); }
  var f = window.fetch;
  if (f) {
    window.fetch = function() {
      window.__jj1p++;
      try { return f.apply(this, arguments).then(function(r){ done(); return r; }, function(e){ done(); throw e; }); }
      catch (e) { done(); throw e; }
    };
  }
  var send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function() {
    var once = false;
    window.__jj1p++;
    this.addEventListener('loadend', function(){ if (!once) { once = true; done(); } });
    return send.apply(this, arguments);
  };
})();"#;

/// Annotate the live DOM. Old annotations are removed first, so nothing a page
/// plants under these attribute names survives into the dump.
const TAG_JS: &str = r#"(function(){
  try {
    var old = document.querySelectorAll('[data-jj1],[data-jj1-h],[data-jj1-v],[data-jj1-c]');
    for (var i = 0; i < old.length; i++) {
      old[i].removeAttribute('data-jj1'); old[i].removeAttribute('data-jj1-h');
      old[i].removeAttribute('data-jj1-v'); old[i].removeAttribute('data-jj1-c');
    }
    var n = 0;
    var SEL = 'a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[role=switch],[role=option],[onclick]';
    var SKIP = {SCRIPT:1, STYLE:1, NOSCRIPT:1, TEMPLATE:1, HEAD:1};
    function walk(el) {
      if (SKIP[el.tagName]) return;
      var cs = getComputedStyle(el), why = '';
      if (cs.display === 'none') why = 'display:none';
      else if (cs.visibility === 'hidden' || cs.visibility === 'collapse') why = 'visibility:hidden';
      else if (parseFloat(cs.opacity) === 0) why = 'opacity:0';
      else if (parseFloat(cs.fontSize) === 0) why = 'zero font-size';
      else {
        var r = el.getBoundingClientRect();
        if (r.width > 0 || r.height > 0) {
          if (r.right < -1000 || r.bottom + window.scrollY < -1000) why = 'off-screen positioning';
          else if (r.width <= 1 && r.height <= 1 && cs.overflow === 'hidden' && cs.position === 'absolute') why = 'clipped';
        }
      }
      if (why) { el.setAttribute('data-jj1-h', why); return; }
      if (el.matches(SEL)) {
        el.setAttribute('data-jj1', String(++n));
        var t = el.tagName;
        if (t === 'INPUT') {
          var ty = (el.type || '').toLowerCase();
          if (ty === 'checkbox' || ty === 'radio') el.setAttribute('data-jj1-c', el.checked ? '1' : '0');
          else if (ty === 'password') { if (el.value) el.setAttribute('data-jj1-v', '(set)'); }
          else if (ty !== 'file') el.setAttribute('data-jj1-v', el.value);
        } else if (t === 'TEXTAREA' || t === 'SELECT') {
          el.setAttribute('data-jj1-v', el.value);
        } else if (el.getAttribute('aria-checked') !== null) {
          el.setAttribute('data-jj1-c', el.getAttribute('aria-checked') === 'true' ? '1' : '0');
        }
      }
      if (el.namespaceURI === 'http://www.w3.org/2000/svg') return;
      for (var c = el.firstElementChild; c; c = c.nextElementSibling) walk(c);
    }
    if (document.body) walk(document.body);
    return 'ok:' + n;
  } catch (e) { return 'err:' + e; }
})()"#;

/// Perform one action on a tagged element. Arguments are appended as a call.
const ACT_JS: &str = r#"(function(node, action, value){
  try {
    if (action === 'scroll') { window.scrollBy(0, Math.round(window.innerHeight * 0.8)); return 'ok'; }
    var el = document.querySelector('[data-jj1="' + node + '"]');
    if (!el) return 'err:that element is no longer on the page; take a new snapshot';
    el.scrollIntoView({block: 'center'});
    if (action === 'click') { el.click(); return 'ok'; }
    if (action === 'type') {
      el.focus();
      if (el.isContentEditable) { el.textContent = value; }
      else {
        var proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        var d = Object.getOwnPropertyDescriptor(proto, 'value');
        if (d && d.set) d.set.call(el, value); else el.value = value;
      }
      el.dispatchEvent(new Event('input', {bubbles: true}));
      el.dispatchEvent(new Event('change', {bubbles: true}));
      return 'ok';
    }
    if (action === 'select') {
      var found = false;
      for (var i = 0; i < el.options.length; i++) {
        var o = el.options[i];
        if (o.value === value || o.text.trim().toLowerCase() === value.toLowerCase()) { el.selectedIndex = i; found = true; break; }
      }
      if (!found) return 'err:no option matches "' + value + '"';
      el.dispatchEvent(new Event('input', {bubbles: true}));
      el.dispatchEvent(new Event('change', {bubbles: true}));
      return 'ok';
    }
    if (action === 'check') {
      var now = el.checked === undefined ? el.getAttribute('aria-checked') === 'true' : el.checked;
      var want = value === '' ? (el.type === 'radio' ? true : !now) : /^(true|1|on|yes)$/i.test(value);
      if (now !== want) el.click();
      return 'ok';
    }
    if (action === 'press') {
      el.focus();
      var opts = {key: value, code: value, bubbles: true, cancelable: true};
      if (value === 'Enter') { opts.keyCode = 13; opts.which = 13; }
      var go = el.dispatchEvent(new KeyboardEvent('keydown', opts));
      el.dispatchEvent(new KeyboardEvent('keypress', opts));
      el.dispatchEvent(new KeyboardEvent('keyup', opts));
      if (go && value === 'Enter' && el.form) { if (el.form.requestSubmit) el.form.requestSubmit(); else el.form.submit(); }
      return 'ok';
    }
    return 'err:unknown action ' + action;
  } catch (e) { return 'err:' + e; }
})"#;

enum Msg {
    Cmd(Value),
    Eof,
    NavStarted,
    NavFinished,
    Ua(String),
    Probe(String),
    Acted(String),
    Tagged,
    Html(String),
}

#[derive(Default)]
struct Rules {
    allow: Vec<String>,
    deny: Vec<String>,
    blocked: Vec<String>,
    allow_private: bool,
    block_media: bool,
    /// Navigations refused since the last reply.
    refused: Vec<String>,
    /// host -> points into a private network (cached lookups).
    private_cache: HashMap<String, bool>,
    /// Sub-resource requests blocked since the last reply.
    resources_blocked: u32,
}

impl Rules {
    fn is_private(&mut self, url: &Url) -> bool {
        if self.allow_private {
            return false;
        }
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        if let Some(p) = self.private_cache.get(&host) {
            return *p;
        }
        let p = points_to_private_network(url);
        self.private_cache.insert(host, p);
        p
    }

    /// Top-level navigation gate. Mirrors the core's policy and blocklist.
    fn permits(&mut self, raw: &str) -> bool {
        let ok = match Url::parse(raw) {
            Ok(url) => match url.scheme() {
                "about" => true,
                "http" | "https" => {
                    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
                    !self.blocked.iter().any(|b| host_matches(&host, b))
                        && !self.deny.iter().any(|d| host_matches(&host, d))
                        && (self.allow.is_empty() || self.allow.iter().any(|a| host_matches(&host, a)))
                        && !self.is_private(&url)
                }
                _ => false,
            },
            Err(_) => false,
        };
        if !ok {
            self.refused.push(raw.chars().take(200).collect());
        }
        ok
    }

    /// Should this sub-resource request be refused? Applies only when
    /// `block_media` is on: media and fonts are skipped, and so is anything
    /// from a blocklisted or denied host (trackers, ads).
    fn blocks_resource(&mut self, kind: ResourceKind, raw: &str) -> bool {
        if !self.block_media {
            return false;
        }
        let block = kind == ResourceKind::Media
            || match Url::parse(raw) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => {
                    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
                    self.blocked.iter().any(|b| host_matches(&host, b)) || self.deny.iter().any(|d| host_matches(&host, d))
                }
                _ => false,
            };
        if block {
            self.resources_blocked += 1;
        }
        block
    }
}

/// Refuse the sub-resource requests the rules do not want loaded.
fn install_resource_filter(webview: &WebView, rules: Arc<Mutex<Rules>>) -> Result<(), String> {
    install_request_filter(webview, move |kind, url| {
        rules.lock().map(|mut r| r.blocks_resource(kind, url)).unwrap_or(false)
    })
}

enum Phase {
    /// Waiting to learn the WebView's own user agent.
    Boot,
    Idle,
    Loading,
    Acting,
    Settling { last: String, stable: u32, since: Instant },
    Tagging,
    Dumping,
}

struct Job {
    started: Instant,
    deadline: Instant,
    timed_out: bool,
}

fn reply(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_ascii_lowercase())).collect())
        .unwrap_or_default()
}

fn eval(webview: &WebView, js: &str, proxy: &EventLoopProxy<Msg>, wrap: fn(String) -> Msg) {
    let p = proxy.clone();
    let _ = webview.evaluate_script_with_callback(js, move |json| {
        let _ = p.send_event(wrap(json));
    });
}

fn main() {
    let mut data_dir = std::env::temp_dir().join("jj1-view");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir" => match args.next() {
                Some(d) => data_dir = PathBuf::from(d),
                None => {
                    eprintln!("jj1-view: --data-dir needs a path");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("jj1-view: unexpected argument {other}. This program is started by jj1.");
                std::process::exit(2);
            }
        }
    }

    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let window = match WindowBuilder::new()
        .with_title("jj1-view")
        .with_visible(false)
        .with_inner_size(LogicalSize::new(1280.0, 900.0))
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            reply(json!({"ok": false, "error": format!("could not create a window: {e}")}));
            std::process::exit(1);
        }
    };

    // Commands arrive on stdin; a reader thread forwards them to the loop.
    let stdin_proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => {
                    let _ = stdin_proxy.send_event(Msg::Cmd(v));
                }
                Err(e) => reply(json!({"ok": false, "error": format!("bad command: {e}")})),
            }
        }
        let _ = stdin_proxy.send_event(Msg::Eof);
    });

    let mut context = WebContext::new(Some(data_dir));
    let rules = Arc::new(Mutex::new(Rules::default()));
    let proxy = event_loop.create_proxy();

    // First a throwaway view on about:blank, only to read the default user
    // agent, so the real view can append an honest JJ1 token to it.
    let mut webview: Option<WebView> = match WebViewBuilder::new_with_web_context(&mut context)
        .with_url("about:blank")
        .build(&window)
    {
        Ok(w) => Some(w),
        Err(e) => {
            reply(json!({"ok": false, "error": format!("could not start the system WebView: {e}")}));
            std::process::exit(1);
        }
    };

    let mut phase = Phase::Boot;
    let mut job: Option<Job> = None;
    let mut queue: VecDeque<Value> = VecDeque::new();
    let mut nav_in_flight = false;
    let boot_started = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);
        let _ = &window;

        let finish = |phase: &mut Phase, job: &mut Option<Job>, v: Value| {
            reply(v);
            *phase = Phase::Idle;
            *job = None;
        };

        match event {
            Event::UserEvent(Msg::Eof) => *control_flow = ControlFlow::Exit,
            Event::UserEvent(Msg::Cmd(v)) => queue.push_back(v),
            Event::UserEvent(Msg::NavStarted) => {
                nav_in_flight = true;
                if let Phase::Settling { stable, .. } = &mut phase {
                    *stable = 0;
                }
            }
            Event::UserEvent(Msg::NavFinished) => {
                nav_in_flight = false;
                if matches!(phase, Phase::Loading) {
                    phase = Phase::Settling { last: String::new(), stable: 0, since: Instant::now() };
                }
            }
            Event::UserEvent(Msg::Ua(json)) => {
                if matches!(phase, Phase::Boot) {
                    let base: String = serde_json::from_str(&json).unwrap_or_default();
                    let ua = format!("{} JJ1/{} (AI-agent browser)", base.trim(), env!("CARGO_PKG_VERSION"));
                    webview = None; // drop the throwaway view first
                    let nav_rules = rules.clone();
                    let load_proxy = proxy.clone();
                    let mut builder = WebViewBuilder::new_with_web_context(&mut context)
                        .with_url("about:blank")
                        .with_initialization_script(INIT_JS)
                        .with_navigation_handler(move |url| nav_rules.lock().map(|mut r| r.permits(&url)).unwrap_or(false))
                        // Popups and downloads are never allowed in the worker.
                        .with_new_window_req_handler(|_url, _features| NewWindowResponse::Deny)
                        .with_download_started_handler(|_url, _path| false)
                        .with_on_page_load_handler(move |ev, _url| {
                            let _ = load_proxy.send_event(match ev {
                                PageLoadEvent::Started => Msg::NavStarted,
                                PageLoadEvent::Finished => Msg::NavFinished,
                            });
                        });
                    if !base.trim().is_empty() {
                        builder = builder.with_user_agent(ua);
                    }
                    match builder.build(&window) {
                        Ok(w) => {
                            if let Err(e) = install_resource_filter(&w, rules.clone()) {
                                eprintln!("jj1-view: no sub-resource filtering: {e}");
                            }
                            webview = Some(w);
                            phase = Phase::Idle;
                        }
                        Err(e) => {
                            reply(json!({"ok": false, "error": format!("could not start the system WebView: {e}")}));
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                }
            }
            Event::UserEvent(Msg::Acted(json)) => {
                if matches!(phase, Phase::Acting) {
                    let res: String = serde_json::from_str(&json).unwrap_or_else(|_| "err:the page did not answer".into());
                    match res.strip_prefix("err:") {
                        Some(e) => finish(&mut phase, &mut job, json!({"ok": false, "error": e})),
                        None => phase = Phase::Settling { last: String::new(), stable: 0, since: Instant::now() },
                    }
                }
            }
            Event::UserEvent(Msg::Probe(sample)) => {
                let mut settled = false;
                if let Phase::Settling { last, stable, since } = &mut phase {
                    if !nav_in_flight {
                        // sample is a JSON string: "complete:12345:0"
                        let body = sample.trim_matches('"');
                        let (dom, pending) = body.rsplit_once(':').unwrap_or((body, "0"));
                        if dom.starts_with("complete") && dom == last.as_str() {
                            *stable += 1;
                        } else {
                            *stable = 0;
                            *last = dom.to_string();
                        }
                        let quiet = pending == "0";
                        settled = since.elapsed() >= MIN_SETTLE
                            && ((quiet && *stable >= STABLE_TICKS) || *stable >= STABLE_TICKS_BUSY);
                    }
                }
                if settled {
                    if let Some(w) = &webview {
                        phase = Phase::Tagging;
                        eval(w, TAG_JS, &proxy, |_| Msg::Tagged);
                    }
                }
            }
            Event::UserEvent(Msg::Tagged) => {
                if matches!(phase, Phase::Tagging) {
                    if let Some(w) = &webview {
                        phase = Phase::Dumping;
                        eval(w, "document.documentElement.outerHTML", &proxy, Msg::Html);
                    }
                }
            }
            Event::UserEvent(Msg::Html(json)) => {
                if matches!(phase, Phase::Dumping) {
                    let html: String = serde_json::from_str(&json).unwrap_or_default();
                    let url = webview.as_ref().and_then(|w| w.url().ok()).unwrap_or_default();
                    let (refused, resources_blocked) = rules
                        .lock()
                        .map(|mut r| (std::mem::take(&mut r.refused), std::mem::take(&mut r.resources_blocked)))
                        .unwrap_or_default();
                    let (elapsed, timed_out) = job
                        .as_ref()
                        .map(|j| (j.started.elapsed().as_millis() as u64, j.timed_out))
                        .unwrap_or((0, false));
                    finish(
                        &mut phase,
                        &mut job,
                        json!({"ok": true, "url": url, "html": html, "elapsed_ms": elapsed, "timed_out": timed_out, "blocked": refused, "resources_blocked": resources_blocked}),
                    );
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) | Event::NewEvents(StartCause::Poll) => {
                let Some(w) = &webview else { return };
                match &phase {
                    Phase::Boot => {
                        if boot_started.elapsed() > Duration::from_secs(20) {
                            reply(json!({"ok": false, "error": "the system WebView did not start within 20 s"}));
                            *control_flow = ControlFlow::Exit;
                        } else {
                            eval(w, "navigator.userAgent", &proxy, Msg::Ua);
                        }
                    }
                    Phase::Idle => {
                        let Some(cmd) = queue.pop_front() else { return };
                        let timeout = cmd.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS);
                        let now = Instant::now();
                        let new_job = Job { started: now, deadline: now + Duration::from_millis(timeout), timed_out: false };
                        match cmd.get("op").and_then(Value::as_str).unwrap_or("") {
                            "quit" => *control_flow = ControlFlow::Exit,
                            "load" => {
                                let url = cmd.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                                let allowed = {
                                    let mut r = rules.lock().unwrap();
                                    r.allow = strings(&cmd, "allow");
                                    r.deny = strings(&cmd, "deny");
                                    r.blocked = strings(&cmd, "blocked");
                                    r.allow_private = cmd.get("allow_private").and_then(Value::as_bool).unwrap_or(false);
                                    r.block_media = cmd.get("block_media").and_then(Value::as_bool).unwrap_or(true);
                                    r.private_cache.clear();
                                    r.refused.clear();
                                    r.permits(&url)
                                };
                                if !allowed {
                                    reply(json!({"ok": false, "error": format!("navigation to {url} is not permitted by the policy")}));
                                } else if let Err(e) = w.load_url(&url) {
                                    reply(json!({"ok": false, "error": format!("could not load {url}: {e}")}));
                                } else {
                                    nav_in_flight = true;
                                    job = Some(new_job);
                                    phase = Phase::Loading;
                                }
                            }
                            "act" => {
                                let node = cmd.get("node").and_then(Value::as_u64).unwrap_or(0);
                                let action = cmd.get("action").and_then(Value::as_str).unwrap_or("");
                                let value = cmd.get("value").and_then(Value::as_str).unwrap_or("");
                                let call = format!("{ACT_JS}({node}, {}, {})", json!(action), json!(value));
                                job = Some(new_job);
                                phase = Phase::Acting;
                                eval(w, &call, &proxy, Msg::Acted);
                            }
                            "dump" => {
                                job = Some(new_job);
                                phase = Phase::Tagging;
                                eval(w, TAG_JS, &proxy, |_| Msg::Tagged);
                            }
                            other => reply(json!({"ok": false, "error": format!("unknown op \"{other}\"")})),
                        }
                    }
                    Phase::Loading | Phase::Settling { .. } => {
                        let late = job.as_ref().is_some_and(|j| Instant::now() > j.deadline);
                        if late {
                            // Out of time: return whatever is there, marked as such.
                            if let Some(j) = &mut job {
                                j.timed_out = true;
                            }
                            phase = Phase::Tagging;
                            eval(w, TAG_JS, &proxy, |_| Msg::Tagged);
                        } else if matches!(phase, Phase::Settling { .. }) {
                            eval(w, PROBE_JS, &proxy, Msg::Probe);
                        }
                    }
                    Phase::Acting | Phase::Tagging | Phase::Dumping => {
                        if job.as_ref().is_some_and(|j| Instant::now() > j.deadline + GRACE) {
                            finish(&mut phase, &mut job, json!({"ok": false, "error": "the page stopped responding"}));
                        }
                    }
                }
            }
            _ => {}
        }
    });
}
