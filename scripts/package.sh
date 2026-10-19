#!/usr/bin/env bash
#
# 在 macOS 上打包两个平台的可分发产物到 dist/。
#
#   ./scripts/package.sh
#
# 前置条件：
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu
#   brew install mingw-w64          # Windows 交叉编译的链接器
#
# Windows 上原生编译走的是 .github/workflows/release.yml，不需要这个脚本。

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
NAME="wiretoss-v${VERSION}"
DIST="dist"
STAGE="$(mktemp -d)"

cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

rm -rf "$DIST"
mkdir -p "$DIST"

echo "==> 构建 macOS arm64"
cargo build --release --target aarch64-apple-darwin

echo "==> 构建 macOS x86_64"
cargo build --release --target x86_64-apple-darwin

echo "==> 组装 Wiretoss.app"
./scripts/make-app.sh "$DIST/Wiretoss.app" \
  target/aarch64-apple-darwin/release/wiretoss \
  target/x86_64-apple-darwin/release/wiretoss

# ditto 而不是 zip：它会正确保留 .app 的包结构和扩展属性，
# 用普通 zip 打出来的包解开后有时会丢掉可执行位。
ditto -c -k --sequesterRsrc --keepParent \
  "$DIST/Wiretoss.app" "$DIST/$NAME-macos-universal.zip"
rm -rf "$DIST/Wiretoss.app"

echo "==> 构建 Windows x86_64"
cargo build --release --target x86_64-pc-windows-gnu

echo "==> 打包"
cp README.md target/x86_64-pc-windows-gnu/release/wiretoss.exe "$STAGE/"
(cd "$STAGE" && zip -q "$OLDPWD/$DIST/$NAME-windows-x64.zip" wiretoss.exe README.md)

(cd "$DIST" && shasum -a 256 ./*.zip > SHA256SUMS)

echo
echo "==> 产物"
ls -lh "$DIST"
echo
cat "$DIST/SHA256SUMS"
