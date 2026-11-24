# Builds every plugin and copies the exes listed in payload.txt into payload\. No admin needed.
#   powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1
# cargo only rebuilds what changed, so this is also the fast path after editing one plugin.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
function Build([string]$dir, [string[]]$a) {
    Push-Location $dir
    try { cargo build --release @a; if ($LASTEXITCODE) { throw "cargo build failed in $dir" } } finally { Pop-Location }
}
Build $root '--workspace', '--exclude', 'plugin-launcher' # the launcher embeds payload\, so it is built after the copy
Build "$root\plugins\rust-search" @()                     # outside the workspace (needs panic = unwind, see Cargo.toml)
New-Item -ItemType Directory -Force "$root\payload" | Out-Null
$targets = @{} # project folder -> cargo's real output folder
foreach ($l in Get-Content "$root\payload.txt") {
    if ($l -match '^\s*(#|$)') { continue }
    $name, $src = $l -split '=', 2
    # "<project>/target/<rest>" lives in <project>'s real cargo output folder: a global build.target-dir
    # (~/.cargo/config.toml) moves it out of the repo, and copying <project>\target would ship stale exes
    $src = $src.Trim() -replace '/', '\'
    if ($src -match '^(.*?)\\?target\\(.*)$') {
        $proj = $Matches[1]; $rest = $Matches[2]
        if (-not $targets.ContainsKey($proj)) { $targets[$proj] = & "$PSScriptRoot\target-dir.ps1" $proj }
        $src = Join-Path $targets[$proj] $rest
    } else { $src = Join-Path $root $src }
    Copy-Item $src (Join-Path "$root\payload" $name.Trim()) -Force
}
Write-Host "payload\ refreshed"
