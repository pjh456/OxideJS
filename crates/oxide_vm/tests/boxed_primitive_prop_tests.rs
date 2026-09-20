//! 装箱基元对象（Number/Boolean/Symbol/BigInt 盒）索引写读回钉：
//! 构造期被包基元住专属载荷字段而非属性区，后续索引/命名属性写读与
//! 被包值互不干扰，值与 node 实测一致。

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

/// 读数组结果第 i 个元素（结果必为数组）。
fn elem(result: JsValue, i: usize) -> JsValue {
    let obj = unsafe { &*result.as_js_object_ptr() };
    obj.get_prop_at(i)
}

#[test]
fn boxed_number_fresh_box_has_no_index_payload() {
    // fresh 盒无自有属性：0 键不存在、索引读 undefined、被包值经 Number 还原、
    // 自有属性名空。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "[0 in Object(5), Object(5)[0], Number(Object(5)), Object.getOwnPropertyNames(Object(5)).length]",
    )
    .unwrap();
    let v0 = elem(r, 0);
    assert!(v0.is_bool() && !v0.as_bool(), "0 in Object(5) 应为 false");
    assert!(elem(r, 1).is_undefined(), "Object(5)[0] 应为 undefined");
    assert_eq!(elem(r, 2).as_int(), 5, "Number(Object(5)) 应为 5");
    assert_eq!(elem(r, 3).as_int(), 0, "自有属性名应为空");
}

#[test]
fn boxed_number_index_write_reads_written_value() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = Object(5); o[0] = 77; [0 in o, o[0], Number(o)]").unwrap();
    let v0 = elem(r, 0);
    assert!(v0.is_bool() && v0.as_bool(), "写后 0 in o 应为 true");
    assert_eq!(elem(r, 1).as_int(), 77, "o[0] 应读回写入值 77");
    assert_eq!(elem(r, 2).as_int(), 5, "Number(o) 应保持被包值 5");
}

#[test]
fn boxed_number_repeated_index_write_keeps_payload() {
    // 二次写命中同一键：读回最新写入值，被包基元不被覆写。
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = Object(5); o[0] = 77; o[0] = 99; [o[0], Number(o)]").unwrap();
    assert_eq!(elem(r, 0).as_int(), 99, "o[0] 应读回 99");
    assert_eq!(elem(r, 1).as_int(), 5, "被包值应仍为 5");
}

#[test]
fn boxed_number_named_keys_not_shifted() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = Object(5); o.a = 1; o.b = 2; [o.a, o.b]").unwrap();
    assert_eq!(elem(r, 0).as_int(), 1, "o.a 应为 1");
    assert_eq!(elem(r, 1).as_int(), 2, "o.b 应为 2");
}

#[test]
fn boxed_number_non_index_key_read_write() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = Object(5); o.x = 1; o.x").unwrap();
    assert_eq!(r.as_int(), 1, "o.x 应读回 1");
}

#[test]
fn boxed_number_out_of_range_index_absent() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var o = Object(5); o[0] = 77; [1 in o, 5 in o]").unwrap();
    let v0 = elem(r, 0);
    assert!(v0.is_bool() && !v0.as_bool(), "1 in o 应为 false");
    let v1 = elem(r, 1);
    assert!(v1.is_bool() && !v1.as_bool(), "5 in o 应为 false");
}

#[test]
fn boxed_boolean_index_write_reads_written_value() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var t = Object(true); t[0] = 'x'; [t[0], Boolean(t)]").unwrap();
    assert_eq!(vm.lookup_str(elem(r, 0)).unwrap(), "x", "t[0] 应读回 'x'");
    assert!(elem(r, 1).as_bool(), "Boolean(t) 应保持 true");
}

#[test]
fn boxed_float_index_write_reads_written_value() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var f = Object(1.5); f[0] = 'x'; [f[0], Number(f)]").unwrap();
    assert_eq!(vm.lookup_str(elem(r, 0)).unwrap(), "x", "f[0] 应读回 'x'");
    assert_eq!(elem(r, 1).as_double(), 1.5, "Number(f) 应保持 1.5");
}

#[test]
fn boxed_bigint_index_write_reads_written_value() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var bb = Object(1n); bb[0] = 9; [bb[0], BigInt(bb)]").unwrap();
    assert_eq!(elem(r, 0).as_int(), 9, "bb[0] 应读回 9");
    assert!(elem(r, 1).is_bigint(), "BigInt(bb) 应为 BigInt");
    assert_eq!(vm.bigint_value(elem(r, 1)), &num_bigint::BigInt::from(1), "BigInt(bb) 应为 1n");
}

#[test]
fn boxed_symbol_index_write_reads_written_value() {
    // Symbol 盒消费面（thisSymbolValue 解盒）同批切换，索引写读与其互不干扰。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var s = Object(Symbol('d')); s[0] = 7; [s[0], Symbol.prototype.toString.call(s)]",
    )
    .unwrap();
    assert_eq!(elem(r, 0).as_int(), 7, "s[0] 应读回 7");
    assert_eq!(vm.lookup_str(elem(r, 1)).unwrap(), "Symbol(d)", "解盒 toString 应保持 Symbol(d)");
}

#[test]
fn boxed_primitive_existing_guards_hold() {
    // 既有面守卫：构造器装箱、valueOf、typeof、toString tag 面不随本次机制变化。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "[+new Number(1), new Number(5).valueOf(), typeof new Number(5), Object.prototype.toString.call(Object(5))]",
    )
    .unwrap();
    // 一元 + 结果按引擎算术面为 double 表示（数值相等，显示同 int）。
    assert_eq!(elem(r, 0).as_double(), 1.0, "+new Number(1) 应为 1");
    assert_eq!(elem(r, 1).as_int(), 5, "valueOf 应返回 5");
    assert_eq!(vm.lookup_str(elem(r, 2)).unwrap(), "object", "typeof 应为 object");
    assert_eq!(vm.lookup_str(elem(r, 3)).unwrap(), "[object Number]", "toString tag 应为 [object Number]");
}
