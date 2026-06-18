# Download bundled Node.js and Python (via uv) runtimes for MCPHub Desktop
# Usage: .\scripts\download-runtimes.ps1
# Run from the repository root before building the Tauri app.

param(
    [string]$NodeVersion = "22.14.0",
    [string]$UvVersion = "0.6.12",
    [string]$PythonVersion = "3.12",
    # TargetArch 用于 CI 交叉编译，取值: x64 | arm64 | "" (自动检测)
    # 注意: TargetArch 控制 Node.js 和 Python 的目标架构，但 uv 始终使用宿主架构以便能执行
    [string]$TargetArch = ""
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Dest = Join-Path $ScriptDir "..\src-tauri\runtimes"
$Dest = [System.IO.Path]::GetFullPath($Dest)

Write-Host "==> Downloading runtimes to $Dest"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

# ---------------------------------------------------------------------------
# Detect architecture
# TargetArch 参数可覆盖自动检测（CI 交叉编译时使用）
# 注意: uv 必须使用宿主架构（HostArch）以便能执行；Node.js/Python 使用目标架构
# ---------------------------------------------------------------------------
# 检测宿主架构（用于 uv 下载，确保能在当前机器上运行）
$HostArchRaw = (Get-CimInstance Win32_Processor).Architecture
# 9 = x64, 12 = ARM64
if ($HostArchRaw -eq 12) {
    $HostUvArch = "aarch64-pc-windows-msvc"
} else {
    $HostUvArch = "x86_64-pc-windows-msvc"
}

# 宿主默认目标架构（无 TargetArch 参数时使用）
if ($HostArchRaw -eq 12) {
    $HostTargetArch = "arm64"
} else {
    $HostTargetArch = "x64"
}

# 目标架构（用于 Node.js 和 Python）
if ($TargetArch -ne "") {
    $NodeArch = $TargetArch
} else {
    $NodeArch = $HostTargetArch
}

# uv 始终使用宿主架构，这样才能在 CI runner 上执行
$UvArch = $HostUvArch
Write-Host "--> Host arch: $HostUvArch, Target arch: $NodeArch"

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
    # 使用 uv self version 获取版本号（uv version 在没有 pyproject.toml 时会报错）
    $UvCurrent = & $UvExe self version 2>$null | ForEach-Object { ($_ -split " ")[1] }
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

    # uv zip may contain a subdirectory or have files directly at the root
    $Extracted = Get-ChildItem $TmpDir -Directory | Select-Object -First 1
    if ($null -eq $Extracted) {
        $ExtractedDir = $TmpDir
    } else {
        $ExtractedDir = $Extracted.FullName
    }

    New-Item -ItemType Directory -Force -Path $UvDest | Out-Null
    Copy-Item (Join-Path $ExtractedDir "uv.exe") $UvDest
    if (Test-Path (Join-Path $ExtractedDir "uvx.exe")) {
        Copy-Item (Join-Path $ExtractedDir "uvx.exe") $UvDest
    }

    Remove-Item $TmpZip -Force
    Remove-Item -Recurse -Force $TmpDir
    Write-Host "--> uv v$UvVersion downloaded: $UvExe"
}

# ---------------------------------------------------------------------------
# Download Python via uv
# ---------------------------------------------------------------------------
$PythonInstallDir = Join-Path $UvDest "python"

# uv 解压后目录名格式: cpython-3.12.x+...
# python-build-standalone tar 解压后目录名: python/
$PythonExists = Test-Path $PythonInstallDir
if ($PythonExists) {
    $PythonDirs = Get-ChildItem $PythonInstallDir -Directory | Where-Object {
        $_.Name -like "cpython-$PythonVersion*" -or $_.Name -eq "python"
    }
    $PythonExists = $PythonDirs.Count -gt 0
}

if (-not $PythonExists) {
    Write-Host "--> Downloading Python $PythonVersion..."
    New-Item -ItemType Directory -Force -Path $PythonInstallDir | Out-Null
    # uv python install 不支持 --platform，交叉编译时需要直接下载目标架构的 Python
    if ($TargetArch -ne "" -and $TargetArch -ne $HostTargetArch) {
        if ($TargetArch -eq "arm64") {
            $PyTriple = "aarch64-pc-windows-msvc"
        } else {
            $PyTriple = "x86_64-pc-windows-msvc"
        }
        Write-Host "--> Cross-compiling: downloading Python $PythonVersion for $PyTriple directly..."
        # 通过 GitHub API 获取 python-build-standalone 最新 release 中匹配的 asset
        $Releases = Invoke-RestMethod -Uri "https://api.github.com/repos/astral-sh/python-build-standalone/releases?per_page=5"
        $AssetName = $null
        $DownloadUrl = $null
        foreach ($Rel in $Releases) {
            foreach ($A in $Rel.assets) {
                if ($A.name -match "cpython-$PythonVersion\.\d+\+.*-$PyTriple-install_only\.tar\.gz" -and
                    $A.name -notmatch "stripped") {
                    $AssetName = $A.name
                    $DownloadUrl = $A.browser_download_url
                    break
                }
            }
            if ($AssetName) { break }
        }
        if (-not $DownloadUrl) {
            Write-Error "ERROR: Could not find Python $PythonVersion for $PyTriple in python-build-standalone releases"
            exit 1
        }
        Write-Host "--> Found: $AssetName"
        $TmpDir = [System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid()
        New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null
        $TmpTar = Join-Path $TmpDir "python.tar.gz"
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $TmpTar
        tar -xzf $TmpTar -C $TmpDir
        Remove-Item $TmpTar -Force
        # tar 解压后目录名为 python/，将其内容移到 PythonInstallDir
        $ExtractedPy = Join-Path $TmpDir "python"
        if (Test-Path $ExtractedPy) {
            Get-ChildItem $ExtractedPy | Move-Item -Destination $PythonInstallDir -Force
        }
        Remove-Item -Recurse -Force $TmpDir
    } else {
        $env:UV_PYTHON_INSTALL_DIR = $PythonInstallDir
        & $UvExe python install $PythonVersion
    }
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
