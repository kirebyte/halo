#!/usr/bin/env bash
# Builds Halo's Decky plugin: out/halo/ (the plugin folder) and out/halo.zip (for Decky's
# "Install plugin from ZIP"). Needs node and npm.
#   ./build.sh [--no-install]
set -euo pipefail
cd "$(dirname "$0")"

if [[ "${1:-}" != "--no-install" ]]; then
    if [[ -e package-lock.json ]]; then npm ci --no-audit --no-fund; else npm install --no-audit --no-fund; fi
fi
npm run build

rm -rf out
mkdir -p out/halo/dist out/halo/locales
cp plugin.json package.json main.py README.md out/halo/
cp ../LICENSE out/halo/
cp dist/index.js out/halo/dist/
cp ../locales/*.json out/halo/locales/
python3 - <<'EOF'
import os, zipfile
with zipfile.ZipFile("out/halo.zip", "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk("out/halo"):
        for f in sorted(files):
            p = os.path.join(root, f)
            z.write(p, os.path.relpath(p, "out"))
EOF
echo "built out/halo and out/halo.zip"
