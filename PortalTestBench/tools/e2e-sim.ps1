<#
.SYNOPSIS
  The agent's own path, exercised end to end with no probe, no serial port and no board.

.DESCRIPTION
  A wrapper. This gate is now gate 6 of tools/test.mjs, which runs it directly -- see `e2eSim`
  there for what it asserts and, more importantly, what it does not cover yet.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
node (Join-Path $PSScriptRoot 'test.mjs') --only-e2e
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
