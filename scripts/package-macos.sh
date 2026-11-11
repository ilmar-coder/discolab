#!/usr/bin/env bash
# Cria DiscoLab.app (com smartctl embutido, se instalado via Homebrew).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p discolab -p discolab-cli
APP=target/DiscoLab.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS"
cp target/release/discolab target/release/discolab-cli "$APP/Contents/MacOS/"
SC="$(command -v smartctl || true)"
[ -n "$SC" ] && cp "$SC" "$APP/Contents/MacOS/" && echo "smartctl embutido."
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>DiscoLab</string>
<key>CFBundleIdentifier</key><string>com.discolab.app</string>
<key>CFBundleExecutable</key><string>discolab</string>
<key>CFBundleVersion</key><string>0.1.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PL
echo "Pronto: $APP"
