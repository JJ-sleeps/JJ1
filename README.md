# JJ1

**A browser built for AI agents, plus a browser for the people who work with them.**

JJ1 gives an AI agent the web the way it actually needs it: clean text and a
short, numbered list of things it can click or fill in. No raw HTML, no
screenshots, no 600 MB headless browser. It also ships a normal windowed
browser, `jj1-browser`, with a private local assistant (Deuri) and an
**Agent view** that shows you exactly what an agent would see on any page.

---

## The problem

Agents that use the web today mostly get one of two things, and both are bad:

1. **Raw HTML or the full DOM.** A Wikipedia article is about **155,000
   tokens** of DOM. Almost all of it is markup, navigation and scripts. It
   burns the context window, costs money, and buries the content.
2. **Screenshots and a headless browser.** Headless Chrome costs **around
   540-680 MB of RAM and 12+ processes per page** on a cold start, the agent
   has to guess coordinates from pixels, and every step is slow.

And both are **unsafe by default**. A page can hide text that a person never
sees (`display:none`, HTML comments, off-screen spans) saying *"ignore your
previous instructions and..."*. An agent reading raw page content can't tell
the page's words from yours, and nothing stops it from submitting a form or
reaching into your local network because a page asked it to.

On the human side, people who run models locally hit a different wall: every
machine is different, and loading a model means guessing context length,
GPU offload and K/V-cache settings until it stops running out of memory.

## The solution

JJ1 fetches the page itself and hands the agent:

- **Clean Markdown** of the readable content, and
- an **action map**: every link, button and field, numbered.

```
[1] link "Home" -> /
[4] input[number] "Quantity" name=qty value="1"
[5] checkbox "Gift wrap" name=gift
[6] submit "Add to cart"
```

The agent acts by number (`act 6 click`) and gets back **only what changed**.
Pages that need JavaScript are detected and re-run in the system WebView
automatically. Everything from the page is wrapped as untrusted, hidden text
is stripped and reported, and risky actions wait for confirmation.

### Measured, not claimed

From [bench/results](bench/results) (Core Ultra 9 275HX, 31 GB RAM, cold
starts, JJ1 static tier vs `chrome --headless=new`):

| Page | JJ1 RAM | Headless Chrome RAM (sum / private) | JJ1 tokens (default open) | Chrome DOM tokens |
|---|---:|---:|---:|---:|
| example.com | 10.1 MB | 538 / 215 MB | 119 | 498 |
| news.ycombinator.com | 10.8 MB | 555 / 222 MB | 2,285 | 8,510 |
| bbc.com/news | 11.7 MB | 1,650 / 785 MB | 3,459 | 82,835 |
| Wikipedia: Web browser | 13.8 MB | 654 / 279 MB | 3,142 | 154,952 |
| Wikipedia: Rust | 18.4 MB | 650 / 296 MB | 3,426 | 348,566 |

Across the whole run, JJ1 used **10-27 MB instead of 500+ MB**, and **24-100x fewer tokens** on
content-heavy pages. For JavaScript apps (crates.io, hn.algolia.com) the
WebView tier gets the same content as headless Chrome with about 35-40% less
summed memory. JJ1 is not always faster: very large pages can take longer
to distill than Chrome takes to dump. Details and caveats are in the result
files.

---

## What makes JJ1 different

### For agents (`jj1 mcp`)

- **Action map with stable IDs.** Element numbers stay the same for as long as
  the element exists, even across navigations in the same tab, so
  "click [12]" still means the same button after the page updates.
- **Diffs, not re-reads.** After an action the agent receives only what
  changed on the page.
- **Two tiers, chosen automatically.** A ~10 MB static fetch first; the system
  WebView (in a separate worker that exits when the tab closes) only when the
  page turns out to be a JavaScript app. Idle live tabs are suspended after
  two minutes and reload on use.
- **Cheap lookups.** `jj1_find` returns just the elements and lines that match,
  so the agent doesn't read a whole page to find one button.
- **Hidden fields are visible.** Forms list their hidden inputs, so nothing is
  submitted blind.

### Safety built in, not bolted on

- Page content is wrapped in `<<<UNTRUSTED_PAGE_CONTENT nonce>>>` markers with a
  per-session random nonce, so a page cannot fake the closing marker.
- Hidden text is removed and reported; instruction-like phrases are flagged.
- Non-GET form submissions are refused until the call is repeated with
  `confirm=true`.
- Localhost and private-network addresses are refused, so a page can't aim the
  agent at services inside your network.
- Allow/deny lists, the tracker blocklist and robots.txt are checked on
  **every redirect hop**, not just the first URL.
- The agent can **tighten** its policy but never loosen it (unless you allow
  that in `config.toml`), so an injected agent can't switch off its own guard
  rails.
- JJ1 identifies itself honestly (`JJ1/<version>`) and does not try to get
  around bot protection, CAPTCHAs or paywalls.

### For people (`jj1-browser`)

- **Agent view.** One click shows what an AI agent would receive for the page
  you're on: token cost of the raw HTML vs what the agent gets, how much of the
  assistant's context it would take, the text (as a reading view or raw
  Markdown, for the main content or the whole page), a searchable action map,
  every form with its hidden fields, and anything the injection scan removed,
  with an explanation of why. Copy it as the agent sees it, or as JSON.
- **Deuri, a private local assistant.** A chat panel backed by a model you run
  in [LM Studio](https://lmstudio.ai). Nothing leaves your machine. It can read
  and search the page, search the web, and open pages in your tab.
- **Hardware-aware model setup.** Settings checks your GPU and its video
  memory, your RAM and CPU, lists the models you have installed, and works out
  how to load each one on *your* machine (see below).
- **Context you can manage.** Delete any single message to get its tokens back;
  a meter shows how much of the context window is in use. Past chats are saved
  locally and can be reopened or deleted.
- **Loads only when needed.** The model loads on your first message, not when
  the browser starts, and unloads when idle or when you close the browser.
- **Docked DevTools.** F12, Ctrl+Shift+I or right-click → Inspect open the full
  Edge DevTools *inside* the window, under the page, like Chrome and Edge.
  (WebView2 on its own can only pop them out into a separate window.)
- **Simple extensions.** A folder with `manifest.json` and `content.js` is an
  extension. A small `jj1` API covers screenshots, notifications and logging.
- **The everyday basics**: tabs, multiple windows, favicons, history, tracker
  and ad blocking, view source (Ctrl+U), a light/dark theme toggle and a
  translucent "frosted glass" look.

---

## Deuri model setup: picks settings that fit your machine

Open **Settings → Deuri**. JJ1:

1. **Checks your hardware.** It reads GPU memory from `nvidia-smi` on NVIDIA,
   or from the display driver for AMD and Intel Arc. Integrated graphics and
   virtual display adapters are ignored, because they share system RAM. It
   also reads total RAM and the CPU.
2. **Reads every installed model's real layout** from its GGUF header: layer
   count, K/V heads, key/value sizes, and for hybrid models such as Qwen 3.5,
   which layers actually keep a K/V cache. (Bonsai 2 keeps one on only 16 of
   its 64 layers; a naive estimate would be 4x too big.)
3. **Recommends how to load it**, in this order:
   - everything on the GPU at the longest useful context (up to 128k tokens),
     with the K/V cache at full precision or **q8_0**;
   - everything on the GPU with a smaller but still comfortable context
     (32k tokens or more), using q8_0, or q4_0 if that is what it takes;
   - split between GPU and RAM, with as many layers on the GPU as fit;
   - CPU only, with a modest context, when there's no usable GPU.

   Flash attention is turned on whenever the cache is quantized, because
   llama.cpp requires it.
4. **Explains the choice** in plain words, and shows two live memory bars
   (GPU and RAM, split into model weights and context) that update as you
   change the context, the number of GPU layers or the cache type. It warns
   you before you pick something that won't fit, and flags models that weren't
   trained for tool use (Deuri needs tools to read pages).

With "Auto-tune when I pick a model" on, choosing a model fills in its
recommended settings. Saving a different model or load setting unloads the
old one, and the next message loads it the new way.

Example, RTX 5070 Ti Laptop (12 GB) + 32 GB RAM:

| Model | Fits | Context | GPU layers | K/V cache |
|---|---|---:|---:|---|
| Bonsai 2 27B (6.7 GB) | fully on GPU | 100k | 64 / 64 | q8_0 |
| Qwen2.5 Coder 14B Q4_K_M (8.4 GB) | fully on GPU | 32k | all | q4_0 |
| Qwen2.5 Coder 7B Q4_K_M (4.4 GB) | fully on GPU | 32k | 28 / 28 | off (f16) |

Settings are stored in `%APPDATA%\jj1\deuri.json`. Because LM Studio's `lms`
CLI has no flags for cache quantization or the GPU layer ratio, Deuri writes
LM Studio's per-model load config and lets `lms load` read it.

---

## Install and build

Windows, current user, no admin rights needed:

```
powershell -File install.ps1
```

This builds the release binaries (incrementally, so it's quick when nothing
changed), installs them to `%LOCALAPPDATA%\Programs\JJ1`, and adds **JJ1
Browser** shortcuts to the Start Menu and desktop. Close any open JJ1 windows
first, or the files can't be replaced. `install.ps1 -Uninstall` removes it and
keeps your browsing data in `%APPDATA%\jj1`.

To only build:

```
powershell -File build.ps1          # release
powershell -File build.ps1 -Debug   # debug (what cargo test uses)
```

`build.ps1` runs `cargo build`, which builds `jj1`, the WebView worker
`jj1-view` and `jj1-browser`, and copies `WebView2Loader.dll` beside them
(needed on Windows with the GNU toolchain). Without the worker, `jj1` still
runs; JavaScript pages are then reported as such instead of rendered. To put
`jj1` on your PATH: `cargo install --path crates/jj1-cli`.

## Use it from an agent

```
jj1 mcp                                      # MCP server on stdio
claude mcp add jj1 -- jj1 mcp                # connect it to Claude Code
```

| Tool | What it does |
|---|---|
| `jj1_open(url, tier, max_tokens, max_actions)` | Open a URL in a new tab; returns Markdown + action map |
| `jj1_snapshot(tab, scope, limit, offset)` | The action map of a tab |
| `jj1_act(tab, id, action, value, confirm)` | `click`, `type`, `select`, `check`, `press`, `scroll`; returns the diff |
| `jj1_read(tab, mode, max_tokens, page, links)` | Page text as Markdown, in pages (`article`, `all`, `table`) |
| `jj1_find(tab, query, limit)` | Elements and text lines containing all the words; cheaper than reading the page |
| `jj1_tabs(action, tab)` | `list`, `close`, `switch` |
| `jj1_set_policy(allow, deny, confirm_actions)` | Tighten the policy for this session |

## Use it from the command line

```
jj1 open https://example.com                 # status + title
jj1 open https://news.ycombinator.com -m -a  # Markdown + action map
jj1 open <url> --stats                       # size, time, tokens, memory
jj1 open <url> --tier full                   # always run the page's scripts
jj1 open <url> --tier static                 # never run scripts
jj1 open <url> --json                        # machine-readable
jj1 open https://shop.test/x --file fixtures/injection.html -m -a   # local HTML
jj1 config --init                            # write a default config.toml
jj1 view [url]                               # open the JJ1 browser window
```

## Use it as a browser

| Keys | Does |
|---|---|
| Ctrl+T / Ctrl+W | New tab / close tab |
| Ctrl+N | New window |
| Ctrl+L | Address bar |
| Ctrl+R | Reload |
| Ctrl+J | Open Deuri |
| Ctrl+U | View page source |
| F12, Ctrl+Shift+I / J / C | Docked DevTools |

The toolbar opens **History**, **Extensions**, **Settings**, the **Agent view**
and **Deuri**, and switches between light and dark.

**Extensions** live in `%APPDATA%\jj1\extensions\<name>\`: a `manifest.json`
and a `content.js` that runs on every page and can call `jj1.captureTab()`,
`jj1.notify(text)` and `jj1.log(text)`. The bundled example, **SS
Screenshot**, saves a PNG of the tab to `Pictures\JJ1` when you press **S**
twice quickly.

The browser runs on the system WebView (the engine behind Microsoft Edge), so
it uses about as much memory as Edge does. The lightweight part of JJ1 is the
agent side.

## Limits and honest caveats

- The static tier has no CSS engine: hidden-text detection only sees
  attributes, inline styles and common utility classes.
- In the WebView tier, sub-resource filtering is WebView2-only, and data that
  a page's own scripts send can't be confirmation-gated.
- There is no OS sandbox yet.
- Docked DevTools work through WebView2's debugging server on
  `127.0.0.1:9338` (only DevTools pages from that same address may connect).
  While the browser is open, other programs on this computer could connect to
  it, the same as Chrome or Edge with remote debugging turned on.
- Model-fit numbers are estimates with a safety margin for the desktop and
  runtime; LM Studio's real usage can differ a little.

The full plan and milestone status are in [docs/PLAN.md](docs/PLAN.md).

## Test

```
cargo test                                                    # unit tests + offline end-to-end tests (local test site)
powershell -File bench\mcp-smoke.ps1 -Jj1 <path to jj1.exe>   # static tier, end to end over MCP
powershell -File bench\mcp-live.ps1 -Jj1 <path to jj1.exe>    # WebView tier, end to end over MCP
powershell -File bench\bench.ps1 -Jj1 <path to jj1.exe>       # JJ1 vs headless Chrome/Edge
```

## Layout

```
crates/jj1-net          fetch, redirects, robots.txt, blocklist, rate limit
crates/jj1-distill      HTML -> Markdown, action map, injection scan
crates/jj1-core         config, policy, session, tabs, IDs, diffs, forms, engine trait
crates/jj1-engine-view  `jj1-view` WebView worker, `jj1-browser` window, Deuri,
                        model fitting, docked DevTools, benchmark probe
crates/jj1-mcp          MCP stdio server
crates/jj1-cli          the `jj1` binary
rules/                  default host blocklist
fixtures/               test pages, including prompt-injection pages
bench/                  benchmark and smoke-test scripts, results
docs/                   plan
```

## License

MIT
