//! jj1-net: Tier 0 networking.
//!
//! One shared HTTP client (hyper + rustls via reqwest). JJ1 follows redirects
//! itself so that the blocklist, the caller's policy gate and robots.txt are
//! checked on every hop.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::thread::sleep;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION};
use url::Url;

mod robots;
pub use robots::Robots;

/// Default host blocklist, compiled in from `rules/blocklist.txt`.
pub const DEFAULT_BLOCKLIST: &str = include_str!("../../../rules/blocklist.txt");

const ROBOTS_AGENT: &str = "jj1";

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub user_agent: String,
    pub timeout_secs: u64,
    pub max_body_bytes: usize,
    pub max_redirects: usize,
    pub respect_robots: bool,
    /// Minimum delay between two requests to the same host.
    pub min_interval_ms: u64,
    /// Extra blocked hosts, on top of the built-in list.
    pub blocked_hosts: Vec<String>,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            user_agent: format!(
                "JJ1/{} (AI-agent browser; respects robots.txt)",
                env!("CARGO_PKG_VERSION")
            ),
            timeout_secs: 20,
            max_body_bytes: 4 * 1024 * 1024,
            max_redirects: 10,
            respect_robots: true,
            min_interval_ms: 200,
            blocked_hosts: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum NetError {
    Scheme(String),
    BlockedHost(String),
    Policy(String),
    Robots(String),
    Unsupported { url: String, content_type: String },
    TooManyRedirects(String),
    Http(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::Scheme(u) => write!(f, "Only http and https URLs can be opened (got {u})."),
            NetError::BlockedHost(h) => write!(
                f,
                "Host {h} is on the JJ1 blocklist (ads/trackers). Edit blocked_hosts or rules/blocklist.txt to change this."
            ),
            NetError::Policy(m) => write!(f, "Blocked by policy: {m}"),
            NetError::Robots(u) => write!(
                f,
                "robots.txt disallows {u} for JJ1. Set respect_robots = false in config.toml only if you have permission from the site."
            ),
            NetError::Unsupported { url, content_type } => write!(
                f,
                "{url} is '{content_type}', not a text page. The static tier does not download binary resources."
            ),
            NetError::TooManyRedirects(u) => write!(f, "Too many redirects while opening {u}."),
            NetError::Http(m) => write!(f, "Network error: {m}"),
        }
    }
}

impl std::error::Error for NetError {}

pub enum Request {
    Get,
    /// `application/x-www-form-urlencoded` POST.
    Post(Vec<(String, String)>),
}

#[derive(Debug, Clone)]
pub struct Page {
    /// Final URL after redirects.
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub is_html: bool,
    pub body: String,
    /// Decoded body size in bytes (after decompression).
    pub bytes: usize,
    pub truncated: bool,
    pub redirects: usize,
    pub elapsed_ms: u64,
}

pub struct Net {
    cfg: NetConfig,
    client: Client,
    blocked: Vec<String>,
    robots: HashMap<String, Robots>,
    last_hit: HashMap<String, Instant>,
}

pub fn host_matches(host: &str, entry: &str) -> bool {
    host == entry || (host.len() > entry.len() && host.ends_with(entry) && host[..host.len() - entry.len()].ends_with('.'))
}

impl Net {
    pub fn new(cfg: NetConfig) -> Result<Net, NetError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml;q=0.9,text/plain;q=0.8,application/json;q=0.7"),
        );
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en"));
        let client = Client::builder()
            .user_agent(cfg.user_agent.clone())
            .default_headers(headers)
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .cookie_store(true)
            .build()
            .map_err(|e| NetError::Http(chain(&e)))?;

        let mut blocked: Vec<String> = DEFAULT_BLOCKLIST
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_ascii_lowercase())
            .collect();
        blocked.extend(cfg.blocked_hosts.iter().map(|h| h.trim().to_ascii_lowercase()));

        Ok(Net { cfg, client, blocked, robots: HashMap::new(), last_hit: HashMap::new() })
    }

    pub fn config(&self) -> &NetConfig {
        &self.cfg
    }

    /// Fetch a text page. `gate` is called for every URL in the redirect chain
    /// and can veto it (domain policy).
    pub fn fetch(
        &mut self,
        url: &Url,
        mut req: Request,
        gate: &dyn Fn(&Url) -> Result<(), String>,
    ) -> Result<Page, NetError> {
        let start = Instant::now();
        let mut url = url.clone();
        for hop in 0..=self.cfg.max_redirects {
            self.precheck(&url, gate)?;
            self.throttle(&url);

            let builder = match &req {
                Request::Get => self.client.get(url.clone()),
                Request::Post(fields) => {
                    let body = url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                        .finish();
                    self.client
                        .post(url.clone())
                        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(body)
                }
            };
            let resp = builder.send().map_err(|e| NetError::Http(chain(&e)))?;
            let status = resp.status();

            if status.is_redirection() {
                if let Some(next) = location(&resp, &url) {
                    // 307/308 keep the method; everything else becomes GET.
                    if !matches!(status.as_u16(), 307 | 308) {
                        req = Request::Get;
                    }
                    url = next;
                    continue;
                }
            }
            return self.read_page(resp, url, start, hop);
        }
        Err(NetError::TooManyRedirects(url.to_string()))
    }

    /// Hosts on the blocklist (built-in list plus config).
    pub fn blocked_hosts(&self) -> &[String] {
        &self.blocked
    }

    /// Would JJ1 be allowed to open this URL? Scheme, blocklist, the caller's
    /// policy gate and robots.txt. Used for pages another engine will load.
    pub fn check(&mut self, url: &Url, gate: &dyn Fn(&Url) -> Result<(), String>) -> Result<(), NetError> {
        self.precheck(url, gate)
    }

    fn precheck(&mut self, url: &Url, gate: &dyn Fn(&Url) -> Result<(), String>) -> Result<(), NetError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(NetError::Scheme(url.to_string()));
        }
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        if self.blocked.iter().any(|b| host_matches(&host, b)) {
            return Err(NetError::BlockedHost(host));
        }
        gate(url).map_err(NetError::Policy)?;
        if self.cfg.respect_robots {
            let mut path = url.path().to_string();
            if let Some(q) = url.query() {
                path.push('?');
                path.push_str(q);
            }
            if !self.robots_for(url).allowed(&path) {
                return Err(NetError::Robots(url.to_string()));
            }
        }
        Ok(())
    }

    fn robots_for(&mut self, url: &Url) -> &Robots {
        let origin = url.origin().ascii_serialization();
        if !self.robots.contains_key(&origin) {
            let r = self.load_robots(url);
            self.robots.insert(origin.clone(), r);
        }
        &self.robots[&origin]
    }

    fn load_robots(&self, url: &Url) -> Robots {
        let Ok(mut u) = url.join("/robots.txt") else { return Robots::allow_all() };
        for _ in 0..4 {
            let Ok(resp) = self.client.get(u.clone()).timeout(Duration::from_secs(8)).send() else {
                return Robots::allow_all();
            };
            let status = resp.status();
            if status.is_redirection() {
                match location(&resp, &u) {
                    Some(next) => {
                        u = next;
                        continue;
                    }
                    None => return Robots::allow_all(),
                }
            }
            if !status.is_success() {
                return Robots::allow_all();
            }
            let mut buf = Vec::new();
            let _ = resp.take(512 * 1024).read_to_end(&mut buf);
            return Robots::parse(&String::from_utf8_lossy(&buf), ROBOTS_AGENT);
        }
        Robots::allow_all()
    }

    fn throttle(&mut self, url: &Url) {
        let host = url.host_str().unwrap_or("").to_string();
        let min = Duration::from_millis(self.cfg.min_interval_ms);
        if let Some(last) = self.last_hit.get(&host) {
            let since = last.elapsed();
            if since < min {
                sleep(min - since);
            }
        }
        self.last_hit.insert(host, Instant::now());
    }

    fn read_page(&self, resp: Response, url: Url, start: Instant, redirects: usize) -> Result<Page, NetError> {
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let ct = content_type.to_ascii_lowercase();
        let textual = ct.is_empty()
            || ct.starts_with("text/")
            || ct.contains("html")
            || ct.contains("json")
            || ct.contains("xml");
        if !textual {
            return Err(NetError::Unsupported { url: url.to_string(), content_type });
        }

        let max = self.cfg.max_body_bytes;
        let mut buf = Vec::new();
        resp.take(max as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| NetError::Http(format!("reading body of {url}: {e}")))?;
        let truncated = buf.len() > max;
        buf.truncate(max);

        let body = decode(&buf, &ct);
        let is_html = ct.contains("html")
            || (ct.is_empty() && body.trim_start().starts_with('<'));
        Ok(Page {
            url,
            status,
            content_type,
            is_html,
            bytes: body.len(),
            body,
            truncated,
            redirects,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

fn location(resp: &Response, base: &Url) -> Option<Url> {
    let loc = resp.headers().get(LOCATION)?.to_str().ok()?;
    base.join(loc).ok()
}

/// reqwest's Display omits the cause; walk the chain so errors are actionable.
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        let part = c.to_string();
        if !s.contains(&part) {
            s.push_str(": ");
            s.push_str(&part);
        }
        src = c.source();
    }
    s
}

fn decode(buf: &[u8], content_type: &str) -> String {
    let label = charset_in(content_type).or_else(|| {
        let head = String::from_utf8_lossy(&buf[..buf.len().min(2048)]).to_ascii_lowercase();
        charset_in(&head)
    });
    let enc = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    enc.decode(buf).0.into_owned()
}

fn charset_in(s: &str) -> Option<String> {
    let i = s.find("charset=")?;
    let rest = s[i + 8..].trim_start_matches(|c| c == '"' || c == '\'' || c == ' ');
    let label: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if label.is_empty() { None } else { Some(label) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_matching() {
        assert!(host_matches("doubleclick.net", "doubleclick.net"));
        assert!(host_matches("ads.doubleclick.net", "doubleclick.net"));
        assert!(!host_matches("notdoubleclick.net", "doubleclick.net"));
    }

    #[test]
    fn charset_detection() {
        assert_eq!(charset_in("text/html; charset=iso-8859-1").as_deref(), Some("iso-8859-1"));
        assert_eq!(charset_in("<meta charset=\"utf-8\">").as_deref(), Some("utf-8"));
        assert_eq!(charset_in("text/html"), None);
    }
}
