<#
.SYNOPSIS
  Proves a Windows build can actually start, serve, and install.

.DESCRIPTION
  Everything below is something CI could not previously tell us, and something the operator hit
  on a real VM: a daemon that dies in the loader before any Rust runs, a client that cannot reach
  it over a named pipe, and an installer whose four executables do not end up side by side (the
  daemon spawns the agent host by looking next to itself, so "side by side" is load bearing).

  Four checks, all fatal:
    1. repomond.exe --version runs. Catches a missing C runtime (STATUS_DLL_NOT_FOUND,
       0xC0000135) and a wrong-architecture image (0xC000007B) before anything else.
    2. repomond.exe serves a private pipe with a throwaway data directory, and repomon.exe
       reaches it with a read RPC.
    3. The daemon shuts down when asked.
    4. The NSIS bundle installs silently and leaves repomon-desktop.exe, repomon.exe, repomond.exe, and
       repomon-agent-host.exe in one directory.

  Nothing here touches the runner's real data directory, its real pipe name, or any process it
  did not start itself.
#>

[CmdletBinding()]
param(

  [Parameter(Mandatory = $true)][string]$Triple,

  [string]$RepoRoot = (Get-Location).Path,

  [string]$InstallDir = (Join-Path ([System.IO.Path]::GetTempPath()) ("repomon-smoke-install-" + [System.Guid]::NewGuid().ToString("N")))
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Fail([string]$message) {
  Write-Host "::error::$message"
  exit 1
}

$releaseDir = Join-Path $RepoRoot "target\$Triple\release"
$daemonExe = Join-Path $releaseDir "repomond.exe"
$cliExe = Join-Path $releaseDir "repomon.exe"

foreach ($exe in @($daemonExe, $cliExe)) {
  if (-not (Test-Path -LiteralPath $exe)) { Fail "expected a built binary at $exe" }
}

Write-Host "== repomond.exe --version"
$version = & $daemonExe --version 2>&1
if ($LASTEXITCODE -ne 0) {
  $code = "0x{0:X8}" -f ([uint32]$LASTEXITCODE)
  Fail "repomond.exe --version exited with $LASTEXITCODE ($code): $version. 0xC0000135 means a missing DLL (the Visual C++ runtime), 0xC000007B means a wrong-architecture one."
}
Write-Host $version

$smokeId = [System.Guid]::NewGuid().ToString("N").Substring(0, 12)
$pipe = "\\.\pipe\repomon-smoke-$smokeId"
$dataDir = Join-Path ([System.IO.Path]::GetTempPath()) "repomon-smoke-data-$smokeId"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$env:REPOMON_DATA_DIR = $dataDir

$stdout = Join-Path $dataDir "repomond.out.log"
$stderr = Join-Path $dataDir "repomond.err.log"

Write-Host "== starting repomond.exe on $pipe (data dir: $dataDir)"
$daemon = Start-Process -FilePath $daemonExe -ArgumentList @("--socket", $pipe) `
  -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr

function Show-DaemonLogs {
  foreach ($log in @($stdout, $stderr)) {
    if (Test-Path -LiteralPath $log) {
      Write-Host "---- $log"
      Get-Content -LiteralPath $log -Tail 40 | ForEach-Object { Write-Host $_ }
    }
  }
}

# Probe the pipe directly: the CLI can auto-start another daemon and mask failure of this script’s
# child.
$deadline = (Get-Date).AddSeconds(60)
$bound = $false
while ((Get-Date) -lt $deadline) {
  if ($daemon.HasExited) {
    Show-DaemonLogs
    $code = "0x{0:X8}" -f ([uint32]$daemon.ExitCode)
    Fail "repomond.exe exited before binding the pipe (exit $($daemon.ExitCode) / $code). 0xC0000135 means a missing DLL (the Visual C++ runtime), 0xC000007B means a wrong-architecture one."
  }
  $pipes = [System.IO.Directory]::GetFiles("\\.\pipe\")
  if ($pipes -contains $pipe) { $bound = $true; break }
  Start-Sleep -Milliseconds 250
}

if (-not $bound) {
  Show-DaemonLogs
  # Stop the process this script started, by its own handle. Never by name.
  if (-not $daemon.HasExited) { Stop-Process -Id $daemon.Id -Force }
  Fail "repomond.exe did not bind $pipe within 60s"
}

Write-Host "== repomon.exe --socket $pipe lane list"
$lanes = & $cliExe --socket $pipe lane list 2>&1
if ($LASTEXITCODE -ne 0) {
  Write-Host $lanes
  Show-DaemonLogs
  if (-not $daemon.HasExited) { Stop-Process -Id $daemon.Id -Force }
  Fail "repomon.exe could not read from the daemon on $pipe (exit $LASTEXITCODE)"
}
Write-Host "== repomon.exe reached the daemon on $pipe"

Write-Host "== repomon.exe --socket $pipe daemon stop"
& $cliExe --socket $pipe daemon stop *> $null
if (-not $daemon.WaitForExit(20000)) {
  Show-DaemonLogs
  Stop-Process -Id $daemon.Id -Force
  Fail "repomond.exe ignored daemon stop and had to be killed"
}
Write-Host "== the daemon shut down cleanly"

$bundleDir = Join-Path $releaseDir "bundle\nsis"
if (-not (Test-Path -LiteralPath $bundleDir)) { Fail "no NSIS bundle directory at $bundleDir" }
$installer = Get-ChildItem -LiteralPath $bundleDir -Filter "*-setup.exe" | Select-Object -First 1
if (-not $installer) { Fail "no *-setup.exe in $bundleDir" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Write-Host "== installing $($installer.Name) silently into $InstallDir"
# /S is NSIS silent mode; /D= is its install directory and must come last and unquoted.
$setup = Start-Process -FilePath $installer.FullName -ArgumentList @("/S", "/D=$InstallDir") -PassThru -Wait
if ($setup.ExitCode -ne 0) { Fail "the installer exited with $($setup.ExitCode)" }

$expected = @("repomon-desktop.exe", "repomon.exe", "repomond.exe", "repomon-agent-host.exe")
$found = @{}
foreach ($name in $expected) {
  $hit = Get-ChildItem -LiteralPath $InstallDir -Filter $name -Recurse -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if (-not $hit) {
    Write-Host "---- installed tree"
    Get-ChildItem -LiteralPath $InstallDir -Recurse | ForEach-Object { Write-Host $_.FullName }
    Fail "$name is missing from the installed bundle"
  }
  $found[$name] = $hit.DirectoryName
}

$directories = @($found.Values | Sort-Object -Unique)
if ($directories.Count -ne 1) {
  foreach ($name in $expected) { Write-Host "$name -> $($found[$name])" }
  Fail "the four executables are not in one directory; the daemon spawns the agent host by looking next to itself"
}
Write-Host "== the installer put all four executables in $($directories[0])"

Remove-Item -LiteralPath $dataDir -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "Windows smoke test passed."
