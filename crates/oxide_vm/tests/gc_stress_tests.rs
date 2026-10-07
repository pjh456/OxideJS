//! GC 压力模式开关验证：环境变量 `OXIDE_GC_PRESSURE` 存在即开启，开启后每个
//! 顶层指令边界做一次完整收集（与 `$262.gc()` 强制收集共用同一安全点入口），
//! 缺省时行为与现状完全一致。
//!
//! 单测试函数内顺序跑开/关两臂：同一文件内多测试并行会竞争 `set_var`，
//! 单函数顺序执行免除该竞争（环境变量为进程级，跨测试二进制天然隔离）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

/// 单测辅助：在调用方提供的 VM 上编译并执行源串，返回原始结果值。
fn eval_on(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse error: {}", e[0].message))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("compile error: {e}"))?;
    vm.run(&Arc::new(module))
}

/// 带分配的短脚本：全局可达对象 + 100 次循环字符串分配（产生大量顶层指令边界）。
/// 末条表达式同时是存活钉：对象身份（`$262.global.o === globalThis.o`）与
/// 属性读回（`x + s.length === 46`）在反复收集后必须保持完整。
const SCRIPT: &str = "(function () { \
  globalThis.o = { x: 41, s: 'keep' }; \
  globalThis.o.x = 42; \
  for (var i = 0; i < 100; i++) { var t = 'str' + i; } \
  return globalThis.o.x + globalThis.o.s.length === 46 \
     && $262.global.o === globalThis.o; })()";

/// 开臂：压力模式开启后每个顶层边界做一次完整收集，收集计数与顶层指令边界数
/// 同量级（100 次循环迭代至少 100 个边界），全局可达对象身份与属性读回完整。
/// 关臂：同一脚本在缺省环境下退化为现状——小脚本分配不超水位，零收集。
#[test]
fn gc_pressure_mode_env_toggle() {
    // 开臂：设环境变量后构造 VM（每次构造读环境变量，不缓存）。
    std::env::set_var("OXIDE_GC_PRESSURE", "1");
    let mut vm = Vm::new();
    let result = eval_on(&mut vm, SCRIPT).expect("脚本不应运行失败");
    assert!(result.as_bool(), "存活钉读回应完整");
    let total = vm.session_gc_stats().total_collections;
    assert!(total > 100, "压力模式应每个顶层边界做一次完整收集，实际 {total}");
    // 关臂：移除环境变量后同一脚本走退化路径。
    std::env::remove_var("OXIDE_GC_PRESSURE");
    let mut vm = Vm::new();
    let result = eval_on(&mut vm, SCRIPT).expect("脚本不应运行失败");
    assert!(result.as_bool(), "存活钉读回应完整");
    assert_eq!(vm.session_gc_stats().total_collections, 0, "关臂小脚本分配不超水位，收集应为零");
}
