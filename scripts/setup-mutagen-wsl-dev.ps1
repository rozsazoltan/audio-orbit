param(
    [string]$Distro = "CentOS-Stream-9",
    [string]$WslProjectPath = "/github/rozsazoltan/audio-orbit",
    [string]$WindowsProjectPath = "D:\github\rozsazoltan\audio-orbit",
    [string]$SessionName = "audio-orbit-win-dev"
)

$ErrorActionPreference = "Stop"

function Convert-WslPathToUnc {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DistroName,
        [Parameter(Mandatory = $true)]
        [string]$LinuxPath
    )

    $Normalized = $LinuxPath.Trim()
    if (-not $Normalized.StartsWith("/")) {
        throw "WSL project path must be an absolute Linux path, for example /github/rozsazoltan/audio-orbit."
    }

    $Relative = $Normalized.TrimStart([char]"/").Replace("/", "\")
    return "\\wsl$\$DistroName\$Relative"
}

if (-not (Get-Command mutagen -ErrorAction SilentlyContinue)) {
    throw "Mutagen was not found on PATH. Install mutagen.exe on Windows first, then reopen PowerShell."
}

$WslProjectUnc = Convert-WslPathToUnc -DistroName $Distro -LinuxPath $WslProjectPath

if (-not (Test-Path $WslProjectUnc)) {
    throw "WSL project path was not found from Windows: $WslProjectUnc"
}

New-Item -ItemType Directory -Force -Path $WindowsProjectPath | Out-Null

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
        $WslProjectUnc `
        $WindowsProjectPath
}

Write-Host ""
Write-Host "WSL source:      $WslProjectPath"
Write-Host "Windows mirror:  $WindowsProjectPath"
Write-Host "Mutagen session: $SessionName"
Write-Host ""
Write-Host "Run the Windows dev app from PowerShell:"
Write-Host "  cd $WindowsProjectPath"
Write-Host "  cargo dev"
Write-Host ""
Write-Host "Useful Mutagen commands:"
Write-Host "  mutagen sync list"
Write-Host "  mutagen sync monitor $SessionName"
Write-Host "  mutagen sync flush $SessionName"
Write-Host "  mutagen sync terminate $SessionName"
