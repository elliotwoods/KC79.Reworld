<#
.SYNOPSIS
  build PortalTestBench. A wrapper; the implementation is build.mjs.

.DESCRIPTION
  This script used to be the implementation. It is now three lines, and that is the point: the
  same build has to run on Windows and macOS, and two implementations of one build is the shape
  this tree has been bitten by before -- the one that works is the one you check, and the other
  rots.

  Everything that was in here, including the reasons, moved to build.mjs unchanged.
  Node is already a prerequisite: the web bundle is built with it.

  Kept as an entry point because AGENTS.md names these three scripts, and because
  `powershell -File tools\build.ps1` is in people's shell history.
#>
[CmdletBinding()]
param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest)

$ErrorActionPreference = 'Stop'
node (Join-Path $PSScriptRoot 'build.mjs') @Rest
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
