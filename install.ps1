# Downloads Windows release binaries without a Rust toolchain. REPOMON_INSTALL_DIR overrides the
# default %LOCALAPPDATA%\Programs\repomon destination; REPOMON_VERSION selects a tag instead of
# latest. Omit param() and exit so piping into Invoke-Expression remains safe for an interactive
# shell.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is much faster without the progress bar

$repo = 'AliHamzaAzam/repomon'
$headers = @{ 'User-Agent' = 'repomon-install' }

# Windows PowerShell 5.1 may default to TLS 1.0; GitHub requires TLS 1.2+.
if ($PSVersionTable.PSVersion.Major -lt 6) {
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$dest = if ($env:REPOMON_INSTALL_DIR) {
    $env:REPOMON_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'Programs\repomon'
}

$target = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x86_64-pc-windows-msvc' }
    'ARM64' { 'aarch64-pc-windows-msvc' }
    default { throw "unsupported Windows architecture: $env:PROCESSOR_ARCHITECTURE" }
}

if (-not $env:REPOMON_VERSION -or $env:REPOMON_VERSION -eq 'latest') {
    $tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers $headers).tag_name
} else {
    $tag = $env:REPOMON_VERSION
}
if (-not $tag) { throw 'could not determine release tag' }
$ver = $tag -replace '^v', ''

$tmp = Join-Path ([IO.Path]::GetTempPath()) "repomon-install-$([IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $zip = Join-Path $tmp 'repomon.zip'
    $url = "https://github.com/$repo/releases/download/$tag/repomon-$ver-$target.zip"
    Write-Host "Downloading repomon $ver ($target)..."
    try {
        # -UseBasicParsing: required on Windows PowerShell 5.1 when the IE engine
        # is unavailable; accepted (and ignored) by PowerShell 7+.
        Invoke-WebRequest -Uri $url -OutFile $zip -Headers $headers -UseBasicParsing
    } catch {
        if ($target -eq 'aarch64-pc-windows-msvc') {
            # ARM64 builds are best-effort; fall back to x86_64 under emulation.
            $target = 'x86_64-pc-windows-msvc'
            $url = "https://github.com/$repo/releases/download/$tag/repomon-$ver-$target.zip"
            Write-Host "No ARM64 build for $tag; falling back to x86_64 (runs under emulation)..."
            Invoke-WebRequest -Uri $url -OutFile $zip -Headers $headers -UseBasicParsing
        } else {
            throw
        }
    }
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    $installed = @()
    foreach ($exe in 'repomon.exe', 'repomond.exe', 'repomon-agent-host.exe') {
        $src = Join-Path $tmp $exe
        if (Test-Path $src) {
            try {
                Copy-Item -Path $src -Destination $dest -Force
            } catch {
                throw ("could not replace $exe in ${dest}: $($_.Exception.Message)`n" +
                    "If repomon is running, stop it first (close the TUI and run 'repomond' shutdown or end the process), then re-run the installer.")
            }
            $installed += $exe
        }
    }
    if ($installed.Count -eq 0) { throw "release archive contained no repomon binaries" }
    Write-Host "Installed $($installed -join ', ') to $dest"
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$destNorm = $dest.TrimEnd('\')
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = @($userPath -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') }) -contains $destNorm
if (-not $onPath) {
    $newPath = if ($userPath) { "$userPath;$dest" } else { $dest }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$dest"
    Write-Host "Added $dest to your user PATH (already active in this session; new terminals pick it up automatically)."
}

# Windows agents use the native host; git remains a runtime dependency.
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Host "Warning: 'git' is not installed. repomon needs it. Install Git for Windows:"
    Write-Host '    winget install --id Git.Git'
}

Write-Host ''
Write-Host 'Enable cd-on-exit by adding to your PowerShell profile ($PROFILE):'
Write-Host '    repomon shell-init powershell | Out-String | Invoke-Expression'
Write-Host "Run 'repomon' to get started."
