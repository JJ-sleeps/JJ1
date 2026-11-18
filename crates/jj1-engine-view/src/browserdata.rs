//! Persistent data for `jj1-browser`: settings, browsing history, and saved
//! Deuri chats, all under `%APPDATA%\jj1`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const HISTORY_CAP: usize = 5000;
const CHATS_DIR: &str = "chats";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_json(path: &Path, v: &Value) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_string_pretty(v).unwrap_or_default());
}

fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}\u{2026}", &s[..i]),
        None => s.to_string(),
    }
}

// ----------------------------------------------------------- settings --

/// Browser preferences (`browser.json`).
#[derive(Debug, Clone)]
pub struct Settings {
    /// "duckduckgo" | "google" | "bing" | "brave".
    pub search_engine: String,
    /// "beach" (the photo start page), "blank", or an http(s) URL.
    pub homepage: String,
    pub block_trackers: bool,
    pub save_history: bool,
    /// "light" or "dark".
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            search_engine: "duckduckgo".into(),
            homepage: "beach".into(),
            block_trackers: true,
            save_history: true,
            theme: "light".into(),
        }
    }
}

/// The search URL prefix for an engine key.
pub fn search_base(engine: &str) -> &'static str {
    match engine {
        "google" => "https://www.google.com/search?q=",
        "bing" => "https://www.bing.com/search?q=",
        "brave" => "https://search.brave.com/search?q=",
        _ => "https://duckduckgo.com/?q=",
    }
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("browser.json")
    }

    pub fn load(dir: &Path) -> Settings {
        let mut s = Settings::default();
        if let Some(v) = read_json(&Self::path(dir)) {
            if let Some(e) = v["search_engine"].as_str() {
                s.search_engine = e.to_string();
            }
            if let Some(h) = v["homepage"].as_str() {
                s.homepage = h.to_string();
            }
            s.block_trackers = v["block_trackers"].as_bool().unwrap_or(s.block_trackers);
            s.save_history = v["save_history"].as_bool().unwrap_or(s.save_history);
            if let Some(t) = v["theme"].as_str() {
                s.theme = if t == "dark" { "dark".into() } else { "light".into() };
            }
        }
        s
    }

    pub fn is_dark(&self) -> bool {
        self.theme == "dark"
    }

    pub fn save(&self, dir: &Path) {
        write_json(&Self::path(dir), &self.to_json());
    }

    pub fn to_json(&self) -> Value {
        json!({
            "search_engine": self.search_engine,
            "homepage": self.homepage,
            "block_trackers": self.block_trackers,
            "save_history": self.save_history,
            "theme": self.theme,
        })
    }

    pub fn search_base(&self) -> &'static str {
        search_base(&self.search_engine)
    }
}

// --------------------------------------------------------- history --

fn history_path(dir: &Path) -> PathBuf {
    dir.join("history.json")
}

/// Record a page visit (newest last in the file). Skips built-in pages and
/// collapses an immediate repeat of the same URL. Trimmed to `HISTORY_CAP`.
pub fn record_visit(dir: &Path, url: &str, title: &str) {
    if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let path = history_path(dir);
    let mut visits = read_json(&path).and_then(|v| v["visits"].as_array().cloned()).unwrap_or_default();
    if visits.last().and_then(|v| v["url"].as_str()) == Some(url) {
        // Same page reloaded/updated: just refresh the title and time.
        if let Some(last) = visits.last_mut() {
            last["title"] = json!(clip(title, 300));
            last["ts"] = json!(now());
        }
    } else {
        visits.push(json!({"url": url, "title": clip(title, 300), "ts": now()}));
    }
    if visits.len() > HISTORY_CAP {
        let cut = visits.len() - HISTORY_CAP;
        visits.drain(0..cut);
    }
    write_json(&path, &json!({"visits": visits}));
}

/// All visits, newest first.
pub fn load_history(dir: &Path) -> Vec<Value> {
    let mut visits = read_json(&history_path(dir)).and_then(|v| v["visits"].as_array().cloned()).unwrap_or_default();
    visits.reverse();
    visits
}

pub fn clear_history(dir: &Path) {
    let _ = std::fs::remove_file(history_path(dir));
}

// ----------------------------------------------------------- chats --

fn chats_dir(dir: &Path) -> PathBuf {
    dir.join(CHATS_DIR)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A fresh chat id from the current time.
pub fn new_chat_id() -> String {
    format!("chat-{}", now())
}

/// Title for a saved chat: its first user message, clipped.
fn chat_title(messages: &[Value]) -> String {
    messages
        .iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .map(|s| clip(s.trim(), 60))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Chat".into())
}

/// Save (or overwrite) a chat. Chats with no user message are not saved.
pub fn save_chat(dir: &Path, id: &str, messages: &[Value]) {
    if !valid_id(id) || !messages.iter().any(|m| m["role"] == "user") {
        return;
    }
    let started = read_json(&chats_dir(dir).join(format!("{id}.json")))
        .and_then(|v| v["started"].as_u64())
        .unwrap_or_else(now);
    let doc = json!({
        "id": id,
        "started": started,
        "updated": now(),
        "title": chat_title(messages),
        "messages": messages,
    });
    write_json(&chats_dir(dir).join(format!("{id}.json")), &doc);
}

/// Saved chats as `{id, title, updated}`, most recent first.
pub fn list_chats(dir: &Path) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(chats_dir(dir)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(v) = read_json(&path) {
                out.push(json!({
                    "id": v["id"].as_str().unwrap_or(""),
                    "title": v["title"].as_str().unwrap_or("Chat"),
                    "updated": v["updated"].as_u64().unwrap_or(0),
                }));
            }
        }
    }
    out.sort_by(|a, b| b["updated"].as_u64().unwrap_or(0).cmp(&a["updated"].as_u64().unwrap_or(0)));
    out
}

/// The messages of a saved chat.
pub fn load_chat(dir: &Path, id: &str) -> Option<Vec<Value>> {
    if !valid_id(id) {
        return None;
    }
    read_json(&chats_dir(dir).join(format!("{id}.json"))).and_then(|v| v["messages"].as_array().cloned())
}

pub fn delete_chat(dir: &Path, id: &str) {
    if valid_id(id) {
        let _ = std::fs::remove_file(chats_dir(dir).join(format!("{id}.json")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "jj1-bd-{}-{}-{}",
            std::process::id(),
            now(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn settings_roundtrip() {
        let d = tmp();
        let mut s = Settings::default();
        assert_eq!(s.search_base(), "https://duckduckgo.com/?q=");
        s.search_engine = "google".into();
        s.block_trackers = false;
        s.save(&d);
        let r = Settings::load(&d);
        assert_eq!(r.search_engine, "google");
        assert!(!r.block_trackers);
        assert_eq!(r.search_base(), "https://www.google.com/search?q=");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn history_record_and_clear() {
        let d = tmp();
        record_visit(&d, "https://a.test/", "A");
        record_visit(&d, "https://a.test/", "A again"); // collapses
        record_visit(&d, "https://b.test/", "B");
        record_visit(&d, "jj1://start", "skip");
        let h = load_history(&d);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0]["url"], "https://b.test/"); // newest first
        assert_eq!(h[1]["title"], "A again");
        clear_history(&d);
        assert_eq!(load_history(&d).len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn chats_save_list_load_delete() {
        let d = tmp();
        let msgs = vec![
            json!({"role": "system", "content": "s"}),
            json!({"role": "user", "content": "hello there friend"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        let id = "chat-100";
        save_chat(&d, id, &msgs);
        let list = list_chats(&d);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], "chat-100");
        assert_eq!(list[0]["title"], "hello there friend");
        assert_eq!(load_chat(&d, id).unwrap().len(), 3);
        // A chat with no user message is not saved.
        save_chat(&d, "chat-200", &[json!({"role": "system", "content": "s"})]);
        assert_eq!(list_chats(&d).len(), 1);
        delete_chat(&d, id);
        assert_eq!(list_chats(&d).len(), 0);
        assert!(load_chat(&d, "../etc/passwd").is_none()); // id validation
        let _ = std::fs::remove_dir_all(&d);
    }
}
