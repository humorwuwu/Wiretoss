#!/usr/bin/env bash
#
# 把编译好的二进制组装成 macOS 的 Wiretoss.app。
#
#   ./scripts/make-app.sh <输出路径> <二进制> [另一个架构的二进制...]
#
# 传两个二进制时会先用 lipo 合成通用二进制。
# 本地打包（package.sh）和 CI 的 release 流程共用这一份逻辑。

set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -lt 2 ]; then
  echo "用法：$0 <输出路径> <二进制> [另一个二进制...]" >&2
  exit 1
fi

OUT="$1"
shift
BINS=("$@")

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"

STAGE="$(mktemp -d)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"

if [ ${#BINS[@]} -gt 1 ]; then
  lipo -create "${BINS[@]}" -output "$OUT/Contents/MacOS/wiretoss"
else
  cp "${BINS[0]}" "$OUT/Contents/MacOS/wiretoss"
fi

# lipo 会让输入各自的 ad-hoc 签名作废。Apple Silicon 上**未签名**的二进制
# 内核直接拒绝执行（报 "killed: 9"），所以这一步不能省。
codesign --force --sign - "$OUT/Contents/MacOS/wiretoss"

sed "s/__VERSION__/${VERSION}/g" packaging/macos/Info.plist > "$OUT/Contents/Info.plist"

# 图标：用系统自带的 qlmanage 把 SVG 渲染成 PNG，再 sips + iconutil 转 .icns。
# 这样就不必为了一个图标引入 image 之类的 crate。
#
# SVG 放在 packaging/ 而不是 assets/：assets/ 整个目录会被 rust-embed 编进
# 二进制，而这个图标只有打包时用得上，放进那里等于给每个用户白送一份。
qlmanage -t -s 1024 -o "$STAGE" packaging/icon.svg >/dev/null 2>&1
ICON_PNG="$STAGE/icon.svg.png"

# qlmanage 遇到非法 SVG 时只在 stderr 抱怨一句，退出码依然是 0。
# 不自己验一下产物，就会安安静静打出一个没有图标的 .app。
if [ ! -f "$ICON_PNG" ]; then
  echo "错误：packaging/icon.svg 渲染失败，检查它是不是合法 SVG" >&2
  exit 1
fi

ICONSET="$STAGE/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ICON_PNG" \
    --out "$ICONSET/icon_${size}x${size}.png" >/dev/null 2>&1
  sips -z "$((size * 2))" "$((size * 2))" "$ICON_PNG" \
    --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null 2>&1
done
iconutil -c icns "$ICONSET" -o "$OUT/Contents/Resources/AppIcon.icns"

# 签整个 bundle。没有 Apple 开发者证书只能 ad-hoc 签，
# 用户从网上下载后仍会被 Gatekeeper 拦一次（右键打开即可）。
codesign --force --sign - "$OUT"
codesign --verify --verbose "$OUT"

echo "已生成 $OUT"
