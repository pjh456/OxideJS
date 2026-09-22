//! 字符串盒（String exotic）构造期物化钉测：`boxed_value` 专属载荷与字符索引/
//! length 固有属性面。覆盖 test262 盒面红转绿的语义面——索引/length 读、in/
//! 枚举、描述符、seal/freeze、defineProperty 同值改写放行/异值与属性面变更
//! 拒绝、写保护（strict/sloppy）、delete 返 false、Object.assign/解构消费面、
//! %StringPrototype% 自身盒面、迭代器/Array.from 载荷读、@@toStringTag 覆盖。
//!
//! 语义对齐规范 String exotic：索引属性 writable:false / enumerable:true /
//! configurable:false，length writable:false / enumerable:false /
//! configurable:false；写失败 sloppy 返回被赋值（静默）、strict 抛 TypeError；
//! 同值重定义放行、异值或属性面变更抛 TypeError；delete 返 false；越界新键
//! （如 length 之后的下标）按普通数据属性定义成功。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

#[test]
fn boxed_value_roundtrip() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); return s.valueOf() === 'ab' && s.toString() === 'ab' && Object.prototype.toString.call(s) === '[object String]'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn index_and_length_read() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); return s[0] === 'a' && s[1] === 'b' && s[2] === undefined && s.length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn in_operator() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); return '0' in s && '1' in s && !('2' in s) && 'length' in s; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn forin_enumerates_indexes_only() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const keys = []; for (const k in s) keys.push(k); return keys.length === 2 && keys[0] === '0' && keys[1] === '1'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn keys_and_own_keys() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const k = Object.keys(s).join(','); const o = Object.getOwnPropertyNames(s).sort().join(','); return k === '0,1' && o === '0,1,length'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn gopd_index_and_length() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const d = Object.getOwnPropertyDescriptor(s, '0'); const l = Object.getOwnPropertyDescriptor(s, 'length'); return d.value === 'a' && d.writable === false && d.enumerable === true && d.configurable === false && l.value === 2 && l.writable === false && l.enumerable === false && l.configurable === false; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn gopds_full_surface() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const d = Object.getOwnPropertyDescriptors(s); const ks = Object.keys(d).sort(); return ks.length === 3 && ks[0] === '0' && ks[1] === '1' && ks[2] === 'length' && d['0'].value === 'a' && d['1'].value === 'b' && d.length.value === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn seal_freeze_noop() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); Object.seal(s); Object.freeze(s); Object.preventExtensions(s); return s[0] === 'a' && s.length === 2 && s[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn defineproperty_same_value_ok() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const r = Object.defineProperty(s, '1', { value: 'b' }); return r === s && s[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn defineproperty_diff_value_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); let e = null; try { Object.defineProperty(s, '1', { value: 'x' }); } catch (err) { e = err; } return e instanceof TypeError && s[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn defineproperty_attribute_change_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); let e = null; try { Object.defineProperty(s, '1', { writable: true }); } catch (err) { e = err; } return e instanceof TypeError; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn defineproperty_new_key_ok() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const r = Object.defineProperty(s, '2', { value: 'z' }); return r === s && s[2] === 'z'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_sloppy_returns_value() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const r0 = s[0] = 'z'; const r1 = s.length = 9; return r0 === 'z' && r1 === 9 && s[0] === 'a' && s.length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_strict_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "\"use strict\"; (function(){ const s = new String('ab'); let e1 = null; try { s[0] = 'z'; } catch (err) { e1 = err; } let e2 = null; try { s.length = 9; } catch (err) { e2 = err; } return e1 instanceof TypeError && e2 instanceof TypeError && s[0] === 'a' && s.length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn delete_returns_false() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('ab'); const d0 = delete s[0]; const dl = delete s.length; return d0 === false && dl === false && s[0] === 'a' && s.length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn object_assign_string_source() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const o1 = Object.assign({}, 'ab'); const o2 = Object.assign({}, new String('ab')); return o1[0] === 'a' && o1[1] === 'b' && o1.length === undefined && o2[0] === 'a' && o2[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn object_assign_skips_null_undefined() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const o = Object.assign({}, null, undefined, 'cd'); return o[0] === 'c' && o[1] === 'd' && Object.keys(o).length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn assign_target_string_box_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const t = new String('ab'); let e = null; try { Object.assign(t, 'cd'); } catch (err) { e = err; } return e instanceof TypeError && t[0] === 'a' && t[1] === 'b' && t.length === 2; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn rest_destructure_string_box() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const { 0: a, ...rest } = new String('abc'); return a === 'a' && rest[1] === 'b' && rest[2] === 'c' && rest[0] === undefined; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn string_prototype_box() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const p = String.prototype; return Object.prototype.toString.call(p) === '[object String]' && p.length === 0 && p.valueOf() === ''; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn symbol_tag_via_defineproperty() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('x'); Object.defineProperty(s, Symbol.toStringTag, { value: 'T' }); return Object.prototype.toString.call(s) === '[object T]'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn iterator_over_box() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const arr = [...new String('ab')]; return arr.length === 2 && arr[0] === 'a' && arr[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn array_from_box() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const arr = Array.from(new String('ab')); return arr.length === 2 && arr[0] === 'a' && arr[1] === 'b'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn empty_string_box() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const e = new String(''); return e.length === 0 && e.valueOf() === '' && Object.getOwnPropertyNames(e).length === 1 && e[0] === undefined; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn valueof_and_toststring_receiver() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(function(){ const s = new String('z'); return String.prototype.valueOf.call(s) === 'z' && String.prototype.toString.call(s) === 'z'; })()",
    )
    .unwrap();
    assert!(r.as_bool());
}
