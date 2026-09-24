use std::sync::Arc;

use oxide_builtins::map::{map_clear, map_constructor as new_map, map_delete, map_get, map_has, map_set, map_size};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::VmHost;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn str_val(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

/// 分配一个原型指向 Map.prototype 的占位对象并写入 reg 0，作为构造器调用的 `this`。
fn map_this(vm: &mut Vm) -> JsValue {
    let proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
    // 统一入口分配：epoch 置位与对象表登记一步闭合，执行期晋升收集按表取活。
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
    let val = JsValue::from_js_object(obj);
    vm.set_reg(0, val);
    val
}

// -- direct native fn tests --

#[test]
fn tmap_constructor_returns_object() {
    let mut vm = Vm::new();
    map_this(&mut vm);
    let r = new_map(&mut vm, &[0]).unwrap();
    assert!(r.is_object());
}

#[test]
fn tmap_set_get() {
    let mut vm = Vm::new();
    map_this(&mut vm);
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(42.0));
    vm.set_reg(2, JsValue::float(100.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert_eq!(map_get(&mut vm, &[0, 1]).unwrap().as_double(), 100.0);
    assert!(map_get(&mut vm, &[0, 99]).unwrap().is_undefined());
}

#[test]
fn tmap_has_and_delete() {
    let mut vm = Vm::new();
    map_this(&mut vm);
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(7.0));
    vm.set_reg(2, JsValue::float(8.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert!(map_has(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(map_delete(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(!map_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tmap_size_and_clear() {
    let mut vm = Vm::new();
    map_this(&mut vm);
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
    vm.set_reg(1, JsValue::float(1.0));
    vm.set_reg(2, JsValue::float(10.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    vm.set_reg(1, JsValue::float(2.0));
    vm.set_reg(2, JsValue::float(20.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 2.0);
    map_clear(&mut vm, &[0]).unwrap();
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
}

// ── 经 JS 使用 new 关键字的 eval 测试（每个测试一次 eval）──

#[test]
fn map_new_set_get() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('k', 'v'); m.get('k')").unwrap();
    assert_eq!(str_val(&vm, r), "v");
}

#[test]
fn map_new_get_missing() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.get('nope')").unwrap();
    assert!(r.is_undefined());
}

#[test]
fn map_new_has() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('a', 1); m.has('a')").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "var m2 = new Map(); m2.has('b')").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn map_new_delete() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('x', 10); m.delete('x'); m.has('x')").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn map_new_object_entry_pair_reads_key_and_value() {
    // 对象 entry（{0:'k',1:'v'}）的数字键是整数键：读取须经 ToPropertyKey 规范化。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map([{0:'k',1:'v'}]); m.get('k')").unwrap();
    assert_eq!(str_val(&vm, r), "v");
}

#[test]
fn map_new_mixed_array_and_object_entries() {
    // 数组 entry 与对象 entry 两种形态混合可共存。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map([['a',1],{0:'b',1:2}]); m.get('a') + '/' + m.get('b')").unwrap();
    assert_eq!(str_val(&vm, r), "1/2");
}

// ── SameValueZero 键语义：运行期构造键（非常量池指针）与同值字面量互为同键 ──

#[test]
fn map_runtime_string_key_get_by_literal() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set(String.fromCharCode(120), 1); m.get('x')").unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn map_same_content_string_keys_merge() {
    // 同内容键 set 第二次是覆盖而非新增：size 保持 1、值更新。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('x', 1); m.set(String.fromCharCode(120), 2); m.size + ':' + m.get('x')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1:2");
}

#[test]
fn map_string_key_delete_by_runtime_key() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('x', 1); m.delete(String.fromCharCode(120)); m.has('x')",
    )
    .unwrap();
    assert!(!r.as_bool());
}

#[test]
fn map_bigint_key_same_value_different_boxes() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set(10n, 'v'); m.get(BigInt(10)) + '/' + m.has(11n)").unwrap();
    assert_eq!(str_val(&vm, r), "v/false");
}

#[test]
fn map_nan_and_zero_keys() {
    // NaN 同 NaN 键、0/-0 同键：两键共存，has 双向命中。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set(NaN, 'n'); m.set(-0, 'z'); m.has(NaN) && m.has(0) && m.size === 2",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn map_object_key_identity() {
    // 对象键按引用相等：同对象命中，等值异对象 miss（get 回 undefined）。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var o = {}; var m = new Map(); m.set(o, 1); m.get(o) === 1 && m.get({}) === undefined",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn map_many_runtime_string_keys() {
    // 键量越过小表线性探测阈值后查找走哈希索引，哈希与值等值的一致性在此兑现。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); \
         for (var i = 0; i < 200; i++) { m.set('k' + i, i); } \
         var bad = -1; \
         for (var i = 0; i < 200; i++) { if (m.get(String.fromCharCode(107) + i) !== i) { bad = i; break; } } \
         bad < 0 ? (m.size === 200 ? 'ok' : 'size') : 'bad:' + bad",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "ok");
}

// ── getOrInsert：命中返回现值不插入，缺失末尾追加并返回实参 ──

#[test]
fn map_get_or_insert_missing_appends_and_returns_arg() {
    // 键缺失：末尾追加新条目、返回实参值，size 加一。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 1); var v = m.getOrInsert('b', 2); v + '/' + m.size",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2/2");
}

#[test]
fn map_get_or_insert_present_returns_stored_value() {
    // 键命中：返回现值、不插入，size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 1); var v = m.getOrInsert('a', 99); v + '/' + m.size",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1/1");
}

#[test]
fn map_get_or_insert_present_undefined_value_returns_undefined() {
    // 存储值为 undefined 也计命中：直接返回 undefined，不插入、size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('u', undefined); var hit = m.getOrInsert('u', 7); hit === undefined && m.size === 1",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn map_get_or_insert_zero_key_normalized() {
    // ±0 归一同键：存 -0 后以 +0 查，命中返回现值。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set(-0, 1); m.getOrInsert(0, 9) + '/' + m.size").unwrap();
    assert_eq!(str_val(&vm, r), "1/1");
}

#[test]
fn map_get_or_insert_length_and_name() {
    // 函数描述符：length 2、name 'getOrInsert'。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Map.prototype.getOrInsert.length === 2 && Map.prototype.getOrInsert.name === 'getOrInsert'",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn map_length_descriptor() {
    // Map.length = 0，描述符 { writable:false, enumerable:false, configurable:true }。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(Map, 'length'); \
         d.value === 0 && d.writable === false && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(r.as_bool());
}

// ── getOrInsertComputed：命中短路，缺失回调求值，二扫原位覆盖保序位 ──

#[test]
fn map_get_or_insert_computed_missing_invokes_callback_once() {
    // 键缺失：callback 恰调用一次，返回值末尾追加，size 加一。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 1); var n = 0; \
         var v = m.getOrInsertComputed('b', function () { n += 1; return 'vb'; }); \
         v + '/' + m.size + '/' + n + '/' + m.get('b')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "vb/2/1/vb");
}

#[test]
fn map_get_or_insert_computed_present_skips_callback() {
    // 键命中：callback 不求值，返回存储值，size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 1); var n = 0; \
         var v = m.getOrInsertComputed('a', function () { n += 1; return 99; }); \
         v + '/' + m.size + '/' + n",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1/1/0");
}

#[test]
fn map_get_or_insert_computed_present_undefined_value_returns_undefined() {
    // 存储值为 undefined 也计命中：返回 undefined，不插入、size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('u', undefined); var n = 0; \
         var hit = m.getOrInsertComputed('u', function () { n += 1; return 7; }); \
         (hit === undefined) + '/' + m.size + '/' + n",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true/1/0");
}

#[test]
fn map_get_or_insert_computed_zero_key_canonicalized_for_callback() {
    // 键规范化：-0 键在传 callback 前归一为 +0（Object.is 区分 ±0）。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); \
         var v = m.getOrInsertComputed(-0, function (k) { return (Object.is(k, 0) && (1 / k) === Infinity) ? 5 : 6; }); \
         v + '/' + m.size + '/' + (m.get(-0) === 5)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "5/1/true");
}

#[test]
fn map_get_or_insert_computed_callback_throws_propagates_original() {
    // callback 抛错：原异常值上抛，零条目落表，size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 1); var e = null; \
         try { m.getOrInsertComputed('b', function () { throw 'E'; }); } catch (t) { e = t; } \
         e + '/' + m.size + '/' + m.has('b')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "E/1/false");
}

#[test]
fn map_get_or_insert_computed_rejects_non_callable_and_non_map() {
    // 非函数 callback 抛 TypeError；非 Map receiver 抛 TypeError（receiver 校验先于 callback 检查）。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); var ok = false; \
         try { m.getOrInsertComputed('a', 42); } catch (t) { ok = t instanceof TypeError; } \
         var ok2 = false; \
         try { Map.prototype.getOrInsertComputed.call({}, 'a', function () {}); } catch (t) { ok2 = t instanceof TypeError; } \
         ok + '/' + ok2",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true/true");
}

#[test]
fn map_get_or_insert_computed_second_scan_overwrites_in_place() {
    // callback 内 set 同键后返回 3：二扫命中，原位覆盖并返回，插入序保留，size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('first', 'f'); \
         var v = m.getOrInsertComputed('k', function () { m.set('k', 0); return 3; }); \
         v + '/' + m.get('k') + '/' + m.size + '/' + \
         (function () { var out = []; m.forEach(function (val, key) { out.push(key); }); return out.join(','); })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "3/3/2/first,k");
}

#[test]
fn map_get_or_insert_computed_length_name_and_descriptor() {
    // 函数描述符：length 2、name 'getOrInsertComputed'，原型属性 writable/enumerable/configurable 三属性。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var f = Map.prototype.getOrInsertComputed; \
         var d = Object.getOwnPropertyDescriptor(Map.prototype, 'getOrInsertComputed'); \
         f.length === 2 && f.name === 'getOrInsertComputed' && \
         d.writable === true && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(r.as_bool());
}

// ── 活表序位语义：删除留洞、重加/新增末尾追加、空槽跳过、每轮重读槽数 ──

#[test]
fn map_for_each_delete_current_and_readd_visits_tail() {
    // 删当前键+重加：原槽留洞跳过、重加条目末尾再访问，count=3、size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 0); m.set('b', 1); var r = []; \
         m.forEach(function (v, k) { if (r.length === 0) { m.delete('a'); m.set('a', 'x'); } r.push(k + '=' + v); }); \
         r.join('|') + '|' + r.length + '|' + m.size",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "a=0|b=1|a=x|3|2");
}

#[test]
fn map_for_each_delete_future_key_skips() {
    // 删未来键不重加：该键不访问，其后的键不被连带跳过。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 0); m.set('b', 1); m.set('c', 2); var r = []; \
         m.forEach(function (v, k) { if (k === 'a') m.delete('b'); r.push(k); }); \
         r.join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "a,c");
}

#[test]
fn map_for_each_new_key_during_iteration_visited() {
    // 迭代期新增键：末尾追加并被访问，count=初始+1。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 0); m.set('b', 1); var r = []; \
         m.forEach(function (v, k) { if (k === 'b') m.set('c', 2); r.push(k); }); \
         r.join(',') + '|' + m.size",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "a,b,c|3");
}

#[test]
fn map_for_each_delete_after_visit_readd_visits_again() {
    // 访问后删+完成前重加：再访问（a, b, c, a′ 形），size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('a', 0); m.set('b', 1); m.set('c', 2); var r = []; \
         m.forEach(function (v, k) { if (k === 'a' && v === 0) { m.delete('a'); m.set('a', 'x'); } r.push(k + '=' + v); }); \
         r.join('|') + '|' + m.size",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "a=0|b=1|c=2|a=x|3");
}

#[test]
fn map_for_of_delete_and_readd_visits_readded() {
    // for..of 迭代期 delete(2)+set(2)：重加条目末尾再访问，键序 1,2,3,2。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set(1, 10); m.set(2, 20); m.set(3, 30); var r = []; \
         for (var e of m) { if (e[0] === 2 && e[1] === 20) { m.delete(2); m.set(2, 'x'); } r.push(e[0]); } \
         r.join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1,2,3,2");
}

#[test]
fn map_entries_exhausted_then_set_stays_done() {
    // entries 迭代器耗尽后 set 新键不复活：后续 next() 恒 done。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set(1, 1); m.set(2, 2); var it = m.entries(); \
         var a = it.next(); var b = it.next(); var c = it.next(); \
         m.set(3, 3); var d = it.next(); \
         a.value[0] + ',' + b.value[0] + ',' + c.done + ',' + d.done",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1,2,true,true");
}

#[test]
fn map_get_or_insert_computed_second_scan_keeps_order() {
    // 二扫原位覆盖保序位：键值更新不移动序位，forEach 序不变、size 不变。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set('first', 'f'); \
         var v = m.getOrInsertComputed('k', function () { m.set('k', 0); return 3; }); \
         var ord = []; m.forEach(function (val, key) { ord.push(key); }); \
         v + '/' + m.get('k') + '/' + m.size + '/' + ord.join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "3/3/2/first,k");
}

#[test]
fn map_delete_then_set_same_key_moves_to_tail() {
    // delete 后 set 同键：原槽留洞、重加条目末尾追加，forEach 序 [1, 3, 2]。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var m = new Map(); m.set(1, 1); m.set(2, 2); m.set(3, 3); \
         m.delete(2); m.set(2, 'x'); var r = []; \
         m.forEach(function (v, k) { r.push(k + ':' + v); }); r.join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1:1,3:3,2:x");
}
