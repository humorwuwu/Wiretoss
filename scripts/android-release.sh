#!/usr/bin/env bash
# 打已签名的 Android APK + AAB，并复制到 release/android/。
#
# 前置：
#   1. packaging/android/keystore.properties 存在，且指向有效 keystore
#      （由首次配置时生成；这是密钥，已 gitignore，请备份）
#   2. JDK 17+，默认使用 Homebrew openjdk@21
#   3. android/local.properties 指向 Android SDK
#
# 用法：
#   ./scripts/android-release.sh
set -euo pipefail
cd "$(dirname "$0")/.."

export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}"
export PATH="$JAVA_HOME/bin:$PATH"

if [ ! -f packaging/android/keystore.properties ]; then
    echo "错误：缺少 packaging/android/keystore.properties，无法签名。" >&2
    echo "请先生成 keystore，并把它和密码文件备份到安全位置。" >&2
    exit 1
fi

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
NAME="wiretoss-v${VERSION}-android-universal"

echo "==> 构建已签名 APK / AAB"
(cd android && ./gradlew :app:assembleRelease :app:bundleRelease --no-daemon)

mkdir -p release/android
cp android/app/build/outputs/apk/release/app-release.apk "release/android/${NAME}.apk"
cp android/app/build/outputs/bundle/release/app-release.aab "release/android/${NAME}.aab"
(cd release/android && shasum -a 256 ./*.apk ./*.aab > SHA256SUMS)

echo
echo "==> 产物"
ls -lh release/android
echo
cat release/android/SHA256SUMS
