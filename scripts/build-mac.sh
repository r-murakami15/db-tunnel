#!/usr/bin/env bash
# Mac用の DB Tunnel.app をビルドする（ホストにRustは不要）。
#
#   1. Dockerコンテナ内でフロントエンドをビルドし、cargo-zigbuild で macOS 向けにクロスコンパイル
#   2. ホストで .app バンドルを組み立て、アドホック署名（codesign -s -）する
#   3. --dmg を付けると .dmg も作る
#
# 使い方:
#   scripts/build-mac.sh                # このMacのCPU向け（Apple Silicon なら arm64）
#   scripts/build-mac.sh --universal    # Apple Silicon + Intel のユニバーサル
#   scripts/build-mac.sh --dmg          # .dmg も作る
#
# 成果物: dist-mac/DB Tunnel.app（と dist-mac/DB Tunnel_<version>_<arch>.dmg）
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

UNIVERSAL=0
MAKE_DMG=0
for arg in "$@"; do
  case "$arg" in
    --universal) UNIVERSAL=1 ;;
    --dmg) MAKE_DMG=1 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "不明なオプション: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "このスクリプトはMacで実行してください" >&2
  exit 1
fi

if [[ "$UNIVERSAL" == 1 ]]; then
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
  ARCH_LABEL=universal
elif [[ "$(uname -m)" == "arm64" ]]; then
  TARGETS=(aarch64-apple-darwin)
  ARCH_LABEL=aarch64
else
  TARGETS=(x86_64-apple-darwin)
  ARCH_LABEL=x64
fi

APP_NAME="DB Tunnel"
BIN_NAME="db-tunnel"
VERSION="$(plutil -extract version raw -o - src-tauri/tauri.conf.json)"
IDENTIFIER="$(plutil -extract identifier raw -o - src-tauri/tauri.conf.json)"
OUT="$ROOT/dist-mac"

echo "==> Dockerイメージを準備"
docker compose build dev

echo "==> macOS SDK をDockerボリュームへ用意"
if ! docker compose run --rm -T dev test -e /opt/macos-sdk/MacOSX.sdk/SDKSettings.json; then
  SDK_DIR="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
  if [[ -z "$SDK_DIR" || ! -d "$SDK_DIR" ]]; then
    SDK_DIR="/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk"
  fi
  if [[ ! -d "$SDK_DIR" ]]; then
    echo "macOS SDK が見つかりません。'xcode-select --install' で Command Line Tools を入れてください" >&2
    exit 1
  fi
  SDK_REAL="$(cd "$SDK_DIR" && pwd -P)"
  SDK_BASENAME="$(basename "$SDK_REAL")"
  echo "    $SDK_REAL をコピーします"
  tar -C "$(dirname "$SDK_REAL")" -cf - "$SDK_BASENAME" 2>/dev/null \
    | docker compose run --rm -T dev bash -c \
        "tar -xf - -C /opt/macos-sdk 2>/dev/null; ln -sfn '$SDK_BASENAME' /opt/macos-sdk/MacOSX.sdk"
fi
# zig のリンカ向けに .tbd を補正（何度実行しても同じ結果）
docker compose run --rm -T dev bash scripts/patch-sdk-tbd.sh

echo "==> フロントエンドとRustをビルド（${TARGETS[*]}）"
docker compose run --rm -T \
  -e TARGETS="${TARGETS[*]}" \
  dev bash -euo pipefail -c '
    npm install --no-audit --no-fund
    npm run build
    mkdir -p /work/dist-mac/bin
    for t in $TARGETS; do
      (cd src-tauri && cargo zigbuild --release --target "$t" --features tauri/custom-protocol)
      cp "$CARGO_TARGET_DIR/$t/release/db-tunnel" "/work/dist-mac/bin/db-tunnel-$t"
    done
  '

echo "==> .app バンドルを作成"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [[ "${#TARGETS[@]}" -gt 1 ]]; then
  lipo -create -output "$APP/Contents/MacOS/$BIN_NAME" \
    "$OUT/bin/$BIN_NAME-aarch64-apple-darwin" "$OUT/bin/$BIN_NAME-x86_64-apple-darwin"
else
  cp "$OUT/bin/$BIN_NAME-${TARGETS[0]}" "$APP/Contents/MacOS/$BIN_NAME"
fi
chmod +x "$APP/Contents/MacOS/$BIN_NAME"
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>ja</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleExecutable</key><string>$BIN_NAME</string>
  <key>CFBundleIdentifier</key><string>$IDENTIFIER</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><true/>
</dict>
</plist>
PLIST

echo "==> アドホック署名（Apple Silicon で起動するために必要。配布用のコード署名ではない）"
codesign --force --deep --sign - "$APP"
codesign --verify --verbose=1 "$APP"

if [[ "$MAKE_DMG" == 1 ]]; then
  echo "==> .dmg を作成"
  DMG="$OUT/${APP_NAME}_${VERSION}_${ARCH_LABEL}.dmg"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  echo "    $DMG"
fi

echo
echo "完了: $APP"
echo "起動: open \"$APP\""
