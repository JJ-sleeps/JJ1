//! `jj1` - a lightweight, AI-agent-native browser.            println!("{status} {}", tab.title());
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jj1_core::{BotAuth, Config, OsVault, Session, Vault};
use serde_json::json;

#[derive(Parser)]
#[command(name = "jj1", version, about = "A lightweight browser that AI agents drive directly")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Fetch a page and print its status and title (add -m / -a for content)
    Open {
        /// URL to fetch (or, with --file, the URL the file pretends to come from)
        url: String,
        /// auto (escalate to the WebView when the page needs JavaScript), static, or full
        #[arg(long, default_value = "auto")]
        tier: String,
        /// Read HTML from this local file instead of the network
        #[arg(long, value_name = "PATH")]
        file: Option<std::path::PathBuf>,
        /// Print the page as Markdown
        #[arg(short, long)]
        markdown: bool,
        /// Print the numbered action map
        #[arg(short, long)]
        actions: bool,
        /// Whole page instead of main content only
        #[arg(long)]
        all: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Print size, time, token and memory figures
        #[arg(long)]
        stats: bool,
    },
    /// Run as an MCP server on stdin/stdout (for Claude Code and other agents)
    Mcp,
    /// Open the JJ1 browser window (for a person)
    View {
        /// Address or search to open
        url: Option<String>,
        /// Also open the Agent view of the page
        #[arg(long)]
        agent_view: bool,
    },
    /// Score how well a page works for AI agents (and screen readers), with fixes
    Audit {
        /// URL to audit (or, with --file, the URL the file pretends to come from)
        url: String,
        /// static (what lightweight agents see), auto, or full (after JavaScript)
        #[arg(long, default_value = "static")]
        tier: String,
        /// Read HTML from this local file instead of the network
        #[arg(long, value_name = "PATH")]
        file: Option<std::path::PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Exit with an error when the score is below this (for CI)
        #[arg(long, value_name = "N")]
        min_score: Option<u32>,
    },
    /// Saved sign-ins for jj1_login (kept in Windows Credential Manager)
    Vault {
        #[command(subcommand)]
        cmd: VaultCmd,
    },
    /// Show this install's request-signing key (Web Bot Auth) and how to publish it
    Identity,
    /// Show the config file location and the effective settings
    Config {
        /// Write a config file with the default settings if none exists
        #[arg(long)]
        init: bool,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Save a sign-in for a site; the password is asked for and not shown
    Add {
        /// The site, e.g. example.com (also covers its subdomains)
        host: String,
        /// Username or email for that site
        username: String,
    },
    /// List saved sign-ins (sites and usernames only)
    List,
    /// Delete the saved sign-in for a site
    Remove { host: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jj1: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Config { init } => {
            let path = Config::path();
            if init && !path.exists() {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(&path, Config::default().to_toml()).map_err(|e| format!("{}: {e}", path.display()))?;
                println!("Wrote {}", path.display());
            }
            let cfg = Config::load()?;
            println!(
                "# {} ({})\n{}",
                path.display(),
                if path.exists() { "loaded" } else { "not present, showing defaults" },
                cfg.to_toml()
            );
            Ok(())
        }
        Cmd::View { url, agent_view } => {
            let name = if cfg!(windows) { "jj1-browser.exe" } else { "jj1-browser" };
            let exe = std::env::current_exe()
                .ok()
                .and_then(|me| me.parent().map(|dir| dir.join(name)))
                .filter(|p| p.exists())
                .ok_or_else(|| format!("{name} is not installed next to jj1. Build it with build.ps1."))?;
            let mut cmd = std::process::Command::new(&exe);
            if let Some(u) = url {
                cmd.arg(u);
            }
            if agent_view {
                cmd.arg("--agent-view");
            }
            cmd.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
            Ok(())
        }
        Cmd::Audit { url, tier, file, json, min_score } => {
            let mut session = Session::new(Config::load()?)?;
            let id = match file {
                Some(path) => {
                    let html = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    session.open_html(html, &url)?
                }
                None => session.open(&url, &tier)?,
            };
            let a = session.audit_tab(id)?;
            if json {
                let checks: Vec<_> = a
                    .checks
                    .iter()
                    .map(|c| {
                        json!({"id": c.id, "severity": c.severity.as_str(), "title": c.title, "advice": c.advice,
                               "count": c.count, "penalty": c.penalty, "examples": c.examples})
                    })
                    .collect();
                let out = json!({"url": a.url, "title": a.title, "score": a.score, "grade": a.grade, "actions": a.actions,
                                 "page_tokens": a.page_tokens, "html_tokens": a.html_tokens, "checks": checks});
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                print!("{}", a.report());
            }
            match min_score {
                Some(min) if a.score < min => Err(format!("score {} is below the minimum {min}", a.score)),
                _ => Ok(()),
            }
        }
        Cmd::Vault { cmd } => {
            let mut vault = OsVault;
            match cmd {
                VaultCmd::Add { host, username } => {
                    let host = jj1_core::normalize_host(&host);
                    let pw = jj1_core::vault::read_secret(&format!("Password for {username} on {host} (not shown): "))?;
                    if pw.is_empty() {
                        return Err("No password given; nothing saved.".into());
                    }
                    vault.save(&host, &username, &pw)?;
                    println!("Saved the sign-in for {username} on {host}. Agents can use it with jj1_login on {host} but never see it.");
                }
                VaultCmd::List => {
                    let list = vault.list()?;
                    if list.is_empty() {
                        println!("No saved sign-ins. Add one with: jj1 vault add <site> <username>");
                    }
                    for (host, user) in list {
                        println!("{host}\t{user}");
                    }
                }
                VaultCmd::Remove { host } => {
                    let host = jj1_core::normalize_host(&host);
                    if vault.remove(&host)? {
                        println!("Removed the sign-in for {host}.");
                    } else {
                        println!("No saved sign-in for {host}.");
                    }
                }
            }
            Ok(())
        }
        Cmd::Identity => {
            let cfg = Config::load()?;
            let path = jj1_core::bot_key_path();
            let existed = path.exists();
            let seed = jj1_core::botauth_seed(&path)?;
            let agent = cfg.bot_auth.signature_agent.trim().to_string();
            let auth = BotAuth::new(seed, Some(agent.clone()));
            println!("Signing key: {}{}", path.display(), if existed { "" } else { " (just created)" });
            println!("keyid: {}", auth.keyid());
            println!(
                "signing: {}",
                if cfg.bot_auth.enabled { "on" } else { "off (set [bot_auth] enabled = true in config.toml)" }
            );
            println!("\nPublish this JSON at <your site>/.well-known/http-message-signatures-directory");
            println!("(served as application/http-message-signatures-directory+json):\n");
            println!("{}", auth.directory_json());
            println!(
                "\nThen set in {}:\n\n[bot_auth]\nenabled = true\nsignature_agent = \"{}\"",
                Config::path().display(),
                if agent.is_empty() { "https://your-site.example" } else { &agent }
            );
            Ok(())
        }
        Cmd::Mcp => {
            let session = Session::new(Config::load()?)?;
            jj1_mcp::serve(session).map_err(|e| e.to_string())
        }
        Cmd::Open { url, tier, file, markdown, actions, all, json, stats } => {
            let mut session = Session::new(Config::load()?)?;
            let id = match file {
                Some(path) => {
                    let html = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    session.open_html(html, &url)?
                }
                None => session.open(&url, &tier)?,
            };
            let tab = session.tab(id)?;

            let md = if all { tab.whole_page_markdown() } else { tab.doc.markdown.clone() };
            let map = tab.action_map("full", usize::MAX, 0);
            let payload_tokens = jj1_core::estimate_tokens(&md) + jj1_core::estimate_tokens(&map);

            if json {
                let acts: Vec<_> = tab
                    .actions
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.id, "kind": a.raw.kind, "label": a.raw.label,
                            "name": a.raw.name, "href": a.raw.href, "line": tab.line(a),
                        })
                    })
                    .collect();
                let findings: Vec<_> = tab
                    .doc
                    .findings
                    .iter()
                    .map(|f| json!({"kind": f.kind, "detail": f.detail, "snippet": f.snippet}))
                    .collect();
                let out = json!({
                    "url": tab.url.as_str(), "status": tab.status, "title": tab.title(),
                    "tier": tab.tier, "bytes": tab.bytes, "time_ms": tab.elapsed_ms,
                    "tokens": payload_tokens, "raw_tokens": tab.raw_tokens,
                    "mcp_tokens": jj1_core::estimate_tokens(&session.report(id, 1500, 80)?),
                    "needs_js": tab.doc.needs_js && tab.tier == "static", "note": tab.note, "peak_rss_kb": peak_rss_kb(),
                    "markdown": md, "actions": acts, "findings": findings,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                return Ok(());
            }

            let status = if tab.status == 0 { "(rendered)".to_string() } else { tab.status.to_string() };
            println!("{status} {}", tab.title());
            println!("{}", tab.url);
            if let Some(note) = &tab.note {
                println!("note: {note}");
            } else if tab.doc.needs_js && tab.tier == "static" {
                println!("note: looks like a JavaScript app shell; the static tier is missing content (try --tier full)");
            }
            if markdown {
                println!("\n{md}");
            }
            if actions {
                println!("\n--- ACTIONS ---\n{map}");
            }
            if !tab.doc.findings.is_empty() {
                println!("\n--- INJECTION SCAN: {} finding(s) ---", tab.doc.findings.len());
                for f in &tab.doc.findings {
                    println!("! {} ({}): {}", f.kind, f.detail, f.snippet);
                }
            }
            if stats {
                let ratio = if payload_tokens > 0 { tab.raw_tokens as f64 / payload_tokens as f64 } else { 0.0 };
                println!(
                    "\n--- STATS ---\ntier: {}\nbytes: {}\ntime: {} ms\ntokens (markdown + actions): ~{}\ntokens (raw page): ~{}\nreduction: {:.1}x\nactions: {}\npeak RSS: {}",
                    tab.tier,
                    tab.bytes,
                    tab.elapsed_ms,
                    payload_tokens,
                    tab.raw_tokens,
                    ratio,
                    tab.actions.len(),
                    peak_rss_kb().map(|k| format!("{:.1} MB", k as f64 / 1024.0)).unwrap_or_else(|| "n/a".into()),
                );
            }
            Ok(())
        }
    }
}

/// Peak resident memory of this process, in KiB.
#[cfg(windows)]
fn peak_rss_kb() -> Option<u64> {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
    }
    let mut c = ProcessMemoryCounters::default();
    let size = std::mem::size_of::<ProcessMemoryCounters>() as u32;
    c.cb = size;
    // SAFETY: `c` is a correctly sized, writable PROCESS_MEMORY_COUNTERS and
    // the pseudo-handle from GetCurrentProcess is always valid.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, size) };
    if ok != 0 { Some(c.peak_working_set_size as u64 / 1024) } else { None }
}

#[cfg(not(windows))]
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}
