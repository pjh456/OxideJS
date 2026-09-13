//! 并行负载下内存面腐化的稳定判别探针（执行期两档收集全路径覆盖）。
//!
//! 每个测试独享 `Vm`（低 session GC 阈值强制单 run 内反复触发执行期
//! epoch 晋升 + session 原地 sweep），JS 负载覆盖：对象元素数组读回、
//! 原生盒（Set/Map）对象边、Promise 结算值、挂起帧（生成器/异步）
//! 持串/BigInt 边跨收集边界恢复；断言读回值的 tag 位与载荷，
//! 悬垂读命中复用块即红。
//!
//! 盒键只用对象键：字符串键的 SameValueZero 值等值在 SetKey 位比较下
//! 未闭合（动态构造串键按指针判异），属独立语义缺陷，不在此面覆盖。
//!
//! 运行纪律：与负载子集二进制同批 `--test-threads` 梯度并行，
//! `MALLOC_PERTURB_` 放大已释放块扰动；先串行基线确认扰动不引入新红。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_types::value::JsValue;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

fn vm_with_threshold(bytes: usize) -> Vm {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(bytes);
    Vm::with_kernel_core(KernelCore::new(config))
}

/// 执行源码并格式化完成值：串取内容、Promise 取 drain 后结算值。
fn eval_src(vm: &mut Vm, source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let result = vm.run(&Arc::new(module)).expect("run");
    format_value(vm, result)
}

fn format_value(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        vm.lookup_str(val).expect("串值").to_string()
    } else if val.is_object() {
        // SAFETY: 完成值对象由 VM 自有，本 session 内有效。
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            match promise_settled_value(obj) {
                Some((true, v)) => format_value(vm, v),
                Some((false, v)) => format!("rejected:{}", format_value(vm, v)),
                None => "<pending>".to_string(),
            }
        } else {
            format!("{val:?}")
        }
    } else {
        format!("{val:?}")
    }
}

/// 对象元素数组跨执行期收集读回：元素经 epoch 晋升 session，churn 制造
/// 死 session 对象供原地 sweep；替换后元素值与串载荷逐一比对。
#[test]
fn inrun_array_element_readback_after_churn() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "var a = []; \
         for (var i = 0; i < 120; i++) { a.push({ v: i, s: 'p' + i }); } \
         var t; \
         for (var i = 0; i < 30000; i++) { t = { c: i, s: 'churn' + i }; a[i % 120] = { v: i % 120, s: 'q' + (i % 120) }; } \
         var bad = -1; \
         for (var i = 0; i < 120; i++) { \
           if (a[i].v !== i || a[i].s !== 'q' + i) { bad = i; break; } \
         } \
         bad < 0 ? 'ok' : 'bad:' + bad",
    );
    assert_eq!(out, "ok", "元素读回应全对");
}

/// 挂起生成器跨执行期收集恢复：挂起帧寄存器与状态盒边在晋升/sweep 后
/// 仍指向存活值，恢复迭代序列与数组读回均正确。
#[test]
fn inrun_suspended_generator_resumes_after_collect() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "var a = []; \
         for (var i = 0; i < 80; i++) { a.push({ v: i }); } \
         function* g() { yield a[0].v; yield a[79].v; } \
         var it = g(); \
         var first = it.next().value; \
         var t; \
         for (var i = 0; i < 30000; i++) { t = { c: i }; } \
         var second = it.next().value; \
         var bad = (first !== 0 || second !== 79) ? 'gen' : ''; \
         for (var i = 0; i < 80; i++) { if (a[i].v !== i) { bad = 'arr' + i; break; } } \
         bad || 'ok'",
    );
    assert_eq!(out, "ok", "挂起恢复与数组读回应全对");
}

/// 挂起帧持长串与 BigInt 边跨执行期收集：帧内边并入统一 mark 分发，
/// 恢复后结算值读回（悬垂串/BigInt 即错值）。
#[test]
fn inrun_suspended_frame_string_bigint_edges() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "function* g(x, y) { \
           var t; \
           for (var i = 0; i < 30000; i++) { t = { c: i, s: 'z' + i }; } \
           yield String(x) + y; \
         } \
         var it = g('a'.repeat(64), 10n); \
         it.next().value",
    );
    assert_eq!(out, format!("{}10", "a".repeat(64)), "挂起帧串/BigInt 边读回");
}

/// 异步函数挂起帧持 BigInt 跨执行期收集恢复：结算值经微任务 drain 读回。
#[test]
fn inrun_async_suspended_bigint_readback() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "async function f(x) { await 0; return x + 1n; } \
         var t; \
         for (var i = 0; i < 30000; i++) { t = { c: i, s: 'churn' + i }; } \
         f(41n).then(function (v) { return String(v); })",
    );
    assert_eq!(out, "42", "BigInt 结算值读回");
}

/// Set/Map 盒持对象键跨执行期收集：键存活、读回值与 size 正确
/// （盒边在 mark 侧并入统一分发，悬垂即错值/崩溃）。
#[test]
fn inrun_native_box_edges_survive_collect() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "var s = new Set(); var m = new Map(); var keys = []; \
         for (var i = 0; i < 60; i++) { var k = { id: i }; keys.push(k); s.add(k); m.set(k, i); } \
         var t; \
         for (var i = 0; i < 30000; i++) { t = { c: i, s: 'churn' + i }; } \
         var bad = -1; \
         for (var i = 0; i < 60; i++) { \
           var v = m.get(keys[i]); \
           if (v !== i || !s.has(keys[i])) { bad = i; break; } \
         } \
         if (bad < 0 && s.size !== 60) bad = 1000; \
         bad < 0 ? 'ok' : 'bad:' + bad",
    );
    assert_eq!(out, "ok", "盒键读回应全对");
}

/// 大面积 churn 制造死 session 对象（原地 sweep 释放），存活根集
/// （global 子树 + 挂起生成器）读回不腐化。
#[test]
fn inrun_dead_session_sweep_keeps_live_roots() {
    let mut vm = vm_with_threshold(4096);
    let out = eval_src(
        &mut vm,
        "globalThis.holder = []; \
         for (var i = 0; i < 100; i++) { globalThis.holder.push({ v: i }); } \
         function* g() { for (var i = 0; i < 10; i++) { yield i; } } \
         var it = g(); it.next(); \
         for (var round = 0; round < 6; round++) { \
           var next = []; \
           for (var i = 0; i < 100; i++) { next.push({ v: i + round }); } \
           globalThis.holder = next; \
         } \
         var bad = -1; \
         for (var i = 0; i < 100; i++) { if (globalThis.holder[i].v !== i + 5) { bad = i; break; } } \
         bad < 0 ? 'ok:' + it.next().value : 'bad:' + bad",
    );
    assert_eq!(out, "ok:1", "holder 读回全对且迭代器恢复");
}

/// Set 串值经 values() 迭代器读回跨执行期收集：迭代器包装对象已登记
/// epoch 对象表，晋升后不死；读回串载荷即验证整条边链。
#[test]
fn inrun_set_string_value_survives_collect() {
    let mut vm = vm_with_threshold(512);
    let out = eval_src(
        &mut vm,
        "(function(){ \
         var s = 'setbox'.repeat(8); \
         var st = new Set(); st.add(s); globalThis.st = st; })(); \
         for (var i = 0; i < 2000; i++) { var t = 'z'.repeat(48); } \
         globalThis.st.values().next().value",
    );
    assert_eq!(out, "setbox".repeat(8));
}
