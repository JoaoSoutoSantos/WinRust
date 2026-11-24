# Prints cargo's real output folder for the project in -Dir (default: the repo root). Dot-source-free helper:
#   $t = & "$PSScriptRoot\target-dir.ps1"            -> e.g. C:\repo\target, or a build.target-dir from ~/.cargo/config.toml
#   $t = & "$PSScriptRoot\target-dir.ps1" plugins\rust-search
# The scripts used to assume <project>\target and silently copied stale exes when a global target-dir was set.
param([string]$Dir = '')
$root = Split-Path $PSScriptRoot
$p = if ($Dir) { Join-Path $root $Dir } else { $root }
Push-Location $p
try {
    $json = cargo metadata --format-version 1 --no-deps 2>$null
    if ($LASTEXITCODE -or -not $json) { return (Join-Path $p 'target') }
    return (($json | ConvertFrom-Json).target_directory -replace '/', '\')
} finally { Pop-Location }
