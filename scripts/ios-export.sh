#!/usr/bin/env bash
# 归档并导出 iOS .ipa。
#
# 前置：
#   1. Xcode 已登录付费 Apple Developer 账号
#      （Xcode > Settings > Accounts；没有这一步 xcodebuild 会报
#       "No Account for Team"）
#   2. 设置 WIRETOSS_TEAM_ID 为你的 10 位 Team ID
#   3. 已安装 xcodegen（brew install xcodegen）
#
# 用法：
#   WIRETOSS_TEAM_ID=XXXXXXXXXX ./scripts/ios-export.sh
#   # App Store 分发包（需要分发证书，Xcode 会自动申请）：
#   WIRETOSS_TEAM_ID=XXXXXXXXXX WIRETOSS_EXPORT_METHOD=app-store-connect ./scripts/ios-export.sh
#
# 注意：分发签名和上传 App Store Connect 必须用账号本人操作；不要把密码/2FA 交给脚本。
set -euo pipefail
cd "$(dirname "$0")/.."

TEAM_ID="${WIRETOSS_TEAM_ID:-}"
METHOD="${WIRETOSS_EXPORT_METHOD:-development}"
if [ -z "$TEAM_ID" ]; then
    echo "请先设置 WIRETOSS_TEAM_ID（Xcode 账号里的 Team ID）" >&2
    exit 1
fi
command -v xcodegen >/dev/null || {
    echo "需要 xcodegen：brew install xcodegen" >&2
    exit 1
}

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
NAME="wiretoss-v${VERSION}-ios-arm64"
ARCHIVE="ios/build/Wiretoss.xcarchive"
EXPORT_DIR="ios/build/export"
EXPORT_PLIST="ios/build/ExportOptions.plist"

(cd ios && xcodegen generate)

rm -rf "$ARCHIVE" "$EXPORT_DIR"
mkdir -p ios/build

echo "==> 归档（${METHOD}）"
xcodebuild archive \
    -project ios/Wiretoss.xcodeproj \
    -scheme Wiretoss \
    -configuration Release \
    -destination 'generic/platform=iOS' \
    -archivePath "$ARCHIVE" \
    -derivedDataPath ios/build/ArchiveDD \
    DEVELOPMENT_TEAM="$TEAM_ID" \
    CODE_SIGN_STYLE=Automatic \
    -allowProvisioningUpdates

cat > "$EXPORT_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>${METHOD}</string>
    <key>signingStyle</key>
    <string>automatic</string>
    <key>teamID</key>
    <string>${TEAM_ID}</string>
    <key>stripSwiftSymbols</key>
    <true/>
    <key>compileBitcode</key>
    <false/>
    <key>destination</key>
    <string>export</string>
</dict>
</plist>
PLIST

echo "==> 导出 IPA"
xcodebuild -exportArchive \
    -archivePath "$ARCHIVE" \
    -exportPath "$EXPORT_DIR" \
    -exportOptionsPlist "$EXPORT_PLIST" \
    -allowProvisioningUpdates

IPA="$(find "$EXPORT_DIR" -maxdepth 1 -name '*.ipa' | head -1)"
if [ -z "$IPA" ]; then
    echo "导出失败：没有找到 .ipa" >&2
    exit 1
fi

mkdir -p release/ios
cp "$IPA" "release/ios/${NAME}.ipa"
(cd release/ios && shasum -a 256 ./*.ipa > SHA256SUMS)
echo
echo "==> 产物"
ls -lh release/ios
cat release/ios/SHA256SUMS
