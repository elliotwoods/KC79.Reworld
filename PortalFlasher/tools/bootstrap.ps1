<#
.SYNOPSIS
    Get a fresh clone to the point where build.ps1 and test.ps1 work.

.DESCRIPTION
    Idempotent by design: every step checks before it acts, so running this on an already-working
    checkout is fast and silent rather than a re-download. That matters because the honest use for
    it is "something is broken and I do not know what" as much as "I just cloned this".

    Two things about this repository that a generic bootstrap would get wrong, both documented at
    length in AGENTS.md:

    - The root has a pre-existing broken submodule entry. `fonts` is committed as a gitlink with
      no .gitmodules record, so a plain `git submodule update --init --recursive` at the root
      errors out. This names the submodules it wants instead of asking for all of them.
    - vendor/cef is usually a junction to a sibling clone, to avoid a second 420 MB copy. That is
      a local convenience and not something to recreate here, so this checks the version behind it
      and says what to do rather than silently fetching over the top of it.

.PARAMETER SkipCef
    Do not touch vendor/cef. For CI, or for working on the Rust side without a browser.
#>
[CmdletBinding()]
param(
    [switch] $SkipCef
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$app = Split-Path -Parent $PSScriptRoot
$framework = Join-Path $app 'third_party\av-frameworks'
$repo = Split-Path -Parent $app

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }
function Ok($message) { Write-Host "    $message" -ForegroundColor DarkGray }
function Warn($message) { Write-Host "    $message" -ForegroundColor Yellow }

# ---------------------------------------------------------------- submodules

Step 'Submodules'
# Named explicitly. `--recursive` at the repository root fails on the `fonts` gitlink, and the
# failure is confusing enough that people conclude their clone is broken.
$paths = @('PortalFlasher/third_party/av-frameworks', 'PortalFW/lib/msgpack-arduino')
foreach ($path in $paths) {
    $full = Join-Path $repo ($path -replace '/', '\')
    if (Test-Path (Join-Path $full '.git')) {
        Ok "$path already present"
        continue
    }
    Ok "initialising $path"
    git -C $repo submodule update --init --recursive -- $path
    if ($LASTEXITCODE -ne 0) { throw "could not initialise $path" }
}

# av-frameworks has submodules of its own, and the framework's crates do not build without them.
git -C $framework submodule update --init --recursive
if ($LASTEXITCODE -ne 0) { throw 'could not initialise the framework submodules' }

# ---------------------------------------------------------------- toolchain

Step 'Toolchain'
foreach ($tool in @('cargo', 'node', 'npm')) {
    $found = Get-Command $tool -ErrorAction SilentlyContinue
    if (-not $found) { throw "$tool is not on PATH" }
    Ok "$tool -> $($found.Source)"
}

# rust-toolchain.toml pins the version; this makes rustup honour it now rather than in the middle
# of the first build.
$pinned = Select-String -Path (Join-Path $app 'rust-toolchain.toml') -Pattern '^\s*channel\s*=\s*"(.+)"'
if ($pinned) {
    $channel = $pinned.Matches[0].Groups[1].Value
    Ok "toolchain $channel"
    # No `2>&1`. Windows PowerShell wraps a native command's stderr in ErrorRecords, and with
    # $ErrorActionPreference = 'Stop' that turns rustup's ordinary "syncing channel updates"
    # progress line into a terminating error. rustup's own output is worth seeing anyway.
    rustup toolchain install $channel --profile minimal
    if ($LASTEXITCODE -ne 0) { throw "could not install the $channel toolchain" }
}

# ---------------------------------------------------------------- CEF

if ($SkipCef) {
    Step 'CEF (skipped)'
} else {
    Step 'CEF'
    $vendor = Join-Path $framework 'vendor\cef'
    $lockPath = Join-Path $framework 'cef.lock'
    $wanted = (Get-Content $lockPath -Raw | ConvertFrom-Json).version

    if (-not (Test-Path $vendor)) {
        Ok "fetching $wanted"
        node (Join-Path $framework 'tools\fetch-cef.mjs')
        if ($LASTEXITCODE -ne 0) { throw 'could not fetch CEF' }
    } else {
        # A junction shares its contents with whatever clone it points at, so a mismatch here is
        # not something to fix by downloading -- it means the sibling clone has moved on, and
        # overwriting it would break that one too.
        $item = Get-Item $vendor -Force
        $linked = $item.LinkType -in @('Junction', 'SymbolicLink')
        $stamp = Join-Path $vendor 'VERSION'
        $have = if (Test-Path $stamp) { (Get-Content $stamp -Raw).Trim() } else { $null }

        if ($have -and $have -ne $wanted) {
            if ($linked) {
                Warn "vendor/cef is a junction to $($item.Target) holding $have, but cef.lock wants $wanted."
                Warn 'Update that clone, or delete the junction and re-run to fetch a private copy.'
            } else {
                Ok "replacing $have with $wanted"
                node (Join-Path $framework 'tools\fetch-cef.mjs')
                if ($LASTEXITCODE -ne 0) { throw 'could not fetch CEF' }
            }
        } else {
            Ok "$wanted present$(if ($linked) { " (junction -> $($item.Target))" })"
        }
    }
}

# ---------------------------------------------------------------- web

Step 'Web dependencies'
$web = Join-Path $app 'web'
# `npm ci` needs a lockfile and wipes node_modules; `npm install` is right when neither has moved.
$lock = Join-Path $web 'package-lock.json'
$modules = Join-Path $web 'node_modules'
# From inside the directory, not with `--prefix`. `--prefix` sets where npm *installs*, but
# `install` and `ci` still read package.json from the working directory -- so from here it would
# look for one at the application root and fail with ENOENT on a file that was never meant to
# exist. (`npm --prefix <dir> run <script>` is different, and does work: a run script executes
# with the package as its cwd.)
Push-Location $web
try {
    if ((Test-Path $lock) -and -not (Test-Path $modules)) {
        Ok 'npm ci'
        npm ci
    } else {
        Ok 'npm install'
        npm install
    }
    if ($LASTEXITCODE -ne 0) { throw 'npm failed' }
} finally {
    Pop-Location
}

Step 'Ready'
Ok 'tools\build.ps1   -- build the application'
Ok 'tools\test.ps1    -- everything that gates a commit'
