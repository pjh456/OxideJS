//! matchAll 已编译正则载体的生命周期钉：字符串模式 / 非 global 正则两条建载体
//! 路径的 `native_fn` 槽持有 `Box<regress::Regex>`，死载体须经 sweep 释放
//! （释放字节计入收集统计）、存活载体跨完整收集深拷贝后迭代器仍正确推进；
//! 载体无 RegExp 可见属性面（toString 保持普通对象标签）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;
use regress::Regex;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

const ROUNDS: u64 = 256;

/// 字符串模式路径：N 个死载体（包装器 + 载体）经完整收集走 sweep 死分支，
/// 释放字节必须含 N 份编译正则——守卫未命中时该部分恒不计入。
#[test]
fn string_pattern_match_all_boxes_freed_by_sweep() {
    let mut vm = Vm::new();
    eval(&mut vm, "for (let i = 0; i < 256; i++) { 'aaaa'.matchAll('a'); }").unwrap();
    let freed_before = vm.session_gc_stats().total_bytes_freed;
    vm.reset();
    let freed_delta = vm.session_gc_stats().total_bytes_freed - freed_before;
    let min_regex_bytes = ROUNDS * std::mem::size_of::<Regex>() as u64;
    assert!(
        freed_delta >= min_regex_bytes,
        "sweep 释放 {freed_delta} 字节，应含 {ROUNDS} 份编译正则（下界 {min_regex_bytes}）"
    );
}

/// 非 global 正则路径：`RegExp.prototype.matchAll` 复制补 g 建载体，同口径断言。
#[test]
fn non_global_regexp_match_all_boxes_freed_by_sweep() {
    let mut vm = Vm::new();
    // 真则常驻（存活根），循环内只产死载体：释放下界唯一来源是载体 Box。
    eval(&mut vm, "var r = /ab/; for (let i = 0; i < 256; i++) { r[Symbol.matchAll]('abab'); }").unwrap();
    let freed_before = vm.session_gc_stats().total_bytes_freed;
    vm.reset();
    let freed_delta = vm.session_gc_stats().total_bytes_freed - freed_before;
    let min_regex_bytes = ROUNDS * std::mem::size_of::<Regex>() as u64;
    assert!(
        freed_delta >= min_regex_bytes,
        "sweep 释放 {freed_delta} 字节，应含 {ROUNDS} 份编译正则（下界 {min_regex_bytes}）"
    );
}

/// 存活载体跨完整收集：克隆臂深拷贝 Box 后迭代器仍按原语义耗尽。
#[test]
fn string_pattern_match_all_iterator_survives_full_collect() {
    let mut vm = Vm::new();
    eval(&mut vm, "var it = 'abab'.matchAll('ab');").unwrap();
    vm.reset();
    let result = eval(
        &mut vm,
        "var out = []; var st = it.next(); while (!st.done) { out.push(String(st.value[0])); st = it.next(); } out.join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "ab,ab");
}

/// 非 global 正则路径的存活载体同样跨收集保持有效。
#[test]
fn non_global_regexp_match_all_iterator_survives_full_collect() {
    let mut vm = Vm::new();
    eval(&mut vm, "var r = /ab/; var it = r[Symbol.matchAll]('abab');").unwrap();
    vm.reset();
    let result = eval(
        &mut vm,
        "var out = []; var st = it.next(); while (!st.done) { out.push(String(st.value[0])); st = it.next(); } out.join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "ab,ab");
}

/// 载体经内部属性可达，但无 RegExp 属性面：toString 保持普通对象标签。
#[test]
fn match_all_carrier_has_no_regexp_surface() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.prototype.toString.call('a'.matchAll('a').__mal_re__)").unwrap();
    assert_eq!(to_str(&vm, result), "[object Object]");
}
