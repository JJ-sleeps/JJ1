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

### Next steps

1. Use `jj1` from Claude Code for a real task (registered, not yet exercised).
2. `git init` and first commit (not done; nothing has been committed).
3. Finish M3: test a login in a React/Next.js app; close idle live tabs after
   a timeout; filter sub-resources in the WebView (needs WebView2's
   WebResourceRequested, which `wry` does not expose directly).
4. M4 remainder: screenshots and `jj1 view` (the WebView itself is in place).
5. Smaller items found while building: `<base>`-less relative forms are
   handled, but `<style>`-sheet hidden text is not detected; cookies are
   memory-only; no per-page fetch of sub-resources means the tracker blocklist
   only matters for navigations until a JS tier exists.
