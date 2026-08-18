<#
.SYNOPSIS
  Idempotent one-time setup for PortalTestBench.

.DESCRIPTION
  Three jobs, in order:

    1. Point `third_party/av-frameworks` at a framework checkout. This app deliberately does
       NOT carry its own submodule: PortalFlasher already pins one in the same repository, and
       a second checkout would be several hundred megabytes AND a second pinned revision that
       could drift from PortalFlasher's without anything saying so. We create a directory
       junction instead -- the same trick PortalFlasher uses for `vendor/cef`, and explicitly
       permitted by the framework's operator-app-starter.md ("teams that maintain one sibling
       clone for several projects MAY replace that directory with a local link"). Cargo path
       dependencies, npm's `file:` dependency and check-av-app.ps1 all resolve through it
       unchanged.

    2. Record/verify `framework.lock`. The junction means the framework revision is decided by
       PortalFlasher. That is fine, but it must not be *silent* -- so we write the revision we
       bootstrapped against and warn loudly when it has moved.

    3. `npm install` for the web package, run from INSIDE web/. Not `npm --prefix web install`:
       for `install` (as opposed to `run`) npm resolves the `file:` dependency relative to the
       process cwd, not the prefix, and silently produces a broken tree. PortalFlasher's
       bootstrap documents the same trap.

.NOTES
  PowerShell 5.1. `powershell -File`, never `pwsh` -- PowerShell 7 is not installed here.
  Never redirect a native executable's stderr with 2>&1: in 5.1 that wraps each line in an
  ErrorRecord and sets $? to $false even on exit code 0.
#>
[CmdletBinding()]
param(
    # Use a framework checkout other than PortalFlasher's (e.g. C:\dev\av-frameworks).
    [string] $FrameworkPath,
    # Skip the npm step when only the Rust side is being set up.
    [switch] $SkipWeb
)

$ErrorActionPreference = 'Stop'
$app  = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $app
$link = Join-Path $app 'third_party\av-frameworks'
$lock = Join-Path $app 'framework.lock'

function Write-Step { param([string] $Message) Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Warn { param([string] $Message) Write-Host "!!  $Message" -ForegroundColor Yellow }

# --- 1. the framework junction ---------------------------------------------------------
if (-not $FrameworkPath) {
    $FrameworkPath = Join-Path $repo 'PortalFlasher\third_party\av-frameworks'
}

if (-not (Test-Path (Join-Path $FrameworkPath 'crates\av-operator-app\Cargo.toml'))) {
    $sibling = Join-Path (Split-Path -Parent $repo) 'av-frameworks'
    if (Test-Path (Join-Path $sibling 'crates\av-operator-app\Cargo.toml')) {
        Write-Warn "PortalFlasher's submodule is not initialised; falling back to $sibling"
        $FrameworkPath = $sibling
    } else {
        throw @"
No usable av-frameworks checkout found.

Tried: $FrameworkPath
       $sibling

Initialise PortalFlasher's submodule by name -- a bare ``git submodule update --init
--recursive`` at the repository root fails on the broken 'fonts' gitlink:

    git -C "$repo" submodule update --init PortalFlasher/third_party/av-frameworks
"@
    }
}

$existing = Get-Item -LiteralPath $link -ErrorAction SilentlyContinue
if ($existing -and $existing.LinkType -ne 'Junction') {
    throw "$link exists and is not a junction. Remove it and re-run; this app must not own a second framework checkout."
}
if ($existing -and $existing.Target -notcontains (Resolve-Path $FrameworkPath).Path) {
    Write-Warn "Re-pointing the junction to $FrameworkPath"
    Remove-Item -LiteralPath $link -Force
    $existing = $null
}
if (-not $existing) {
    Write-Step "Linking third_party/av-frameworks -> $FrameworkPath"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $link) | Out-Null
    # New-Item -ItemType Junction needs an elevated shell on some configurations; mklink /J
    # does not, and is what PortalFlasher uses.
    cmd /c mklink /J "$link" "$FrameworkPath" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mklink /J failed with exit code $LASTEXITCODE" }
} else {
    Write-Step 'Framework junction already present'
}

# --- 2. the recorded revision -----------------------------------------------------------
$head = (& git -C $FrameworkPath rev-parse HEAD)
if ($LASTEXITCODE -ne 0) {
    Write-Warn 'Could not read the framework revision; skipping the lock check.'
} elseif (Test-Path $lock) {
    $recorded = (Get-Content -LiteralPath $lock -Raw).Trim()
    if ($recorded -ne $head) {
        Write-Warn @"
The framework checkout has moved since this app was bootstrapped.
  recorded: $recorded
  actual:   $head
This is shared with PortalFlasher. Re-run the full test suite before trusting a build, and
update framework.lock deliberately once you have.
"@
    } else {
        Write-Step "Framework at $($head.Substring(0,10)) (matches framework.lock)"
    }
} else {
    Set-Content -LiteralPath $lock -Value $head -Encoding utf8
    Write-Step "Recorded framework revision $($head.Substring(0,10)) in framework.lock"
}

# --- 3. the web package -----------------------------------------------------------------
if (-not $SkipWeb) {
    $web = Join-Path $app 'web'
    Write-Step 'npm install (from inside web/)'
    Push-Location $web
    try {
        if (Test-Path (Join-Path $web 'package-lock.json')) { npm ci } else { npm install }
        if ($LASTEXITCODE -ne 0) { throw "npm failed with exit code $LASTEXITCODE" }
    } finally { Pop-Location }
}

# --- advisory checks ---------------------------------------------------------------------
$pio = Get-Command pio -ErrorAction SilentlyContinue
if (-not $pio) {
    $penv = Join-Path $env:USERPROFILE '.platformio\penv\Scripts\pio.exe'
    if (Test-Path $penv) { Write-Step "PlatformIO at $penv" }
    else { Write-Warn 'PlatformIO not found. `ptb build` will not work; flashing prebuilt artefacts still will.' }
}

Write-Host ''
Write-Step 'Bootstrap complete. Next: powershell -File tools\build.ps1'
