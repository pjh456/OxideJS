#!/usr/bin/env bash
# ASan 常设门：按地址 sanitizer 配方跑全 workspace 单测。
#
# 用法: scripts/asan_gate.sh
#   无参数。nightly 工具链加地址 sanitizer 构建并跑 cargo test --workspace。
#
# 配方说明：detect_leaks=0 因 PermInterner 与静态表设计性永久泄漏，
#   泄漏检测纯噪声；quarantine_size_mb=64 收隔离区，压高并发下内存放大。
#   RUSTC 指到 asan_rustc_wrapper.sh：仅 oxide_* crate 带 sanitizer 插桩，
#   依赖树（含 proc-macro）不带（proc-macro 动态库加载约束，见包装器注释）。
#
# 退出码: 0 全部通过；1 构建失败或测试失败或 ASan 报告；2 nightly 工具链缺失。

set -u

cd "$(dirname "$0")/.."

# 前置检查：nightly 工具链必须已装（rust-toolchain.toml 钉 stable，须显式切换）。
if ! RUSTUP_TOOLCHAIN=nightly cargo --version >/dev/null 2>&1; then
  echo "缺 nightly 工具链：rustup toolchain install nightly" >&2
  exit 2
fi

export RUSTUP_TOOLCHAIN=nightly
export RUSTFLAGS="-Zsanitizer=address"
export CARGO_TARGET_DIR=target-asan
export ASAN_OPTIONS="detect_leaks=0:quarantine_size_mb=64"
# proc-macro 动态库加载进无 ASan 运行时的 rustc 会失败，
# 经包装器剥离其 sanitizer 标志（见包装器头部注释）。
export RUSTC="$PWD/scripts/asan_rustc_wrapper.sh"

cargo test --workspace
