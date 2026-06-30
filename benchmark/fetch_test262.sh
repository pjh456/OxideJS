#!/bin/bash
# 下载 test262 测试套件到仓库 tests/ 目录
set -e
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT/tests"
if [ -d test262/test ]; then
    echo "test262 已存在: $(find test262/test -name '*.js' | wc -l) 测试"
    exit 0
fi
echo "下载 test262 (约 500MB)..."
git clone --depth 1 https://github.com/tc39/test262.git test262
echo "完成"
