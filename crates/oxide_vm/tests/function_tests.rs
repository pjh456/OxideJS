use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

// --- Function Declaration Basics ---

#[test]
fn fd_return_literal() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return 42; } f()").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn fd_return_void() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { 1; } f()").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn fd_hoisting() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "foo(); function foo() { return 1; }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn fd_with_params() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function add(a,b) { return a + b; } add(2, 3)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn fd_single_param() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function echo(x) { return x; } echo(99)").unwrap();
    assert_eq!(result.as_int(), 99);
}

// --- Function Expression ---

#[test]
fn fe_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var f = function() { return 99; }; f()").unwrap();
    assert_eq!(result.as_int(), 99);
}

#[test]
fn fe_with_params() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var mul = function(x,y) { return x * y; }; mul(6, 7)").unwrap();
    assert!((result.as_double() - 42.0).abs() < 0.0001);
}

// --- Cross-function calls ---

#[test]
fn cross_func_call() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function a() { return 1; } function b() { return a() + 2; } b()").unwrap();
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}

#[test]
fn two_funcs_called_from_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function a() { return 1; } function b() { return 2; } a() + b()").unwrap();
    assert!(result.is_double(), "expected double, got: {:?}", result);
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}

#[test]
fn call_preserves_previous_call_result_register() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function g() { return 10; } function h() { return 20; } function f() { return g() + h(); } f()",
    )
    .unwrap();
    assert!((result.as_double() - 30.0).abs() < 0.0001);
}

#[test]
fn call_preserves_local_across_nested_bytecode_call() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function g() { return 10; } function h() { return 20; } function f() { var x = g(); return x + h(); } f()",
    )
    .unwrap();
    assert!((result.as_double() - 30.0).abs() < 0.0001);
}

// --- Builtins inside functions ---

#[test]
fn fd_calls_builtin_return() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return Math.abs(-5); } f()").unwrap();
    assert!(result.is_double(), "expected double, got: {:?}", result);
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn fd_calls_builtin_with_param() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x) { return Math.abs(x); } f(-10)").unwrap();
    assert!(result.is_double(), "expected double, got: {:?}", result);
    assert!((result.as_double() - 10.0).abs() < 0.0001);
}

#[test]
fn fd_calls_builtin_two_args() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x) { return Math.pow(x, 2); } f(3)").unwrap();
    assert!(result.is_double(), "expected double, got: {:?}", result);
    assert!((result.as_double() - 9.0).abs() < 0.0001);
}

// --- Multiple FDs, first FD calls second ---

#[test]
fn fd_chain_call() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function a() { return 10; } function b() { return a(); } b()").unwrap();
    assert_eq!(result.as_int(), 10);
}

// --- new Xxx() in function ---

#[test]
fn fd_returns_new_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return new Object(); } f()").unwrap();
    assert!(result.is_object(), "expected object, got: {:?}", result);
}

#[test]
fn fd_returns_new_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return new Array(3); } f()").unwrap();
    assert!(result.is_object());
}

// --- this expression ---

#[test]
fn this_in_function_reads_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return this; } f()").unwrap();
    assert!(result.is_undefined(), "expected undefined in strict-mode call, got: {:?}", result);
}

#[test]
fn this_in_function_assign_member() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "function f(m) { this.message = m; } f('hello'); 1").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn this_member_access() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "function f() { this.x = 42; return this.x; } f()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn member_call_uses_receiver_as_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o = { x: 42, f: function() { return this.x; } }; o.f()").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn this_in_constructor_sets_proto() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function Ctor(m) { this.message = m; } Ctor.prototype = new Object(); 1").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn function_declaration_has_default_prototype_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function Ctor() {} Ctor.prototype.constructor === Ctor").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn function_default_prototype_accepts_member_assignment() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function Test262Error(message) { this.message = message || ''; } Test262Error.prototype.toString = function () { return this.message; }; new Test262Error('ok').toString()",
    )
    .unwrap();
    let rendered = vm
        .kernel_core()
        .string_forge()
        .lookup(result.as_string_index())
        .unwrap_or_default();
    assert_eq!(rendered, "ok");
}
