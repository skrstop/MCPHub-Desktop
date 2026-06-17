#!/usr/bin/env bash
# =============================================================================
# Verify Tauri updater signing configuration
# Usage: bash scripts/verify-signing.sh
# =============================================================================
set -euo pipefail

echo "==> Verifying Tauri updater signing configuration..."
echo ""

# Check if tauri.conf.json exists
TAURI_CONFIG="src-tauri/tauri.conf.json"
if [[ ! -f "$TAURI_CONFIG" ]]; then
    echo "ERROR: $TAURI_CONFIG not found"
    exit 1
fi

# Check if pubkey is configured
PUBKEY=$(grep -o '"pubkey": *"[^"]*"' "$TAURI_CONFIG" | cut -d'"' -f4)
if [[ -z "$PUBKEY" || "$PUBKEY" == "" ]]; then
    echo "❌ FAIL: pubkey is empty in $TAURI_CONFIG"
    echo ""
    echo "Please run: bash scripts/generate-signing-key.sh"
    echo "Then update the pubkey in $TAURI_CONFIG"
    exit 1
else
    echo "✅ pubkey is configured in $TAURI_CONFIG"
fi

# Check if private key exists
PRIVATE_KEY="$HOME/.tauri/mcphub.key"
PRIVATE_KEY_ALT="src-tauri/updater/mcphub.key"
if [[ -f "$PRIVATE_KEY" ]]; then
    echo "✅ Private key found at $PRIVATE_KEY"
elif [[ -f "$PRIVATE_KEY_ALT" ]]; then
    echo "✅ Private key found at $PRIVATE_KEY_ALT"
    PRIVATE_KEY="$PRIVATE_KEY_ALT"
else
    echo "⚠️  WARN: Private key not found at $PRIVATE_KEY or $PRIVATE_KEY_ALT"
    echo "   This is okay for CI builds (uses GitHub Secrets)"
    echo "   For local builds, run: bash scripts/generate-signing-key.sh"
fi

# Check if public key exists
PUBLIC_KEY="$HOME/.tauri/mcphub.key.pub"
PUBLIC_KEY_ALT="src-tauri/updater/mcphub.key.pub"
if [[ -f "$PUBLIC_KEY" ]]; then
    echo "✅ Public key found at $PUBLIC_KEY"
elif [[ -f "$PUBLIC_KEY_ALT" ]]; then
    echo "✅ Public key found at $PUBLIC_KEY_ALT"
    PUBLIC_KEY="$PUBLIC_KEY_ALT"

    # Verify keys match
    if [[ -f "$PRIVATE_KEY" ]]; then
        STORED_PUBKEY=$(cat "$PUBLIC_KEY")
        if [[ "$PUBKEY" == "$STORED_PUBKEY" ]]; then
            echo "✅ Keys match: tauri.conf.json pubkey matches ~/.tauri/mcphub.key.pub"
        else
            echo "❌ FAIL: Keys do not match!"
            echo ""
            echo "tauri.conf.json pubkey: ${PUBKEY:0:50}..."
            echo "~/.tauri/mcphub.key.pub: ${STORED_PUBKEY:0:50}..."
            echo ""
            echo "Please update tauri.conf.json with the correct pubkey"
            exit 1
        fi
    fi
else
    echo "⚠️  WARN: Public key not found at $PUBLIC_KEY"
fi

# Check if GitHub Secrets are documented
echo ""
echo "📋 GitHub Secrets Checklist:"
echo "   □ TAURI_SIGNING_PRIVATE_KEY - Private key content"
echo "   □ TAURI_SIGNING_PRIVATE_KEY_PASSWORD - Key password (if any)"
echo ""

# Check if release.yml exists
RELEASE_YML=".github/workflows/release.yml"
if [[ -f "$RELEASE_YML" ]]; then
    echo "✅ Release workflow found at $RELEASE_YML"

    # Check if signing is configured in release.yml
    if grep -q "TAURI_SIGNING_PRIVATE_KEY" "$RELEASE_YML"; then
        echo "✅ TAURI_SIGNING_PRIVATE_KEY referenced in release.yml"
    else
        echo "❌ FAIL: TAURI_SIGNING_PRIVATE_KEY not found in release.yml"
        exit 1
    fi
else
    echo "❌ FAIL: Release workflow not found at $RELEASE_YML"
    exit 1
fi

echo ""
echo "==> Verification complete!"
echo ""
echo "If all checks passed, your signing configuration is ready."
echo "To test the update flow:"
echo "  1. Build the app: npm run build"
echo "  2. Create a release: git tag v1.0.17 && git push origin v1.0.17"
echo "  3. Check GitHub Actions for build status"
echo "  4. Download and test the update"
