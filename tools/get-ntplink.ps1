# Downloads ntplink.exe, NativeTerm's client for non-SSH sessions, from a
# release of the PuTTY fork (built by its CI from upstream PuTTY plus the
# fork's patches/ folder), checks it against the release's SHA256SUMS, and
# puts it next to the shim in target\debug and target\release (the ones
# that exist). Uses the GitHub CLI (`gh`), signed in: the fork is private.
#
#   powershell -File tools\get-ntplink.ps1 [-Tag <release tag>] [-Arch x86_64|aarch64|i686]
#                                          [-Package <package folder>]
#
# -Package: for NativeTerm's release package instead: ntplink.exe goes to
# <folder>\tools and PuTTY's licence to <folder>\licenses\PuTTY.txt (pass
# -Tag there, so a package is reproducible).
#
# To build it from source instead: tools\build-ntplink.cmd.
param(
    [string]$Tag = "",
    [ValidateSet("x86_64", "aarch64", "i686")]
    [string]$Arch = $(if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }),
    [string]$Repo = "fivetime/putty",
    [string]$Package = ""
)
$ErrorActionPreference = "Stop"
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw "the GitHub CLI (gh) is needed: https://cli.github.com" }
$root = Split-Path -Parent $PSScriptRoot
if (-not $Tag) {
    $Tag = gh release view -R $Repo --json tagName -q .tagName
    if ($LASTEXITCODE -ne 0 -or -not $Tag) { throw "no release found in $Repo (signed in to gh with access to it?)" }
}

$work = Join-Path $root "target\ntplink-download\$Tag"
New-Item -ItemType Directory -Force $work | Out-Null
gh release download $Tag -R $Repo -p "*-windows-$Arch.zip" -p SHA256SUMS -D $work --clobber
if ($LASTEXITCODE -ne 0) { throw "download of $Tag failed" }
$zip = Get-ChildItem $work -Filter "*-windows-$Arch.zip" | Select-Object -First 1
if (-not $zip) { throw "release $Tag has no windows-$Arch zip" }
$expected = Get-Content (Join-Path $work SHA256SUMS) |
    Where-Object { $_ -match "\s\*?$([regex]::Escape($zip.Name))\s*$" } | ForEach-Object { ($_ -split "\s+")[0] }
$actual = (Get-FileHash $zip.FullName -Algorithm SHA256).Hash
if (-not $expected -or $actual -ne $expected.ToUpperInvariant()) { throw "checksum mismatch for $($zip.Name)" }

$unpacked = Join-Path $work $zip.BaseName
Remove-Item $unpacked -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive $zip.FullName -DestinationPath $work -Force
$exe = Get-ChildItem $unpacked -Recurse -Filter ntplink.exe | Select-Object -First 1
if (-not $exe) { throw "$($zip.Name) has no ntplink.exe" }
if ($Package) {
    $licence = Join-Path $exe.DirectoryName "LICENCE"
    if (-not (Test-Path $licence)) { throw "$($zip.Name) has no LICENCE" }
    New-Item -ItemType Directory -Force (Join-Path $Package "tools"), (Join-Path $Package "licenses") | Out-Null
    Copy-Item $exe.FullName (Join-Path $Package "tools") -Force
    Copy-Item $licence (Join-Path $Package "licenses\PuTTY.txt") -Force
    "ntplink.exe ($Tag, $Arch) -> $Package\tools, licence -> $Package\licenses\PuTTY.txt"
    return
}
foreach ($profile in "debug", "release") {
    $dir = Join-Path $root "target\$profile"
    if (Test-Path $dir) {
        Copy-Item $exe.FullName $dir -Force
        "ntplink.exe ($Tag, $Arch) -> target\$profile"
    }
}
