# Installs the prebuilt `prismpdf` CLI from GitHub Releases (Windows).
#
#   powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/theunloop/prism-pdf/main/scripts/install.ps1 | iex"
#
# Environment (or parameters, when the script is run from a file):
#   PRISMPDF_VERSION      release to install, e.g. "0.4.1" (default: the latest release)
#   PRISMPDF_INSTALL_DIR  directory the binary is copied into
#                         (default: %LOCALAPPDATA%\Programs\prismpdf)
#
# The script picks the archive matching this machine (win-x64, win-arm64, or win-x86), verifies
# it against the release's SHA256SUMS file, copies `prismpdf.exe` into place, and appends the
# directory to the *user* PATH if it is missing — no administrator rights needed. Runs on both
# Windows PowerShell 5.1 and PowerShell 7+; macOS and Linux use scripts/install.sh instead.

[CmdletBinding()]
param(
    [string]$Version = $env:PRISMPDF_VERSION,
    [string]$InstallDir = $env:PRISMPDF_INSTALL_DIR
)

$ErrorActionPreference = 'Stop'
$Repo = 'theunloop/prism-pdf'

# Windows PowerShell 5.1 defaults to TLS 1.0; GitHub requires 1.2+.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\prismpdf' }

# PROCESSOR_ARCHITECTURE reports the *process* architecture; ARCHITEW6432 holds the real one
# when a 32-bit PowerShell runs on a 64-bit OS.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$rid = switch ($arch) {
    'AMD64' { 'win-x64' }
    'ARM64' { 'win-arm64' }
    'x86'   { 'win-x86' }
    default { throw "install.ps1: unsupported architecture: $arch" }
}

if (-not $Version) {
    # /releases/latest redirects to /releases/tag/v<x.y.z>; reading the redirect target avoids
    # the GitHub API and its per-IP rate limit. WebRequest (not Invoke-WebRequest) handles the
    # unfollowed redirect identically on 5.1 and 7+.
    $req = [System.Net.WebRequest]::Create("https://github.com/$Repo/releases/latest")
    $req.Method = 'HEAD'
    $req.AllowAutoRedirect = $false
    $resp = $req.GetResponse()
    try { $location = $resp.Headers['Location'] } finally { $resp.Close() }
    if ($location -notmatch '/releases/tag/v(.+)$') {
        throw "install.ps1: could not resolve the latest release (got: $location)"
    }
    $Version = $Matches[1]
}
$Version = $Version.TrimStart('v')

$archive = "prismpdf-v$Version-$rid.zip"
$sums = "SHA256SUMS-v$Version.txt"
$base = "https://github.com/$Repo/releases/download/v$Version"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "prismpdf-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading $archive (v$Version, $rid)..."
    Invoke-WebRequest -Uri "$base/$archive" -OutFile (Join-Path $tmp $archive) -UseBasicParsing
    Invoke-WebRequest -Uri "$base/$sums" -OutFile (Join-Path $tmp $sums) -UseBasicParsing

    $pattern = "^([0-9a-f]{64})\s+\*?$([regex]::Escape($archive))$"
    $expected = (Get-Content (Join-Path $tmp $sums)) -match $pattern |
        ForEach-Object { ([regex]::Match($_, $pattern)).Groups[1].Value } | Select-Object -First 1
    if (-not $expected) { throw "install.ps1: $sums has no entry for $archive" }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $archive)).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "install.ps1: checksum mismatch for $archive`n  expected: $expected`n  actual:   $actual"
    }
    Write-Host 'Checksum verified.'

    Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp
    $bin = Join-Path $tmp "prismpdf-v$Version-$rid\prismpdf.exe"
    if (-not (Test-Path $bin)) { throw 'install.ps1: archive did not contain the expected binary' }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item $bin (Join-Path $InstallDir 'prismpdf.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host "Installed prismpdf v$Version to $(Join-Path $InstallDir 'prismpdf.exe')"

# Append to the user PATH (never the machine PATH) so `prismpdf` resolves in new shells; the
# current session's PATH is updated too so it works immediately.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';') + ($env:Path -split ';') |
    Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') } |
    Where-Object { $_ -eq $InstallDir.TrimEnd('\') }
if (-not $onPath) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$InstallDir".Trim(';'), 'User')
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to your user PATH (new shells pick it up automatically)."
}

# An arm64 binary on an x64 host (or vice versa) cannot run; report the version when it can.
try { Write-Host "  $(& (Join-Path $InstallDir 'prismpdf.exe') --version)" } catch { }
