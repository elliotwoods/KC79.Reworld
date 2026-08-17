<#
.SYNOPSIS
    Everything that gates a commit, in the order that fails fastest.

.DESCRIPTION
    Six checks. The last one is the one that would otherwise be forgotten.

      1. cargo test        -- the state machine, the image model, the flash sequences
      2. vitest            -- the pure page models
      3. tsc               -- types the tests do not reach
      4. cargo clippy      -- scoped, never --all (see below)
      5. check-av-app.ps1  -- the framework's structural contract
      6. submodule clean   -- third_party/av-frameworks must be untouched

    **Scoped clippy, never --all.** `cargo clippy --all` and `cargo fmt --all` reach through path
    dependencies into third_party/av-frameworks and rewrite the submodule's own sources. It has
    happened twice here, 62 files each time, and it is invisible until `git status` shows a dirty
    gitlink at commit time. Check 6 exists because check 4 used to cause the failure it detects.

    This runs no hardware. The probe backend is compiled but nothing here attaches to anything --
    a test suite that needed a board on the bench would stop being run.

.PARAMETER Fast
    Rust tests only. For a tight edit loop; not for a commit.
#>
[CmdletBinding()]
param(
    [switch] $Fast
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$app = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $app 'Cargo.toml'
$framework = Join-Path $app 'third_party\av-frameworks'
$web = Join-Path $app 'web'
$failures = @()

function Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }
function Check($name, [scriptblock] $body) {
    Step $name
    & $body
    if ($LASTEXITCODE -ne 0) {
        $script:failures += $name
        Write-Host "    FAILED" -ForegroundColor Red
    }
}

Check 'cargo test' { cargo test --manifest-path $manifest --workspace }

if (-not $Fast) {
    # Through the package scripts, not `npm exec`. `--prefix` moves where npm resolves packages
    # but not the working directory, so `npm --prefix web exec -- vitest` runs vitest with this
    # directory's config -- which is a different config, wanting a jsdom this project does not
    # install. A `run` script executes with the package as its cwd, which is what is wanted.
    Check 'vitest' { npm --prefix $web run test }
    # `build` is `tsc --noEmit && vite build`, so this is the type check as well as the bundle.
    # Running it here is not waste: `web_assets!` embeds web/dist at compile time, and a dist left
    # behind by an older page is a stale binary nothing warns about.
    Check 'tsc + bundle' { npm --prefix $web run build }
    # The crate list is explicit and stays that way. See the note above.
    Check 'clippy' {
        cargo clippy --manifest-path $manifest -p portal-swd -p portal-flasher `
            --all-targets --all-features -- -D warnings
    }
    Check 'check-av-app' {
        & (Join-Path $framework 'tools\check-av-app.ps1') -AppPath $app
    }

    # Last, because the checks above are what dirties it.
    Step 'submodule clean'
    $dirty = git -C $framework status --porcelain
    if ($dirty) {
        $failures += 'submodule clean'
        Write-Host '    third_party/av-frameworks is dirty:' -ForegroundColor Red
        $dirty | Select-Object -First 10 | ForEach-Object { Write-Host "      $_" -ForegroundColor Red }
        Write-Host "    restore with: git -C third_party/av-frameworks checkout -- ." -ForegroundColor Yellow
    }
}

Write-Host ''
if ($failures.Count -gt 0) {
    Write-Host "FAILED: $($failures -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host 'All checks passed.' -ForegroundColor Green
