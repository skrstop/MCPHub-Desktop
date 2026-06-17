#!/usr/bin/env bash
# =============================================================================
# Update pubkey in tauri.conf.json
# Usage: bash scripts/update-pubkey.sh
# =============================================================================
set -euo pipefail

TAURI_CONFIG="src-tauri/tauri.conf.json"

echo "==> Updating pubkey in $TAURI_CONFIG"
echo ""

# Check if public key exists
PUBLIC_KEY="$HOME/.tauri/mcphub.key.pub"
if [[ ! -f "$PUBLIC_KEY" ]]; then
    echo "❌ Public key not found at $PUBLIC_KEY"
    echo ""
    echo "Please generate keys first:"
    echo "  mkdir -p ~/.tauri"
    echo "  npx tauri signer generate -w ~/.tauri/mcphub.key"
    echo ""
    echo "See GENERATE_KEYS.md for detailed instructions."
    exit 1
fi

# Read public key
PUBKEY=$(cat "$PUBLIC_KEY")
echo "✅ Public key found: ${PUBKEY:0:50}..."

# Check if tauri.conf.json exists
if [[ ! -f "$TAURI_CONFIG" ]]; then
    echo "❌ $TAURI_CONFIG not found"
    exit 1
fi

# Update pubkey using Python
python3 -c "
import json
import sys

config_path = '$TAURI_CONFIG'
pubkey = '''$PUBKEY'''

try:
    with open(config_path, 'r', encoding='utf-8') as f:
        config = json.load(f)

    # Update pubkey
    if 'plugins' not in config:
        config['plugins'] = {}
    if 'updater' not in config['plugins']:
        config['plugins']['updater'] = {}

    config['plugins']['updater']['pubkey'] = pubkey

    with open(config_path, 'w', encoding='utf-8') as f:
        json.dump(config, f, indent=2, ensure_ascii=False)

    print('✅ pubkey updated successfully in $TAURI_CONFIG')
except Exception as e:
    print(f'❌ Failed to update pubkey: {e}')
    sys.exit(1)
"

echo ""
echo "Next steps:"
echo "1. Add private key to GitHub Secrets:"
echo "   - TAURI_SIGNING_PRIVATE_KEY: \$(cat ~/.tauri/mcphub.key)"
echo "   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD: your-password (if any)"
echo ""
echo "2. Test the build:"
echo "   npm run build"
echo ""
echo "3. Create a release:"
echo "   git tag v1.0.17"
echo "   git push origin v1.0.17"
