# Install JJ1 for the current user (no admin rights needed).
#   powershell -File install.ps1              # build if needed, install, add shortcuts
#   powershell -File install.ps1 -NoDesktop   # Start Menu shortcut only
#   powershell -File install.ps1 -Uninstall   # remove the program and shortcuts
#
# Installs to %LOCALAPPDATA%\Programs\JJ1 and creates a "JJ1 Browser" shortcut
# in the Start Menu (and on the desktop). It does not touch PATH, the registry
# or your default browser. Browsing data lives in %APPDATA%\jj1 and is left
# alone by -Uninstall.
param([switch]$Uninstall, [switch]$NoDesktop)

$dest = Join-Path $env:LOCALAPPDATA "Programs\JJ1"
$startMenu = Join-Path ([Environment]::GetFolderPath("Programs")) "JJ1 Browser.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("Desktop")) "JJ1 Browser.lnk"

if ($Uninstall) {
    Remove-Item $startMenu, $desktop -ErrorAction SilentlyContinue
    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    Write-Host "JJ1 removed. Browsing data in $env:APPDATA\jj1 was kept."
    exit 0
}

$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$src = Join-Path $target "release"
if (-not (Test-Path (Join-Path $src "jj1-browser.exe"))) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "build.ps1")
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

New-Item -ItemType Directory -Force $dest | Out-Null
$files = "jj1.exe", "jj1-view.exe", "jj1-browser.exe", "WebView2Loader.dll"
foreach ($f in $files) {
    $from = Join-Path $src $f
    if (Test-Path $from) { Copy-Item $from $dest -Force } else { Write-Host "missing: $f (run build.ps1 first)"; exit 1 }
}

$shell = New-Object -ComObject WScript.Shell
$links = @($startMenu)
if (-not $NoDesktop) { $links += $desktop }
foreach ($path in $links) {
    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = Join-Path $dest "jj1-browser.exe"
    $lnk.WorkingDirectory = $dest
    $lnk.Description = "JJ1 Browser"
    $lnk.Save()
}

Write-Host "Installed to $dest"
Write-Host "Shortcuts: $($links -join ', ')"
Write-Host "Agent side: `"$dest\jj1.exe`" mcp"
