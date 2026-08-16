use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

fn vm_with_threshold(bytes: usize) -> Vm {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(bytes);
    Vm::with_kernel_core(KernelCore::new(config))
}

/// 长循环拼接产生海量 session 字符串：执行期自动触发仅字符串 GC，
/// 结果正确且 session 字节有界（不触发则累计 ~20 MiB 远超低阈值）。
#[test]
fn long_string_loop_triggers_runtime_gc_and_bounds_session() {
    let mut vm = vm_with_threshold(4096);
    let module = compile("var s; for (var i = 0; i < 500000; i++) { s = 'str' + i; } s.length");
    let result = vm.run(&module).expect("run");

    assert_eq!(format!("{}", result), "9");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
    assert!(
        vm.session_bytes_allocated() < 1_000_000,
        "session 字节应受阈值约束，实际 {}",
        vm.session_bytes_allocated()
    );
}

/// 拼接结果写 global（逃逸根），跨 run 后触发执行期 GC 仍可读——
/// 验证非会话存活串不因执行期回收丢失。
#[test]
fn session_string_survives_across_runs_with_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("globalThis.kept = 'he' + 'llo'; 0");
    vm.run(&first).expect("run1");

    let second = compile("var s; for (var i = 0; i < 500000; i++) { s = 'y' + i; } globalThis.kept");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept 应为字符串").to_string();

    assert_eq!(text, "hello");
    assert!(vm.session_gc_stats().total_collections > 0, "run2 应触发执行期字符串 GC");
}

/// reset（轻量重置）保留 session 字符串：跨 eval 存活语义不被执行期 GC 破坏。
#[test]
fn reset_preserves_session_strings_after_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("globalThis.kept = 'x' + 'y'; 0");
    vm.run(&first).expect("run1");
    vm.reset();

    let second = compile("globalThis.kept");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept 应为字符串").to_string();
    assert_eq!(text, "xy");
}

/// strings-only 收集后 reset 完整收集：global 子树中 session 对象属性里的串
/// 跨两次收集存活（strings-only 残留 mark 位不得短路完整收集的 mark DFS）。
#[test]
fn strings_only_then_reset_keeps_global_subtree_strings() {
    let mut vm = vm_with_threshold(4096);
    let first =
        compile("globalThis.kept = { s: 'he' + 'llo' }; var t; for (var i = 0; i < 500000; i++) { t = 'x' + i; } 0");
    vm.run(&first).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");
    vm.reset();

    let second = compile("globalThis.kept.s");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept.s 应为字符串").to_string();
    assert_eq!(text, "hello");
}

/// 低阈值执行期字符串 GC 下 builtin 构造期局部串跨分配点安全（触发点
/// 在指令边界，native 调用内不回收）：RegExp exec 写入构造中数组的匹配串/
/// 捕获组串不被后续分配回收，返回数组内容正确。
#[test]
fn regexp_exec_strings_survive_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var text = 'a'.repeat(500) + 'b'.repeat(500); var m = /(a+)(b+)/.exec(text); \
         m[0] + '|' + m[1] + '|' + m[2]",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("exec 结果应为字符串").to_string();
    let expected = format!("{}|{}|{}", "a".repeat(500) + &"b".repeat(500), "a".repeat(500), "b".repeat(500));
    assert_eq!(text, expected);
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// 低阈值下 Temporal.ZonedDateTime 构造的局部串（timeZone/calendar 槽）跨
/// 分配安全：对象槽不因执行期回收而悬垂，getter 返回原内容。
#[test]
fn temporal_zoned_date_time_strings_survive_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         var z = new Temporal.ZonedDateTime(0n, 'UTC'); z.timeZoneId + '|' + z.calendarId",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("ZDT 槽应为字符串").to_string();
    assert_eq!(text, "UTC|iso8601");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// 低阈值下 replace 函数 replacer 的回调参数 Vec（局部逐项分配）跨分配安全：
/// 回调内拼接正确，说明参数串未被执行期回收释放。
#[test]
fn replace_replacer_args_survive_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         'a-b-c'.replace(/(a)-(b)-(c)/g, function(m, p1, p2, p3, pos, s) { return p1 + p2 + p3; })",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("replace 结果应为字符串").to_string();
    assert_eq!(text, "abc");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// 嵌套 dispatch（map 回调经 call_function_sync → 内联字节码执行）内分配超水位时，
/// 调用方寄存器窗口（`save_inline_state` 存入 inline 状态，非 GC 根）持的 session
/// 串不得被执行期回收：回调期间触发收集会把它当死串释放，回拷后成悬垂。
/// `held` 经 middle 参数位于低号寄存器，恰落在回调写寄存器区（0..n_registers），
/// 修复前回调内触发收集即把它释放并复用（结果变垃圾串），修复后回调期间不回收。
#[test]
fn map_callback_caller_reg_strings_survive_nested_dispatch() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         function middle(held) { \
           var r = [1, 2, 3].map(function(x) { var t; for (var j = 0; j < 300; j++) { t = 'z' + j; } return x * 2; }); \
           return held; } \
         var out = middle('mid' + 'str'); out",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("held 应为字符串").to_string();
    assert_eq!(text, "midstr");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// 函数 replacer 回调同样重入嵌套 dispatch：调用方 regs 持串跨回调存活，
/// 回调体内分配不回收调用方窗口中的串。
#[test]
fn replace_replacer_caller_reg_strings_survive_nested_dispatch() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         function middle(held) { \
           var out = 'a-b-c'.replace(/-/g, function(m, pos, str) { var t; for (var j = 0; j < 300; j++) { t = 'y' + j; } return '[' + m + ']'; }); \
           return held + '|' + out; } \
         var out = middle('repl' + 'acer'); out",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串").to_string();
    assert_eq!(text, "replacer|a[-]b[-]c");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// generator.next() 经生成器恢复重入嵌套 dispatch：调用方 regs（参数串与累加器）
/// 在生成器体执行期间不得被回收。修复前该场景直接段错误（悬垂指针解引用）。
#[test]
fn generator_next_loop_caller_reg_strings_survive_nested_dispatch() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         function middle(held) { \
           function* gen() { for (var i = 0; i < 5; i++) { var t; for (var j = 0; j < 200; j++) { t = 'g' + j; } yield i; } } \
           var it = gen(); var acc = 0; for (var i = 0; i < 5; i++) { acc += it.next().value; } \
           return held + '|' + acc; } \
         var out = middle('gen' + 'str'); out",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串").to_string();
    assert_eq!(text, "genstr|10");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// 嵌套回调组合（map 回调内再调 map）：两层内联 dispatch 叠加，任一层指令边界
/// 触发收集都不回收调用方窗口中的串。
#[test]
fn nested_inline_callbacks_caller_reg_strings_survive_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         function middle(held) { \
           var r = [1, 2].map(function(x) { \
             var inner = [10, 20].map(function(y) { var t; for (var j = 0; j < 200; j++) { t = 'n' + j; } return y; }); \
             return inner[0] + inner[1] + x; }); \
           return held + '|' + r.join(','); } \
         var out = middle('nest' + 'ed'); out",
    );
    let result = vm.run(&module).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串").to_string();
    assert_eq!(text, "nested|31,32");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}

/// full_reset 清空 session 内存：执行期回收释放过的死串不与完全重置的
/// 整体释放路径重复释放（无双重释放崩溃），重置后引擎可继续运行。
#[test]
fn full_reset_clears_session_after_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("var s; for (var i = 0; i < 200000; i++) { s = 'a' + i; } s.length");
    vm.run(&first).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");

    vm.full_reset();

    assert_eq!(vm.session_bytes_allocated(), 0, "full_reset 后 session 字节应为 0");
    let second = compile("'ok' + '!'");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("拼接结果应为字符串").to_string();
    assert_eq!(text, "ok!");
}

/// full_reset 统一释放执行期分配过的 upvalue cell 无 double-free：重置后引擎
/// 可继续运行并重建新闭包（cell 独立堆分配，随 full_reset 恰好释放一次）。
#[test]
fn closure_cells_freed_by_full_reset_without_double_free() {
    let mut vm = vm_with_threshold(1);
    let first = compile("var x = 1; function f() { return x; } globalThis.f = f; f()");
    let result = vm.run(&first).expect("run1");
    assert_eq!(format!("{}", result), "1");

    vm.full_reset();

    let second = compile("'ok' + '!'");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("拼接结果应为字符串").to_string();
    assert_eq!(text, "ok!");
}
