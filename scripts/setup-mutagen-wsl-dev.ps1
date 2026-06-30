param(
    [string]$SessionName = "audio-orbit-win-dev",
    [string]$WindowsProjectPath
)

$ErrorActionPreference = "Stop"

function Get-WorkspaceRoot {
    if (-not $PSScriptRoot) {
        throw "This script must be run from a saved .ps1 file inside the repository scripts directory."
    }

    return (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

if (-not (Get-Command mutagen -ErrorAction SilentlyContinue)) {
    throw "Mutagen was not found on PATH. Install mutagen.exe on Windows first, then reopen PowerShell."
}

$SourceWorkspace = Get-WorkspaceRoot

if (-not (Test-Path $SourceWorkspace)) {
    throw "Workspace root was not found: $SourceWorkspace"
}

Write-Host "Audio Orbit Mutagen Windows development setup"
Write-Host ""
Write-Host "Source workspace: $SourceWorkspace"
Write-Host ""

if ([string]::IsNullOrWhiteSpace($WindowsProjectPath)) {
    $WindowsProjectPath = Read-Host "Windows mirror path, for example D:\github\<owner>\audio-orbit"
}

$WindowsProjectPath = $WindowsProjectPath.Trim().Trim('"')
if ([string]::IsNullOrWhiteSpace($WindowsProjectPath)) {
    throw "Windows mirror path is required."
}

$SourceFullPath = [System.IO.Path]::GetFullPath($SourceWorkspace)
$TargetFullPath = [System.IO.Path]::GetFullPath($WindowsProjectPath)

if ($SourceFullPath.TrimEnd('\') -ieq $TargetFullPath.TrimEnd('\')) {
    throw "The source workspace and Windows mirror path must be different."
}

New-Item -ItemType Directory -Force -Path $TargetFullPath | Out-Null

$ExistingSession = mutagen sync list --long 2>$null | Select-String -SimpleMatch "Name: $SessionName"
if ($ExistingSession) {
    Write-Host "Mutagen session '$SessionName' already exists."
    Write-Host "Use 'mutagen sync monitor $SessionName' to watch it, or terminate it first if you want to recreate it."
} else {
    mutagen sync create `
        --name $SessionName `
        --sync-mode two-way-safe `
        --ignore-vcs `
        --ignore ".cache" `
        --ignore "target" `
        --ignore "*.zip" `
        $SourceFullPath `
        $TargetFullPath
}

Write-Host ""
Write-Host "Source workspace: $SourceFullPath"
Write-Host "Windows mirror:   $TargetFullPath"
Write-Host "Mutagen session:  $SessionName"
Write-Host ""
Write-Host "Run the Windows dev app from PowerShell:"
Write-Host "  cd $TargetFullPath"
Write-Host "  cargo dev"
Write-Host ""
Write-Host "Keep Git operations on the source workspace side."
Write-Host ""
Write-Host "Useful Mutagen commands:"
Write-Host "  mutagen sync list"
Write-Host "  mutagen sync monitor $SessionName"
Write-Host "  mutagen sync flush $SessionName"
Write-Host "  mutagen sync terminate $SessionName"
