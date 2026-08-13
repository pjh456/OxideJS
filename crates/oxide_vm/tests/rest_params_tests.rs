//! rest 参数集成测试：实参收集、length 交互、与 spread/箭头/闭包组合。
//! 每个用例独立编译执行，断言顶层表达式结果。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> JsValue {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let mut vm = Vm::new();
    vm.run(&module).expect("vm run failed")
}

fn eval_int(source: &str) -> i32 {
    let v = eval(source);
    if v.is_int() {
        v.as_int()
    } else if v.is_double() {
        v.as_double() as i32
    } else {
        panic!("expected number, got {v}")
    }
}

fn eval_str(source: &str) -> String {
    let v = eval(source);
    if v.is_string() {
        oxide_runtime_api::to_string(v)
    } else {
        format!("{v}")
    }
}

#[test]
fn rest_collects_remaining_arguments() {
    assert_eq!(eval_int("function f(...args){return args.length} f(1,2,3)"), 3);
    assert_eq!(eval_str("function f(...args){return args.join(',')} f(1,2,3)"), "1,2,3");
    assert_eq!(eval_int("function f(...args){return args.length} f()"), 0);
    assert_eq!(eval_int("function f(...args){return args[0]+args[1]} f(7,8)"), 15);
}

#[test]
fn rest_after_fixed_params_starts_at_fixed_count() {
    assert_eq!(eval_str("function f(a,...rest){return a+':'+rest.join(',')} f(1,2,3)"), "1:2,3");
    assert_eq!(eval_int("function f(a,b,...rest){return rest.length} f(1,2,3,4,5)"), 3);
    assert_eq!(eval_int("function f(a,b,...rest){return rest.length} f(1)"), 0);
}

#[test]
fn rest_is_a_mutable_array() {
    assert_eq!(eval_str("function f(...args){return Array.isArray(args)&&'y'} f(1)"), "y");
    assert_eq!(eval_str("function f(a,...r){r.push(4); return r.join(',')} f(1,2,3)"), "2,3,4");
    assert_eq!(eval_str("function f(...a){a[0]=99; return a.join(',')} f(1,2,3)"), "99,2,3");
}

#[test]
fn rest_combines_with_spread_and_arrow() {
    assert_eq!(eval_int("function f(...rest){return rest.length} f(...[1,2,3])"), 3);
    assert_eq!(eval_int("var f=(...args)=>args.length; f(1,2,3)"), 3);
}

#[test]
fn rest_captured_by_closure() {
    assert_eq!(eval_int("function f(...a){return ()=>a.length} f(1,2,3)()"), 3);
}

#[test]
fn rest_does_not_count_toward_function_length() {
    assert_eq!(eval("function f(...r){}; f.length"), JsValue::int(0));
    assert_eq!(eval("function f(a,b=1,...r){}; f.length"), JsValue::int(1));
}
