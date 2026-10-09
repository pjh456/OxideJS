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

/// 数值断言：整数运算保 int，容忍 int/double 两种表示。
fn assert_num(result: JsValue, expected: f64) {
    let actual = if result.is_int() { result.as_int() as f64 } else { result.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

/// 判断值是否为函数对象：经对象指针解出 header 函数标志，非对象返回假。
fn is_function_value(v: JsValue) -> bool {
    let ptr = v.as_js_object_ptr();
    if ptr.is_null() {
        return false;
    }
    unsafe { (*ptr).is_function() }
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
    assert_num(result, 5.0);
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
    assert_num(result, 3.0);
}

#[test]
fn two_funcs_called_from_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function a() { return 1; } function b() { return 2; } a() + b()").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn call_preserves_previous_call_result_register() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function g() { return 10; } function h() { return 20; } function f() { return g() + h(); } f()",
    )
    .unwrap();
    assert_num(result, 30.0);
}

#[test]
fn call_preserves_local_across_nested_bytecode_call() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function g() { return 10; } function h() { return 20; } function f() { var x = g(); return x + h(); } f()",
    )
    .unwrap();
    assert_num(result, 30.0);
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

// ── 多个函数声明，首个调用第二个 ──

#[test]
fn fd_chain_call() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function a() { return 10; } function b() { return a(); } b()").unwrap();
    assert_eq!(result.as_int(), 10);
}

// ── 函数内的 new Xxx() ──

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

// ── this 表达式 ──

#[test]
fn this_in_function_reads_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f() { return this; } f()").unwrap();
    // sloppy 普通调用 this 绑定全局对象（ECMA-262 10.4.3）。
    let global = vm.session().global_object().as_ptr() as *mut oxide_types::object::JsObject as usize;
    assert_eq!(result.as_js_object_ptr() as usize, global, "expected global object, got: {:?}", result);
}

#[test]
fn this_in_function_assign_member() {
    let mut vm = Vm::new();
    // sloppy this 为全局对象：成员写入落到全局，不抛错。
    let result = eval(&mut vm, "function f(m) { this.message = m; } f('hello'); 1").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn this_member_access() {
    let mut vm = Vm::new();
    // sloppy this 为全局对象：读写的成员落在全局，返回写入值。
    let result = eval(&mut vm, "function f() { this.x = 42; return this.x; } f()").unwrap();
    assert_eq!(result.as_int(), 42);
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
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "ok");
}

// ── 函数名不可写局部绑定（函数作用域内读回函数对象）──

#[test]
fn fn_name_binding_readable() {
    let mut vm = Vm::new();
    // 函数名登记进函数作用域，体内裸读 f 解析到名绑定，读回函数对象本身。
    let result = eval(&mut vm, "function f() { return f; } f()").unwrap();
    assert!(is_function_value(result), "expected the function itself, got: {:?}", result);
}

#[test]
fn fn_name_binding_shadows_outer_var() {
    let mut vm = Vm::new();
    // 外层 var f 持 'x'；函数表达式名绑定 f 在函数作用域内优先，体内读回函数对象而非 'x'。
    let result = eval(&mut vm, "var f = 'x'; var g = function f() { return typeof f; }; g()").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "function");
}

#[test]
fn fn_name_var_suppression() {
    let mut vm = Vm::new();
    // 体内容器 var f 同名：不建不可写绑定，var 绑定由帧槽写入初始化为函数对象。
    let result = eval(&mut vm, "function f() { var f; return f; } f()").unwrap();
    assert!(is_function_value(result), "expected the function, got: {:?}", result);
}

#[test]
fn fn_name_param_priority() {
    let mut vm = Vm::new();
    // 形参同名：形参绑定持有实参，名绑定被抑制，读回实参值。
    let result = eval(&mut vm, "function f(f) { return f; } f(1)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn fn_name_capture_side() {
    let mut vm = Vm::new();
    // 嵌套函数捕获外层名绑定（upvalue），内层读回外层函数对象。
    let result = eval(&mut vm, "function f() { return function() { return f; }; } f()()").unwrap();
    assert!(is_function_value(result), "expected the outer function, got: {:?}", result);
}

#[test]
fn fn_name_arguments_special_case() {
    let mut vm = Vm::new();
    // 名等于 arguments 且 arguments 对象已建：抑制名绑定，arguments 解析到 arguments 对象。
    let result = eval(&mut vm, "function arguments() { return arguments; } arguments()").unwrap();
    assert!(result.is_object(), "expected the arguments object, got: {:?}", result);
}

#[test]
fn fn_name_recursive_call() {
    let mut vm = Vm::new();
    // 递归调用依赖名绑定读回函数对象再调用，覆盖寄存器写回回归面。
    let result = eval(&mut vm, "function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); } fact(5)").unwrap();
    assert_num(result, 120.0);
}

// --- 默认参数自引用 TDZ 守卫 ---

/// 断言运行期抛出未捕获 ReferenceError（默认参数 TDZ 守卫的期望形态）。
fn assert_dflt_tdz_reference_error(source: &str, msg: &str) {
    let err = eval(&mut Vm::new(), source).unwrap_err();
    assert!(err.contains("ReferenceError"), "{msg}: expected an uncaught ReferenceError, got: {err}");
}

#[test]
fn dflt_param_self_ref_throws() {
    // 默认值自引用当前形参：形参环境未初始化绑定，运行期抛 ReferenceError。
    assert_dflt_tdz_reference_error("function f(x = x) {} f()", "self-ref");
}

#[test]
fn dflt_param_ref_later_throws() {
    // 默认值引用后位形参：后位形参在默认值求值期尚未初始化，抛 ReferenceError。
    assert_dflt_tdz_reference_error("function f(x = y, y) {} f()", "ref-later");
}

#[test]
fn dflt_param_ref_prior_legal() {
    // 默认值引用前位形参：前位形参已初始化，读取合法，y 得 x 的值。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x, y = x) { return y; } f(3)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn dflt_param_arg_defined_no_eval() {
    // 实参非 undefined：默认值不求值，自引用不抛。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x = x) { return x; } f(1)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn dflt_param_object_dstr_self_ref_throws() {
    // 对象解构自引用抛 ReferenceError。
    assert_dflt_tdz_reference_error("function f({x = x}) {} f({})", "object-dstr-self");
}

#[test]
fn dflt_param_arguments_not_in_tdz() {
    // arguments 不在 TDZ 集：默认值引用 arguments 合法。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x = arguments[1]) { return x; } f(undefined, 2)").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn dflt_param_nested_fn_no_throw() {
    // 默认值为嵌套函数：嵌套函数体在子 ctx 编译，经 upvalue 读初始化后的值，不抛。
    // 闭包调用后读回 x 为默认值函数对象本身（非 undefined）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(x = function() { return x; }) { return x; } f()()").unwrap();
    assert!(is_function_value(result), "expected the default function, got: {:?}", result);
}

#[test]
fn dflt_param_typeof_self_ref_throws() {
    // typeof 对 TDZ 绑定抛 ReferenceError（经 emit_expression 同形）。
    assert_dflt_tdz_reference_error("function f(x = typeof x) {} f()", "typeof-self");
}

#[test]
fn dflt_param_generator_self_ref_throws() {
    // 生成器默认值自引用抛 ReferenceError（参数初始化在调用时刻）。
    assert_dflt_tdz_reference_error("function* g(x = x) {} g()", "generator-self");
}

#[test]
fn dflt_param_strict_self_ref_throws() {
    // 严格模式默认值自引用同样抛 ReferenceError（TDZ 与模式无关）。
    assert_dflt_tdz_reference_error("'use strict'; function f(x = x) {} f()", "strict-self");
}
