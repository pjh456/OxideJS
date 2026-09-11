//! 调用实参 spread 执行语义测试：`f(...args)`、`new F(...args)`、`super(...args)`。
//! 覆盖静态/混合实参顺序、迭代器协议、IteratorClose、native 目标、求值顺序。

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

fn to_str(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        vm.lookup_str(val).unwrap_or_default()
    } else {
        format!("{}", val)
    }
}

fn to_num(val: JsValue) -> f64 {
    if val.is_int() {
        val.as_int() as f64
    } else {
        val.as_double()
    }
}

#[test]
fn spread_single_literal_preserves_all_args() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){return arguments.length}; f(...[1,2,3])").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn spread_interleaved_static_and_spread_keeps_source_order() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(a,b,c){return [a,b,c].join('-')}; f('x', ...['y'], 'z')").unwrap();
    assert_eq!(to_str(&vm, result), "x-y-z");
}

#[test]
fn spread_multiple_sources_append_in_order() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){return arguments.length}; f(...[1], 2, ...[3,4])").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn spread_native_target() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max(...[1,5,3])").unwrap();
    assert_eq!(to_num(result), 5.0);
}

#[test]
fn spread_new_expression_bytecode_ctor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new (function(a,b){this.sum=a+b})(...[2,3]).sum").unwrap();
    assert_eq!(to_num(result), 5.0);
}

#[test]
fn spread_new_expression_native_ctor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new Date(...[2020,1,1]).getFullYear()").unwrap();
    assert_eq!(to_num(result), 2020.0);
}

#[test]
fn spread_super_call_passes_args() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(a,b,c){ this.v=[a,b,c].join('') } }; \
         class B extends A { constructor(){ super(...[1,2,3]) } }; new B().v",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "123");
}

#[test]
fn spread_non_iterable_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { (function(){})(...null); 'no-throw' } catch(e) { e.constructor.name }").unwrap();
    assert_eq!(to_str(&vm, result), "TypeError");
}

#[test]
fn spread_invokes_iterator_close_on_abrupt_completion() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var closed=0; try { (function(){}) (...{[Symbol.iterator]:function(){ return { \
            next:function(){ throw new Error('boom') }, \
            return:function(){ closed=1; return {} } } }}) } catch(e) {}; closed",
    )
    .unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn spread_does_not_close_iterator_on_normal_completion() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var closed=0; var i=0; (function(){}) (...{[Symbol.iterator]:function(){ return { \
            next:function(){ return ++i<=2 ? {done:false,value:i} : {done:true} }, \
            return:function(){ closed=1; return {} } } }}); closed",
    )
    .unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn spread_closure_capture_survives() {
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "function outer(){var x=42; function g(a){return a+x}; return g(...[x])} outer()").unwrap();
    assert_eq!(to_num(result), 84.0);
}

#[test]
fn spread_string_source() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){return arguments.length}; f(...'ab')").unwrap();
    assert_eq!(result.as_int(), 2);
}
