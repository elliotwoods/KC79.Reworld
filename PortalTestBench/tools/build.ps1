<#
.SYNOPSIS
  Build PortalTestBench: web bundle first, then cargo.

.DESCRIPTION
  The order is load-bearing, not a preference. `av_operator_app::web_assets!` resolves
  `web/dist` at COMPILE time, so a cargo build that runs first embeds whatever the last web
  build left behind. The symptom is a binary that starts cleanly and serves a stale page --
  which reads as a host bug, not a missing build step. Never run the two in parallel.

  Every cargo invocation passes an absolute --manifest-path. There is a second complete Cargo
  workspace behind `third_party/av-frameworks` (a junction into PortalFlasher's submodule), and
  a shell that has wandered in there builds the framework instead, successfully, with nothing
  saying so.

.NOTES
  PowerShell 5.1. Do not redirect native stderr with 2>&1.
#>
[CmdletBinding()]
param(
    [switch] $Release,
    # Skip the web bundle when only Rust changed AND web/dist already exists.
    [switch] $SkipWeb
)

$ErrorActionPreference = 'Stop'
$app      = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $app 'Cargo.toml'
$web      = Join-Path $app 'web'
$dist     = Join-Path $web 'dist'

function Write-Step { param([string] $Message) Write-Host "==> $Message" -ForegroundColor Cyan }

if (-not (Test-Path (Join-Path $app 'third_party\av-frameworks\crates\av-operator-app\Cargo.toml'))) {
    throw 'third_party/av-frameworks is missing. Run: powershell -File tools\bootstrap.ps1'
}

# --- 1. web ------------------------------------------------------------------------------
if ($SkipWeb) {
    if (-not (Test-Path (Join-Path $dist 'index.html'))) {
        throw '-SkipWeb was given but web/dist/index.html does not exist. Build the web package once first.'
    }
    Write-Step 'Skipping the web bundle (web/dist reused)'
} else {
    Write-Step 'Building the web bundle'
    Push-Location $web
    try {
        npm run build
        if ($LASTEXITCODE -ne 0) { throw "web build failed with exit code $LASTEXITCODE" }
    } finally { Pop-Location }
}

# --- 2. cargo ----------------------------------------------------------------------------
$profileArgs = @()
if ($Release) { $profileArgs = @('--release') }

Write-Step "Building the Rust workspace$(if ($Release) { ' (release)' })"
cargo build --manifest-path $manifest -p portal-test-bench -p ptb @profileArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$target = Join-Path $app "target\$(if ($Release) { 'release' } else { 'debug' })"
Write-Host ''
Write-Step 'Build complete.'
Write-Host "  $target\portal-test-bench.exe             # native window + http://127.0.0.1:8770"
Write-Host "  $target\portal-test-bench.exe --headless  # the same page, no window"
Write-Host "  $target\portal-test-bench.exe --simulate  # a modelled module, no hardware"
Write-Host "  $target\ptb.exe state                     # the agent's view of the same bench"
