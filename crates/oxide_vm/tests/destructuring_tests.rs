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

#[test]
fn destructured_callback_parameters_shadow_outer_bindings() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "let expected=100, result=0; [[1,2]].forEach(([value, expected]) => { result=value+expected; }); result",
    )
    .unwrap();
    assert_num(result, 3.0);
}

// 回归：赋值目标含嵌套 rest 模式（rest 元素本身是模式，如 `[...[x]]`）的 for-of
// 曾无限循环 / 报错，因为计数器相对 emitter 少计了 rest 目标的指令，破坏了循环跳转
// 偏移。（对象 rest 嵌套目标按规范是语法错误，故不测。）
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
    // 外层迭代一次；a=1，rest=[2,3,4]，再从 rest 解构 [x,y] -> x=2, y=3
    let result = eval(&mut vm, "var a,x,y; for([a,...[x,y]] of [[1,2,3,4]]){} a*100+x*10+y").unwrap();
    assert_num(result, 123.0);
}

#[test]
fn standalone_nested_rest_assignment() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var x; [...[x]]=[1,2,3]; x").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn object_rest_excludes_numeric_bound_key_from_array_source() {
    // 数组元素区整数键：pattern 绑定 "0" 后 rest 不得再包含 "0"。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const {0:x, ...r} = [1,2,3]; Object.keys(r).length").unwrap();
    assert_eq!(result.as_int(), 2);
    let result = eval(&mut vm, "const {0:x, ...r} = [1,2,3]; x === 1").unwrap();
    assert!(result.as_bool());
    // rest 从 1 开始：r[1] 是源元素 2，r[0] 已被排除为 undefined。
    let result = eval(&mut vm, "const {0:x, ...r} = [1,2,3]; r[1] + r[2]").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn object_rest_excludes_multiple_numeric_keys() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const {0:x, 1:y, ...r} = [1,2,3,4]; Object.keys(r).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.as_str().to_string();
    assert_eq!(s, "2,3");
}

#[test]
fn object_rest_keeps_numeric_key_from_shape_source() {
    // 对象源数字键（shape 链整数键）同样排除。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const {0:x, ...r} = {0:'a',1:'b',2:'c'}; Object.keys(r).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.as_str().to_string();
    assert_eq!(s, "1,2");
}
