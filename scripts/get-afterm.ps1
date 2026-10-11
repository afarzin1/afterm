<#
.SYNOPSIS
Download afterm.exe from the latest pipeline run and drop it over the installed
copy. Avoids the browser, which blocks unsigned executables.

.DESCRIPTION
Nothing here needs the installer: Tauri compiles the frontend into afterm.exe,
so replacing that one file updates an existing installation.

It reads from whichever Azure DevOps organisation and project run
azure-pipelines.yml; name them with -Organisation and -Project. Authentication
uses a Personal Access Token with "Build (read)" scope, created under User
settings > Personal access tokens in that organisation. Pass it as -Token or
set it once:

    setx AZDO_PAT "your-token"

.EXAMPLE
    ./get-afterm.ps1 -Organisation my-org -Project my-project
    Download the newest afterm.exe from main into the current directory.

.EXAMPLE
    ./get-afterm.ps1 -Organisation my-org -Project my-project -Install
    Download it, close afterm if it is running, and replace the installed copy.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Organisation,
    [Parameter(Mandatory)][string]$Project,
    [string]$Branch = 'main',
    [string]$Artifact = 'afterm-exe',
    [string]$Token = $env:AZDO_PAT,
    [string]$Destination = (Get-Location).Path,
    # Replace the installed executable rather than only downloading it.
    [switch]$Install
)

$ErrorActionPreference = 'Stop'

if (-not $Token) {
    throw 'No token. Pass -Token, or run: setx AZDO_PAT "your-token" (then reopen the shell)'
}

$auth = @{ Authorization = 'Basic ' + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes(":$Token")) }
$base = "https://dev.azure.com/$Organisation/$Project/_apis"

Write-Host "Looking for the newest successful run on $Branch..."
$runs = Invoke-RestMethod -Headers $auth -Uri `
    "$base/build/builds?branchName=refs/heads/$Branch&resultFilter=succeeded&statusFilter=completed&`$top=1&api-version=7.0"

if (-not $runs.value) { throw "No successful run found on $Branch." }
$run = $runs.value[0]
Write-Host ("  run {0}, finished {1}" -f $run.buildNumber, $run.finishTime)

# The exe is published on every artifact-producing run, packaged or not.
$artifacts = Invoke-RestMethod -Headers $auth -Uri `
    "$base/build/builds/$($run.id)/artifacts?api-version=7.0"
$wanted = $artifacts.value | Where-Object { $_.name -eq $Artifact }
if (-not $wanted) {
    $names = ($artifacts.value | ForEach-Object { $_.name }) -join ', '
    throw "Run $($run.buildNumber) has no '$Artifact' artifact. It published: $names"
}

# Artifacts download as a zip regardless of how few files they hold.
$zip = Join-Path $env:TEMP "afterm-$($run.id).zip"
Write-Host 'Downloading...'
Invoke-WebRequest -Headers $auth -Uri $wanted.resource.downloadUrl -OutFile $zip

$unpacked = Join-Path $env:TEMP "afterm-$($run.id)"
if (Test-Path $unpacked) { Remove-Item $unpacked -Recurse -Force }
Expand-Archive -Path $zip -DestinationPath $unpacked
$exe = Get-ChildItem $unpacked -Recurse -Filter 'afterm.exe' | Select-Object -First 1
if (-not $exe) { throw 'No afterm.exe inside the artifact.' }

if (-not $Install) {
    $out = Join-Path $Destination 'afterm.exe'
    Copy-Item $exe.FullName $out -Force
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
        throw 'Could not find an installed afterm.exe. Run the installer once, or use -Destination.'
    }
}

Copy-Item $exe.FullName $installed -Force
Write-Host "Replaced $installed" -ForegroundColor Green
Write-Host ("Built from run {0} ({1})" -f $run.buildNumber, $run.sourceVersion.Substring(0, 8))
