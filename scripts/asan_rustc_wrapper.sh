#!/usr/bin/env bash
# ASan 构建用 rustc 包装器：仅引擎 crate 与最终链接单元带地址 sanitizer 标志。
#
# 门的目标是检测引擎代码的真实内存错误，故只给 oxide_* 工作区 crate
# 插桩，整个依赖树（含 proc-macro）不带 sanitizer 编译。两个原因：
#   1. proc-macro 以动态库加载进未带 ASan 运行时的 rustc 进程，
#      带 sanitizer 的 proc-macro 动态库加载即失败（未定义 ASan 符号）；
#   2. proc-macro 动态库把自身依赖（proc-macro2、syn 等）以 rlib 链入，
#      只剥 proc-macro 自身的标志不够，ASan 插桩的依赖仍会带进动态库。
#
# 最终链接单元（--test 测试目标、--crate-type bin 的二进制目标）也要带
# 标志：它们链入带插桩的引擎 rlib，须由 rustc 链接 ASan 运行时；
# 构建脚本（crate 名 build_script_build）是独立可执行文件，不插桩。
#
# 用法：常设门把 RUSTC 指到本脚本。按 --crate-name 与编译类型参数裁决，
# 保留或剥离 -Zsanitizer=address 后透传给 rustc。

crate_name=""
is_bin=0
has_test=0
prev=""
for a in "$@"; do
  if [ "$prev" = "--crate-name" ]; then
    crate_name="$a"
  fi
  if [ "$prev" = "--crate-type" ]; then
    case "$a" in
      bin) is_bin=1 ;;
    esac
  fi
  case "$a" in
    --crate-type=bin) is_bin=1 ;;
    --test) has_test=1 ;;
  esac
  prev="$a"
done

keep=0
case "$crate_name" in
  build_script_build) ;;
  oxide*) keep=1 ;;
  *)
    if [ "$is_bin" -eq 1 ] || [ "$has_test" -eq 1 ]; then
      keep=1
    fi
    ;;
esac

if [ "$keep" -eq 1 ]; then
  exec rustc "$@"
fi

out=()
for a in "$@"; do
  case "$a" in
    -Zsanitizer=address) continue ;;
  esac
  out+=("$a")
done

exec rustc "${out[@]}"
