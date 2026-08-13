use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

/// 数值断言：整数运算保 int，容忍 int/double 两种表示。
fn assert_num(result: JsValue, expected: f64) {
    let actual = if result.is_int() { result.as_int() as f64 } else { result.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

#[test]
fn function_call_changes_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.call(null, 10, 5)").unwrap();
    assert!((result.as_double() - 10.0).abs() < 0.0001);
}

#[test]
fn function_apply_changes_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.apply(null, [10, 5])").unwrap();
    assert!((result.as_double() - 10.0).abs() < 0.0001);
}

#[test]
fn function_bind_creates_wrapper() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var b = Math.max.bind(null, 1); b(5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn function_constructor_is_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Function").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function");
}

#[test]
fn function_call_bind_supports_uncurried_native_methods() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var __push = Function.prototype.call.bind(Array.prototype.push); var a = []; __push(a, 'x'); a.length",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(1));
}

#[test]
fn function_call_invokes_bytecode_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function add(a, b) { return a + b; } add.call(null, 2, 3)").unwrap();
    assert_num(result, 5.0);
}

#[test]
fn function_apply_invokes_bytecode_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function add(a, b) { return a + b; } add.apply(null, [2, 3])").unwrap();
    assert_num(result, 5.0);
}

#[test]
fn function_bind_invokes_bytecode_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function add1(a) { return a + 1; } var bound = add1.bind(null); bound(2)").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn function_call_preserves_bytecode_throw_kind() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "function fail() { throw new TypeError('boom'); } fail.call(null)").unwrap_err();
    assert!(err.contains("uncaught TypeError: boom"), "got: {err}");
}

#[test]
fn function_to_string_includes_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.toString()").unwrap();
    assert!(result.is_string());
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function max() { [native code] }");
}

#[test]
fn function_to_string_non_function_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "Function.prototype.toString.call(1)").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
}

#[test]
fn function_call_returns_object_stays_valid() {
    // 回归：call_function_sync 曾在一个带独立 epoch 的子 VM 中运行字节码；
    // 子 VM epoch 中分配的对象在子 VM 销毁后成为悬垂指针。
    // 本测试强制在调用返回后解引用返回对象。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return {x: 42}; } f.call(null).x").unwrap();
    assert_eq!(result, JsValue::int(42));
}

#[test]
fn function_apply_returns_object_stays_valid() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(a) { return {v: a + 1}; } f.apply(null, [9]).v").unwrap();
    assert_num(result, 10.0);
}

#[test]
fn getter_returns_object_stays_valid() {
    // 同类 bug：字节码 accessor 经 ordinary_get 同步路径（target_reg=None）在子 VM
    // 中运行，返回对象在子 VM 销毁后被释放。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o = { get p() { return {z: 7}; } }; o.p.z").unwrap();
    assert_eq!(result, JsValue::int(7));
}
