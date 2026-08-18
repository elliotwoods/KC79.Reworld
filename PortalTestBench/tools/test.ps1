<#
.SYNOPSIS
  Every gate that has to be green before claiming PortalTestBench works.

.DESCRIPTION
  Ordered fail-fastest-first so a broken build is reported in seconds rather than minutes.

  Gate 4 (clippy) is SCOPED to this workspace's own packages, deliberately. An unscoped
  `cargo clippy --all` or `cargo fmt --all` reaches through the third_party junction and
  rewrites the pinned framework -- and because that junction points at PortalFlasher's
  submodule, it dirties PortalFlasher's checkout too. That is worse here than it is there.
  Gate 7 exists to catch it if it happens anyway.

  Gate 6 is the one that matters most for this product: it exercises the agent's own path
  (ptb -> engine -> verdict -> NDJSON) end to end with no probe, no serial port and no board.

.NOTES
  PowerShell 5.1. Do not redirect native stderr with 2>&1.
#>
[CmdletBinding()]
param(
    # Gates 1-3 only: the inner loop while writing code.
    [switch] $Fast
)

$ErrorActionPreference = 'Stop'
$app      = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $app 'Cargo.toml'
$web      = Join-Path $app 'web'
$fw       = Join-Path $app 'third_party\av-frameworks'
$gate     = 0

function Start-Gate { param([string] $Message) $script:gate++; Write-Host "`n==> [$script:gate] $Message" -ForegroundColor Cyan }
function Assert-Ok  { param([string] $What) if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" } }

Start-Gate 'cargo test (engine, verdicts, plan validation, transports, protocol goldens)'
cargo test --manifest-path $manifest --workspace
Assert-Ok 'cargo test'

Start-Gate 'vitest (the pure page models)'
Push-Location $web
try {
    npx vitest run
    Assert-Ok 'vitest'
} finally { Pop-Location }

Start-Gate 'tsc + vite build'
Push-Location $web
try {
    npm run build
    Assert-Ok 'web build'
} finally { Pop-Location }

if ($Fast) { Write-Host "`nFast gates passed (1-3). Run without -Fast before claiming anything works." -ForegroundColor Green; exit 0 }

Start-Gate 'clippy (scoped -- never --all, see the note above)'
cargo clippy --manifest-path $manifest -p bench-core -p portal-test-bench -p ptb --all-targets --all-features -- -D warnings
Assert-Ok 'clippy'

Start-Gate 'check-av-app (the framework application contract)'
powershell -File (Join-Path $fw 'tools\check-av-app.ps1') -AppPath $app
Assert-Ok 'check-av-app.ps1'

Start-Gate 'simulated end-to-end (the agent path, with no hardware)'
& (Join-Path $PSScriptRoot 'e2e-sim.ps1')
Assert-Ok 'simulated end-to-end'

Start-Gate 'framework checkout still clean'
$dirty = & git -C $fw status --porcelain
if ($LASTEXITCODE -ne 0) {
    Write-Host '    (could not read framework git status; skipping)' -ForegroundColor Yellow
} elseif ($dirty) {
    throw @"
The pinned framework checkout is dirty:
$dirty
Something ran an unscoped fmt/clippy/fix. This junction is PortalFlasher's submodule, so its
checkout is dirty too. Revert it there before committing anything.
"@
}

Write-Host "`nAll gates passed." -ForegroundColor Green
