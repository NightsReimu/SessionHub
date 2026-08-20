#!/bin/bash
# 手动打 DMG（tauri build 的 dmg 步骤依赖 Finder AppleScript，在某些会话里会超时）
# 用法：先 npx tauri build，再运行 scripts/make-dmg.sh
set -e
cd "$(dirname "$0")/../src-tauri/target/release/bundle/dmg"
VERSION=$(grep -m1 '"version"' ../../../../tauri.conf.json | cut -d'"' -f4)
ARCH=$(uname -m)
[ "$ARCH" = "arm64" ] && ARCH=aarch64
ln -sfn ../macos/SessionHub.app SessionHub.app
bash bundle_dmg.sh \
  --volname SessionHub \
  --icon SessionHub.app 180 170 \
  --app-drop-link 480 170 \
  --window-size 660 400 \
  --hide-extension SessionHub.app \
  --volicon icon.icns \
  "SessionHub_${VERSION}_${ARCH}.dmg" \
  SessionHub.app
echo "==> $(ls -la SessionHub_${VERSION}_${ARCH}.dmg)"
