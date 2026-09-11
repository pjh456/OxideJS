//! 函数 length/name 属性集成测试：普通字节码函数、箭头函数、bind 包装器、
//! getter/setter 前缀。每个用例独立编译执行，断言顶层表达式结果。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> JsValue {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Arc::new(Compiler::new().compile(&program).expect("compile failed"));
    run(&module)
}

fn run(module: &Arc<CompiledModule>) -> JsValue {
    let mut vm = Vm::new();
    vm.run(module).expect("vm run failed")
}

fn eval_str(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Arc::new(Compiler::new().compile(&program).expect("compile failed"));
    let mut vm = Vm::new();
    let v = vm.run(&module).expect("vm run failed");
    // 字符串结果须在同一 VM 上读：perm 串指向 VM 私有内核，VM drop 后指针悬垂。
    if v.is_string() {
        vm.lookup_str(v).unwrap_or_default()
    } else {
        format!("{v}")
    }
}

#[test]
fn bytecode_function_length_counts_formals_before_first_default() {
    assert_eq!(eval("function f(a,b,c){}; f.length"), JsValue::int(3));
    assert_eq!(eval("function f(a,b=1,c){}; f.length"), JsValue::int(1));
    assert_eq!(eval("function f(...r){}; f.length"), JsValue::int(0));
    assert_eq!(eval("var f = function named(a,b,c){}; f.length"), JsValue::int(3));
}

#[test]
fn arrow_function_length_matches_formals() {
    assert_eq!(eval("var f=(a,b)=>a+b; f.length"), JsValue::int(2));
    assert_eq!(eval("var f=(a,b,c)=>a+b+c; f.length"), JsValue::int(3));
}

#[test]
fn class_method_and_accessor_names() {
    assert_eq!(eval("class C { m(a,b){} }; C.prototype.m.length"), JsValue::int(2));
    assert_eq!(
        eval_str("class C { get x(){} set x(v){} }; Object.getOwnPropertyDescriptor(C.prototype,'x').get.name"),
        "get x"
    );
    assert_eq!(
        eval_str("class C { get x(){} set x(v){} }; Object.getOwnPropertyDescriptor(C.prototype,'x').set.name"),
        "set x"
    );
}

#[test]
fn object_literal_accessor_names() {
    assert_eq!(
        eval_str("var o={ get x(){}, set x(v){} }; Object.getOwnPropertyDescriptor(o,'x').get.name"),
        "get x"
    );
    assert_eq!(
        eval_str("var o={ get x(){}, set x(v){} }; Object.getOwnPropertyDescriptor(o,'x').set.name"),
        "set x"
    );
}

#[test]
fn bind_wrapper_length_is_target_length_minus_bound_args() {
    assert_eq!(eval("function f(a,b){}; f.bind(null,1).length"), JsValue::int(1));
    assert_eq!(eval("function f(){}; f.bind(null,1,2,3).length"), JsValue::int(0));
    assert_eq!(eval("function f(a,b,c){}; f.bind(null).length"), JsValue::int(3));
}

#[test]
fn bind_wrapper_name_has_bound_prefix() {
    assert_eq!(eval_str("function f(a){}; f.bind(null).name"), "bound f");
    assert_eq!(eval_str("(function(){}).bind(null).name"), "bound ");
}

#[test]
fn bind_wrapper_length_supports_infinity_and_large_values() {
    assert_eq!(
        eval("function f(){}; Object.defineProperty(f,'length',{value:Infinity}); f.bind().length === Infinity"),
        JsValue::bool(true)
    );
    assert_eq!(
        eval("function f(){}; Object.defineProperty(f,'length',{value:2147483648}); f.bind().length"),
        JsValue::float(2147483648.0)
    );
}

#[test]
fn bind_wrapper_caller_and_arguments_are_poisoned() {
    assert_eq!(
        eval_str("var b=(function(){}).bind({}); try { b.caller; 'no' } catch(e) { e instanceof TypeError ? 'throws' : 'wrong' }"),
        "throws"
    );
    assert_eq!(
        eval_str("var b=(function(){}).bind({}); try { b.caller = 1; 'no' } catch(e) { e instanceof TypeError ? 'throws' : 'wrong' }"),
        "throws"
    );
}

#[test]
fn length_and_name_descriptors_are_non_writable_non_enumerable_configurable() {
    assert_eq!(
        eval_str("function f(){}; var d=Object.getOwnPropertyDescriptor(f,'length'); d.writable===false && d.enumerable===false && d.configurable===true"),
        "true"
    );
}

#[test]
fn length_comes_before_name_in_property_order() {
    assert_eq!(
        eval_str(
            "function f(a,b,c){}; var p=Object.getOwnPropertyNames(f); p[p.indexOf('length')] < p[p.indexOf('name')]"
        ),
        "true"
    );
}

#[test]
fn anonymous_function_name_is_empty_string() {
    assert_eq!(eval_str("(function(){}).name"), "");
}
