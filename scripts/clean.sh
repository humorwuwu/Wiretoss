#!/usr/bin/env bash
# 阶段结束后清 Cargo 中间产物。保留 release/。
#   bash scripts/clean.sh [--dry-run]
set -euo pipefail
cd "$(dirname "$0")/.."
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1
remove() {
  [ -e "$1" ] || return 0
  echo "  $1"
  [ "$DRY" = "1" ] || rm -rf "$1"
}
echo "中间产物："
remove target
remove dist
find . -name '.DS_Store' -not -path './.git/*' -delete 2>/dev/null || true
echo "保留：release/"
