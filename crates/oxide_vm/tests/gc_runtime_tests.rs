use std::sync::Arc;

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
    let result = vm.run(&Arc::new(module)).expect("run");

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
    vm.run(&Arc::new(first)).expect("run1");

    let second = compile("var s; for (var i = 0; i < 500000; i++) { s = 'y' + i; } globalThis.kept");
    let result = vm.run(&Arc::new(second)).expect("run2");
    let text = vm.lookup_str(result).expect("kept 应为字符串").to_string();

    assert_eq!(text, "hello");
    assert!(vm.session_gc_stats().total_collections > 0, "run2 应触发执行期字符串 GC");
}

/// reset（轻量重置）保留 session 字符串：跨 eval 存活语义不被执行期 GC 破坏。
#[test]
fn reset_preserves_session_strings_after_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("globalThis.kept = 'x' + 'y'; 0");
    vm.run(&Arc::new(first)).expect("run1");
    vm.reset();

    let second = compile("globalThis.kept");
    let result = vm.run(&Arc::new(second)).expect("run2");
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
    vm.run(&Arc::new(first)).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");
    vm.reset();

    let second = compile("globalThis.kept.s");
    let result = vm.run(&Arc::new(second)).expect("run2");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    let result = vm.run(&Arc::new(module)).expect("run");
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
    vm.run(&Arc::new(first)).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");

    vm.full_reset();

    assert_eq!(vm.session_bytes_allocated(), 0, "full_reset 后 session 字节应为 0");
    let second = compile("'ok' + '!'");
    let result = vm.run(&Arc::new(second)).expect("run2");
    let text = vm.lookup_str(result).expect("拼接结果应为字符串").to_string();
    assert_eq!(text, "ok!");
}

/// 低阈值下 reset 触发完整对象 sweep（移动式搬移 + 重标）：写全局的函数对象
/// 不被当死对象释放，run2 直接调用该闭包——upvalue 读回创建期值，子模块按
/// 创建期表代际解析。
#[test]
fn reset_sweep_preserves_global_function_object() {
    let mut vm = vm_with_threshold(1024);
    let first = compile(
        "function make(){ var o = {v: 9}; globalThis.fn = function(){ return o.v; }; } make(); \
         for (var i = 0; i < 2000; i++) { var t = 'p' + i + 'q' + i; } \
         typeof globalThis.fn === 'function'",
    );
    let result = vm.run(&Arc::new(first)).expect("run1");
    assert!(result.is_bool() && result.as_bool());
    assert!(vm.session_gc_stats().total_collections > 0, "低阈值应触发执行期收集");
    vm.reset();

    let second = compile("globalThis.fn() === 9");
    let result = vm.run(&Arc::new(second)).expect("run2");
    assert!(result.is_bool() && result.as_bool());
}

/// full_reset 统一释放执行期分配过的 upvalue cell 无 double-free：重置后引擎
/// 可继续运行并重建新闭包（cell 独立堆分配，随 full_reset 恰好释放一次）。
#[test]
fn closure_cells_freed_by_full_reset_without_double_free() {
    let mut vm = vm_with_threshold(1);
    let first = compile("var x = 1; function f() { return x; } globalThis.f = f; f()");
    let result = vm.run(&Arc::new(first)).expect("run1");
    assert_eq!(format!("{}", result), "1");

    vm.full_reset();

    let second = compile("'ok' + '!'");
    let result = vm.run(&Arc::new(second)).expect("run2");
    let text = vm.lookup_str(result).expect("拼接结果应为字符串").to_string();
    assert_eq!(text, "ok!");
}

/// 死闭包的 upvalue 列表在对象 sweep 死分支释放（死对象不克隆、Box 无其他
/// 持有者，恰好一次）后，收尾统一释放见已置空字段不双放：full_reset 后引擎
/// 可继续运行并重建新闭包；存活闭包走存活分支（克隆与原件共享 Box），
/// 不受死分支影响。
#[test]
fn dead_closure_upvalues_freed_by_sweep_without_double_free() {
    let mut vm = vm_with_threshold(4096);
    // run1：两个 3-upvalue 闭包逃逸 global（成 session 对象），同 run 内调用
    // 存活闭包确认 upvalue 语义完好。捕获变量置于函数作用域：顶层 var 按全局
    // 对象属性单一真值读取，不产生 cell 捕获。
    let first = compile(
        "(function(){var a = 1; var b = 2; var c = 3; \
         globalThis.dead = function() { return a + b + c; }; \
         globalThis.live = function() { return a + b + c; }; return globalThis.live();})()",
    );
    let result = vm.run(&Arc::new(first)).expect("run1");
    assert_eq!(format!("{}", result), "6");

    // run2：撤销死闭包的根引用（不可达）。
    let second = compile("globalThis.dead = undefined; 0");
    vm.run(&Arc::new(second)).expect("run2");

    // 完整收集：死闭包走死分支（释放 upvalue 列表），存活闭包走存活分支
    // （克隆与原件共享 Box，不在死分支释放）。
    vm.collect_session_gc();
    assert!(vm.session_gc_stats().last_collection_objects_dead >= 1, "死闭包应经 sweep 死分支");

    // 跨 run 存活核：sweep 后存活闭包克隆仍挂在 global 上，直接调用读回
    // upvalue 和（子模块按创建期表代际解析）。
    let third = compile("globalThis.live() === 6");
    let result = vm.run(&Arc::new(third)).expect("run3");
    assert!(result.is_bool() && result.as_bool());

    // 收尾统一 upvalue 释放与死分支不双放（字段已置空、死对象已出表）。
    vm.full_reset();

    // 重置后引擎健康：重建新 3-upvalue 闭包，同 run 调用语义正确。
    let fourth = compile(
        "(function(){var x = 10; var y = 20; var z = 30; \
         globalThis.f = function() { return x + y + z; }; return globalThis.f();})()",
    );
    let result = vm.run(&Arc::new(fourth)).expect("run4");
    assert_eq!(format!("{}", result), "60");

    // 字节账目 A/B 差分样本：两条同构 arrow 仅 upvalue 数不同（3 vs 0），
    // 各走独立 VM 的「创建 → 撤根 → 完整收集」。两次收集的字节账目差即死
    // arrow 的 upvalue 列表 Box 字节数——对象本体、length/name 属性区、函数
    // 名 session 串在 A/B 恒等，差分相消（各 1 个死对象 + 1 条死串）。
    // 捕获变量置于函数作用域（顶层 var 按全局对象属性读取，不产生 cell
    // 捕获）。死分支不释放 upvalue 列表（泄漏）时差分为 0，本断言转红。
    let collect_freed = |src: &str| -> u64 {
        let mut v = vm_with_threshold(4096);
        v.run(&Arc::new(compile(src))).expect("run create");
        v.run(&Arc::new(compile("globalThis.arrow = undefined; 0")))
            .expect("run unroot");
        v.collect_session_gc();
        let stats = v.session_gc_stats();
        assert!(stats.last_collection_objects_dead >= 1, "arrow 闭包应经 sweep 死分支");
        stats.last_collection_bytes_freed
    };
    let freed_with_captures = collect_freed(
        "(function(){var a = 1; var b = 2; var c = 3; \
         globalThis.arrow = () => a + b + c; return 0;})()",
    );
    let freed_no_captures = collect_freed("globalThis.arrow = () => 7; 0");
    let upvalue_box_min = (std::mem::size_of::<Vec<*mut oxide_types::object::Cell>>()
        + 3 * std::mem::size_of::<*mut oxide_types::object::Cell>()) as u64;
    assert!(
        freed_with_captures >= freed_no_captures + upvalue_box_min,
        "两次收集的字节账目差应为死闭包 upvalue 列表 Box（≥{upvalue_box_min} B），实际差 {}",
        freed_with_captures - freed_no_captures
    );
}

/// run 边界回收无引用的表代际：不产生存活函数对象的 run 不使注册表条目数
/// 随 run 数单调增；每 run 逃逸的函数对象钉住其创建期代际，条目数相应增长。
#[test]
fn unreferenced_table_gens_do_not_grow_with_runs() {
    let mut vm = Vm::new();
    for _ in 0..12 {
        vm.run(&Arc::new(compile("0"))).expect("run");
    }
    let baseline = vm.table_gen_count();
    assert!(baseline <= 2, "无存活函数对象跨 run，注册表条目应有界（≤2），实际 {baseline}");
    for i in 0..5 {
        let src = format!("globalThis.keep{i} = function() {{ return {i}; }}; 0");
        vm.run(&Arc::new(compile(&src))).expect("run");
    }
    let pinned = vm.table_gen_count();
    assert!(pinned > baseline, "每 run 函数对象钉住创建期代际，条目数应增长，实际 {pinned}");
}

/// shift 首位 hole 落原型链 getter：getter 临时对象经结果寄存器钉为 GC 根，
/// 跨循环内 setter 分配窗口与低阈值执行期收集存活，返回身份保持。
#[test]
fn shift_getter_temporary_object_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         var box = null; var a = [, 2, 3]; \
         Object.defineProperty(Array.prototype, '0', { \
           get: function() { box = { m: 1 }; return box; }, \
           set: function(v) { \
             Object.defineProperty(this, 0, { value: v, writable: true, enumerable: true, configurable: true }); \
             var t = 'z'; for (var j = 0; j < 20; j++) { t = t + t; } } }); \
         var r = a.shift(); (r === box) + ':' + r.m + ':' + a.length + ':' + a[0] + ':' + a[1]",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串");
    assert_eq!(text, "true:1:2:2:3");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发收集");
}

/// reverse 双存在臂下值跨上端 Get 窗口：getter 临时对象经结果寄存器钉为 GC 根，
/// 跨低阈值执行期收集存活，落位身份保持。
#[test]
fn reverse_getter_temporary_object_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
         var box = null; var a = new Array(2); \
         Object.defineProperty(Array.prototype, '0', { \
           get: function() { box = { m: 1 }; return box; }, \
           set: function(v) { Object.defineProperty(this, 0, { value: v, writable: true, enumerable: true, configurable: true }); } }); \
         Object.defineProperty(Array.prototype, '1', { \
           get: function() { var t = 'z'; for (var j = 0; j < 20; j++) { t = t + t; } return 'up'; }, \
           set: function(v) { Object.defineProperty(this, 1, { value: v, writable: true, enumerable: true, configurable: true }); } }); \
         a.reverse(); \
         (a[1] === box) + ':' + a[0] + ':' + a.length + ':' + a[1].m",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串");
    assert_eq!(text, "true:up:2:1");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发收集");
}

// ── 原生盒内字符串/BigInt 边存活（mark 边收集去对象预过滤） ─────────────────

/// 盒持唯一引用 session 串（Promise 结算值）：churn 窗口内执行期收集
/// 仅能经状态盒边闭合存活，结算交付后读回精确值。churn 串与盒串同长：
/// 若 box 释放后其内存被同长分配复用，读回即错值。
#[test]
fn promise_result_string_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let first = compile(
        "(function(){ \
         var s = 'prombox'.repeat(8); \
         var p = Promise.resolve(s); \
         p.then(function(v){ globalThis.got = v; }); \
         globalThis.p = p; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'z'.repeat(56); } \
         0",
    );
    vm.run(&Arc::new(first)).expect("run1");

    let second = compile("globalThis.got");
    let result = vm.run(&Arc::new(second)).expect("run2");
    let text = vm.lookup_str(result).expect("got 应为字符串").to_string();

    assert_eq!(text, "prombox".repeat(8));
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期字符串 GC");
}

/// 盒持唯一引用 session BigInt（Promise 结算值，运行时乘积非常量池字面量）：
/// 同窗口存活闭合经 live_bigints，结算交付后读回精确值。
#[test]
fn promise_result_bigint_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let first = compile(
        "(function(){ \
         var b = 987654321n * 123456789n; \
         var p = Promise.resolve(b); \
         p.then(function(v){ globalThis.got = v; }); \
         globalThis.p = p; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'churn' + i; } \
         0",
    );
    vm.run(&Arc::new(first)).expect("run1");

    let second = compile("globalThis.got");
    let result = vm.run(&Arc::new(second)).expect("run2");

    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    assert!(result.is_bigint(), "got 应为 BigInt");
    assert_eq!(vm.bigint_value(result), &expected);
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期 GC");
}

/// 挂起生成器帧寄存器持唯一引用 session BigInt：yield 挂起后仅状态盒
/// 可达，churn 窗口收集闭合存活，恢复读回精确值。
#[test]
fn suspended_generator_bigint_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "(function(){ \
         function* gen() { var b = 987654321n * 123456789n; yield 1; return b; } \
         var g = gen(); g.next(); globalThis.g = g; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'churn' + i; } \
         globalThis.g.next().value",
    );
    let result = vm.run(&Arc::new(module)).expect("run");

    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    assert!(result.is_bigint(), "恢复返回值应为 BigInt");
    assert_eq!(vm.bigint_value(result), &expected);
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期 GC");
}

/// 挂起异步函数帧持唯一引用 session BigInt：await 挂起后仅状态盒可达，
/// 微任务恢复结算后读回精确值。
#[test]
fn suspended_async_function_bigint_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let first = compile(
        "(function(){ \
         async function f() { var b = 987654321n * 123456789n; await 0; return b; } \
         var p = f(); \
         p.then(function(v){ globalThis.got = v; }); \
         globalThis.p = p; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'churn' + i; } \
         0",
    );
    vm.run(&Arc::new(first)).expect("run1");

    let second = compile("globalThis.got");
    let result = vm.run(&Arc::new(second)).expect("run2");

    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    assert!(result.is_bigint(), "got 应为 BigInt");
    assert_eq!(vm.bigint_value(result), &expected);
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期 GC");
}

/// 挂起异步生成器帧持唯一引用 session BigInt：首次 next 挂起后仅状态盒
/// 可达，挂起期 churn 窗口的收集闭合存活，同 run 内恢复并交付精确值。
#[test]
fn suspended_async_generator_bigint_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let first = compile(
        "async function* gen() { var b = 987654321n * 123456789n; yield 1; return b; } \
         async function run() { \
           var it = gen(); globalThis.g = it; \
           var p1 = it.next(); \
           for (var i = 0; i < 2000; i++) { var t = 'churn' + i; } \
           await p1; \
           return (await it.next()).value; } \
         run().then(function(v){ globalThis.got = v; })",
    );
    vm.run(&Arc::new(first)).expect("run1");

    let second = compile("globalThis.got");
    let result = vm.run(&Arc::new(second)).expect("run2");

    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    assert!(result.is_bigint(), "got 应为 BigInt");
    assert_eq!(vm.bigint_value(result), &expected);
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期 GC");
}

/// 盒持唯一引用 session 串（Map 值）：churn 窗口内收集经盒边闭合存活，
/// get 读回精确值。churn 串与盒串同长：若 box 释放后其内存被同长
/// 分配复用，读回即错值。
#[test]
fn map_value_string_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "(function(){ \
         var s = 'mapbox'.repeat(8); \
         var m = new Map(); m.set('k', s); globalThis.m = m; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'z'.repeat(48); } \
         globalThis.m.get('k')",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("map 值应为字符串").to_string();

    assert_eq!(text, "mapbox".repeat(8));
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期字符串 GC");
}

/// 盒持唯一引用 session 串（Set 元素）：churn 窗口内收集经盒边闭合存活，
/// values 迭代读回精确值。churn 串与盒串同长：若 box 释放后其内存被
/// 同长分配复用，读回即错值。
#[test]
fn set_value_string_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "(function(){ \
         var s = 'setbox'.repeat(8); \
         var st = new Set(); st.add(s); globalThis.st = st; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'z'.repeat(48); } \
         globalThis.st.values().next().value",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("set 元素应为字符串").to_string();

    assert_eq!(text, "setbox".repeat(8));
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期字符串 GC");
}

/// 盒持唯一引用 session 串（DisposableStack adopt 值）：churn 窗口内收集
/// 经条目边闭合存活，dispose 回调交付后读回精确值。churn 串与盒串同长：
/// 若 box 释放后其内存被同长分配复用，读回即错值。
#[test]
fn disposable_stack_value_string_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "(function(){ \
         var s = 'stackbox'.repeat(8); \
         var st = new DisposableStack(); \
         st.adopt(s, function(v){ globalThis.got = v; }); \
         globalThis.st = st; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'z'.repeat(64); } \
         globalThis.st.dispose(); \
         globalThis.got",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("adopt 值应为字符串").to_string();

    assert_eq!(text, "stackbox".repeat(8));
    assert!(vm.session_gc_stats().total_collections > 0, "churn 应触发执行期字符串 GC");
}

/// 低阈值下基元 this 的装箱体（Number 包装对象）跨执行期 GC 完整存活：
/// reverse/fill/copyWithin/splice 把基元 this 装箱后跨 length getter 用户窗口
/// 使用，装箱体身份（instanceof Number + 被包值）在收集后保持完整（装箱体
/// 此前仅存于 Rust 局部，不在根集）。
#[test]
fn arraylike_boxed_this_identity_survives_runtime_gc() {
    let mut vm = vm_with_threshold(512);
    let module = compile(
        "var g = {}; for (var i = 0; i < 20; i++) { g['k' + i] = 'v'.repeat(64); } \
          var savedLen = 2; \
          Object.defineProperty(Number.prototype, 'length', { \
            configurable: true, \
            get: function () { g['y' + Math.random()] = 'w'.repeat(32); return savedLen; }, \
            set: function (v) { savedLen = v; } }); \
          var r1 = Array.prototype.reverse.call(5); \
          var r2 = Array.prototype.fill.call(5, 'v', 0, 2); \
          var r3 = Array.prototype.copyWithin.call(5, 0, 0, 1); \
          var r4 = Array.prototype.splice.call(5, 0); \
          delete Number.prototype.length; \
          [r1 instanceof Number, r1.valueOf(), r2 instanceof Number, r2.valueOf(), \
           r3 instanceof Number, r3.valueOf(), Array.isArray(r4), r4.length].join('|')",
    );
    let result = vm.run(&Arc::new(module)).expect("run");
    let text = vm.lookup_str(result).expect("结果应为字符串").to_string();

    assert_eq!(text, "true|5|true|5|true|5|true|2");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
}
