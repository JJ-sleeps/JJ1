//! Configuration file (`config.toml`) and where it lives.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Actions that can be put on the confirm list.
pub const CONFIRMABLE: &[&str] = &["submit", "submit_get", "cross_origin"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Leave empty for the honest default `JJ1/<version>`.
    pub user_agent: String,
    pub timeout_secs: u64,
    pub max_body_kb: usize,
    pub respect_robots: bool,
    /// Minimum delay between requests to the same host.
    pub min_interval_ms: u64,
    /// Extra blocked hosts on top of the built-in list.
    pub blocked_hosts: Vec<String>,
    /// Allow localhost and private-network addresses. Off by default so a web
    /// page cannot point the agent's browser at services inside your network.
    pub allow_private_network: bool,
    /// Stop the WebView behind a live tab after this many idle seconds to free
    /// memory (0 = never). The tab is reloaded the next time it is used.
    pub live_idle_secs: u64,
    /// In the WebView tier, do not download images, media or fonts, and block
    /// requests to blocklisted hosts.
    pub webview_block_media: bool,
    /// Append every agent action to `audit.log` next to the config file.
    pub audit_log: bool,
    /// With tier="auto", retry in the system WebView when the static tier
    /// finds a JavaScript app shell.
    pub js_escalation: bool,
    /// Keep WebView cookies and storage between sessions. Off by default: each
    /// session gets a fresh profile that is deleted when it ends.
    pub persistent_profile: bool,
    /// How long a WebView page may take to load and settle.
    pub webview_timeout_secs: u64,
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    /// If non-empty, only these domains (and subdomains) may be opened.
    pub allow: Vec<String>,
    /// These domains (and subdomains) are never opened. Wins over `allow`.
    pub deny: Vec<String>,
    /// Actions that need `confirm=true`: "submit" (non-GET forms),
    /// "submit_get" (GET forms), "cross_origin" (clicks leaving the site).
    pub confirm_actions: Vec<String>,
    /// Let the agent relax the policy through `jj1_set_policy`. Off by default.
    pub agent_can_loosen: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            user_agent: String::new(),
            timeout_secs: 20,
            max_body_kb: 4096,
            respect_robots: true,
            min_interval_ms: 200,
            blocked_hosts: Vec::new(),
            allow_private_network: false,
            live_idle_secs: 120,
            webview_block_media: true,
            audit_log: true,
            js_escalation: true,
            persistent_profile: false,
            webview_timeout_secs: 30,
            policy: PolicyConfig::default(),
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        PolicyConfig {
            allow: Vec::new(),
            deny: Vec::new(),
            confirm_actions: vec!["submit".to_string()],
            agent_can_loosen: false,
        }
    }
}

/// `%APPDATA%\jj1` on Windows, `$XDG_CONFIG_HOME/jj1` or `~/.config/jj1`
/// elsewhere. `JJ1_CONFIG_DIR` overrides both.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("JJ1_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("jj1");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("jj1");
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    PathBuf::from(home.unwrap_or_default()).join(".config").join("jj1")
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Load `config.toml`; a missing file means defaults.
    pub fn load() -> Result<Config, String> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}
