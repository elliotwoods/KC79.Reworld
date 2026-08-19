<#
.SYNOPSIS
    Build the web bundle and then the application.

.DESCRIPTION
    In that order, and not in parallel: `web_assets!` embeds web/dist at compile time, so a Rust
    build that runs first embeds whatever the last web build left behind. The symptom is a binary
    that serves a stale page with no warning anywhere, which is a genuinely difficult thing to
    notice.

    Every cargo invocation passes an absolute --manifest-path. AGENTS.md records why: cargo
    resolves a relative manifest against the current directory, and this application has a second
    complete workspace inside third_party/av-frameworks. A shell that has wandered in there builds
    the framework instead, successfully, and nothing says so.

.PARAMETER Release
    Optimised build. What actually goes on the bench.

.PARAMETER SkipWeb
    Reuse the existing web/dist. For a Rust-only edit, where the page has not changed.
#>
[CmdletBinding()]
param(
    [switch] $Release,
    [switch] $SkipWeb
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$app = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $app 'Cargo.toml'
$web = Join-Path $app 'web'

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }

if (-not $SkipWeb) {
    Step 'Web bundle'
    if (-not (Test-Path (Join-Path $web 'node_modules'))) {
        throw 'web/node_modules is missing -- run tools\bootstrap.ps1 first'
    }
    npm --prefix $web run build
    if ($LASTEXITCODE -ne 0) { throw 'the web build failed' }
} elseif (-not (Test-Path (Join-Path $web 'dist\index.html'))) {
    # -SkipWeb with nothing to skip would embed an empty directory and produce a binary that
    # serves a blank page.
    throw 'web/dist is empty, so there is nothing to skip -- run without -SkipWeb'
}

Step "Application ($(if ($Release) { 'release' } else { 'debug' }))"
$args = @(
    'build',
    '--manifest-path', $manifest,
    '-p', 'portal-flasher',
    # The default composed-window shell refuses to start without its dedicated CEF renderer,
    # GPU and utility-process helper beside the main executable. It is a workspace binary rather
    # than a dependency target, so Cargo will not build it unless it is named explicitly here.
    '-p', 'av-gui-subprocess'
)
if ($Release) { $args += '--release' }
cargo @args
if ($LASTEXITCODE -ne 0) { throw 'the cargo build failed' }

$profile = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $app "target\$profile\portal-flasher.exe"
Step 'Built'
Write-Host "    $exe" -ForegroundColor DarkGray
Write-Host '    --simulate   no probe needed' -ForegroundColor DarkGray
Write-Host '    --headless   serve the page without opening a window' -ForegroundColor DarkGray
