# JJ1 vs headless Chromium (Edge or Chrome) on the same pages, same machine.
#   powershell -File bench\bench.ps1 [-Jj1 path] [-Browser path] [-Urls bench\urls.txt]
#
# Per page it records:
#   JJ1     : peak working set of the one process, wall time, tokens sent to the
#             model by a default jj1_open call, tokens of the whole page
#             (all text + every action), whether the page needs JavaScript.
#   Browser : peak of (sum of working sets over the whole process tree), sampled
#             about 5x per second; wall time for `--headless --dump-dom`; tokens
#             of the dumped DOM (what an agent gets if it reads raw HTML).
#
# Caveats, stated up front:
#   - Summed working sets double-count memory shared between browser processes,
#     so the browser's "private" column (sum of private bytes) is also reported
#     as a lower bound.
#   - Sampling can miss short peaks, which favours the browser.
#   - Tokens are estimated as characters / 4.
#   - The browser runs JavaScript; JJ1's static tier does not. Pages marked
#     needs_js are NOT a fair win for JJ1: it did less work and got less content.
param(
    [string]$Jj1 = "jj1",
    [string]$Browser = "",
    [string]$Urls = "$PSScriptRoot\urls.txt",
    [string]$Out = "$PSScriptRoot\results",
    [int]$TimeoutSec = 45
)

if (-not $Browser) {
    $Browser = @(
        "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        "C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        "C:\Program Files\Google\Chrome\Application\chrome.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Browser) { Write-Error "No Edge/Chrome found; pass -Browser."; exit 1 }
$exeName = Split-Path $Browser -Leaf
New-Item -ItemType Directory -Force $Out | Out-Null
$tmp = Join-Path $env:TEMP "jj1-bench"
New-Item -ItemType Directory -Force $tmp | Out-Null

function Get-Tree($rootPid, $name) {
    $all = Get-CimInstance Win32_Process -Filter "Name='$name'" -Property ProcessId, ParentProcessId, WorkingSetSize, PrivatePageCount
    $keep = @{ $rootPid = $true }
    do {
        $added = $false
        foreach ($p in $all) {
            if (-not $keep[[int]$p.ProcessId] -and $keep[[int]$p.ParentProcessId]) { $keep[[int]$p.ProcessId] = $true; $added = $true }
        }
    } while ($added)
    $all | Where-Object { $keep[[int]$_.ProcessId] }
}

function Run-Browser($url) {
    $profileDir = Join-Path $tmp ("profile-" + [guid]::NewGuid().ToString("N"))
    $dom = Join-Path $tmp "dom.html"
    $argv = @("--headless=new", "--disable-gpu", "--no-first-run", "--disable-extensions",
              "--user-data-dir=`"$profileDir`"", "--virtual-time-budget=8000", "--dump-dom", "`"$url`"")
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $Browser -ArgumentList $argv -RedirectStandardOutput $dom -RedirectStandardError (Join-Path $tmp "err.txt") -PassThru -WindowStyle Hidden
    $peakWs = 0; $peakPriv = 0; $peakProcs = 0; $timedOut = $false
    while (-not $proc.HasExited) {
        $tree = @(Get-Tree $proc.Id $exeName)
        $ws = ($tree | Measure-Object WorkingSetSize -Sum).Sum
        $priv = ($tree | Measure-Object PrivatePageCount -Sum).Sum
        if ($ws -gt $peakWs) { $peakWs = $ws }
        if ($priv -gt $peakPriv) { $peakPriv = $priv }
        if ($tree.Count -gt $peakProcs) { $peakProcs = $tree.Count }
        if ($sw.Elapsed.TotalSeconds -gt $TimeoutSec) {
            $timedOut = $true
            & taskkill /PID $proc.Id /T /F 2>$null | Out-Null
            break
        }
        Start-Sleep -Milliseconds 100
    }
    $sw.Stop()
    $chars = 0
    if (Test-Path $dom) { $chars = (Get-Content $dom -Raw -Encoding UTF8).Length }
    Start-Sleep -Milliseconds 300
    Remove-Item $profileDir -Recurse -Force -ErrorAction SilentlyContinue
    [pscustomobject]@{
        ws_mb = [math]::Round($peakWs / 1MB, 1); priv_mb = [math]::Round($peakPriv / 1MB, 1)
        procs = $peakProcs; ms = [int]$sw.ElapsedMilliseconds; tokens = [int]($chars / 4)
        ok = (-not $timedOut) -and $chars -gt 200
    }
}

function Run-Jj1($url) {
    $errFile = Join-Path $tmp "jj1-err.txt"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $raw = & $Jj1 open $url --json 2>$errFile
    $code = $LASTEXITCODE
    $sw.Stop()
    if ($code -ne 0) {
        return [pscustomobject]@{ ok = $false; error = ((Get-Content $errFile -Raw) -replace '^jj1: ', '').Trim(); ms = [int]$sw.ElapsedMilliseconds }
    }
    $j = ($raw | Out-String) | ConvertFrom-Json
    [pscustomobject]@{
        ok = $true; error = ""; ms = [int]$sw.ElapsedMilliseconds; status = $j.status
        rss_mb = [math]::Round($j.peak_rss_kb / 1024, 1); mcp_tokens = $j.mcp_tokens
        full_tokens = $j.tokens; raw_tokens = $j.raw_tokens; needs_js = [bool]$j.needs_js
    }
}

function Median($values) {
    $v = @($values | Sort-Object)
    if ($v.Count -eq 0) { return 0 }
    $m = [int][math]::Floor($v.Count / 2)
    if ($v.Count % 2) { $v[$m] } else { ($v[$m - 1] + $v[$m]) / 2 }
}

$rows = @()
$list = Get-Content $Urls | Where-Object { $_.Trim() -and -not $_.StartsWith("#") }
$i = 0
foreach ($url in $list) {
    $i++
    Write-Host ("[{0}/{1}] {2}" -f $i, $list.Count, $url)
    $j = Run-Jj1 $url
    $b = Run-Browser $url
    $rows += [pscustomobject]@{ url = "$url"; jj1 = $j; browser = $b }
}

$stamp = Get-Date -Format "yyyy-MM-dd"
$rows | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $Out "bench-$stamp.json") -Encoding UTF8

$both = @($rows | Where-Object { $_.jj1.ok -and $_.browser.ok })
$static = @($both | Where-Object { -not $_.jj1.needs_js })
$md = @()
$md += "# JJ1 benchmark - $stamp"
$md += ""
$md += "Machine: $((Get-CimInstance Win32_Processor).Name.Trim()), $([math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)) GB RAM, $((Get-CimInstance Win32_OperatingSystem).Caption)."
$md += "JJ1: $((& $Jj1 --version) -join ' ') (static tier, release build). Browser: $exeName $((Get-Item $Browser).VersionInfo.ProductVersion), ``--headless=new --dump-dom``."
$md += "One run per page, cold start for both. See the caveats at the top of ``bench.ps1``."
$md += ""
$md += "| Page | JJ1 RAM (MB) | Browser RAM sum / private (MB) | JJ1 time (ms) | Browser time (ms) | JJ1 tokens (default open) | JJ1 tokens (whole page) | Browser DOM tokens | Note |"
$md += "|---|---:|---:|---:|---:|---:|---:|---:|---|"
foreach ($r in $rows) {
    $j = $r.jj1; $b = $r.browser
    $note = ""
    if (-not $j.ok) { $note = "JJ1 refused/failed: " + (((($j.error -replace '\s+', ' ') -replace '^.*?jj1: ', '') -replace ' At \S+:\d+ char:.*$', '') -replace '\|', '/') }
    elseif ($j.status -ge 400) { $note = "JJ1 got HTTP $($j.status)" }
    elseif ($j.needs_js) { $note = "needs JS: static tier is missing content" }
    if (-not $b.ok) { $note = ($note + " browser failed/timed out").Trim() }
    $jr = if ($j.ok) { $j.rss_mb } else { "-" }
    $jt = if ($j.ok) { $j.mcp_tokens } else { "-" }
    $jf = if ($j.ok) { $j.full_tokens } else { "-" }
    $md += "| $($r.url) | $jr | $($b.ws_mb) / $($b.priv_mb) | $($j.ms) | $($b.ms) | $jt | $jf | $($b.tokens) | $note |"
}
$md += ""
$md += "## Summary"
$md += ""
$md += "Pages where both tools returned a page: $($both.Count) of $($rows.Count). Of those, $($static.Count) did not need JavaScript."
$md += ""
function Line($label, $set) {
    if ($set.Count -eq 0) { return "| $label | - | - | - | - | - | - |" }
    $jr = Median ($set | ForEach-Object { $_.jj1.rss_mb }); $bw = Median ($set | ForEach-Object { $_.browser.ws_mb }); $bp = Median ($set | ForEach-Object { $_.browser.priv_mb })
    $jm = Median ($set | ForEach-Object { $_.jj1.ms }); $bm = Median ($set | ForEach-Object { $_.browser.ms })
    $jt = Median ($set | ForEach-Object { $_.jj1.mcp_tokens }); $bt = Median ($set | ForEach-Object { $_.browser.tokens })
    $worstJ = ($set | ForEach-Object { $_.jj1.rss_mb } | Measure-Object -Maximum).Maximum
    "| $label | $jr (worst $worstJ) | $bw / $bp | {0:N0}% / {1:N0}% | $jm vs $bm | $jt vs $bt | {2:N1}x |" -f (100 * (1 - $jr / $bw)), (100 * (1 - $jr / $bp)), ($bt / [math]::Max($jt, 1))
}
$md += "| Set (medians) | JJ1 RAM MB | Browser RAM MB (sum / private) | RAM saved vs sum / vs private | Time ms (JJ1 vs browser) | Tokens (JJ1 default open vs DOM) | Token reduction |"
$md += "|---|---:|---:|---:|---:|---:|---:|"
$md += Line "All pages both loaded" $both
$md += Line "Pages that work without JS" $static
$md += ""
$md | Set-Content (Join-Path $Out "bench-$stamp.md") -Encoding UTF8
Write-Host ("wrote " + (Join-Path $Out "bench-$stamp.md"))
