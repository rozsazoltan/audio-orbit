$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..")
$CacheRoot = Join-Path $RepoRoot ".cache"

New-Item -ItemType Directory -Force -Path $CacheRoot | Out-Null

$env:CARGO_HOME = Join-Path $CacheRoot "cargo-home"
$env:CARGO_TARGET_DIR = Join-Path $CacheRoot "cargo-target"
$env:AUDIO_ORBIT_CACHE_ROOT = $CacheRoot
$env:AUDIO_ORBIT_APP_DATA_DIR = Join-Path $CacheRoot "app-data"

$Commit = "dev"
try {
    $ResolvedCommit = git -C $RepoRoot rev-parse --short=12 HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($ResolvedCommit)) {
        $Commit = $ResolvedCommit.Trim()
    }
} catch {
    $Commit = "dev"
}

$env:AUDIO_ORBIT_DEV_VERSION = "v0.0.0-$Commit"

Write-Host "Audio Orbit dev cache: $CacheRoot"
Write-Host "Audio Orbit dev version: $env:AUDIO_ORBIT_DEV_VERSION"

Push-Location $RepoRoot
try {
    cargo run --bin audio-orbit -- @args
} finally {
    Pop-Location
}
