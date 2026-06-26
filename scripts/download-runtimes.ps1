# Download bundled Node.js and Python (via uv) runtimes for MCPHub Desktop
# Usage: .\scripts\download-runtimes.ps1
# Run from the repository root before building the Tauri app.

param(
    [string]$NodeVersion = "24.17.0",
    [string]$UvVersion = "0.11.23",
    [string]$PythonVersion = "3.12",
    # TargetArch 用于 CI 交叉编译，取值: x64 | arm64 | "" (自动检测)
    # 注意: TargetArch 控制 Node.js 和 Python 的目标架构，但 uv 始终使用宿主架构以便能执行
    [string]$TargetArch = ""
)

$ErrorActionPreference = "Stop"

# Helper: run an external executable and capture its stdout without triggering
# the PowerShell "StandardOutputEncoding" error that occurs in some CI environments.
function Get-ExeOutput {
    param([string]$ExePath, [string[]]$Args)
    try {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $ExePath
        foreach ($a in $Args) { $psi.ArgumentList.Add($a) }
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $proc = [System.Diagnostics.Process]::Start($psi)
        $stdout = $proc.StandardOutput.ReadToEnd()
        $proc.WaitForExit()
        return $stdout.Trim()
    } catch {
        return ""
    }
}

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
    # Skip version check when cross-compiling (ARM64 binary can't run on x64 host)
    if ($TargetArch -ne "" -and $TargetArch -ne $HostTargetArch) {
        Write-Host "--> Node.js already present (cross-compile, skipping version check)"
    } else {
        $NodeCurrent = Get-ExeOutput $NodeExe @("--version")
        if ($NodeCurrent -eq "v$NodeVersion") {
            Write-Host "--> Node.js v$NodeVersion already present, skipping"
        } else {
            Write-Host "--> Node.js found ($NodeCurrent), re-downloading v$NodeVersion..."
            Remove-Item -Recurse -Force $NodeDest
        }
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
    $UvCurrentRaw = Get-ExeOutput $UvExe @("self", "version")
    $UvCurrent = ($UvCurrentRaw -split " ")[1]
    if ($UvCurrent -eq $UvVersion) {
        Write-Host "--> uv v$UvVersion already present, skipping"
    } else {
        Write-Host "--> uv found ($UvCurrent), re-downloading v$UvVersion..."
        Remove-Item -Recurse -Force $UvDest
    }
}

if (-not (Test-Path $UvExe)) {
    New-Item -ItemType Directory -Force -Path $UvDest | Out-Null
    $UvObtained = $false

    # ── Method 1: Copy from any existing system uv (version must match) ─────
    $SystemUv = Get-Command uv -ErrorAction SilentlyContinue
    if ($SystemUv) {
        $SystemUvVerRaw = Get-ExeOutput $SystemUv.Source @("self", "version")
        $SystemUvVer = ($SystemUvVerRaw -split " ")[1]
        if ($SystemUvVer -eq $UvVersion) {
            Write-Host "--> Using system uv $SystemUvVer at $($SystemUv.Source)"
            Copy-Item $SystemUv.Source (Join-Path $UvDest "uv.exe")
            $UvObtained = $true
        } else {
            Write-Host "--> System uv version ($SystemUvVer) != target ($UvVersion), skipping"
        }
    }

    # ── Method 2: Direct download with retry support ──
    if (-not $UvObtained) {
        Write-Host "--> Downloading uv v$UvVersion from GitHub..."
        $UvUrl = "https://github.com/astral-sh/uv/releases/download/$UvVersion/uv-$UvArch.zip"
        $TmpZip = [System.IO.Path]::GetTempFileName() + ".zip"
        $TmpDir = [System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid()

        $DownloadOk = $false
        for ($attempt = 1; $attempt -le 5; $attempt++) {
            Write-Host "    Attempt $attempt/5..."
            try {
                Invoke-WebRequest -Uri $UvUrl -OutFile $TmpZip -UseBasicParsing
                $DownloadOk = $true
                break
            } catch {
                Write-Host "    Download failed: $($_.Exception.Message)"
                if ($attempt -lt 5) {
                    Write-Host "    Retrying in 5 seconds..."
                    Start-Sleep -Seconds 5
                }
            }
        }

        if (-not $DownloadOk) {
            Write-Host ""
            Write-Host "ERROR: All download methods failed."
            Write-Host "Please install uv manually and re-run:"
            Write-Host "  winget install astral-sh.uv"
            Write-Host "  pip install uv"
            exit 1
        }

        Expand-Archive -Path $TmpZip -DestinationPath $TmpDir

        # uv zip may contain a subdirectory or have files directly at the root
        $Extracted = Get-ChildItem $TmpDir -Directory | Select-Object -First 1
        if ($null -eq $Extracted) {
            $ExtractedDir = $TmpDir
        } else {
            $ExtractedDir = $Extracted.FullName
        }

        Copy-Item (Join-Path $ExtractedDir "uv.exe") $UvDest
        if (Test-Path (Join-Path $ExtractedDir "uvx.exe")) {
            Copy-Item (Join-Path $ExtractedDir "uvx.exe") $UvDest
        }

        Remove-Item $TmpZip -Force
        Remove-Item -Recurse -Force $TmpDir
        $UvObtained = $true
        Write-Host "--> uv v$UvVersion downloaded: $UvExe"
    }
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
        # 先尝试 GitHub API，失败时回退到直接构造 URL
        $AssetName = $null
        $DownloadUrl = $null
        try {
            $Releases = Invoke-RestMethod -Uri "https://api.github.com/repos/astral-sh/python-build-standalone/releases?per_page=5"
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
        } catch {
            Write-Host "    GitHub API unavailable: $($_.Exception.Message)"
        }

        # 回退：直接构造已知版本的下载 URL
        if (-not $DownloadUrl) {
            Write-Host "--> GitHub API unavailable or no matching asset found, trying direct URL construction..."
            $PythonReleaseVersion = "2024.10.16"
            $DirectUrl = "https://github.com/astral-sh/python-build-standalone/releases/download/$PythonReleaseVersion/cpython-$PythonVersion.1+$PythonReleaseVersion-$PyTriple-install_only.tar.gz"
            try {
                $Response = Invoke-WebRequest -Uri $DirectUrl -Method Head -UseBasicParsing
                if ($Response.StatusCode -eq 200) {
                    $DownloadUrl = $DirectUrl
                    Write-Host "--> Found Python via direct URL: $(Split-Path $DownloadUrl -Leaf)"
                }
            } catch {
                Write-Host "    Direct URL also unavailable: $DirectUrl"
            }
        }

        if (-not $DownloadUrl) {
            Write-Error "ERROR: Could not find Python $PythonVersion for $PyTriple"
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
# Skip version check for cross-compiled binaries (ARM64 binary can't run on x64 host)
if ($TargetArch -ne "" -and $TargetArch -ne $HostTargetArch) {
    $NodeVer = "v$NodeVersion (cross-compiled, not verified)"
} else {
    $NodeVer = Get-ExeOutput $NodeExe @("--version")
    if (-not $NodeVer) { $NodeVer = "(version check failed)" }
}
$UvVer = Get-ExeOutput $UvExe @("version")
if (-not $UvVer) { $UvVer = "(version check failed)" }
Write-Host ""
Write-Host "==> Runtimes ready in src-tauri/runtimes/"
Write-Host "    Node.js : $NodeVer"
Write-Host "    uv      : $UvVer"
Write-Host ""
Write-Host "Run 'cargo tauri build' or 'cargo tauri dev' to use bundled runtimes."
