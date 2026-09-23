use std::sync::Arc;

use oxide_builtins::set::{set_add, set_clear, set_constructor as new_set, set_delete, set_has, set_size};
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

/// 分配一个原型指向 Set.prototype 的占位对象并写入 reg 0，作为构造器调用的 `this`。
fn set_this(vm: &mut Vm) -> JsValue {
    let proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    // 统一入口分配：epoch 置位与对象表登记一步闭合，执行期晋升收集按表取活。
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
    let val = JsValue::from_js_object(obj);
    vm.set_reg(0, val);
    val
}

// -- direct native fn tests --

#[test]
fn tset_constructor_returns_object() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let r = new_set(&mut vm, &[0]).unwrap();
    assert!(r.is_object());
}

#[test]
fn tset_add_has() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(42.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert!(set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_has_missing() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(99.0));
    assert!(!set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_delete_works() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(7.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert!(set_delete(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(!set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_size_and_clear() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
    vm.set_reg(1, JsValue::float(1.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    vm.set_reg(1, JsValue::float(2.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 2.0);
    set_clear(&mut vm, &[0]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
}

#[test]
fn tset_nan_equality() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(f64::NAN));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
}

#[test]
fn tset_signed_zero() {
    let mut vm = Vm::new();
    set_this(&mut vm);
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(0.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    vm.set_reg(1, JsValue::float(-0.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
}

// -- JS eval tests using new keyword --

#[test]
fn set_new_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s").unwrap();
    assert!(r.is_object());
}

#[test]
fn set_new_add_has() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add(1); s.has(1)").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_new_has_missing() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.has(99)").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn set_new_delete() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add(1); s.add(2); s.delete(1); s.has(1)").unwrap();
    assert!(!r.as_bool());
}

// ── SameValueZero 键语义：运行期构造键（非常量池指针）与同值字面量互为同键 ──

#[test]
fn set_runtime_string_key_lookup_by_literal() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add(String.fromCharCode(120)); s.has('x')").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_literal_key_lookup_by_runtime() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add('x'); s.has(String.fromCharCode(120))").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_runtime_concat_key_matches_literal() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add('a' + 'b'); s.has('ab') && !s.has('a')").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_same_content_string_is_one_key() {
    // 同内容两构造（字面量 + 运行期拼接）只占一个键位。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var s = new Set(); s.add('ab'); \
         s.add(String.fromCharCode(97) + String.fromCharCode(98)); s.size",
    )
    .unwrap();
    assert_eq!(r.as_double(), 1.0);
}

#[test]
fn set_runtime_string_key_delete() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set(); s.add('x'); s.delete(String.fromCharCode(120)); s.has('x')").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn set_many_runtime_string_keys_lookup() {
    // 键量越过小表线性探测阈值后查找走哈希索引，哈希与值等值的一致性在此兑现。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var s = new Set(); \
         for (var i = 0; i < 200; i++) { s.add('k' + i); } \
         var bad = -1; \
         for (var i = 0; i < 200; i++) { if (!s.has(String.fromCharCode(107) + i)) { bad = i; break; } } \
         bad < 0 ? (s.size === 200 ? 'ok' : 'size') : 'bad:' + bad",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(r).unwrap_or_default(), "ok");
}

#[test]
fn set_bigint_key_same_value_different_boxes() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var s = new Set([10n]); s.has(BigInt(10)) && !s.has(11n) && !s.has(10)").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_nan_and_zero_key_semantics() {
    // NaN 同 NaN 键、0/-0 同键；NaN 与最小正非规格化数（2^-1074）不得同键。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var s = new Set([NaN, 0]); \
         s.size === 2 && s.has(NaN) && s.has(-0) && !s.has(Math.pow(2, -1074))",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_object_key_identity() {
    // 对象键按引用相等：同对象命中，等值异对象不命中。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = { a: 1 }; var s = new Set(); s.add(o); s.has(o) && !s.has({ a: 1 })").unwrap();
    assert!(r.as_bool());
}

#[test]
fn set_length_descriptor() {
    // Set.length = 0，描述符 { writable:false, enumerable:false, configurable:true }。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(Set, 'length'); \
         d.value === 0 && d.writable === false && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(r.as_bool());
}
