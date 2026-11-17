# M3 engine benchmark: what does it cost to render a JavaScript page?
#   powershell -File bench\bench-js.ps1 -Jj1 <jj1.exe> -Probe <jj1-view-probe.exe> [-Runs 3]
#
# Engines compared on each page:
#   static   : JJ1 Tier 0 (no JavaScript) - the baseline that fails on these pages
#   webview  : system WebView (WebView2 on Windows) via jj1-view-probe, hidden window
#   chrome   : headless Chrome, --dump-dom
# For webview and chrome the rendered DOM is then fed through JJ1's distiller
# (`jj1 open --file`) to get the tokens an agent would actually receive.
#
# Memory = peak over time of the summed working set / private bytes of the whole
# process tree, sampled a few times per second. Each run is a cold start with a
# fresh profile directory. Medians over -Runs runs are reported.
param(
    [Parameter(Mandatory)][string]$Jj1,
    [Parameter(Mandatory)][string]$Probe,
    [string]$Chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe",
    [string[]]$Urls = @("https://crates.io", "https://hn.algolia.com/?q=rust", "https://quotes.toscrape.com/js/"),
    [int]$Runs = 3,
    [int]$TimeoutSec = 45,
    [string]$Out = ""
)
if (-not $Out) { $Out = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) "results" }

New-Item -ItemType Directory -Force $Out | Out-Null
$tmp = Join-Path $env:TEMP "jj1-bench-js"
New-Item -ItemType Directory -Force $tmp | Out-Null

function Get-Tree($rootPid) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, WorkingSetSize, PrivatePageCount
    $keep = @{ $rootPid = $true }
    do {
        $added = $false
        foreach ($p in $all) {
            if (-not $keep[[int]$p.ProcessId] -and $keep[[int]$p.ParentProcessId]) { $keep[[int]$p.ProcessId] = $true; $added = $true }
        }
    } while ($added)
    $all | Where-Object { $keep[[int]$_.ProcessId] }
}

# Start a process, sample its tree until it exits, return peaks and wall time.
function Measure-Tree($exe, $argv, $stdoutFile) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $exe -ArgumentList $argv -RedirectStandardOutput $stdoutFile -RedirectStandardError (Join-Path $tmp "err.txt") -PassThru -WindowStyle Hidden
    $peakWs = 0; $peakPriv = 0; $peakProcs = 0; $timedOut = $false
    while (-not $proc.HasExited) {
        $tree = @(Get-Tree $proc.Id)
        $ws = ($tree | Measure-Object WorkingSetSize -Sum).Sum
        $priv = ($tree | Measure-Object PrivatePageCount -Sum).Sum
        if ($ws -gt $peakWs) { $peakWs = $ws }
        if ($priv -gt $peakPriv) { $peakPriv = $priv }
        if ($tree.Count -gt $peakProcs) { $peakProcs = $tree.Count }
        if ($sw.Elapsed.TotalSeconds -gt $TimeoutSec) { $timedOut = $true; & taskkill /PID $proc.Id /T /F 2>$null | Out-Null; break }
        Start-Sleep -Milliseconds 50
    }
    $sw.Stop()
    [pscustomobject]@{ ws_mb = $peakWs / 1MB; priv_mb = $peakPriv / 1MB; procs = $peakProcs; ms = [int]$sw.ElapsedMilliseconds; timed_out = $timedOut }
}

# Run the rendered DOM through JJ1's distiller.
function Distill($url, $domFile) {
    if (-not (Test-Path $domFile) -or (Get-Item $domFile).Length -lt 50) { return $null }
    $raw = & $Jj1 open $url --file $domFile --json 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    ($raw | Out-String) | ConvertFrom-Json
}

function Run-Static($url) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $raw = & $Jj1 open $url --json 2>$null
    $sw.Stop()
    $j = ($raw | Out-String) | ConvertFrom-Json
    [pscustomobject]@{ ws_mb = $j.peak_rss_kb / 1024; priv_mb = $j.peak_rss_kb / 1024; procs = 1; ms = [int]$sw.ElapsedMilliseconds
        dom_tokens = $j.raw_tokens; agent_tokens = $j.mcp_tokens; content_tokens = $j.tokens; needs_js = [bool]$j.needs_js; ok = $true }
}

function Run-Rendered($engine, $url) {
    $profileDir = Join-Path $tmp ("profile-" + [guid]::NewGuid().ToString("N"))
    $dom = Join-Path $tmp "dom-$engine.html"
    Remove-Item $dom -ErrorAction SilentlyContinue
    if ($engine -eq "webview") {
        $m = Measure-Tree $Probe @("`"$url`"", "--out", "`"$dom`"", "--data-dir", "`"$profileDir`"", "--timeout-secs", "30") (Join-Path $tmp "probe.json")
    } else {
        $m = Measure-Tree $Chrome @("--headless=new", "--disable-gpu", "--no-first-run", "--disable-extensions", "--user-data-dir=`"$profileDir`"", "--virtual-time-budget=8000", "--dump-dom", "`"$url`"") $dom
    }
    $d = Distill $url $dom
    Start-Sleep -Milliseconds 400
    Remove-Item $profileDir -Recurse -Force -ErrorAction SilentlyContinue
    $m | Add-Member -PassThru -NotePropertyMembers @{
        dom_tokens = if ($d) { $d.raw_tokens } else { 0 }
        agent_tokens = if ($d) { $d.mcp_tokens } else { 0 }
        content_tokens = if ($d) { $d.tokens } else { 0 }
        needs_js = if ($d) { [bool]$d.needs_js } else { $true }
        ok = [bool]$d -and -not $m.timed_out
    }
}

function Median($values) {
    $v = @($values | Sort-Object)
    if ($v.Count -eq 0) { return 0 }
    $m = [int][math]::Floor($v.Count / 2)
    if ($v.Count % 2) { $v[$m] } else { ($v[$m - 1] + $v[$m]) / 2 }
}

$rows = @()
foreach ($url in $Urls) {
    foreach ($engine in "static", "webview", "chrome") {
        $samples = @()
        for ($i = 1; $i -le $Runs; $i++) {
            Write-Host ("{0}  {1}  run {2}/{3}" -f $url, $engine, $i, $Runs)
            $samples += if ($engine -eq "static") { Run-Static $url } else { Run-Rendered $engine $url }
        }
        $rows += [pscustomobject]@{
            url = $url; engine = $engine; runs = $samples.Count; ok = @($samples | Where-Object ok).Count
            ws_mb = [math]::Round((Median ($samples | ForEach-Object ws_mb)), 1)
            priv_mb = [math]::Round((Median ($samples | ForEach-Object priv_mb)), 1)
            procs = (Median ($samples | ForEach-Object procs))
            ms = [int](Median ($samples | ForEach-Object ms))
            dom_tokens = [int](Median ($samples | ForEach-Object dom_tokens))
            agent_tokens = [int](Median ($samples | ForEach-Object agent_tokens))
            content_tokens = [int](Median ($samples | ForEach-Object content_tokens))
            needs_js = [bool](@($samples | Where-Object needs_js).Count -gt ($samples.Count / 2))
        }
    }
}

$stamp = Get-Date -Format "yyyy-MM-dd"
$rows | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $Out "bench-js-$stamp.json") -Encoding UTF8
$md = @()
$md += "# JJ1 M3 engine benchmark - $stamp"
$md += ""
$md += "Machine: $((Get-CimInstance Win32_Processor).Name.Trim()), $([math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)) GB RAM, $((Get-CimInstance Win32_OperatingSystem).Caption)."
$wv = Get-ChildItem "C:\Program Files (x86)\Microsoft\EdgeWebView\Application" -Directory -ErrorAction SilentlyContinue | Where-Object Name -match '^\d' | Select-Object -First 1 -ExpandProperty Name
$md += "Engines: JJ1 static tier; WebView2 runtime $wv through ``jj1-view-probe`` (hidden window); headless Chrome $((Get-Item $Chrome).VersionInfo.ProductVersion)."
$md += "Medians of $Runs cold-start runs per cell. RAM is the peak of the summed process tree (working set / private bytes)."
$md += ""
$md += "| Page | Engine | RAM sum (MB) | RAM private (MB) | Processes | Time (ms) | Rendered DOM tokens | Content tokens after distilling | Tokens in a default open | Got the content? |"
$md += "|---|---|---:|---:|---:|---:|---:|---:|---:|---|"
foreach ($r in $rows) {
    $got = if ($r.ok -lt $r.runs) { "failed $($r.runs - $r.ok) of $($r.runs) runs" } elseif ($r.needs_js) { "no (empty shell)" } else { "yes" }
    $md += "| $($r.url) | $($r.engine) | $($r.ws_mb) | $($r.priv_mb) | $($r.procs) | $($r.ms) | $($r.dom_tokens) | $($r.content_tokens) | $($r.agent_tokens) | $got |"
}
$md += ""
$md | Set-Content (Join-Path $Out "bench-js-$stamp.md") -Encoding UTF8
Write-Host ("wrote " + (Join-Path $Out "bench-js-$stamp.md"))
