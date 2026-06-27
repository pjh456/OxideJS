use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn assert_num(value: JsValue, expected: f64) {
    let actual = if value.is_int() { value.as_int() as f64 } else { value.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {value:?}");
}

#[test]
fn for_of_array_values() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "let sum=0; for (const x of [1,2,3]) { sum += x; } sum").unwrap();
    assert_num(result, 6.0);
}

#[test]
fn array_binding_and_defaults() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const [a,b=4]=[1]; a+b").unwrap();
    assert_num(result, 5.0);
}

#[test]
fn array_rest_keeps_numeric_index_access() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const [a,...rest]=[1,2,3]; rest[0] + rest[1] + rest.length").unwrap();
    assert_num(result, 7.0);
}

#[test]
fn object_binding_rest_and_computed_key() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const key='x'; const {[key]: a, ...rest}={x:1,y:2}; a + rest.y").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn nested_binding_patterns() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const [[a], {b}] = [[1], {b:2}]; a+b").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn destructuring_assignment_swap_and_object_target() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "let a=1,b=2; [a,b]=[b,a]; ({x:a,y:b}={x:3,y:4}); a*10+b").unwrap();
    assert_num(result, 34.0);
}

#[test]
fn for_of_destructuring_left_side() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "let sum=0; for (const [k,v] of [[1,2],[3,4]]) { sum += k + v; } sum").unwrap();
    assert_num(result, 10.0);
}

#[test]
fn for_of_object_destructuring_assignment_target_runs_once() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var x = null; var count = 0; for ({ x } of [{ x: 3 }]) { count += x; } count").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn for_of_object_destructuring_lexical_binding_runs_once() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "let count = 0; for (let { x: [y], } of [{ x: [45] }]) { count += y; } count").unwrap();
    assert_num(result, 45.0);
}

#[test]
fn function_and_method_parameter_destructuring() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function f([a=4], {x}) { return a+x; } class C { m({y}) { return y; } } f([], {x:1}) + new C().m({y:5})",
    )
    .unwrap();
    assert_num(result, 10.0);
}

// Regression: a for-of whose assignment target has a NESTED rest pattern (the rest
// element is itself a pattern, e.g. `[...[x]]`) used to loop forever / error, because the
// counter undercounted the rest target's instructions vs the emitter, corrupting the loop's
// jump offsets. (Object rest with a nested target is a syntax error per spec, so untested.)
#[test]
fn for_of_assignment_nested_rest_target_runs_once() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var x; var c=0; for([...[x]] of [[1,2,3]]){ c=c+1; } c").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn for_of_assignment_nested_rest_empty_body_binds_inner() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var x; for([...[x]] of [[1,2,3]]){} x").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn for_of_assignment_nested_rest_with_leading_element() {
    let mut vm = Vm::new();
    // outer iterates once; a=1, rest=[2,3,4], then [x,y] from rest -> x=2, y=3
    let result = eval(&mut vm, "var a,x,y; for([a,...[x,y]] of [[1,2,3,4]]){} a*100+x*10+y").unwrap();
    assert_num(result, 123.0);
}

#[test]
fn standalone_nested_rest_assignment() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var x; [...[x]]=[1,2,3]; x").unwrap();
    assert_num(result, 1.0);
}
