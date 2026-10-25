# Builds Levitas and installs it into a server folder kept apart from the source code,
# which is where the server is played from.
#
# Usage:  powershell -ExecutionPolicy Bypass -File tools\deploy.ps1 [-ServerDir <folder>]
# The default folder is levitas-server next to this repository.

param(
    [string]$ServerDir = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) "levitas-server")
)
$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

$exe = Join-Path $ServerDir "levitas.exe"
$running = Get-Process levitas -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }
if ($running) {
    throw "The server in $ServerDir is running. Stop it with Ctrl-C in its window first."
}

$target = Join-Path $repo "target\deploy"
cargo build --release -p levitas-server --target-dir $target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force $ServerDir | Out-Null
Copy-Item (Join-Path $target "release\levitas.exe") $exe -Force

# A world played during development moves to the server folder once.
$oldWorld = Join-Path $repo "world"
$newWorld = Join-Path $ServerDir "world"
if ((Test-Path $oldWorld) -and -not (Test-Path $newWorld)) {
    Move-Item $oldWorld $newWorld
    Write-Host "Moved the world from $oldWorld"
}

$start = Join-Path $ServerDir "start.bat"
if (-not (Test-Path $start)) {
    Set-Content -Encoding ascii $start "@echo off`r`ncd /d `"%~dp0`"`r`nlevitas.exe`r`npause`r`n"
}

Write-Host "Levitas installed in $ServerDir. Run start.bat there to play; stop with Ctrl-C."
