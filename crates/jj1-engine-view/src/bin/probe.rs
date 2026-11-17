//! jj1-view-probe: load one page in the system WebView with no visible
//! window, wait for the DOM to settle, and dump the rendered HTML.
//!
//!   jj1-view-probe <url> [--out dom.html] [--timeout-secs 30] [--data-dir DIR]
//!
//! Prints one JSON line with timings to stdout. Used by the M3 benchmark to
//! measure what a Tier 2 page costs; it is not the final adapter.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebContext, WebViewBuilder};

/// How often the DOM size is sampled.
const POLL: Duration = Duration::from_millis(250);
/// The DOM counts as settled after this many identical samples in a row.
const STABLE_POLLS: u32 = 4;

enum Msg {
    Loaded,
    Len(usize),
    Html(String),
}

struct Args {
    url: String,
    out: Option<PathBuf>,
    timeout: Duration,
    data_dir: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut url = None;
    let mut out = None;
    let mut timeout = 30u64;
    let mut data_dir = std::env::temp_dir().join("jj1-view-probe");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "--timeout-secs" => {
                timeout = it.next().and_then(|v| v.parse().ok()).ok_or("--timeout-secs needs a number")?
            }
            "--data-dir" => data_dir = PathBuf::from(it.next().ok_or("--data-dir needs a path")?),
            _ if url.is_none() => url = Some(a),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    Ok(Args {
        url: url.ok_or("usage: jj1-view-probe <url> [--out FILE] [--timeout-secs N] [--data-dir DIR]")?,
        out,
        timeout: Duration::from_secs(timeout),
        data_dir,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("jj1-view-probe: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        eprintln!("jj1-view-probe: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("jj1-view-probe")
        .with_visible(false)
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
        .map_err(|e| format!("creating window: {e}"))?;

    let mut context = WebContext::new(Some(args.data_dir.clone()));
    let load_proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(args.url.clone())
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(Msg::Loaded);
            }
        })
        .build(&window)
        .map_err(|e| format!("creating webview: {e}"))?;

    let proxy = event_loop.create_proxy();
    let start = Instant::now();
    let mut loaded_at: Option<Duration> = None;
    let mut last_len = 0usize;
    let mut stable = 0u32;
    let mut dump_requested = false;
    let mut timed_out = false;

    event_loop.run(move |event, _, control_flow| {
        // Keep the window and context alive for the life of the loop.
        let _ = (&window, &context);
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);

        let request_dump = |dump_requested: &mut bool| {
            if *dump_requested {
                return;
            }
            *dump_requested = true;
            let p = proxy.clone();
            let _ = webview.evaluate_script_with_callback("document.documentElement.outerHTML", move |json| {
                let _ = p.send_event(Msg::Html(json));
            });
        };

        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) | Event::NewEvents(StartCause::Poll) => {
                if start.elapsed() > args.timeout {
                    timed_out = true;
                    request_dump(&mut dump_requested);
                    // If even the dump never answers, give up shortly after.
                    if start.elapsed() > args.timeout + Duration::from_secs(5) {
                        eprintln!("jj1-view-probe: timed out without a response from the page");
                        std::process::exit(1);
                    }
                } else if loaded_at.is_some() && !dump_requested {
                    let p = proxy.clone();
                    let _ = webview.evaluate_script_with_callback(
                        "document.documentElement.outerHTML.length",
                        move |json| {
                            let _ = p.send_event(Msg::Len(json.trim().parse().unwrap_or(0)));
                        },
                    );
                }
            }
            Event::UserEvent(Msg::Loaded) => {
                loaded_at.get_or_insert(start.elapsed());
            }
            Event::UserEvent(Msg::Len(n)) => {
                if n == last_len && n > 0 {
                    stable += 1;
                } else {
                    stable = 0;
                    last_len = n;
                }
                if stable >= STABLE_POLLS {
                    request_dump(&mut dump_requested);
                }
            }
            Event::UserEvent(Msg::Html(json)) => {
                let html: String = serde_json::from_str(&json).unwrap_or_default();
                if let Some(path) = &args.out {
                    if let Err(e) = std::fs::write(path, &html) {
                        eprintln!("jj1-view-probe: {}: {e}", path.display());
                    }
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "url": args.url,
                        "final_url": webview.url().unwrap_or_default(),
                        "load_ms": loaded_at.map(|d| d.as_millis() as u64),
                        "total_ms": start.elapsed().as_millis() as u64,
                        "html_chars": html.chars().count(),
                        "timed_out": timed_out,
                    })
                );
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
