# Build and run the native protocol tests with MSVC.
#
# These compile the real msgpack-arduino sources on the non-Arduino path -- the same path the
# RS485 bootloader builds against -- so a passing run is evidence about the parser that actually
# ships, not about a re-implementation of it.
#
# MSVC rather than gcc because there is no native gcc/g++ on the Windows bench machines here;
# PlatformIO's `native` platform needs one. If you have gcc, the same sources build with:
#   g++ -std=c++17 -I<lib>/src <lib>/src/msgpack/*.cpp <lib>/src/msgpack/lwrb.c *_test.cpp

[CmdletBinding()]
param(
    [switch]$KeepIntermediates
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$testDir = $PSScriptRoot
$repoRoot = (Resolve-Path (Join-Path $testDir "..\..")).Path
$libSrc = Join-Path $repoRoot "PortalFW\lib\msgpack-arduino\src"

if (-not (Test-Path -LiteralPath (Join-Path $libSrc "msgpack.hpp"))) {
    throw "msgpack-arduino sources not found at $libSrc. Run: git submodule update --init --recursive"
}

# Locate MSVC. vswhere ships with any VS 2017+ installer.
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path -LiteralPath $vswhere)) {
    throw "vswhere.exe not found; a Visual Studio C++ toolchain is required."
}
$vsPath = (& $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath | Out-String).Trim()
if ([string]::IsNullOrWhiteSpace($vsPath)) {
    throw "No Visual Studio installation with the C++ toolchain was found."
}
$vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path -LiteralPath $vcvars)) {
    throw "vcvars64.bat not found at $vcvars"
}

$buildDir = Join-Path $testDir "build"
New-Item -ItemType Directory -Force -Path $buildDir | Out-Null

# The library's C++ translation units, plus lwrb.c which carries its own extern "C" guards.
$librarySources = @(
    "msgpack\COBSRWStream.cpp"
    "msgpack\DataType.cpp"
    "msgpack\deserialize.cpp"
    "msgpack\logError.cpp"
    "msgpack\Messaging.cpp"
    "msgpack\NotArduino.cpp"
    "msgpack\serialize.cpp"
    "msgpack\Serializer.cpp"
    "msgpack\lwrb.c"
) | ForEach-Object { '"' + (Join-Path $libSrc $_) + '"' }

# @() so a single match still exposes .Count under Set-StrictMode.
$tests = @(Get-ChildItem -LiteralPath $testDir -Filter "*_test.cpp" | Sort-Object Name)
if ($tests.Count -eq 0) {
    throw "No *_test.cpp files found in $testDir"
}

$failed = @()
foreach ($test in $tests) {
    $name = [System.IO.Path]::GetFileNameWithoutExtension($test.Name)
    $exe = Join-Path $buildDir "$name.exe"

    Write-Host "=== building $name ===" -ForegroundColor Cyan
    $objDir = Join-Path $buildDir $name
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null

    # /Fo needs a trailing backslash to mean "directory", but a lone \ immediately before the
    # closing quote is read as an escaped quote and swallows the rest of the command line
    # (D8003: missing source filename). Doubling it is the fix.
    # /Gy + /OPT:REF is the MSVC equivalent of the firmware's -ffunction-sections +
    # --gc-sections, and it matters here rather than being an optimisation: msgpack::String's
    # constructors are declared but never defined, on target as well as on the host. Only
    # readStringNew() references them, nothing calls readStringNew, and section GC drops it
    # before the linker asks. Without /OPT:REF the host link fails on symbols the real firmware
    # also does not have.
    $clArgs = @(
        "/nologo", "/std:c++17", "/EHsc", "/O2", "/W3", "/Gy",
        "/I`"$libSrc`"",
        "/Fo:`"$objDir\\`"",
        "/Fe:`"$exe`"",
        "`"$($test.FullName)`"",
        "`"$(Join-Path $testDir 'platform_shim.cpp')`""
    ) + $librarySources + @("/link", "/OPT:REF")

    # cl needs the vcvars environment, which only a cmd session can establish.
    $command = "`"$vcvars`" >nul 2>&1 && cl $($clArgs -join ' ')"
    & cmd /c $command
    if ($LASTEXITCODE -ne 0) {
        $failed += "$name (build)"
        continue
    }

    Write-Host "=== running $name ===" -ForegroundColor Cyan
    & $exe
    if ($LASTEXITCODE -ne 0) {
        $failed += "$name (run)"
    }
    Write-Host ""
}

if (-not $KeepIntermediates) {
    Get-ChildItem -LiteralPath $buildDir -Directory -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failed.Count -gt 0) {
    throw "Failed: $($failed -join ', ')"
}
Write-Host "All native protocol tests passed." -ForegroundColor Green
