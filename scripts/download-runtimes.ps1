# Download bundled Node.js and Python (via uv) runtimes for MCPHub Desktop
# Usage: .\scripts\download-runtimes.ps1
# Run from the repository root before building the Tauri app.

param(
    [string]$NodeVersion = "22.14.0",
    [string]$UvVersion = "0.6.12",
    [string]$PythonVersion = "3.12"
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Dest = Join-Path $ScriptDir "..\src-tauri\runtimes"
$Dest = [System.IO.Path]::GetFullPath($Dest)

Write-Host "==> Downloading runtimes to $Dest"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

# ---------------------------------------------------------------------------
# Detect architecture
# ---------------------------------------------------------------------------
$Arch = (Get-CimInstance Win32_Processor).Architecture
# 9 = x64, 12 = ARM64
if ($Arch -eq 12) {
    $NodeArch = "arm64"
    $UvArch = "aarch64-pc-windows-msvc"
} else {
    $NodeArch = "x64"
    $UvArch = "x86_64-pc-windows-msvc"
}

# ---------------------------------------------------------------------------
# Download Node.js
# ---------------------------------------------------------------------------
$NodeDest = Join-Path $Dest "node"
$NodeExe  = Join-Path $NodeDest "node.exe"

if (Test-Path $NodeExe) {
    $NodeCurrent = & $NodeExe --version 2>$null
    if ($NodeCurrent -eq "v$NodeVersion") {
        Write-Host "--> Node.js v$NodeVersion already present, skipping"
    } else {
        Write-Host "--> Node.js found ($NodeCurrent), re-downloading v$NodeVersion..."
        Remove-Item -Recurse -Force $NodeDest
    }
}

if (-not (Test-Path $NodeExe)) {
    Write-Host "--> Downloading Node.js v$NodeVersion (x64)..."
    $NodeUrl = "https://nodejs.org/dist/v$NodeVersion/node-v$NodeVersion-win-$NodeArch.zip"
    $TmpZip = [System.IO.Path]::GetTempFileName() + ".zip"
    $TmpDir = [System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid()

    Invoke-WebRequest -Uri $NodeUrl -OutFile $TmpZip
    Expand-Archive -Path $TmpZip -DestinationPath $TmpDir

    $Extracted = Get-ChildItem $TmpDir -Directory | Select-Object -First 1

    New-Item -ItemType Directory -Force -Path $NodeDest | Out-Null

    # On Windows, node.exe and node_modules are in the root
    Copy-Item (Join-Path $Extracted.FullName "node.exe") $NodeDest
    Copy-Item -Recurse (Join-Path $Extracted.FullName "node_modules") $NodeDest

    Remove-Item $TmpZip -Force
    Remove-Item -Recurse -Force $TmpDir
    Write-Host "--> Node.js v$NodeVersion downloaded: $NodeExe"
}

# ---------------------------------------------------------------------------
# Download uv
# ---------------------------------------------------------------------------
$UvDest = Join-Path $Dest "uv"
$UvExe  = Join-Path $UvDest "uv.exe"

if (Test-Path $UvExe) {
    $UvCurrent = & $UvExe version 2>$null | ForEach-Object { ($_ -split " ")[1] }
    if ($UvCurrent -eq $UvVersion) {
        Write-Host "--> uv v$UvVersion already present, skipping"
    } else {
        Write-Host "--> uv found ($UvCurrent), re-downloading v$UvVersion..."
        Remove-Item -Recurse -Force $UvDest
    }
}

if (-not (Test-Path $UvExe)) {
    Write-Host "--> Downloading uv v$UvVersion..."
    $UvUrl = "https://github.com/astral-sh/uv/releases/download/$UvVersion/uv-$UvArch.zip"
    $TmpZip = [System.IO.Path]::GetTempFileName() + ".zip"
    $TmpDir = [System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid()

    Invoke-WebRequest -Uri $UvUrl -OutFile $TmpZip
    Expand-Archive -Path $TmpZip -DestinationPath $TmpDir

    $Extracted = Get-ChildItem $TmpDir -Directory | Select-Object -First 1

    New-Item -ItemType Directory -Force -Path $UvDest | Out-Null
    Copy-Item (Join-Path $Extracted.FullName "uv.exe") $UvDest
    if (Test-Path (Join-Path $Extracted.FullName "uvx.exe")) {
        Copy-Item (Join-Path $Extracted.FullName "uvx.exe") $UvDest
    }

    Remove-Item $TmpZip -Force
    Remove-Item -Recurse -Force $TmpDir
    Write-Host "--> uv v$UvVersion downloaded: $UvExe"
}

# ---------------------------------------------------------------------------
# Download Python via uv
# ---------------------------------------------------------------------------
$PythonInstallDir = Join-Path $UvDest "python"

$PythonExists = Test-Path $PythonInstallDir
if ($PythonExists) {
    $PythonDirs = Get-ChildItem $PythonInstallDir -Directory | Where-Object { $_.Name -like "cpython-$PythonVersion*" }
    $PythonExists = $PythonDirs.Count -gt 0
}

if (-not $PythonExists) {
    Write-Host "--> Downloading Python $PythonVersion via uv..."
    $env:UV_PYTHON_INSTALL_DIR = $PythonInstallDir
    & $UvExe python install $PythonVersion
    Write-Host "--> Python $PythonVersion downloaded to: $PythonInstallDir"
} else {
    Write-Host "--> Python $PythonVersion already present, skipping"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "==> Runtimes ready in src-tauri/runtimes/"
Write-Host "    Node.js : $(& $NodeExe --version)"
Write-Host "    uv      : $(& $UvExe version)"
Write-Host ""
Write-Host "Run 'cargo tauri build' or 'cargo tauri dev' to use bundled runtimes."
