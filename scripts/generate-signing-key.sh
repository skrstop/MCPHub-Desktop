#!/usr/bin/env bash
# =============================================================================
# Generate Tauri updater signing key pair
# Usage: bash scripts/generate-signing-key.sh
# =============================================================================
set -euo pipefail

echo "==> Generating Tauri updater signing key pair..."
echo ""

# Check if tauri CLI is available
if ! command -v npx &> /dev/null; then
    echo "ERROR: npx is not installed. Please install Node.js first."
    exit 1
fi

# Generate key pair
echo "Generating key pair..."
npx tauri signer generate -w ~/.tauri/mcphub.key

echo ""
echo "==> Key pair generated successfully!"
echo ""
echo "Next steps:"
echo "1. Copy the PUBLIC KEY below and paste it into src-tauri/tauri.conf.json -> plugins.updater.pubkey"
echo "2. Add the PRIVATE KEY to GitHub Secrets as TAURI_SIGNING_PRIVATE_KEY"
echo "3. Add the private key password (if any) to GitHub Secrets as TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo ""
echo "Public key location: ~/.tauri/mcphub.key.pub"
echo "Private key location: ~/.tauri/mcphub.key"
echo ""
echo "To view the public key:"
echo "  cat ~/.tauri/mcphub.key.pub"
echo ""
