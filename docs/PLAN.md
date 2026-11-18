# JJ1 build plan (v1.1)

JJ1 is a small browser that AI agents drive over MCP. It fetches a page, never
paints pixels unless asked, and hands the agent a compact numbered **Action Map**
plus clean Markdown instead of raw HTML or a screenshot.

The original concept document is kept unchanged in
[JJ1_PLAN_v1.txt](JJ1_PLAN_v1.txt). This file is the working plan: what is
decided, what each milestone must prove, and where the project stands.

## Goals, in priority order

1. **Lightweight** - far less RAM/CPU than headless Chrome, proven by benchmark.
2. **Agent-native** - stable numeric element IDs, small diffed responses, MCP.
3. **Safe** - policy gate, confirmation before submits, prompt-injection scan.
4. **Always runnable** - one `jj1` binary that works at every milestone.

Not goals: a human daily-driver browser, a new JS or rendering engine, or any
CAPTCHA / paywall / bot-protection bypass.

## Decisions made for v1.1

| Topic | Decision | Why |
|---|---|---|
| Language | Rust, one binary `jj1` | Memory safety for hostile input |
| HTTP | `reqwest` (hyper + rustls underneath), blocking API | Same stack the v1 plan named, far less code to own. No OpenSSL. |
| Redirects | Followed by JJ1 itself, hop by hop | Policy, blocklist and robots.txt are checked on every hop, not just the first URL |
| HTML | `scraper` (html5ever) | Spec-compliant parser |
| Concurrency | Single-threaded session, synchronous | An MCP stdio server handles one call at a time; no async runtime to own |
| MCP | Hand-written JSON-RPC 2.0 over stdio | Six tools; no SDK dependency |
| Config | TOML at `%APPDATA%\jj1\config.toml` (Windows) or `~/.config/jj1/config.toml` | |
| Dev platform | Windows first (GNU toolchain), Linux next | The v1 plan said Linux first; this machine is Windows, and M0-M2 are portable. The OS sandbox (M5) still starts on Linux. |
| Tier 1 engine | Not chosen. Decide at M3 from benchmark numbers. | Options A (Lightpanda), B (QuickJS), C (skip to WebView) as in v1 |
| Element IDs | Assigned per tab from a signature (kind, name, href, label, ordinal) and kept across navigations | "click [2]" stays valid; diffs after navigation only show what changed |
| Untrusted content | Wrapped in delimiters carrying a per-session random nonce | A page cannot fake the closing delimiter |
| `jj1_set_policy` | Tighten-only from the agent unless `agent_can_loosen = true` in config | An injected agent must not be able to switch off its own guard rails |

## Milestones

| | Scope | Exit criterion | Status |
|---|---|---|---|
| M0 | Workspace, CLI, config, logging | `jj1 open <url>` prints status and title | see "Status" |
| M1 | Tier 0 fetch, request filter, robots.txt, Markdown, Action Map with stable IDs | `jj1 open <url> --actions` works on 20 static sites, under 25 MB RSS | see "Status" |
| M2 | stdio MCP server (`open`, `snapshot`, `read`, `act`, `tabs`, `set_policy`), benchmark harness | Connects to Claude Code; benchmark table with real numbers | see "Status" |
| M3 | JS tier, automatic escalation | Logs into and navigates a React/Next.js demo app | built on the system WebView; exit criterion partly met (see "Status") |
| M4 | Tier 2 system WebView, screenshots, `jj1 view` | Any page that fails in Tier 1 works in Tier 2 | WebView in place (used as the JS tier); screenshots and `jj1 view` not started |
| M5 | OS sandbox, encrypted session vault, fuzzing, red-team fixtures | All hidden-instruction fixtures flagged | not started |
| M6 | Session forking, tab suspension, packaging, docs | Release builds for Linux/macOS/Windows | not started |

M2 is the "is this valuable" gate: if the measured savings are not there,
the plan changes before any JS engine work starts.

## What M0-M2 contain

```
crates/
  jj1-net/      fetch, redirects, size cap, charset decode, robots.txt,
                host blocklist, per-host rate limit, cookie jar (in memory)
  jj1-distill/  HTML -> Markdown, HTML -> actions + forms, injection scan
  jj1-core/     config, policy, session, tabs, stable IDs, diffs, form submit
  jj1-mcp/      JSON-RPC stdio server and tool schemas
  jj1-cli/      `jj1 open | mcp | config`
rules/          default host blocklist (compiled into the binary)
fixtures/       saved test pages, including prompt-injection pages
bench/          benchmark script, URL corpus, results
```

Crates for later milestones (`jj1-engine-js`, `jj1-engine-view`,
`jj1-sandbox`) are added when their milestone starts, not as empty stubs.

### How acting works in Tier 0 (no JavaScript)

| Action | Effect |
|---|---|
| `click` on a link | GET navigation in the same tab |
| `click` on a submit button | Collects the form's fields and sends GET or POST |
| `click` / `check` on checkbox or radio | Changes the stored field state |
| `type`, `select` | Stores the value; it is sent when the form is submitted |
| `press` Enter in a form field | Submits the form |
| `scroll` | No-op (the whole page is already available) |

Every action returns only the diff of the Action Map (`+` added, `-` removed,
`~` changed).

### Safety defaults in M2

- Non-GET form submissions need `confirm=true`; the first call is refused with
  a message naming the target URL and field names.
- Domain allow/deny lists are enforced on every redirect hop.
- robots.txt is respected, the User-Agent is `JJ1/<version>`, and requests to
  one host are spaced out.
- Hidden text is stripped and reported; instruction-like phrases are flagged.
- Password values are never echoed back in the Action Map.

### Known limits of Tier 0 (stated, not hidden)

- No CSS engine: hidden-text detection sees inline styles, the `hidden`
  attribute, `aria-hidden` and common utility classes, not stylesheet rules.
  The policy layer is the real defence; the scanner is defence in depth.
- No JavaScript: app-shell pages are detected and reported as needing the JS
  tier, which does not exist until M3.
- Cookies live in memory for the life of the process. The encrypted vault is M5.
- No OS sandbox yet (M5). Do not point M2 at pages you would not open in curl.
- `multipart/form-data` forms (file uploads) are not supported.

## Benchmark method (M2)

Same machine, same URL corpus, JJ1 versus a headless Chromium-based browser.
Measured: peak working set summed over all processes, wall time, bytes
downloaded, and tokens sent to the model. Medians and worst cases are both
reported. Results go in `bench/results/`.

## Status

### 2026-11-17 - M0, M1, M2 built

| | Exit criterion | Result |
|---|---|---|
| M0 | `jj1 open <url>` prints status and title | Met |
| M1 | Works on 20 static sites, under 25 MB RSS | Met on 22 of 23 static pages in the benchmark corpus (10-18 MB). One 2 MB page (sqlite.org) peaked at 26.5 MB. |
| M2 | Connects to Claude Code; benchmark table with real numbers | MCP server passes a 20-check live end-to-end test (`bench/mcp-smoke.ps1`) speaking real JSON-RPC. Benchmark published in `bench/results/bench-2026-11-17.md`. Registered in Claude Code for this project folder (`claude mcp add jj1 -- <path>\jj1.exe mcp`); `claude mcp list` reports it connected. Not yet used for a real agent task. |

Measured against headless Chrome 157 on 30 pages (medians, pages that work
without JavaScript): 10.8 MB vs 612 MB summed working set / 260 MB private
(95%+ saved), 1.1 s vs 2.8 s, 2,306 vs 14,893 tokens (6.5x; 50-100x on large
pages). 3 of 30 pages need JavaScript and are useless in the static tier; 3
were refused by robots.txt.

Also in place: 18 unit tests, a prompt-injection fixture with 8 planted
attacks (all flagged, hidden link kept out of the action map), audit log.

### What the numbers say about the plan

- The M2 gate passes for static pages: the saving is real and large.
- The token hypothesis (10-50x) is only true for large pages. Median is 6.5x.
- 10% of the corpus needs JavaScript. M3 (JS tier) is the next thing that
  adds coverage; the benchmark must be rerun there, because that is where
  the memory saving will shrink.

### 2026-11-17 - M3 started: engine benchmark

Built `crates/jj1-engine-view` with `jj1-view-probe`: loads a page in the
system WebView (WebView2 here) with a hidden window, waits for the DOM to
settle, dumps the rendered HTML. Results: `bench/results/bench-js-2026-11-17.md`.

| On the 3 JS pages (medians of 3 cold runs) | Static tier | WebView2 | Headless Chrome |
|---|---:|---:|---:|
| Got the content | 0 of 3 | 3 of 3 | 3 of 3 |
| RAM, summed working set | 10 MB | 342-395 MB | 568-673 MB |
| RAM, private bytes | 10 MB | 201-270 MB | 229-282 MB |
| Time | 1.0 s | 3.2-4.1 s | 2.2-8.9 s |

What this means for the Tier 1 decision (section 4 of the v1 plan):

- **Option C (system WebView) works today on Windows** and the distiller runs
  unchanged on its output. But it is 20-40x heavier than the static tier and
  only 0-40% lighter than Chrome, so on JS pages the 70-85% goal is not met by
  this engine alone.
- **Option A (Lightpanda) could not be measured**: no native Windows build,
  and this machine has no WSL or Docker. It also means Lightpanda can never be
  the JS tier of a native Windows `jj1`; at best it is a Linux/macOS tier.
- JJ1's saving on JS pages therefore has to come from *not escalating*: 27 of
  30 corpus pages never need the WebView. Per-session cost with one reused
  WebView is not yet measured.

Recommendation (not yet decided by the user): build automatic escalation
Tier 0 -> system WebView next, behind an engine trait, and treat Lightpanda as
a later Linux/macOS adapter to be benchmarked under WSL or on a Linux box.

Packaging note: on the GNU toolchain the WebView build needs
`WebView2Loader.dll` beside the executable (copied by hand for now from the
`webview2-com-sys` build output).

### 2026-11-17 - M3 built: automatic escalation to the system WebView

Decision taken by the user: build escalation on the system WebView (option C)
now; Lightpanda stays a possible Linux/macOS engine for later.

What exists:

- `jj1-view` (crate `jj1-engine-view`): a worker process holding one live page
  in the system WebView with no visible window. `jj1` starts it on demand and
  talks JSON lines to it. One worker per live tab; closing the tab ends it.
- `Engine` trait in `jj1-core` (`load`, `act`, `dump`) with `ViewWorker` as the
  only implementation, so another engine can be slotted in.
- `tier="auto"` (the default): static first; if the result is a JavaScript app
  shell, the page is reloaded in the WebView and the response says so.
  `tier="full"` (or `"js"`) forces the WebView; `tier="static"` never runs scripts.
- Acting on live pages: click, type, select, check, press and scroll run in the
  real DOM. Script-driven controls (plain buttons, `role=button`, `onclick`)
  appear in the action map as `button`.
- Before each read the worker waits for the DOM to stop changing **and** for
  fetch/XHR requests to finish, then annotates the page: interactive elements
  get a handle, and elements a human cannot see are marked from their
  *computed* style. This closes the "no CSS engine" gap for live pages.
- Same guard rails as the static tier: policy, blocklist and robots.txt are
  checked before loading and again on wherever the page lands; the worker
  refuses top-level navigations to denied hosts, popups and downloads; POST
  form submits need `confirm=true`; passwords are not echoed.
- Honest user agent: the WebView's own string plus `JJ1/<version> (AI-agent browser)`.
- WebView cookies and storage live in a per-session profile that is deleted
  when the session ends (`persistent_profile = true` in config keeps them).

Verified with `bench/mcp-live.ps1` (15 checks, all passing, real sites):
escalation, clicking through a JS-rendered page, login form with confirmation,
policy on live pages, a single-page app (crates.io) opened with its data and
searched by typing + Enter, and workers ending when tabs close. The static
smoke test (20 checks) and 27 unit tests also pass.

Measured cost of one `jj1 mcp` process and everything under it:

| State | Processes | Working set | Private bytes |
|---|---:|---:|---:|
| No page open | 1 | 6 MB | 1 MB |
| One static tab | 1 | 10 MB | 2 MB |
| + first live tab | 9 | ~360 MB | ~210 MB |
| + second live tab | 12 | ~490 MB | ~295 MB |
| Live tabs closed | 1 | 11 MB | 3 MB |

Time: about 4.5 s for the first live page (static attempt + cold WebView),
1.6-3.6 s for later live pages, 1.5-3 s per action that changes the page.

M3 exit criterion ("logs into and navigates a React/Next.js demo app"):
**partly met.** JJ1 logged into a form-based demo site through the live tier
and drove an Ember single-page app. A login inside a React/Next.js app
specifically has not been tested.

Known gaps in the WebView tier (stated, not hidden):

- Sub-resources are not filtered: images, fonts and trackers load inside the
  WebView. The blocklist only stops top-level navigations there.
- Confirmation only covers real form submits. A script that sends data from a
  plain button cannot be recognised, so it is not gated.
- No HTTP status for live pages. Shadow DOM and iframe content are not read.
- Each live tab is its own worker (~130 MB more per extra tab). Nothing closes
  idle live tabs yet.
- Only Windows/WebView2 has been run. macOS and Linux builds are untested.
- On the GNU toolchain `WebView2Loader.dll` must sit beside `jj1-view.exe`
  (copied by hand from the `webview2-com-sys` build output).
- `jj1 open --stats` reports only the `jj1` process, not the worker.

### 2026-11-17 - Improvement round

Chosen by asking where JJ1 was weakest against its own goals (safe,
lightweight, agent-native, always runnable):

| Area | Problem | What changed | Result |
|---|---|---|---|
| Safe | A web page could steer the agent's browser at `localhost`, the home router or a cloud metadata address | Loopback, private, link-local, CGNAT and IPv6 equivalents are refused in both tiers (names are resolved first). `allow_private_network` in config turns it off. | Covered by unit and end-to-end tests |
| Lightweight | A forgotten live tab held about 350 MB for the whole session | Live tabs idle for `live_idle_secs` (default 120) are suspended: the worker ends, the last content is kept, and the tab reloads on its next snapshot or action. The MCP server checks every 5 s, also while the agent is quiet. | Memory returns to about 10 MB; element IDs survive the reload; typed-in values do not |
| Lightweight / safe | The WebView loaded every image, font and tracker | Requests for images, media and fonts, and anything from blocklisted or denied hosts, are answered with an empty 403 inside the WebView (WebView2 only). `webview_block_media = false` turns it off. | 25 requests blocked on bbc.com/news, 42 on a Wikipedia article; pages settle about 15-25% sooner; memory only 3-9% lower |
| Agent-native | Finding one link on a big page meant reading the page | New tool `jj1_find(tab, query)`: elements and text lines containing all the words | One call, a few hundred tokens |
| Always runnable | End-to-end tests needed real websites; nothing covered a framework-style login | `crates/jj1-core/tests/e2e.rs`: a local test site and 5 tests, run by `cargo test`. Includes a single-page app with controlled inputs, fetch and `pushState`, a wrong-then-right login, a `role=button` control, stylesheet-hidden injected text, and idle suspension. | All pass offline in about 14 s |
| Always runnable | The loader DLL had to be copied by hand | `build.ps1` builds and copies it | |

Tests now: 28 unit tests, 5 offline end-to-end tests, 20 static and 15 live
checks against real sites. All pass.

On the M3 exit criterion: the offline SPA test behaves the way React apps do
(controlled inputs that only react to input events, client-side routing), and
it passes. It is hand-written JavaScript, not React itself, so the criterion's
literal wording is still not met.

Still open after this round:

- The private-network guard resolves a name and then lets the HTTP client
  resolve it again, so a DNS answer that changes in between (DNS rebinding) is
  not caught. Sub-resource requests *inside* the WebView are not checked
  against it either.
- Sub-resource filtering exists only for WebView2.
- The live tests are skipped (with a message) if `jj1-view` is not built, so a
  green `cargo test` on a machine without it proves less.

### 2026-11-17 - Human mode started: `jj1-browser`

Scope change, stated plainly: the v1 plan listed "a replacement for Chrome as
a human daily-driver" as a non-goal. The user now wants JJ1 usable like any
other browser from an .exe. The honest position is this: `jj1-browser` is a
thin window around the system WebView (the same engine as Microsoft Edge on
Windows). It will render sites as Edge does and use about as much memory
(477 MB working set / 311 MB private for one Wikipedia tab, measured). It is
not lighter than Edge and is not meant to be; what it adds is JJ1's tracker
blocking and a view of what an agent sees.

Stages:

| | Scope | Status |
|---|---|---|
| H1 | Window, tabs, address bar (URL or search), back/forward/reload, popups open as tabs, page titles, keyboard shortcuts (Ctrl+T/W/L/R + Ctrl+J), start page, persistent profile, tracker blocking, **Agent view**, app icon, `jj1 view`, per-user install script | built |
| H1+ | **Deuri**: a local-model (LM Studio / Bonsai 2) chat panel that reads and navigates the active tab, loaded only when a message is sent, 30k context | built (see below) |
| H2 | Things people expect daily: downloads list, history and bookmarks, find in page, zoom, settings page, restore tabs, private window | not started |
| H3 | Distribution: real installer, updates, code signing, optional "default browser" registration | not started |

What exists (H1):

- `jj1-browser.exe [url] [--agent-view]`, also started by `jj1 view [url]`.
- The toolbar is a small web page in its own child WebView; each tab is
  another child WebView. Commands travel over IPC to the Rust side.
- Pages can send the browser exactly three shortcut requests and nothing else.
- **Agent view** distils the current page with the same code the agent side
  uses and shows token counts, the text, the action map and anything the
  injection scan flagged.
- Requests to hosts on the blocklist are refused; images and fonts load.
- Browsing data persists in `%APPDATA%\jj1\browser-profile`, separate from
  the agent side's throwaway profiles.
- `install.ps1` copies the programs to `%LOCALAPPDATA%\Programs\JJ1` and adds
  Start Menu and desktop shortcuts; `-Uninstall` removes them. It does not
  touch PATH, the registry or the default browser.

Verified: the window opens and renders pages; the tab strip, address bar,
title and icon show correctly; Agent view opens in a new tab with real numbers;
`jj1 view` launches it. Unit tests cover address-bar input handling and
HTML-escaping in Agent view.

**Not verified:** nobody has clicked through it yet. Tab switching and closing,
typing in the address bar, back/forward, reload, new tab and the keyboard
shortcuts are written but have only been compiled, not exercised.

Known gaps in H1: it sends the WebView's normal user agent (no JJ1 token);
no downloads UI beyond the WebView's own; no history, bookmarks, find, zoom
controls or settings; one window only; Windows only.

### 2026-11-17 - Deuri: a local-model assistant inside the browser

A chat panel in `jj1-browser`, backed by a model the user runs locally in
LM Studio (here: Bonsai 2, `bonsai-2-27b-ternary-crack`). New module
`crates/jj1-engine-view/src/deuri.rs` (the LM Studio client, tool loop and
context trimming) and a side panel + tools wired into `browser.rs`.

Requirements the user set, and how each is met:

| Asked | How |
|---|---|
| An agent "Deuri" inside the browser | Toolbar button + Ctrl+J side panel, chat UI with streaming replies, "New chat" |
| Linked to the local LLM Bonsai 2 | Talks to LM Studio's OpenAI-compatible server at `http://127.0.0.1:1234`; model/identifier set in `deuri.json` |
| Load **only when a chat is sent** | Nothing touches LM Studio on browser start. On the first message Deuri starts the server if needed and `lms load`s the model; **verified**: no model at launch, loaded 3 s after the first message |
| ~30k tokens per chat | Model is loaded with a 30,000-token context (`--context-length`); history is trimmed to fit (old page content stubbed first, then whole old turns), leaving room for the reply |

How it works:

- **Lazy, self-releasing.** `ensure_loaded` runs on the first message only. The
  model is loaded with `--ttl 600`, so LM Studio unloads it after 10 min idle;
  the browser also unloads it on exit (`unload_on_exit`, default on). Verified
  both: 3 s cold load, and unloaded when the window closed.
- **Tools over the active tab.** Deuri can `read_page` (the current tab,
  distilled, in ~5k-token parts), `find_on_page`, `open_url` and `search_web`
  (both navigate the active tab and read the result). The event loop runs the
  tool in the WebView; the model thread waits for the result. It cannot click,
  type or submit yet.
- **Safety.** Page text reaches the model wrapped in `UNTRUSTED_PAGE_CONTENT`
  markers with a per-run nonce; the system prompt tells Deuri to treat it as
  data. `open_url`/`search_web` are refused for non-http(s), blocklisted hosts
  and private-network addresses, so a page cannot talk the model into opening
  the user's router. Streaming can be stopped mid-reply.
- **Config.** `deuri.json` in `%APPDATA%\jj1` is written with defaults on first
  run: model, identifier, `context_length`, `idle_unload_secs`, server URL,
  `max_reply_tokens`, `temperature`, `unload_on_exit`, `lms_path`.

Verified against the real model: asked "What is this page about? Name two
authors quoted on it." on quotes.toscrape.com; Deuri called `read_page` and
answered correctly (Einstein, Austen), context panel showed ~2.1k of 30.0k
tokens. 7 unit tests in `jj1-engine-view` (stream reassembly, history trimming,
page-part wrapping, URL guards, address-bar input, agent-view escaping).

Known gaps in Deuri: LM Studio only (no Ollama path, though Ollama is also
installed here); no streaming of the model's hidden "thinking" tokens (shown as
"Thinking..."); tools are read/navigate only; one conversation at a time; the
reply budget (`max_reply_tokens`, 3000) counts thinking tokens, so a long
chain of thought can exhaust it (handled with a clear message, not a hang).

### 2026-11-18 - Chat/browsing history, settings, UI polish

- **Delete fixed.** Deleting a Deuri message used to remove the whole
  question-and-answer exchange; now it removes just that one message and
  repairs the history so it stays valid for the model (orphaned tool
  results and dangling tool-call messages are dropped). Verified: deleting
  a reply left its question in place and freed only its tokens (753 -> 235).
- **Deuri chat history.** Conversations are saved to `%APPDATA%\jj1\chats\`
  after each turn and after edits. A clock button in the panel opens a
  "Past chats" list; clicking one reloads it, each has a delete button, and
  "+ New" saves the current chat and starts a fresh one.
- **Browsing history.** Visits are recorded to `history.json` (newest kept,
  capped at 5000, built-in pages skipped). `jj1://history` lists them with
  search, click-to-open and "Clear all". A clock icon in the toolbar opens it.
- **Settings page** (`jj1://settings`, gear icon): search engine
  (DuckDuckGo/Google/Bing/Brave), homepage (beach / blank / a URL), block
  trackers, save history, clear history, clear cookies & site data, and
  Deuri's model config (model, context length, GPU offload ratio, K/V cache
  quant, flash attention, idle unload, max reply tokens). Browser prefs live
  in `browser.json`; they take effect live (tracker toggle) or on the next
  navigation / model load.
- **UI.** More translucent frosted chrome; the Deuri panel modernized with
  glassy bot bubbles, a gradient user bubble and message animations; the two
  loose toolbar icons replaced by History / Extensions / Settings icons (a
  dropdown menu was tried but a child WebView can't overflow its 80px bounds).

New module `browserdata.rs` (settings, history, chats), unit-tested. The
single-message delete and sanitize are unit-tested in `deuri.rs`. All work
verified live by driving the real window. 52 unit + e2e tests pass.

### 2026-11-17 - Deuri: 100k context, cache quantization, deletable history

Three changes, verified live against Bonsai 2:

- **100,000-token context** (was 30k). `lms load` only takes a context flag,
  not a large-context/cache config, so Deuri now writes LM Studio's per-model
  load config file (`~/.lmstudio/.internal/user-concrete-model-default-config/
  <publisher>/<model>/<gguf>.json`, UTF-8 **no BOM** — a BOM makes LM Studio
  ignore it) and lets `lms load` read it. Confirmed: loads at 100,096 tokens.
- **K/V cache quantization + flash attention + GPU offload.** The config sets
  `kCacheQuantizationType`/`vCacheQuantizationType` to `q8_0`, `flashAttention`
  true (required for quantized cache), and `acceleration.offloadRatio` to
  **0.703125 = 45 of Bonsai 2's 64 layers** ("45 GPU offloads"). New
  `deuri.json` keys: `gpu_offload_ratio`, `kv_cache_quant` ("q8_0" / "q4_0" /
  "f16" for off), `flash_attention`, `lmstudio_home`.
- **Deletable chat history.** Each message has a delete button; deleting one
  removes that whole question-and-answer exchange from the context and the
  token meter drops. Verified: deleting a reply took the context from ~754 to
  ~220 tokens. The model's required `assistant`/`tool` message pairing stays
  valid because deletion always removes a complete turn. New `deuri::transcript`
  and `deuri::delete_exchange`, unit-tested.

Config-file location is by a heuristic: the model's folder name, lower-cased
with a trailing "-gguf" removed, equals the LM Studio model key (true for
Bonsai 2 and the Qwen models here). If the model's folder can't be found,
Deuri falls back to loading with just the context-length flag and says cache
quantization couldn't be set. GPU offload is a *ratio* because that's all
LM Studio's config exposes; 0.703125 is 45/64 for this model — change it for
a model with a different layer count. The existing `deuri.json` (which had the
old 30k) was rewritten to the new values so it takes effect without editing.

### 2026-11-17 - Browser polish: windows, favicons, extensions, premium look

A batch of human-mode (H1/H2) work on `jj1-browser`, all verified by driving
the real window with hardware input and screenshots:

| Asked | Done | Verified |
|---|---|---|
| New session = new window | Ctrl+N (and a toolbar button) spawn a second `jj1-browser` process = a second independent window | process count 1 -> 2 |
| Tabs couldn't be closed with the cursor | Root cause: clicking the "x" fired the tab's own mousedown (switch), which rebuilt the tab strip and destroyed the "x" before its click fired. Fixed: the "x" now closes on `mousedown` with `stopPropagation` | clicked the "x" of 3 tabs -> 2; also middle-click closes |
| Show each site's favicon | A page script fetches the site's icon (same-origin), passes it as a data URL; the tab strip shows it, with a globe fallback | Wikipedia's "W" showed in the tab |
| A space for my own extensions, working | `%APPDATA%\jj1\extensions\<id>\` (manifest.json + content.js). Enabled scripts are injected into every page with a small `jj1` API (`captureTab`, `notify`, `log`). A puzzle-piece toolbar button opens a built-in page listing them and explaining how to add one | extensions page renders; sample loads |
| Test extension: hears "SS", full tab screenshot | Bundled `ss-screenshot` extension: press S twice quickly (not while typing) -> `jj1.captureTab()` -> the browser PrintWindow-captures the tab's content region and saves a PNG to `Pictures\JJ1` | S,S -> saved a 1860x1110 PNG of the page, toolbar excluded |
| Premium look, not basic black: rich white/grey, blurry translucent | Light "frosted glass" chrome (translucent white-grey + `backdrop-filter` blur) over a Windows 11 acrylic window backdrop, forced light regardless of system dark mode, with a light title bar (DWM). The Deuri panel matches | screenshots show the frosted light toolbar + panel |
| Beautiful start page with a beach image, "Welcome to Deuri's world / start your heart out", and a search bar | New start page: full-bleed HD beach photo (embedded, ~113 KB, no network), the heading and subtitle, and a glass search bar that navigates the tab | screenshot confirms |

New deps: `base64` (embed the photo), `png` (encode screenshots), `window-vibrancy`
(acrylic), and `windows` GDI/DWM/Xps features. New library modules
`extensions.rs` and `screenshot.rs`; the WebView request filter moved into the
shared lib so the browser and the agent worker use the same code. 10 new unit
tests (extensions, start-page templating, extensions page, screenshot paths).

Known gaps: the extension API is small (`captureTab`/`notify`/`log`) and content
scripts run in the page, so a hostile page could call the API too (extensions
are the user's own trusted code); favicons over ~120 KB are skipped; acrylic and
the screenshot capture are Windows-only; tabs are not draggable/reorderable and
do not persist across restarts; the screenshot is the visible tab, not the full
scrollable page. The multi-window model is one OS process per window (simple and
isolated), not shared state between windows.

### Next steps

1. Use `jj1` from Claude Code for a real task (registered, not yet exercised).
2. `git init` and first commit (not done; nothing has been committed).
3. Finish M3: test a login in a real React/Next.js app.
4. M4 remainder: screenshots and `jj1 view` (the WebView itself is in place).
5. Smaller items found while building: `<base>`-less relative forms are
   handled, but `<style>`-sheet hidden text is not detected; cookies are
   memory-only; no per-page fetch of sub-resources means the tracker blocklist
   only matters for navigations until a JS tier exists.
