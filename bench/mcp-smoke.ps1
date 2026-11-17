# End-to-end smoke test of `jj1 mcp` over stdio.
#   powershell -File bench\mcp-smoke.ps1 [-Jj1 path\to\jj1.exe]
# Talks real JSON-RPC to the server: handshake, open, click, GET form, POST
# form (must be refused without confirm), policy tightening.
param([string]$Jj1 = "jj1")

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Jj1
$psi.Arguments = "mcp"
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$psi.StandardOutputEncoding = [Text.Encoding]::UTF8
$proc = [System.Diagnostics.Process]::Start($psi)
$script:nextId = 0
$script:failed = 0

function Send($method, $params) {
    $script:nextId++
    $msg = @{ jsonrpc = "2.0"; id = $script:nextId; method = $method }
    if ($params) { $msg.params = $params }
    $proc.StandardInput.WriteLine(($msg | ConvertTo-Json -Depth 8 -Compress))
    $proc.StandardInput.Flush()
    return $proc.StandardOutput.ReadLine() | ConvertFrom-Json
}
function Tool($name, $toolArgs) {
    $r = Send "tools/call" @{ name = $name; arguments = $toolArgs }
    return [pscustomobject]@{ Text = $r.result.content[0].text; IsError = $r.result.isError }
}
function Check($label, $ok) {
    if ($ok) { Write-Host "PASS  $label" } else { Write-Host "FAIL  $label"; $script:failed++ }
}
function IdOf($text, $pattern) {
    foreach ($line in $text -split "`n") { if ($line -match "^\[(\d+)\] $pattern") { return [int]$Matches[1] } }
    return $null
}

$init = Send "initialize" @{ protocolVersion = "2025-06-18"; capabilities = @{}; clientInfo = @{ name = "smoke"; version = "0" } }
Check "initialize returns server info" ($init.result.serverInfo.name -eq "jj1")
$proc.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
$tools = Send "tools/list" $null
Check "tools/list returns 6 tools" ($tools.result.tools.Count -eq 6)

# 1. open + click a link + diff
$r = Tool "jj1_open" @{ url = "https://en.wikipedia.org/wiki/Web_browser"; max_tokens = 300; max_actions = 400 }
Check "open wikipedia article" ((-not $r.IsError) -and $r.Text -match "UNTRUSTED_PAGE_CONTENT" -and $r.Text -match "Web browser")
Write-Host ("      open payload: {0} chars" -f $r.Text.Length)
$search = IdOf $r.Text 'input\[search\]'
$link = IdOf $r.Text 'link "Main page"'
Check "action map has search box and Main page link" ($search -and $link)

# 2. stable IDs across navigation
$r = Tool "jj1_act" @{ id = $link; action = "click" }
Check "click link navigates" ((-not $r.IsError) -and $r.Text -match "Main_Page" -and $r.Text -match "diff \(\+")
$snap = Tool "jj1_snapshot" @{ limit = 400 }
Check "Main page link kept its ID after navigation" ((IdOf $snap.Text 'link "Main page"') -eq $link)
$r = Tool "jj1_read" @{ max_tokens = 200 }
Check "read returns paged markdown" ((-not $r.IsError) -and $r.Text -match "page 1 of")
$r = Tool "jj1_act" @{ id = $search; action = "type"; value = "Rust" }
$r = Tool "jj1_act" @{ id = $search; action = "press"; value = "Enter" }
Check "robots.txt is respected (Wikipedia disallows search for bots)" ($r.IsError -and $r.Text -match "robots.txt disallows")

# 3. GET form: type + press Enter (Hacker News search box)
$r = Tool "jj1_open" @{ url = "https://news.ycombinator.com"; max_actions = 400 }
$q = IdOf $r.Text 'input\[text\].* name=q'
Check "open Hacker News, find search box" ((-not $r.IsError) -and $q)
$r = Tool "jj1_act" @{ id = $q; action = "type"; value = "rust browser" }
Check "type returns a one-line diff" ((-not $r.IsError) -and $r.Text -match '~\[' -and $r.Text -match 'rust browser')
$r = Tool "jj1_act" @{ id = $q; action = "press"; value = "Enter" }
Check "press Enter submits the GET form without confirm" ((-not $r.IsError) -and $r.Text -match "q=rust\+browser")
Write-Host ("      " + (($r.Text -split "`n") | Select-Object -First 4 | Out-String).Trim().Replace("`n", "`n      "))

# 4. POST form needs confirmation
$r = Tool "jj1_open" @{ url = "https://httpbin.org/forms/post" }
Check "open httpbin form" (-not $r.IsError)
$name = IdOf $r.Text 'input\[text\]'
$submit = IdOf $r.Text 'submit'
$null = Tool "jj1_act" @{ id = $name; action = "type"; value = "JJ1 smoke test" }
$r = Tool "jj1_act" @{ id = $submit; action = "click" }
Check "POST submit refused without confirm" ($r.IsError -and $r.Text -match "CONFIRMATION REQUIRED")
$r = Tool "jj1_act" @{ id = $submit; action = "click"; confirm = $true }
Check "POST submit goes through with confirm" ((-not $r.IsError) -and $r.Text -match "submitted form")
$r = Tool "jj1_read" @{ mode = "all" }
Check "server echoed the typed value" ($r.Text -match "JJ1 smoke test")

# 5. policy
$r = Tool "jj1_set_policy" @{ deny = @("example.com") }
Check "deny list can be extended" ((-not $r.IsError) -and $r.Text -match "example.com")
$r = Tool "jj1_open" @{ url = "https://example.com" }
Check "denied domain is refused" ($r.IsError -and $r.Text -match "deny list")
$r = Tool "jj1_open" @{ url = "https://www.googletagmanager.com/gtm.js" }
Check "blocklisted tracker host is refused" ($r.IsError -and $r.Text -match "blocklist")
$r = Tool "jj1_open" @{ url = "https://example.org"; tier = "turbo" }
Check "unknown tier gives an actionable error" ($r.IsError -and $r.Text -match "Use auto, static, js or full")
$r = Tool "jj1_tabs" @{ action = "list" }
Check "tabs list shows three tabs" ($r.Text -match "tab=1" -and $r.Text -match "tab=3")

$proc.Refresh(); $peak = $proc.PeakWorkingSet64
$proc.StandardInput.Close()
$proc.WaitForExit(5000) | Out-Null
Write-Host ("server peak working set: {0:N1} MB" -f ($peak / 1MB))
if ($script:failed) { Write-Host "$($script:failed) check(s) FAILED"; exit 1 } else { Write-Host "all checks passed" }
