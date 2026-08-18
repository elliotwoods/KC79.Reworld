<#
.SYNOPSIS
  The agent's own path, exercised end to end with no probe, no serial port and no board.

.DESCRIPTION
  This is the gate that matters most for this product. The GUI gets looked at; the CLI path is
  the one an agent uses unattended, and it is the one that rots silently. What it must prove,
  once the engine lands (M3):

    ptb --local --sim run plans\smoke.toml --wait
        exits 0, and the session NDJSON holds exactly one `verdict` line reading "pass"

    ptb --local --sim --inject fail-backlash run plans\smoke.toml --wait
        exits 1 and names the criterion that failed -- a fail must be a real answer, not an error

    ptb --local --sim run plans\soak-short.toml --detach ; ptb abort
        exits 2, and the NDJSON holds an `escape` line -- the abort path is never exercised by
        accident, so it has to be exercised on purpose

  Until then this asserts what does exist and says plainly what it is not yet covering, rather
  than passing quietly and reading as though the whole path were green.

.NOTES
  PowerShell 5.1. Do not redirect native stderr with 2>&1.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$app = Split-Path -Parent $PSScriptRoot
$ptb = Join-Path $app 'target\debug\ptb.exe'

if (-not (Test-Path $ptb)) {
    throw "ptb.exe not found at $ptb. Run: powershell -File tools\build.ps1"
}

# --- what is wired today -----------------------------------------------------------------
$version = & $ptb version
if ($LASTEXITCODE -ne 0) { throw "ptb version exited $LASTEXITCODE" }

$parsed = $version | ConvertFrom-Json
if (-not $parsed.report_profile) { throw "ptb version did not report a report profile: $version" }
# The profile string is written into every session file and is how a reader years later knows
# what shape of NDJSON they are holding. If it changes, it changes deliberately.
if ($parsed.report_profile -ne 'bench/1') {
    throw "report profile is '$($parsed.report_profile)', expected 'bench/1'"
}
Write-Host "    ptb version ok -- report profile $($parsed.report_profile)"

# A command that is not wired yet must fail loudly rather than print an empty document that
# would read like an answer.
#
# Note what is NOT written here: `& $ptb state 2>$null`. In PowerShell 5.1 redirecting a native
# executable's stderr wraps each line in an ErrorRecord, and under `$ErrorActionPreference =
# 'Stop'` that terminates the script even though the exe did exactly what we are asserting it
# does. Let stderr through and read the exit code instead.
$expectedFailure = $false
try {
    $ErrorActionPreference = 'Continue'
    & $ptb state | Out-Null
    $expectedFailure = ($LASTEXITCODE -ne 0)
} finally {
    $ErrorActionPreference = 'Stop'
}
if (-not $expectedFailure) {
    throw 'ptb state exited 0 but the bench worker does not exist yet -- it must not fake success'
}
Write-Host '    ptb state correctly refuses rather than printing an empty answer'

# --- what is not covered yet -------------------------------------------------------------
Write-Host ''
Write-Host '    NOT YET COVERED (lands with the engine, M3):' -ForegroundColor Yellow
Write-Host '      - run a plan to a pass verdict and assert the NDJSON' -ForegroundColor Yellow
Write-Host '      - a failing criterion exits 1 and names itself' -ForegroundColor Yellow
Write-Host '      - abort exits 2 and records an escape' -ForegroundColor Yellow

# Exit explicitly. Without this the caller sees $LASTEXITCODE left over from the last native
# command above -- which is deliberately non-zero -- and reads a passing gate as a failure.
exit 0
