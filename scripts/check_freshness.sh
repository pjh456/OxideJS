#!/usr/bin/env bash
# 二进制新鲜度检查：判定二进制是否落后于当前 HEAD。
#
# 用法: scripts/check_freshness.sh [二进制路径 ...]
#   缺省检查 target/debug/oxide 与 target/debug/test262-runner。
#
# 两条判据，均满足才判「新鲜」：
#   1. 二进制 --version 自报的 git 短哈希与 git rev-parse --short HEAD 一致；
#   2. 无源文件比二进制新（find crates third_party -name '*.rs' -newer <bin> 计数为 0）。
#
# 退出码: 0 全部新鲜；1 任一 stale 或缺失。无 git 环境跳过判据 1（只查判据 2）。

set -u

cd "$(dirname "$0")/.."

bins=("$@")
if [ ${#bins[@]} -eq 0 ]; then
  bins=(target/debug/oxide target/debug/test262-runner)
fi

head_hash="$(git rev-parse --short HEAD 2>/dev/null || true)"

stale=0
for bin in "${bins[@]}"; do
  if [ ! -x "$bin" ]; then
    echo "stale: $bin (not built)"
    stale=1
    continue
  fi

  # 判据 1: 二进制自报哈希与 HEAD 一致（构建期无 git 的 "unknown" 视为不可自证）。
  if [ -n "$head_hash" ]; then
    bin_hash="$(printf '%s\n' "$("$bin" --version 2>/dev/null | head -n1)" | grep -oE '\([0-9a-f]{7,}\)' | tr -d '()')"
    if [ "$bin_hash" != "$head_hash" ]; then
      echo "stale: $bin (self-reported ${bin_hash:-none}, HEAD $head_hash)"
      stale=1
      continue
    fi
  fi

  # 判据 2: 无源文件比二进制新。
  newer="$(find crates third_party -name '*.rs' -newer "$bin" 2>/dev/null | wc -l)"
  if [ "$newer" -gt 0 ]; then
    echo "stale: $bin ($newer source file(s) newer than binary)"
    stale=1
    continue
  fi

  echo "fresh: $bin"
done

exit "$stale"
