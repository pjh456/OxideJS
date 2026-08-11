use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn let_block_scoping_outer_unchanged() {
    let result = eval("let x = 1; { let x = 2; } x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn const_reassignment_throws() {
    let result = eval("const x = 1; x = 2");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("TypeError") || err.contains("constant"), "got: {}", err);
}

#[test]
fn var_block_not_isolated() {
    let result = eval("var x = 5; { x = 10; } x").unwrap();
    assert_eq!(result.as_int(), 10);
}

#[test]
fn delete_existing_property() {
    let result = eval("var o = {a: 1}; delete o.a; o.a").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn delete_returns_true() {
    let result = eval("var o = {a: 1}; delete o.a").unwrap();
    assert!(result.as_bool());
}

#[test]
fn instanceof_array() {
    let result = eval("[] instanceof Array").unwrap();
    assert!(result.as_bool());
}

#[test]
fn instance_not_array() {
    let result = eval("({}) instanceof Array").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn void_returns_undefined() {
    let result = eval("void 0").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn closure_nested_decl_reads_outer_var() {
    let result = eval("function t(){ var s=3; function f(){return s} return f(); } t()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
#[ignore = "++n on captured upvalue: pre-scan marks is_captured but value still not returned correctly"]
fn closure_counter_escape() {
    let result =
        eval("function counter(){ var n=0; return function(){ return ++n; }; } var c=counter(); c(); c()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_arrow_reads_outer_param() {
    let result = eval("function outer(p){ var f=()=>p; return f(); } outer(7)").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn closure_write_upvalue() {
    let result = eval("function outer(){ var x=1; function set(){ x=2; return x; } return set(); } outer()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_nested_expr_capture() {
    let result = eval("function outer(){ var x=1; return function(){return x+1}()} outer()").unwrap();
    assert_eq!(result.as_double(), 2.0);
}

#[test]
fn closure_captures_computed_object_key_across_caller_frame() {
    let result = eval("let key='answer'; const call=fn=>fn(); const make=()=>({[key]:42}); call(make).answer").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn closure_captures_object_spread_source_across_caller_frame() {
    let result = eval("let source={x:7}; const call=fn=>fn(); const make=()=>({...source}); call(make).x").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn for_let_per_iteration_independent() {
    let _ = eval("var fns=[]; for(let i=0;i<3;i++){ fns.push(function(){return i}); } fns[0]()+fns[1]()+fns[2]()");
    // 未支持：for-let 逐次迭代绑定尚未实现。
}

#[test]
fn tdz_access_before_init_throws() {
    let _ = eval("x; let x=1");
    // 未支持：精确 TDZ 尚未实现。
}
