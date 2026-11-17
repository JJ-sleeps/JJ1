//! The engine seam: anything that can render a JavaScript page and act on it.
//!
//! Today there is one implementation, `ViewWorker`, which drives the
//! `jj1-view` process (system WebView). The trait exists so another engine
//! (for example Lightpanda on Linux) can be added without touching the session.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::Res;

/// A rendered page: the DOM after scripts ran, annotated by the engine.
pub struct Rendered {
    /// URL the page ended up on.
    pub url: String,
    pub html: String,
    pub elapsed_ms: u64,
    /// The page did not settle in time; `html` is whatever was there.
    pub timed_out: bool,
    /// Navigations the engine refused because of the rules.
    pub refused: Vec<String>,
}

/// Host rules the engine enforces on every top-level navigation.
#[derive(Debug, Clone, Default)]
pub struct NavRules {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub blocked: Vec<String>,
}

pub trait Engine {
    fn load(&mut self, url: &str, rules: &NavRules, timeout_ms: u64) -> Res<Rendered>;
    /// Act on the element tagged `node` in the last rendered page.
    fn act(&mut self, node: u32, action: &str, value: &str, timeout_ms: u64) -> Res<Rendered>;
    /// Re-read the current page without acting.
    fn dump(&mut self) -> Res<Rendered>;
}

/// Client for one `jj1-view` worker process (one live page).
pub struct ViewWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ViewWorker {
    /// `JJ1_VIEW_WORKER` if set, otherwise `jj1-view` next to this executable.
    pub fn locate() -> Res<PathBuf> {
        if let Some(p) = std::env::var_os("JJ1_VIEW_WORKER") {
            return Ok(PathBuf::from(p));
        }
        let name = if cfg!(windows) { "jj1-view.exe" } else { "jj1-view" };
        let path = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join(name)))
            .ok_or("cannot find this program's own folder")?;
        if path.exists() {
            Ok(path)
        } else {
            Err(format!(
                "the WebView worker ({name}) is not installed next to jj1 (looked for {}). Build it with `cargo build --release -p jj1-engine-view`.",
                path.display()
            ))
        }
    }

    pub fn spawn(data_dir: &Path) -> Res<ViewWorker> {
        let exe = Self::locate()?;
        let _ = std::fs::create_dir_all(data_dir);
        let mut cmd = Command::new(&exe);
        cmd.arg("--data-dir").arg(data_dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
        let stdin = child.stdin.take().ok_or("worker has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("worker has no stdout")?);
        Ok(ViewWorker { child, stdin, stdout })
    }

    fn request(&mut self, cmd: Value) -> Res<Rendered> {
        writeln!(self.stdin, "{cmd}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("the WebView worker is gone: {e}"))?;
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).map_err(|e| format!("reading from the WebView worker: {e}"))?;
        if n == 0 {
            return Err("the WebView worker exited unexpectedly (is the system WebView runtime installed?)".into());
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from the WebView worker: {e}"))?;
        if v["ok"].as_bool() != Some(true) {
            return Err(v["error"].as_str().unwrap_or("the WebView worker failed").to_string());
        }
        Ok(Rendered {
            url: v["url"].as_str().unwrap_or("").to_string(),
            html: v["html"].as_str().unwrap_or("").to_string(),
            elapsed_ms: v["elapsed_ms"].as_u64().unwrap_or(0),
            timed_out: v["timed_out"].as_bool().unwrap_or(false),
            refused: v["blocked"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        })
    }
}

impl Engine for ViewWorker {
    fn load(&mut self, url: &str, rules: &NavRules, timeout_ms: u64) -> Res<Rendered> {
        self.request(json!({
            "op": "load", "url": url, "timeout_ms": timeout_ms,
            "allow": rules.allow, "deny": rules.deny, "blocked": rules.blocked,
        }))
    }

    fn act(&mut self, node: u32, action: &str, value: &str, timeout_ms: u64) -> Res<Rendered> {
        self.request(json!({"op": "act", "node": node, "action": action, "value": value, "timeout_ms": timeout_ms}))
    }

    fn dump(&mut self) -> Res<Rendered> {
        self.request(json!({"op": "dump"}))
    }
}

impl Drop for ViewWorker {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"op": "quit"}));
        let _ = self.stdin.flush();
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
