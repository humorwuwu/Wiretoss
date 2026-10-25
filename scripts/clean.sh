#!/usr/bin/env bash
# 阶段结束后清中间产物。保留 release/、移动端签名文件和本地 SDK 配置。
#   bash scripts/clean.sh [--dry-run] [--deep]
# 删除：
#   target/、dist/、ios/build/、android/.gradle/、android/.kotlin/、
#   android/build/、android/app/build/
#   .DS_Store、临时测试报告
# --deep 额外删除 android/local.properties（Android Studio/Gradle 可重新生成）
# 不删除：release/、packaging/android/ 里的 keystore 和 keystore.properties、
#         源码、Gradle wrapper、Xcode 工程文件
set -euo pipefail
cd "$(dirname "$0")/.."
DRY=0
DEEP=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY=1 ;;
        --deep) DEEP=1 ;;
    esac
done
remove() {
    [ -e "$1" ] || return 0
    echo "  $1"
    [ "$DRY" = "1" ] || rm -rf "$1"
}
echo "中间产物："
remove target
remove dist
remove ios/build
remove android/.gradle
remove android/.kotlin
remove android/build
remove android/app/build
if [ "$DEEP" = "1" ]; then
    remove android/local.properties
fi
find . -name '.DS_Store' -not -path './.git/*' -delete 2>/dev/null || true
echo "保留：release/、packaging/android/ 签名文件、源码"
