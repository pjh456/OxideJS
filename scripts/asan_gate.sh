#!/usr/bin/env bash
# ASan 常设门：按地址 sanitizer 配方跑全 workspace 单测。
#
# 用法: scripts/asan_gate.sh
#   无参数。nightly 工具链加地址 sanitizer 构建并跑 cargo test --workspace --no-fail-fast。
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
# 禁用 sccache：其 Rust direct mode 由 server 自行解析工具链 rustc 直接编译
# rlib 单元，绕过上面的 RUSTC 包装器链，依赖树（version_check、autocfg 等）
# 会带着 sanitizer 插桩编译，链接进 build script 时因缺 ASan 运行时而失败。
# 门的构建量小（仅依赖树多一次编译），不需要 sccache 加速。
export CARGO_BUILD_RUSTC_WRAPPER=""

# no-fail-fast：预存失败测试不应中断内存安全门，须跑完全部测试目标
# 再汇总（门判据是 ASan 报告与失败清单，任一失败目标不阻断其余目标）。
cargo test --workspace --no-fail-fast
