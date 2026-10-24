#!/usr/bin/env bash
#
# 在 macOS 上打包可分发产物：
#   release/macos/Wiretoss.app
#   release/macos/wiretoss-v<version>-macos-universal.zip
#   release/windows/wiretoss-v<version>-windows-x64.zip
#
#   ./scripts/package.sh
#
# 前置条件：
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu
#   brew install mingw-w64          # Windows 交叉编译的链接器
#
# Windows 上原生编译走的是 .github/workflows/release.yml，不需要这个脚本。
# 那个 workflow 里的 dist/ 是 Actions 下载目录，不是本仓库的发行目录，不要改它来「统一命名」。

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
NAME="wiretoss-v${VERSION}"
REL_MAC="release/macos"
REL_WIN="release/windows"
STAGE="$(mktemp -d)"

cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

mkdir -p "$REL_MAC" "$REL_WIN"
rm -rf "$REL_MAC/Wiretoss.app"
rm -f "$REL_MAC"/*.zip "$REL_MAC"/*.sha256 "$REL_WIN"/*.zip "$REL_WIN"/*.sha256

echo "==> 构建 macOS arm64"
cargo build --release --target aarch64-apple-darwin

echo "==> 构建 macOS x86_64"
cargo build --release --target x86_64-apple-darwin

echo "==> 组装 Wiretoss.app"
./scripts/make-app.sh "$REL_MAC/Wiretoss.app" \
  target/aarch64-apple-darwin/release/wiretoss \
  target/x86_64-apple-darwin/release/wiretoss

# ditto 而不是 zip：它会正确保留 .app 的包结构和扩展属性，
# 用普通 zip 打出来的包解开后有时会丢掉可执行位。
ditto -c -k --sequesterRsrc --keepParent \
  "$REL_MAC/Wiretoss.app" "$REL_MAC/$NAME-macos-universal.zip"

echo "==> 构建 Windows x86_64"
cargo build --release --target x86_64-pc-windows-gnu

echo "==> 打包"
cp README.md target/x86_64-pc-windows-gnu/release/wiretoss.exe "$STAGE/"
(cd "$STAGE" && zip -q "$OLDPWD/$REL_WIN/$NAME-windows-x64.zip" wiretoss.exe README.md)

(cd "$REL_MAC" && shasum -a 256 ./*.zip > SHA256SUMS)
(cd "$REL_WIN" && shasum -a 256 ./*.zip > SHA256SUMS)

echo
echo "==> 产物"
ls -lh "$REL_MAC" "$REL_WIN"
echo
echo "-- macos --"
cat "$REL_MAC/SHA256SUMS"
echo "-- windows --"
cat "$REL_WIN/SHA256SUMS"
