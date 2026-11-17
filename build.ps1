# Build JJ1 and put everything the WebView tier needs next to the binaries.
#   powershell -File build.ps1            # release build
#   powershell -File build.ps1 -Debug     # debug build (what `cargo test` uses)
param([switch]$Debug)

$profileName = if ($Debug) { "debug" } else { "release" }
$cargoArgs = @("build")
if (-not $Debug) { $cargoArgs += "--release" }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$out = Join-Path $target $profileName

# On the GNU toolchain jj1-view.exe loads WebView2Loader.dll at run time. The
# webview2-com-sys build script unpacks it; copy the 64-bit one beside the exe.
if ($env:OS -eq "Windows_NT") {
    $dll = Get-ChildItem (Join-Path $out "build") -Recurse -Filter WebView2Loader.dll -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '[\\/]x64[\\/]' } | Select-Object -First 1
    if ($dll) {
        Copy-Item $dll.FullName $out -Force
        Write-Host "copied WebView2Loader.dll"
    } else {
        Write-Host "WebView2Loader.dll not found in the build output (not needed on the MSVC toolchain)"
    }
}
Write-Host "binaries: $out"
Get-ChildItem $out -Filter "jj1*.exe" | ForEach-Object { "  {0}  {1:N1} MB" -f $_.Name, ($_.Length / 1MB) }
