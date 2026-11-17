# JJ1

A lightweight browser that AI agents drive directly over MCP.

JJ1 fetches a page and gives the agent clean Markdown plus a numbered
**action map** instead of raw HTML or a screenshot. The agent acts by number
(`click [3]`) and gets back only what changed.

Status: milestones M0-M2 of [docs/PLAN.md](docs/PLAN.md) are built, and M3
mostly. There are two tiers:

- **static** (about 10 MB): plain fetch, no JavaScript. Used first, always.
- **full** (about 350 MB while a live page is open): the system WebView in a
  separate `jj1-view` worker process. Used automatically when the static
  result is a JavaScript app shell, or on request. The worker ends when the
  tab is closed.

## Build

```
powershell -File build.ps1
```

This runs `cargo build --release`, which builds `jj1` and the WebView worker
`jj1-view` into the same folder, and copies `WebView2Loader.dll` beside them
(needed on Windows with the GNU toolchain). `build.ps1 -Debug` does the same
for the debug profile that `cargo test` uses.

Without the worker, `jj1` still runs; JavaScript pages are then reported as
such instead of rendered.

The binary is `jj1` (`jj1.exe` on Windows) in the cargo target directory.
On this machine that is `C:\Users\jagra\.cargo-target\jj1\release\jj1.exe`
(set in the git-ignored `.cargo/config.toml` to keep build output out of
OneDrive). To put `jj1` on your PATH:

```
cargo install --path crates/jj1-cli
```

## Use

```
jj1 open https://example.com                 # status + title
jj1 open https://news.ycombinator.com -m -a  # Markdown + action map
jj1 open <url> --stats                       # size, time, tokens, memory
jj1 open <url> --tier full                   # always run the page's scripts
jj1 open <url> --tier static                 # never run scripts
jj1 open <url> --json                        # machine-readable
jj1 open https://shop.test/x --file fixtures/injection.html -m -a   # local HTML
jj1 config --init                            # write a default config.toml
jj1 mcp                                      # MCP server on stdio
```

Connect it to Claude Code:

```
claude mcp add jj1 -- jj1 mcp
```

## Use it as a browser

```
jj1 view                       # open the JJ1 browser window
jj1 view example.com           # ...on a page
powershell -File install.ps1   # install for this user, with Start Menu and desktop shortcuts
```

`jj1-browser` is a normal window with tabs and an address bar on the system
WebView (the engine behind Microsoft Edge on Windows), plus JJ1's tracker
blocking and an **Agent view** button that shows what an AI agent would receive
for the page you are on. It uses about as much memory as Edge does; the
lightweight part of JJ1 is the agent side.

### Deuri, the local assistant

The **Deuri** button (or Ctrl+J) opens a chat panel backed by a model you run
locally in [LM Studio](https://lmstudio.ai) — no cloud, nothing leaves your
machine. Deuri can read the page you are on, find things on it, search the web
and open pages, all in your active tab.

The model is loaded **only when you send your first message** (not when the
browser starts), given a 30k-token context, and unloaded when it sits idle or
when you close the browser. Settings live in `%APPDATA%\jj1\deuri.json`
(model name, context length, server URL, idle timeout):

```json
{ "model": "bonsai-2-27b-ternary-crack", "identifier": "deuri",
  "context_length": 30000, "idle_unload_secs": 600,
  "server": "http://127.0.0.1:1234" }
```

To use it: install LM Studio, download a chat model (Deuri works best with a
tool-capable one), and make sure the `lms` CLI is on its default path. Deuri
starts LM Studio's local server itself on the first message.

## MCP tools

| Tool | What it does |
|---|---|
| `jj1_open(url, tier, max_tokens, max_actions)` | Open a URL in a new tab; returns Markdown + action map |
| `jj1_snapshot(tab, scope, limit, offset)` | The action map of a tab |
| `jj1_act(tab, id, action, value, confirm)` | `click`, `type`, `select`, `check`, `press`, `scroll`; returns the diff |
| `jj1_read(tab, mode, max_tokens, page, links)` | Page text as Markdown, in pages (`article`, `all`, `table`) |
| `jj1_find(tab, query, limit)` | Elements and text lines containing all the words; cheaper than reading the page |
| `jj1_tabs(action, tab)` | `list`, `close`, `switch` |
| `jj1_set_policy(allow, deny, confirm_actions)` | Tighten the policy for this session |

An action map looks like this:

```
[1] link "Home" -> /
[4] input[number] "Quantity" name=qty value="1"
[5] checkbox "Gift wrap" name=gift
[6] submit "Add to cart"
```

Element numbers stay the same for as long as the element exists, including
across navigations within a tab.

## Safety

- Everything that comes from a web page is wrapped in
  `<<<UNTRUSTED_PAGE_CONTENT nonce>>>` markers with a per-session random nonce.
- Hidden text is removed and reported; instruction-like phrases are flagged.
- Non-GET form submissions are refused until the call is repeated with
  `confirm=true`.
- Allow/deny domain lists, the tracker blocklist and robots.txt are checked on
  every redirect hop.
- Localhost and private-network addresses are refused unless you set
  `allow_private_network = true`, so a page cannot aim the browser at services
  inside your network.
- In the WebView tier, images, media, fonts and requests to blocklisted hosts
  are not loaded, and popups and downloads are refused.
- Live tabs left idle for two minutes are suspended to free memory and reload
  when used again.
- The agent can tighten the policy but not loosen it (unless you set
  `agent_can_loosen = true` in `config.toml`).
- JJ1 identifies itself honestly (`JJ1/<version>`) and does not try to get
  around bot protection, CAPTCHAs or paywalls.

Limits are listed in [docs/PLAN.md](docs/PLAN.md). The important ones: in the
static tier there is no CSS engine (hidden-text detection only sees attributes,
inline styles and common utility classes); in the WebView tier sub-resources
filtering is WebView2-only and script-driven data sends cannot be
confirmation-gated; and there is no OS sandbox yet.

## Test

```
cargo test                                                    # unit tests + offline end-to-end tests (local test site)
powershell -File bench\mcp-smoke.ps1 -Jj1 <path to jj1.exe>   # static tier, end to end over MCP
powershell -File bench\mcp-live.ps1 -Jj1 <path to jj1.exe>    # WebView tier, end to end over MCP
powershell -File bench\bench.ps1 -Jj1 <path to jj1.exe>       # JJ1 vs headless Chrome/Edge
```

Benchmark results are in [bench/results](bench/results).

## Layout

```
crates/jj1-net      fetch, redirects, robots.txt, blocklist, rate limit
crates/jj1-distill  HTML -> Markdown, action map, injection scan
crates/jj1-core     config, policy, session, tabs, IDs, diffs, forms, engine trait
crates/jj1-engine-view  `jj1-view` WebView worker, `jj1-browser` window, benchmark probe
crates/jj1-mcp      MCP stdio server
crates/jj1-cli      the `jj1` binary
rules/              default host blocklist
fixtures/           test pages
bench/              benchmark and smoke-test scripts, results
docs/               plan
```
