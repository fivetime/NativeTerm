<#
.SYNOPSIS
Checks the portable crates for the Linux and macOS targets from a Windows
machine: clippy with warnings as errors, tests and examples included,
without linking (no cross linker is needed).

.DESCRIPTION
Requires the targets once:

    rustup target add x86_64-unknown-linux-gnu aarch64-apple-darwin

SQLite is the system's library off Windows; its build script only needs to
know not to look for it here, so SQLITE3_NO_PKG_CONFIG and SQLITE3_LIB_DIR
are set for the run (nothing is linked).

Crates not listed are Windows-only (native-term-win) or the prototypes.

.EXAMPLE
tools\check-portable.ps1
tools\check-portable.ps1 -Target x86_64-unknown-linux-gnu
#>
param(
    [string[]]$Target = @("x86_64-unknown-linux-gnu", "aarch64-apple-darwin"),
    [string[]]$Crate = @(
        "native-term-i18n",
        "native-term-config",
        "native-term-session",
        "native-term-sftp",
        "native-term-os",
        "native-term-platform",
        "native-term-app",
        "native-term-shim",
        "native-term-wezterm"
    )
)

# cargo talks on stderr; only its exit code says whether it failed
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$env:SQLITE3_NO_PKG_CONFIG = "1"
$env:SQLITE3_LIB_DIR = [System.IO.Path]::GetTempPath()

$installed = & rustup target list --installed
$failed = @()
foreach ($t in $Target) {
    if ($installed -notcontains $t) {
        Write-Host "target $t is not installed: rustup target add $t" -ForegroundColor Yellow
        $failed += $t
        continue
    }
    $packages = $Crate | ForEach-Object { @("-p", $_) }
    Write-Host "== clippy --target $t" -ForegroundColor Cyan
    & cargo clippy --target $t --tests --examples @packages -- -D warnings
    if ($LASTEXITCODE -ne 0) { $failed += $t }
}

if ($failed.Count -gt 0) {
    Write-Host "portable check failed for: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "portable check passed for: $($Target -join ', ')" -ForegroundColor Green
