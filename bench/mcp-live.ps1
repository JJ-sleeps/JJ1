# End-to-end test of the WebView tier through `jj1 mcp`.
#   powershell -File bench\mcp-live.ps1 -Jj1 path\to\jj1.exe
# Needs jj1-view.exe (and on the GNU toolchain WebView2Loader.dll) beside jj1.exe.
# Covers: automatic escalation, clicking through a JS-rendered page, a
# single-page-app search, a login form with confirmation, policy on live
# pages, and what the whole process tree costs in memory.
param([Parameter(Mandatory)][string]$Jj1)

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
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Send "tools/call" @{ name = $name; arguments = $toolArgs }
    return [pscustomobject]@{ Text = $r.result.content[0].text; IsError = $r.result.isError; Ms = $sw.ElapsedMilliseconds }
}
function Check($label, $ok, $detail) {
    if ($ok) { Write-Host "PASS  $label" } else { Write-Host "FAIL  $label"; if ($detail) { Write-Host ("      " + ($detail -replace "`n", "`n      ")) }; $script:failed++ }
}
function IdOf($text, $pattern) {
    foreach ($line in $text -split "`n") { if ($line -match "^[+~]?\[(\d+)\] $pattern") { return [int]$Matches[1] } }
    return $null
}
function Tree-Memory($rootPid) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, WorkingSetSize, PrivatePageCount
    $keep = @{ $rootPid = $true }
    do { $added = $false; foreach ($p in $all) { if (-not $keep[[int]$p.ProcessId] -and $keep[[int]$p.ParentProcessId]) { $keep[[int]$p.ProcessId] = $true; $added = $true } } } while ($added)
    $tree = @($all | Where-Object { $keep[[int]$_.ProcessId] })
    "{0} processes, {1:N0} MB working set, {2:N0} MB private" -f $tree.Count, (($tree | Measure-Object WorkingSetSize -Sum).Sum / 1MB), (($tree | Measure-Object PrivatePageCount -Sum).Sum / 1MB)
}

$null = Send "initialize" @{ protocolVersion = "2025-06-18"; capabilities = @{}; clientInfo = @{ name = "live"; version = "0" } }
Write-Host ("memory, no page open:        " + (Tree-Memory $proc.Id))

# 1. A static page stays in the static tier.
$r = Tool "jj1_open" @{ url = "https://quotes.toscrape.com/" }
Check "static page stays static" ((-not $r.IsError) -and $r.Text -match "tier=static") $r.Text.Substring(0, [math]::Min(300, $r.Text.Length))
Write-Host ("memory, one static tab:      " + (Tree-Memory $proc.Id))

# 2. A JavaScript page escalates by itself.
$r = Tool "jj1_open" @{ url = "https://quotes.toscrape.com/js/" }
Check "JS page escalates to the WebView tier (cold, $($r.Ms) ms)" ((-not $r.IsError) -and $r.Text -match "tier=full" -and $r.Text -match "escalated" -and $r.Text -match "Albert Einstein") $r.Text.Substring(0, [math]::Min(400, $r.Text.Length))
Write-Host ("memory, + one live tab:      " + (Tree-Memory $proc.Id))
$next = IdOf $r.Text 'link "Next'
$login = IdOf $r.Text 'link "Login"'
$r = Tool "jj1_act" @{ id = $next; action = "click" }
Check "click Next on the live page ($($r.Ms) ms)" ((-not $r.IsError) -and $r.Text -match "page/2" -and $r.Text -match "diff \(") $r.Text
$r2 = Tool "jj1_read" @{ max_tokens = 300 }
Check "page 2 has different quotes" ((-not $r2.IsError) -and $r2.Text -notmatch "Albert Einstein" -and $r2.Text -match "by ") $r2.Text
$snap = Tool "jj1_snapshot" @{}
Check "unchanged link kept its ID on page 2; Next (new target) got a new one" (((IdOf $snap.Text 'link "Login"') -eq $login) -and ((IdOf $snap.Text 'link "Next') -ne $next)) $snap.Text

# 3. Login form in the live tier: typing, confirmation, POST, result.
$r = Tool "jj1_open" @{ url = "https://quotes.toscrape.com/login"; tier = "full" }
Check "open login page with tier=full ($($r.Ms) ms, second worker)" ((-not $r.IsError) -and $r.Text -match "tier=full") $r.Text
$user = IdOf $r.Text 'input\[text\].* name=username'
$pass = IdOf $r.Text 'input\[password\]'
$go = IdOf $r.Text 'submit'
$r = Tool "jj1_act" @{ id = $user; action = "type"; value = "jj1-test" }
Check "typing shows up in the diff" ((-not $r.IsError) -and $r.Text -match 'value="jj1-test"') $r.Text
$r = Tool "jj1_act" @{ id = $pass; action = "type"; value = "not-a-real-password" }
Check "password value is not echoed" ((-not $r.IsError) -and $r.Text -notmatch "not-a-real-password" -and $r.Text -match "value=\(hidden\)") $r.Text
$r = Tool "jj1_act" @{ id = $go; action = "click" }
Check "live POST submit refused without confirm" ($r.IsError -and $r.Text -match "CONFIRMATION REQUIRED") $r.Text
$r = Tool "jj1_act" @{ id = $go; action = "click"; confirm = $true }
Check "login goes through with confirm ($($r.Ms) ms)" ((-not $r.IsError) -and $r.Text -match 'link "Logout"') $r.Text
Write-Host ("memory, + second live tab:   " + (Tree-Memory $proc.Id))

# 4. Policy applies to live pages too.
$null = Tool "jj1_set_policy" @{ deny = @("goodreads.com") }
$snap = Tool "jj1_snapshot" @{}
$gr = IdOf $snap.Text 'link "GoodReads.com"'
$r = Tool "jj1_act" @{ id = $gr; action = "click" }
Check "link to a denied domain is refused on a live page" ($r.IsError -and $r.Text -match "deny list") $r.Text
$r = Tool "jj1_open" @{ url = "https://www.goodreads.com/quotes"; tier = "full" }
Check "denied domain cannot be opened with tier=full" ($r.IsError -and $r.Text -match "deny list") $r.Text

# 5. A real single-page app: crates.io (client-side routing, data over XHR).
$r = Tool "jj1_open" @{ url = "https://crates.io"; max_tokens = 400; max_actions = 200 }
$crateLinks = ([regex]::Matches($r.Text, '(?m)^\[\d+\] link .* -> /crates/')).Count
Check "SPA opens with its data loaded ($($r.Ms) ms, $crateLinks crate links)" ((-not $r.IsError) -and $r.Text -match "tier=full" -and $crateLinks -ge 10) $r.Text.Substring(0, [math]::Min(600, $r.Text.Length))
$q = IdOf $r.Text 'input\[[a-z]+\].* name=q'
$null = Tool "jj1_act" @{ id = $q; action = "type"; value = "serde" }
$r = Tool "jj1_act" @{ id = $q; action = "press"; value = "Enter" }
Check "SPA search by typing and pressing Enter ($($r.Ms) ms)" ((-not $r.IsError) -and $r.Text -match "search\?q=serde" -and $r.Text -match 'link "serde" -> /crates/serde') $r.Text
$null = Tool "jj1_tabs" @{ action = "close"; tab = 4 }

# 6. Closing tabs stops their workers.
$null = Tool "jj1_tabs" @{ action = "close"; tab = 2 }
$null = Tool "jj1_tabs" @{ action = "close"; tab = 3 }
Start-Sleep -Seconds 2
Write-Host ("memory, live tabs closed:    " + (Tree-Memory $proc.Id))
$left = @(Get-CimInstance Win32_Process -Filter "Name='jj1-view.exe'" | Where-Object { $_.ParentProcessId -eq $proc.Id }).Count
Check "no worker processes left after closing live tabs" ($left -eq 0) "$left still running"

$proc.StandardInput.Close()
$proc.WaitForExit(8000) | Out-Null
if ($script:failed) { Write-Host "$($script:failed) check(s) FAILED"; exit 1 } else { Write-Host "all checks passed" }
