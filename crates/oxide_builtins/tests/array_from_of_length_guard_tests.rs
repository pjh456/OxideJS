//! Array.from / Array.of 结果对象完整性收口引擎钉：frozen / sealed /
//! preventExtensions / 非可写自有索引目标按规范 CreateDataPropertyOrThrow 与
//! Set(length) 语义抛 TypeError，新数组热路径与回退臂零漂移。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval_value(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, result) = eval_value(source)?;
    vm.lookup_str(result)
        .ok_or_else(|| "completion value is not a string".to_string())
}

// frozen 目标（iterable 源）：首元素写即抛 TypeError，目标零改动。
#[test]
fn test_from_frozen_target_iterable_throws_untouched() {
    let out = eval_str(
        "(() => { const A = Object.freeze([]); function Cf() { return A; } \
         let k = 'no-throw'; try { Array.from.call(Cf, [1, 2, 3]); } catch (e) { k = e.constructor.name; } \
         return k + '|' + A.length + '|' + (0 in A); })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError|0|false");
}

// frozen 目标零元素源：元素写不触发，length 写抛 TypeError。
#[test]
fn test_from_frozen_target_empty_throws_length() {
    let out = eval_str(
        "(() => { function Cf() { return Object.freeze([]); } \
         let k = 'no-throw'; try { Array.from.call(Cf, []); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// frozen 目标 array-like 源：首元素写抛 TypeError。
#[test]
fn test_from_frozen_target_arraylike_throws() {
    let out = eval_str(
        "(() => { function Cf() { return Object.freeze([]); } \
         let k = 'no-throw'; \
         try { Array.from.call(Cf, { length: 2, 0: 'x', 1: 'y' }); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// sealed 目标两形：空目标新槽不可扩展抛 TypeError；预置目标已有槽非可配置
// 重定义抛 TypeError 且原值不动。
#[test]
fn test_from_sealed_target_throws_existing_slot_preserved() {
    let out = eval_str(
        "(() => { const S0 = Object.seal([]); function Cf0() { return S0; } \
         let k1 = 'no-throw'; try { Array.from.call(Cf0, [0, 1]); } catch (e) { k1 = e.constructor.name; } \
         const S9 = Object.seal([9]); function Cf9() { return S9; } \
         let k2 = 'no-throw'; try { Array.from.call(Cf9, [0, 1]); } catch (e) { k2 = e.constructor.name; } \
         return k1 + '|' + k2 + '|' + S9[0]; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError|TypeError|9");
}

// preventExtensions 目标空数组：新槽不可扩展抛 TypeError。
#[test]
fn test_from_prevent_extensions_target_throws() {
    let out = eval_str(
        "(() => { function Cf() { return Object.preventExtensions([]); } \
         let k = 'no-throw'; try { Array.from.call(Cf, [0, 1]); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// preventExtensions 目标预置 [9] 且源仅重写已有槽：元素写成功、length 同长
// 不抛，结果即原对象。
#[test]
fn test_from_prevent_extensions_existing_slot_rewritten() {
    let out = eval_str(
        "(() => { const A = Object.preventExtensions([9]); function Cf() { return A; } \
         const r = Array.from.call(Cf, [7]); \
         return String(r === A) + '|' + A.length + '|' + A[0]; })()",
    )
    .unwrap();
    assert_eq!(out, "true|1|7");
}

// 非可写可配置自有索引 "0"（iterable 源）：CreateDataPropertyOrThrow 覆盖为
// present，描述符恢复可写可枚举。
#[test]
fn test_from_nonwritable_own_index_overwritten() {
    let out = eval_str(
        "(() => { function Cf() { const a = []; \
         Object.defineProperty(a, '0', { value: 1, writable: false, configurable: true }); return a; } \
         const r = Array.from.call(Cf, [2]); \
         const d = Object.getOwnPropertyDescriptor(r, '0'); \
         return r[0] + '|' + d.writable + '|' + d.enumerable; })()",
    )
    .unwrap();
    assert_eq!(out, "2|true|true");
}

// Array.of frozen 目标：首元素写抛 TypeError。
#[test]
fn test_of_frozen_target_throws() {
    let out = eval_str(
        "(() => { function Cf() { return Object.freeze([]); } \
         let k = 'no-throw'; try { Array.of.call(Cf, 1, 2); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// Array.of frozen 目标零参：元素写不触发，length 写抛 TypeError。
#[test]
fn test_of_frozen_target_zero_args_throws_length() {
    let out = eval_str(
        "(() => { function Cf() { return Object.freeze([]); } \
         let k = 'no-throw'; try { Array.of.call(Cf); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "TypeError");
}

// 冻结数组作 this（非构造器）：回退臂新建普通数组，原 this 不动。
#[test]
fn test_from_frozen_this_fallback_new_array() {
    let out = eval_str(
        "(() => { const r = Array.from.call(Object.freeze([1, 2]), 'xy'); \
         return r.join(',') + '|' + String(Object.isFrozen(r)); })()",
    )
    .unwrap();
    assert_eq!(out, "x,y|false");
}

// Array.of 冻结 this 零参构造器回退：新建普通数组。
#[test]
fn test_of_frozen_this_fallback_new_array() {
    let out = eval_str(
        "(() => { const r = Array.of.call(Object.freeze([1, 2]), 3, 4); \
         return r.join(',') + '|' + String(Object.isFrozen(r)); })()",
    )
    .unwrap();
    assert_eq!(out, "3,4|false");
}

// mapFn 抛错时序：映射先于元素写执行，RangeError 原样透传（frozen 目标不干扰）。
#[test]
fn test_from_mapfn_error_propagates_before_element_write() {
    let out = eval_str(
        "(() => { function Cf() { return Object.freeze([]); } \
         let k = 'no-throw'; \
         try { Array.from.call(Cf, [1], () => { throw new RangeError('m'); }); } \
         catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "RangeError");
}

// 零漂移四连：常规 from/of 结果值与节点位不变。
#[test]
fn test_from_of_plain_paths_unchanged() {
    let out = eval_str(
        "(() => { return Array.from([1, 2, 3]).join(',') + '|' + Array.from('ab').join(',') + '|' + \
         Array.from({ length: 2, 0: 'x' }).join(',') + '|' + Array.of(7, 8).join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "1,2,3|a,b|x,|7,8");
}

// 继承 length setter 抛错（非数组目标）：setter 异常原样透传。
#[test]
fn test_from_inherited_length_setter_error_propagates() {
    let out = eval_str(
        "(() => { function Cf() { const o = {}; \
         Object.defineProperty(o, 'length', { set: function () { throw new RangeError('ls'); } }); \
         return o; } \
         let k = 'no-throw'; try { Array.from.call(Cf, [1, 2]); } catch (e) { k = e.constructor.name; } \
         return k; })()",
    )
    .unwrap();
    assert_eq!(out, "RangeError");
}

// species 回归：结果 instanceof C、构造器恰调用一次、构造 this 即结果对象。
#[test]
fn test_from_species_ctor_contract() {
    let out = eval_str(
        "(() => { let callCount = 0, thisVal = null; \
         function C() { callCount++; thisVal = this; } \
         C.prototype = []; \
         const r = Array.from.call(C, [1, 2]); \
         return String(r instanceof C) + '|' + callCount + '|' + String(thisVal === r) + '|' + r.join(','); })()",
    )
    .unwrap();
    assert_eq!(out, "true|1|true|1,2");
}
