#!/usr/bin/env bash
# =============================================================================
# Setup GitHub Secrets for Tauri updater
# Usage: bash scripts/setup-github-secrets.sh
# =============================================================================
set -euo pipefail

echo "==> Setting up GitHub Secrets for Tauri updater"
echo ""

# Check if gh CLI is installed
if ! command -v gh &> /dev/null; then
    echo "❌ GitHub CLI (gh) is not installed."
    echo ""
    echo "Please install it first:"
    echo "  brew install gh"
    echo ""
    echo "Or visit: https://cli.github.com/"
    exit 1
fi

# Check if authenticated
if ! gh auth status &> /dev/null; then
    echo "❌ Not authenticated with GitHub CLI."
    echo ""
    echo "Please run: gh auth login"
    exit 1
fi

# Check if private key exists
PRIVATE_KEY_PATH="src-tauri/updater/mcphub.key"
if [[ ! -f "$PRIVATE_KEY_PATH" ]]; then
    echo "❌ Private key not found at $PRIVATE_KEY_PATH"
    exit 1
fi

# Read private key
PRIVATE_KEY=$(cat "$PRIVATE_KEY_PATH")
echo "✅ Private key found: ${PRIVATE_KEY:0:50}..."

# Get repository name
REPO=$(gh repo view --json nameWithOwner -q '.nameWithOwner')
echo "📦 Repository: $REPO"

# Set TAURI_SIGNING_PRIVATE_KEY
echo ""
echo "Setting TAURI_SIGNING_PRIVATE_KEY..."
if gh secret set TAURI_SIGNING_PRIVATE_KEY --body "$PRIVATE_KEY"; then
    echo "✅ TAURI_SIGNING_PRIVATE_KEY set successfully"
else
    echo "❌ Failed to set TAURI_SIGNING_PRIVATE_KEY"
    exit 1
fi

# Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
echo ""
echo "Setting TAURI_SIGNING_PRIVATE_KEY_PASSWORD..."
echo "Note: If you didn't set a password when generating the key, just press Enter"
read -p "Enter password (or press Enter for empty): " PASSWORD
if gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body "$PASSWORD"; then
    echo "✅ TAURI_SIGNING_PRIVATE_KEY_PASSWORD set successfully"
else
    echo "❌ Failed to set TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
    exit 1
fi

echo ""
echo "==> GitHub Secrets configured successfully!"
echo ""
echo "Next steps:"
echo "1. Test the build: npm run build"
echo "2. Create a release: git tag v1.0.17 && git push origin v1.0.17"
echo "3. Monitor the build: https://github.com/$REPO/actions"
echo "4. Check the release: https://github.com/$REPO/releases"
