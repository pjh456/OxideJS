use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
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
fn function_apply_non_callable_this_throws_catchable_type_error() {
    // 不可调用 this（实例原型链含函数）经 apply 转发：抛可捕获 TypeError，
    // 不得绕过 try/catch 成为引擎级错误。
    let mut vm = Vm::new();
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    let name = eval(
        &mut vm,
        "function FACTORY(){} FACTORY.prototype = Function(); var o = new FACTORY(); try { o.apply(); } catch (e) { e.name }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(name).unwrap_or_default(), "TypeError");
}

#[test]
fn function_apply_non_object_argarray_throws_type_error() {
    // CreateListFromArrayLike：非对象 argArray（非 nullish）抛可捕获 TypeError；
    // 目标非可调用先抛（先于 argArray 检查）；null/undefined argArray 为无实参不抛。
    let mut vm = Vm::new();
    for src in [
        "try { function f(){} f.apply(null, true) } catch (e) { e.name }",
        "try { function f(){} f.apply(null, NaN) } catch (e) { e.name }",
        "try { function f(){} f.apply(null, '1,2,3') } catch (e) { e.name }",
        "try { function f(){} f.apply(null, Symbol()) } catch (e) { e.name }",
        "try { Function.prototype.apply.call(undefined, {}, true) } catch (e) { e.name }",
    ] {
        let name = eval(&mut vm, src).unwrap();
        assert_eq!(vm.lookup_str(name).unwrap_or_default(), "TypeError", "for {}", src);
    }
    let result = eval(&mut vm, "function f(){ return arguments.length; } f.apply(null, null)").unwrap();
    assert_num(result, 0.0);
    let result = eval(&mut vm, "function f(){ return arguments.length; } f.apply(null, undefined)").unwrap();
    assert_num(result, 0.0);
}

#[test]
fn function_call_non_callable_target_keeps_type_error_kind() {
    // call 转发到非可调用目标（原始值 thisArg）：内层递归无原值可恢复时，
    // 错误种类仍须保持 TypeError。
    let mut vm = Vm::new();
    for src in [
        "try { Function.prototype.call.call(undefined, {}) } catch (e) { e.name }",
        "try { Function.prototype.call.call(null, {}) } catch (e) { e.name }",
        "try { Function.prototype.call.call({}, {}) } catch (e) { e.name }",
        "try { Function.prototype.call.call(undefined) } catch (e) { e.name }",
    ] {
        let name = eval(&mut vm, src).unwrap();
        assert_eq!(vm.lookup_str(name).unwrap_or_default(), "TypeError", "for {}", src);
    }
}

#[test]
fn function_call_callable_native_target_still_works() {
    // 守卫不得误伤合法 native 目标：经 call/apply 转发 native 函数照常执行。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.call(null, 3, 9)").unwrap();
    assert_num(result, 9.0);
    let result = eval(&mut vm, "Math.max.apply(null, [3, 9])").unwrap();
    assert_num(result, 9.0);
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

#[test]
fn bound_function_newtarget_pins() {
    let mut vm = Vm::new();
    // Reflect.construct 以 bound 包装为构造器：new.target 逐层替换到最内层
    // target（SameValue(F, newTarget) → target），三种 newTarget 形态同值。
    for src in [
        "var nt; function A() { nt = new.target; } var B = A.bind(); var C = B.bind(); Reflect.construct(C, [], C); nt === A",
        "var nt; function A() { nt = new.target; } var B = A.bind(); var C = B.bind(); Reflect.construct(C, [], A); nt === A",
        "var nt; function A() { nt = new.target; } var B = A.bind(); var C = B.bind(); Reflect.construct(C, [], B); nt === A",
    ] {
        let result = eval(&mut vm, src).unwrap();
        assert!(result.as_bool(), "for {}", src);
    }
    // 构造实例原型链指向最内层 target 的 prototype。
    let result = eval(
        &mut vm,
        "function A() {} var B = A.bind(); var C = B.bind(); Object.getPrototypeOf(Reflect.construct(C, [], C)) === A.prototype",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn bound_function_length_name_pins() {
    let mut vm = Vm::new();
    // length：非自身 length（原型链 42 泄漏形态）→ 0；自身非 Number → 0；
    // 自身 Number 经 ToIntegerOrInfinity 减绑定实参数。
    let result = eval(
        &mut vm,
        "function bar() {} Object.setPrototypeOf(bar, {length: 42}); delete bar.length; Function.prototype.bind.call(bar, null, 1).length",
    )
    .unwrap();
    assert_num(result, 0.0);
    let result = eval(
        &mut vm,
        "function t() {} Object.defineProperty(t, 'length', {value: '5'}); t.bind(null, 1).length",
    )
    .unwrap();
    assert_num(result, 0.0);
    let result = eval(
        &mut vm,
        "function t() {} Object.defineProperty(t, 'length', {value: 3.66}); t.bind().length",
    )
    .unwrap();
    assert_num(result, 3.0);
    let result = eval(
        &mut vm,
        "function t() {} Object.defineProperty(t, 'length', {value: Infinity}); t.bind().length",
    )
    .unwrap();
    assert!(result.as_double() == f64::INFINITY);
    // name：getter 抛错须原值传播；chained 读 bound 包装自身 name。
    let result = eval(
        &mut vm,
        "var threw = false; try { Object.defineProperty(function(){}, 'name', {get: function() { throw new Error('x'); }}).bind(); } catch (e) { threw = e.message === 'x'; } threw",
    )
    .unwrap();
    assert!(result.as_bool());
    let result = eval(&mut vm, "function target() {} target.bind().name").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "bound target");
    let result = eval(&mut vm, "function target() {} target.bind().bind().name").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "bound bound target");
}

/// 执行源码并取顶层结果；Promise 结果 drain 到 settled 值（口径与 async_tests.rs 一致）。
fn eval_drain(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    let result = eval(vm, source)?;
    if result.is_object() {
        let obj = unsafe { &*result.as_js_object_ptr() };
        if obj.is_promise_obj() {
            match oxide_vm::promise::promise_settled_value(obj) {
                Some((true, v)) => return Ok(v),
                Some((false, v)) => return Err(format!("rejected: {v}")),
                None => return Err("promise pending".to_string()),
            }
        }
    }
    Ok(result)
}

#[test]
fn function_call_apply_bind_async_target_returns_promise() {
    // call/apply/bind 作用于异步目标：结果是 Promise，drain 后的值与直接调用一致。
    let mut vm = Vm::new();
    let result = eval_drain(&mut vm, "async function f(x){ return x; } f.call(null, 1)").unwrap();
    assert_eq!(result, JsValue::int(1));
    let result = eval_drain(&mut vm, "async function f(x){ return x; } f.apply(null, [2])").unwrap();
    assert_eq!(result, JsValue::int(2));
    let result = eval_drain(&mut vm, "async function f(x){ return x; } var b = f.bind(null); b(3)").unwrap();
    assert_eq!(result, JsValue::int(3));
}

#[test]
fn function_call_async_target_rejection_keeps_value() {
    // 异步目标抛错：Promise 以原值拒绝，.then 的拒绝臂收到原值。
    let mut vm = Vm::new();
    let result = eval_drain(
        &mut vm,
        "async function f(){ throw 42; } f.call(null).then(v => 'ok:'+v, e => 'err:'+e)",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "err:42");
}

#[test]
fn function_call_bind_generator_target_returns_iterator() {
    // call/bind 作用于同步生成器目标：返回迭代器对象（next() 结果对象，非函数体返回值）。
    let mut vm = Vm::new();
    // call 结果是迭代器对象，不是函数体返回值 99。
    let result = eval(&mut vm, "function* g(){ return 99; } typeof g.call(null)").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "object");
    // 迭代器首个 next() 返回 done: true（生成器未 yield 即 return）。
    let result = eval(&mut vm, "function* g(){ return 99; } g.call(null).next().done").unwrap();
    assert!(result.as_bool());
    // bind 作用于生成器目标：包装器调用同样返回迭代器对象。
    let result = eval(&mut vm, "function* g(){ return 99; } typeof g.bind(null)()").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "object");
}

#[test]
fn function_bind_async_generator_target_returns_iterator() {
    // bind 作用于异步生成器目标：返回异步生成器迭代器对象。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "async function* g(){ return 99; } typeof g.bind(null)()").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "object");
}

#[test]
fn function_call_generator_param_default_throw_keeps_catch_value() {
    // 生成器目标带抛错的参数默认值，经 call/apply/bind 调用且调用点在 try/catch 内：
    // catch 参数须是抛出的原值 42，不得被占位迭代器对象覆盖。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function* g(x = (function(){ throw 42; })()) { yield 1; } try { g.call(null); } catch (e) { e; }",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(42));
    let result = eval(
        &mut vm,
        "function* g(x = (function(){ throw 42; })()) { yield 1; } try { g.apply(null, []); } catch (e) { e; }",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(42));
    let result = eval(
        &mut vm,
        "function* g(x = (function(){ throw 42; })()) { yield 1; } try { g.bind(null)(); } catch (e) { e; }",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(42));
}
