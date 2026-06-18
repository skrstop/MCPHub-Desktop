#!/usr/bin/env bash
# =============================================================================
# Download bundled Node.js and Python (via uv) runtimes for MCPHub Desktop
# Usage: bash scripts/download-runtimes.sh
# Run from the repository root before building the Tauri app.
# =============================================================================
set -euo pipefail

# Versions — override via env vars if needed
NODE_VERSION="${NODE_VERSION:-22.14.0}"
UV_VERSION="${UV_VERSION:-0.6.12}"
PYTHON_VERSION="${PYTHON_VERSION:-3.12}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$SCRIPT_DIR/../src-tauri/runtimes"

echo "==> Downloading runtimes to $DEST"
mkdir -p "$DEST"

# ---------------------------------------------------------------------------
# Platform detection
# TARGET_ARCH 环境变量可覆盖自动检测的架构（ci 交叉编译使用）
# 取值: arm64 | x64
# ---------------------------------------------------------------------------
OS=$(uname -s)
ARCH=$(uname -m)

# 宿主架构（用于 uv 下载，确保能在当前机器上运行）
if [[ "$ARCH" == "aarch64" || "$ARCH" == "arm64" ]]; then
  HOST_UV_ARCH_DARWIN="aarch64-apple-darwin"
  HOST_UV_ARCH_LINUX="aarch64-unknown-linux-gnu"
  HOST_TARGET_ARCH="arm64"
else
  HOST_UV_ARCH_DARWIN="x86_64-apple-darwin"
  HOST_UV_ARCH_LINUX="x86_64-unknown-linux-gnu"
  HOST_TARGET_ARCH="x64"
fi

# 允许通过 TARGET_ARCH 覆盖目标架构（CI 交叉编译时使用）
if [[ -n "${TARGET_ARCH:-}" ]]; then
  ARCH_OVERRIDE="${TARGET_ARCH}"
else
  ARCH_OVERRIDE="$HOST_TARGET_ARCH"
fi

case "$OS" in
  Darwin)
    PLATFORM="darwin"
    # Node.js 使用目标架构（打包到最终应用）
    if [[ "$ARCH_OVERRIDE" == "arm64" ]]; then
      NODE_ARCH="arm64"
    else
      NODE_ARCH="x64"
    fi
    # uv 始终使用宿主架构，这样才能在 CI runner 上执行
    UV_ARCH="$HOST_UV_ARCH_DARWIN"
    NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-darwin-${NODE_ARCH}.tar.gz"
    ;;
  Linux)
    PLATFORM="linux"
    # Node.js 使用目标架构（打包到最终应用）
    if [[ "$ARCH_OVERRIDE" == "arm64" ]]; then
      NODE_ARCH="arm64"
    else
      NODE_ARCH="x64"
    fi
    # uv 始终使用宿主架构，这样才能在 CI runner 上执行
    UV_ARCH="$HOST_UV_ARCH_LINUX"
    NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${NODE_ARCH}.tar.gz"
    ;;
  *)
    echo "ERROR: Unsupported OS: $OS. Use scripts/download-runtimes.ps1 on Windows."
    exit 1
    ;;
esac

echo "--> Host uv arch: $UV_ARCH, Target node arch: $NODE_ARCH"

UV_URL="https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${UV_ARCH}.tar.gz"

# ---------------------------------------------------------------------------
# Download Node.js
# ---------------------------------------------------------------------------
NODE_DEST="$DEST/node"

if [[ -f "$NODE_DEST/bin/node" ]]; then
  NODE_CURRENT=$("$NODE_DEST/bin/node" --version 2>/dev/null || echo "unknown")
  if [[ "$NODE_CURRENT" == "v${NODE_VERSION}" ]]; then
    echo "--> Node.js v${NODE_VERSION} already present, skipping download"
  else
    echo "--> Node.js found ($NODE_CURRENT), re-downloading v${NODE_VERSION}..."
    rm -rf "$NODE_DEST"
  fi
fi

if [[ ! -f "$NODE_DEST/bin/node" ]]; then
  echo "--> Downloading Node.js v${NODE_VERSION} (${NODE_ARCH})..."
  TMP_NODE=$(mktemp -d)
  TMP_ARCHIVE=$(mktemp "$TMP_NODE/node-XXXXXX.tar.gz")
  curl -fL --http1.1 --progress-bar --retry 3 --retry-delay 5 --connect-timeout 30 --max-time 300 -o "$TMP_ARCHIVE" "$NODE_URL"
  tar -xz -C "$TMP_NODE" -f "$TMP_ARCHIVE"
  rm -f "$TMP_ARCHIVE"

  # Find extracted directory (name varies by platform/arch)
  EXTRACTED=$(ls -d "$TMP_NODE"/node-v*/ | head -1)

  mkdir -p "$NODE_DEST/bin"
  mkdir -p "$NODE_DEST/lib"

  # Copy the node binary
  cp "$EXTRACTED/bin/node" "$NODE_DEST/bin/node"
  chmod +x "$NODE_DEST/bin/node"

  # Copy the npm module directory (required for npx/npm to work)
  cp -r "$EXTRACTED/lib/node_modules" "$NODE_DEST/lib/"

  rm -rf "$TMP_NODE"
  echo "--> Node.js v${NODE_VERSION} downloaded: $NODE_DEST/bin/node"
fi

# ---------------------------------------------------------------------------
# Download uv
# ---------------------------------------------------------------------------
UV_DEST="$DEST/uv"

if [[ -f "$UV_DEST/uv" ]]; then
  # 使用 uv self version 获取版本号（uv version 在没有 pyproject.toml 时会报错）
  UV_CURRENT=$("$UV_DEST/uv" self version 2>/dev/null | awk '{print $2}' || echo "unknown")
  if [[ "$UV_CURRENT" == "${UV_VERSION}" ]]; then
    echo "--> uv v${UV_VERSION} already present, skipping download"
  else
    echo "--> uv found ($UV_CURRENT), re-downloading v${UV_VERSION}..."
    rm -rf "$UV_DEST"
  fi
fi

if [[ ! -f "$UV_DEST/uv" ]]; then
  mkdir -p "$UV_DEST"
  UV_OBTAINED=false

  # ── Method 1: Copy from any existing system uv ──────────────────────────
  SYSTEM_UV=$(command -v uv 2>/dev/null || true)
  if [[ -n "$SYSTEM_UV" && -x "$SYSTEM_UV" ]]; then
    echo "--> Using system uv at $SYSTEM_UV"
    cp "$SYSTEM_UV" "$UV_DEST/uv"
    chmod +x "$UV_DEST/uv"
    UV_OBTAINED=true
  fi

  # ── Method 2: Install via pip3 into a temp venv ─────────────────────────
  if [[ "$UV_OBTAINED" == "false" ]]; then
    PYTHON3=$(command -v python3 2>/dev/null || true)
    if [[ -n "$PYTHON3" ]]; then
      echo "--> Installing uv via pip (python3)..."
      TMP_VENV=$(mktemp -d)
      if "$PYTHON3" -m venv "$TMP_VENV" 2>/dev/null; then
        if "$TMP_VENV/bin/pip" install --quiet uv 2>/dev/null; then
          PIP_UV=$(find "$TMP_VENV/bin" -maxdepth 1 -name "uv" -type f | head -1)
          if [[ -z "$PIP_UV" ]]; then
            PIP_UV=$(find "$TMP_VENV" -name "uv" ! -name "*.py" -type f | head -1)
          fi
          if [[ -n "$PIP_UV" && -x "$PIP_UV" ]]; then
            cp "$PIP_UV" "$UV_DEST/uv"
            chmod +x "$UV_DEST/uv"
            UV_OBTAINED=true
            echo "--> uv installed via pip3"
          fi
        fi
      fi
      rm -rf "$TMP_VENV"
    fi
  fi

  # ── Method 3: Install via Homebrew (macOS) — slow due to auto-update ─────
  if [[ "$UV_OBTAINED" == "false" ]] && command -v brew &>/dev/null; then
    echo "--> Installing uv via Homebrew (HOMEBREW_NO_AUTO_UPDATE=1)..."
    if HOMEBREW_NO_AUTO_UPDATE=1 brew install uv 2>&1; then
      BREW_UV=$(command -v uv 2>/dev/null || true)
      if [[ -n "$BREW_UV" && -x "$BREW_UV" ]]; then
        cp "$BREW_UV" "$UV_DEST/uv"
        chmod +x "$UV_DEST/uv"
        UV_OBTAINED=true
        echo "--> uv installed via Homebrew: $BREW_UV"
      fi
    fi
  fi

  # ── Method 4: Direct download with resume support ───────────────────────
  if [[ "$UV_OBTAINED" == "false" ]]; then
    echo "--> Downloading uv v${UV_VERSION} from GitHub (slow network fallback)..."
    PARTIAL="$UV_DEST/uv.tar.gz.partial"
    DOWNLOAD_OK=false
    for attempt in 1 2 3 4 5; do
      echo "    Attempt $attempt/5..."
      RESUME_FLAG=""
      [[ -f "$PARTIAL" ]] && RESUME_FLAG="-C -"
      # shellcheck disable=SC2086
      if curl -fL $RESUME_FLAG --http1.1 --progress-bar \
             --connect-timeout 30 --max-time 600 \
             -o "$PARTIAL" "$UV_URL"; then
        DOWNLOAD_OK=true
        break
      fi
      echo "    Download interrupted, retrying in 5 s..."
      sleep 5
    done
    if [[ "$DOWNLOAD_OK" != "true" ]]; then
      echo ""
      echo "ERROR: All download methods failed."
      echo "Please install uv manually and re-run:"
      echo "  brew install uv        # macOS"
      echo "  pip3 install uv        # any platform"
      echo "  curl -LsSf https://astral.sh/uv/install.sh | sh"
      rm -f "$PARTIAL"
      exit 1
    fi
    TMP_UV=$(mktemp -d)
    tar -xz -C "$TMP_UV" -f "$PARTIAL"
    rm -f "$PARTIAL"
    EXTRACTED=$(ls -d "$TMP_UV"/uv-*/ | head -1)
    cp "$EXTRACTED/uv" "$UV_DEST/uv"
    chmod +x "$UV_DEST/uv"
    [[ -f "$EXTRACTED/uvx" ]] && cp "$EXTRACTED/uvx" "$UV_DEST/uvx" && chmod +x "$UV_DEST/uvx"
    rm -rf "$TMP_UV"
    UV_OBTAINED=true
    echo "--> uv v${UV_VERSION} downloaded: $UV_DEST/uv"
  fi
fi

# ---------------------------------------------------------------------------
# Download Python via uv (into bundled runtimes dir)
# ---------------------------------------------------------------------------
PYTHON_INSTALL_DIR="$UV_DEST/python"

# uv 解压后目录名格式: cpython-3.12.x+...
# python-build-standalone tar 解压后目录名: python/
if [[ -d "$PYTHON_INSTALL_DIR" ]] && { ls "$PYTHON_INSTALL_DIR"/cpython-${PYTHON_VERSION}* 1>/dev/null 2>&1 || [[ -d "$PYTHON_INSTALL_DIR/python" ]]; }; then
  echo "--> Python ${PYTHON_VERSION} already present, skipping download"
else
  echo "--> Downloading Python ${PYTHON_VERSION}..."
  mkdir -p "$PYTHON_INSTALL_DIR"
  # uv python install 不支持 --platform，交叉编译时需要直接下载目标架构的 Python
  if [[ "$ARCH_OVERRIDE" != "$HOST_TARGET_ARCH" ]]; then
    if [[ "$ARCH_OVERRIDE" == "arm64" ]]; then
      PY_TRIPLE="aarch64-unknown-linux-gnu"
    else
      PY_TRIPLE="x86_64-unknown-linux-gnu"
    fi
    echo "--> Cross-compiling: downloading Python ${PYTHON_VERSION} for $PY_TRIPLE directly..."
    # 通过 GitHub API 获取 python-build-standalone 最新 release 中匹配的 asset
    DOWNLOAD_URL=$(curl -s "https://api.github.com/repos/astral-sh/python-build-standalone/releases?per_page=5" \
      | python3 -c "
import json, sys, re
releases = json.load(sys.stdin)
pattern = re.compile(r'cpython-${PYTHON_VERSION}\.\d+\+.*-${PY_TRIPLE}-install_only\.tar\.gz')
for rel in releases:
    for asset in rel.get('assets', []):
        if pattern.match(asset['name']) and 'stripped' not in asset['name']:
            print(asset['browser_download_url'])
            sys.exit(0)
print('', end='')
")
    if [[ -z "$DOWNLOAD_URL" ]]; then
      echo "ERROR: Could not find Python ${PYTHON_VERSION} for $PY_TRIPLE in python-build-standalone releases"
      exit 1
    fi
    echo "--> Found: $(basename "$DOWNLOAD_URL")"
    TMP_PY=$(mktemp -d)
    curl -fL --retry 3 --retry-delay 5 -o "$TMP_PY/python.tar.gz" "$DOWNLOAD_URL"
    tar -xzf "$TMP_PY/python.tar.gz" -C "$TMP_PY"
    # tar 解压后目录名为 python/，将其内容移到 PYTHON_INSTALL_DIR
    if [[ -d "$TMP_PY/python" ]]; then
      cp -a "$TMP_PY/python/." "$PYTHON_INSTALL_DIR/"
    fi
    rm -rf "$TMP_PY"
  else
    UV_PYTHON_INSTALL_DIR="$PYTHON_INSTALL_DIR" \
      "$UV_DEST/uv" python install "${PYTHON_VERSION}"
  fi
  echo "--> Python ${PYTHON_VERSION} downloaded to: $PYTHON_INSTALL_DIR"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "==> Runtimes ready in src-tauri/runtimes/"
echo "    Node.js : $($NODE_DEST/bin/node --version)"
echo "    uv      : $($UV_DEST/uv self version 2>/dev/null || $UV_DEST/uv --version 2>/dev/null || echo 'installed')"
if [[ -d "$PYTHON_INSTALL_DIR" ]]; then
  PYTHON_BIN=$(find "$PYTHON_INSTALL_DIR" -name "python3" -type f | head -1)
  if [[ -n "$PYTHON_BIN" ]]; then
    echo "    Python  : $($PYTHON_BIN --version 2>&1)"
  fi
fi
echo ""
echo "Run 'cargo tauri build' or 'cargo tauri dev' to use bundled runtimes."
