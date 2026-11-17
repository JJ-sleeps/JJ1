//! `jj1` - a lightweight, AI-agent-native browser.            println!("{status} {}", tab.title());
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jj1_core::{Config, Session};
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
    /// Show the config file location and the effective settings
    Config {
        /// Write a config file with the default settings if none exists
        #[arg(long)]
        init: bool,
    },
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
