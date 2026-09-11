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
fn function_apply_passes_large_arg_array_to_bytecode_target() {
    // 回归：apply 曾把实参数组静默截断到 55 个；大参数集必须完整到达目标函数。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = []; for (var i = 0; i < 1000; i++) { a[i] = i; } (function(){ return arguments.length; }).apply(null, a)",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(1000));
}

#[test]
fn function_apply_large_args_reach_last_element() {
    // 大参数集经 spill 溢出区送达后，末位实参可被目标读取（不止计数正确）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = []; for (var i = 0; i < 300; i++) { a[i] = i; } (function(){ return arguments[299]; }).apply(null, a)",
    )
    .unwrap();
    assert_num(result, 299.0);
}

#[test]
fn function_apply_large_args_to_native_from_code_point() {
    // native 目标（String.fromCodePoint）接收大实参集：整串完整拼接，验证
    // harness `String.fromCodePoint.apply(null, codePoints)` 不再截断。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = []; for (var i = 0; i < 1000; i++) { a[i] = 65 + (i % 26); } String.fromCodePoint.apply(null, a).length",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(1000));
}

#[test]
fn getter_returns_object_stays_valid() {
    // 同类 bug：字节码 accessor 经 ordinary_get 同步路径（target_reg=None）在子 VM
    // 中运行，返回对象在子 VM 销毁后被释放。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o = { get p() { return {z: 7}; } }; o.p.z").unwrap();
    assert_eq!(result, JsValue::int(7));
}

#[test]
fn function_symbol_has_instance_bound() {
    let mut vm = Vm::new();
    // @@hasInstance 已绑定在 Function.prototype 上且可调用。
    let result = eval(&mut vm, "typeof Function.prototype[Symbol.hasInstance]").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "function");
    // 非对象 / 非可调用 this → false（不抛）。
    let cases = [
        ("Function.prototype[Symbol.hasInstance].call(42, {})", false),
        ("Function.prototype[Symbol.hasInstance].call({}, {})", false),
        ("Function.prototype[Symbol.hasInstance].call()", false),
        // 左操作数非对象 → false。
        ("(function(){}).constructor[Symbol.hasInstance](42)", false),
        // 原型链命中 / 未命中。
        ("var f = function(){}; var o = new f(); f[Symbol.hasInstance](o)", true),
        ("var f = function(){}; f[Symbol.hasInstance]({})", false),
        ("var f = function(){}; var o = Object.create(new f()); f[Symbol.hasInstance](o)", true),
        // bound 递归到 target。
        ("var BC = function(){}; var bc = new BC(); BC.bind()[Symbol.hasInstance](bc)", true),
        ("function C(){} C.bind(null)[Symbol.hasInstance]({})", false),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn function_symbol_has_instance_poisoned_prototype_throws() {
    let mut vm = Vm::new();
    // 可调用但 prototype 非对象 → TypeError（OrdinaryHasInstance 唯一抛错点）。
    let err = eval(
        &mut vm,
        "var f = function(){}; f.prototype = 1; try { f[Symbol.hasInstance]({}) } catch (e) { e }",
    )
    .unwrap();
    let name = eval(
        &mut vm,
        "var f = function(){}; f.prototype = null; try { f[Symbol.hasInstance]({}) } catch (e) { e.name }",
    )
    .unwrap();
    let _ = err;
    assert_eq!(vm.lookup_str(name).unwrap_or_default(), "TypeError");
}

#[test]
fn bound_function_construct_semantics() {
    let mut vm = Vm::new();
    let num_cases = [
        // 绑定实参 + 新对象 this。
        ("function C(v){ this.v = v; } var B = C.bind({}, 1); new B().v", 1.0),
        ("function C(v){ this.v = v; } var B = C.bind({}, 1); new B(2).v", 1.0),
        ("function C(a, b){ this.s = a + b; } var B = C.bind(null, 2); new B(3).s", 5.0),
        // 多层 bound 链：绑定实参按 内层先、外层后 拼接。
        (
            "function C(v){ this.v = v; } var B = C.bind({}, 1); var D = B.bind({}, 2); new D().v",
            1.0,
        ),
        // native 构造器 target。
        ("var arr = Array.bind(null); new arr(3).length", 3.0),
    ];
    for (src, expected) in num_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_num(result, expected);
    }
    let bool_cases = [
        // 构造实例原型链指向 target.prototype（new.target 替换为 target）。
        ("function C(){} var B = C.bind({}); new B() instanceof C", true),
        // instanceof bound 递归到 target：newB 链含 C.prototype，故对 B 也为 true。
        ("function C(){} var B = C.bind({}); new B() instanceof B", true),
        ("function C(){} var B = C.bind({}); B instanceof Function", true),
        // native 构造器 target。
        ("var arr = Array.bind(null); new arr(3) instanceof Array", true),
    ];
    for (src, expected) in bool_cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn bound_function_construct_errors() {
    let mut vm = Vm::new();
    // 不可构造 target（arrow / native 方法）经 bound 构造 → TypeError。
    let err = eval(&mut vm, "var a = (()=>{}).bind(null); try { new a() } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(err).unwrap_or_default(), "TypeError");
    let err = eval(&mut vm, "var m = Math.max.bind(null); try { new m() } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(err).unwrap_or_default(), "TypeError");
    // 派生类构造器经 bound 构造：super() 装配 this，实例属派生类。
    let result = eval(
        &mut vm,
        "class A { constructor(v){ this.v = v; } } class D extends A { constructor(){ super(9); } } var B = D.bind({}); new B().v",
    )
    .unwrap();
    assert_num(result, 9.0);
}
