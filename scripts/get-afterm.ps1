<#
.SYNOPSIS
Download afterm.exe from the latest GitHub release and, with -Install, drop it
over the installed copy.

.DESCRIPTION
Nothing here needs the installer: Tauri compiles the frontend into afterm.exe,
so replacing that one file updates an existing installation. Releases are
public, so no token is needed.

.EXAMPLE
    ./get-afterm.ps1
    Download the newest afterm.exe into the current directory.

.EXAMPLE
    ./get-afterm.ps1 -Install
    Download it, close afterm if it is running, and replace the installed copy.

.EXAMPLE
    ./get-afterm.ps1 -Tag v1.0.1 -Install
    Install a specific release rather than the latest.
#>
[CmdletBinding()]
param(
    [string]$Repository = 'afarzin1/afterm',
    # A release tag such as v1.0.1. Empty means the latest release.
    [string]$Tag = '',
    [string]$Destination = (Get-Location).Path,
    # Replace the installed executable rather than only downloading it.
    [switch]$Install
)

$ErrorActionPreference = 'Stop'

$api = if ($Tag) {
    "https://api.github.com/repos/$Repository/releases/tags/$Tag"
} else {
    "https://api.github.com/repos/$Repository/releases/latest"
}

Write-Host "Looking up $(if ($Tag) { $Tag } else { 'the latest release' }) of $Repository..."
try {
    $release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'get-afterm' }
} catch {
    throw "No release found at $api. Has one been published yet?"
}
Write-Host ("  {0}, published {1}" -f $release.tag_name, $release.published_at)

$asset = $release.assets | Where-Object { $_.name -eq 'afterm.exe' } | Select-Object -First 1
if (-not $asset) {
    $names = ($release.assets | ForEach-Object { $_.name }) -join ', '
    throw "Release $($release.tag_name) has no afterm.exe. It has: $names"
}

$download = Join-Path $env:TEMP "afterm-$($release.tag_name).exe"
Write-Host 'Downloading...'
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $download -Headers @{ 'User-Agent' = 'get-afterm' }

if (-not $Install) {
    $out = Join-Path $Destination 'afterm.exe'
    Move-Item $download $out -Force
    Write-Host "Saved to $out" -ForegroundColor Green
    return
}

# Windows will not overwrite a running executable.
$running = Get-Process afterm -ErrorAction SilentlyContinue
if ($running) {
    Write-Host 'Closing afterm...'
    $running | Stop-Process
    Start-Sleep -Seconds 2
}

$installed = Join-Path $env:LOCALAPPDATA 'afterm\afterm.exe'
if (-not (Test-Path $installed)) {
    $found = Get-Command afterm -ErrorAction SilentlyContinue
    if ($found) { $installed = $found.Source } else {
        throw 'Could not find an installed afterm.exe. Run the installer once, or leave off -Install.'
    }
}

Move-Item $download $installed -Force
Write-Host "Replaced $installed with $($release.tag_name)" -ForegroundColor Green
