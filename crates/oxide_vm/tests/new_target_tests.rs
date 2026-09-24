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

fn assert_bool(value: JsValue, expected: bool) {
    assert!(value.is_bool(), "expected boolean, got {value:?}");
    assert_eq!(value.as_bool(), expected);
}

#[test]
fn function_call_new_target_is_undefined() {
    // 普通调用不传构造目标：`new.target` 应为 undefined。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return new.target === undefined; } f()").unwrap();
    assert_bool(result, true);
}

#[test]
fn construct_new_target_is_constructor() {
    // `new f()` 以 f 为构造目标：`new.target` 应为 f 本身。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.t = new.target === f; } new f().t").unwrap();
    assert_bool(result, true);
}

#[test]
fn class_constructor_new_target_is_class() {
    // 类构造器内 `new.target` 应为被 new 的类自身。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(){ this.t = new.target; } } new A().t === A").unwrap();
    assert_bool(result, true);
}

#[test]
fn derived_class_new_target_is_derived_constructor() {
    // 子类构造经 super() 链路：基类构造器内 `new.target` 仍是派生类（new 表达式目标）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(){ this.t = new.target; } } class B extends A {} new B().t === B",
    )
    .unwrap();
    assert_bool(result, true);
}

#[test]
fn nested_function_new_target_is_undefined() {
    // 内层普通函数是独立调用帧，不继承外层构造目标的 `new.target`。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function(){ var f = function(){ return new.target; }; return f() === undefined; })()",
    )
    .unwrap();
    assert_bool(result, true);
}

#[test]
fn typeof_and_void_new_target() {
    // unary 表达式组合：构造调用下 `typeof new.target` 为 "function"，普通调用下为 "undefined"。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return typeof new.target; } f()").unwrap();
    assert_eq!(vm.lookup_str(result).expect("typeof should be string"), "undefined");

    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.t = typeof new.target; } new f().t").unwrap();
    assert_eq!(vm.lookup_str(result).expect("typeof should be string"), "function");

    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.u = void new.target; } new f().u").unwrap();
    assert!(result.is_undefined());
}

#[test]
#[ignore = "已知规范缺口：箭头函数 new.target 词法继承未实现（VM 对 arrow 帧写 undefined），修复（闭包捕获）后移除"]
fn arrow_function_inherits_outer_new_target() {
    // 规范要求箭头函数词法继承外层 `new.target`；当前 VM 对 arrow 帧直接写
    // undefined（普通 CALL 路径传 undefined 为 new.target），无词法捕获——
    // 已知规范缺口，登记不修；修复（闭包捕获）后移除 #[ignore]。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ return (() => new.target)(); } new f() === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn import_meta_is_explicit_error() {
    // `import.meta` 需模块命名空间对象：显式编译报错（含 "not supported" 供
    // test262 分类 Skip），不留静默错误结果。
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, "import.meta").map_err(|e| format!("Parse error: {:?}", e));
    let program = program.expect("import.meta should parse in module context");
    let err = match Compiler::new().compile(&program) {
        Ok(_) => panic!("import.meta must not compile"),
        Err(e) => e,
    };
    assert!(err.contains("import.meta not yet supported"), "unexpected compile error: {err}");
}

#[test]
fn native_construct_does_not_pollute_new_target() {
    // native 构造（非 spread）不污染调用方 new.target：`new Date()` 后构造器
    // 返回 `new.target` 应仍为外层构造目标 f（255 槽随调用恢复，返回 f 自身）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); return new.target; } (new f()) === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_does_not_pollute_this() {
    // native 构造后同帧 this 保持外层构造的新对象：receiver 槽随调用保存/恢复（254 同修）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); this.t = this instanceof f; } new f().t").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_spread_does_not_pollute_new_target() {
    // spread 构造变体与普通构造一致：`new Date(...[])` 后 new.target 不被污染。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(...[]); return new.target; } (new f()) === f").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_then_plain_call_new_target_is_undefined() {
    // 普通调用帧内 native 构造后，new.target 仍为 undefined（帧语义不受构造调用干扰）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ new Date(); return new.target === undefined; } f()").unwrap();
    assert_bool(result, true);
}

#[test]
fn native_construct_keeps_instance_semantics() {
    // 收口 call_function_sync 后 native 构造语义不变：返回真实实例且原型链正确。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "(function(){ var d = new Date(0); return d instanceof Date; })()").unwrap();
    assert_bool(result, true);
}

// ── GpFC 通用构造面（读体按构造器种类分支 + 构造体自重读）回归钉 ──

fn throwing_proto_ctor(source_tail: &str) -> String {
    format!(
        "var bound = (function(){{}}).bind(); \
         Object.defineProperty(bound, 'prototype', {{ get: function() {{ \
         calls++; throw new Error('boom'); }} }}); \
         var calls = 0; {source_tail}"
    )
}

#[test]
fn gpfc_new_expression_bytecode_ctor_propagates_getter_throw() {
    // 字节码构造器 prototype 为访问器且 getter 抛错：new 表达式须原值上抛（getter 触发）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var F = function(){}; var calls = 0; \
         Object.defineProperty(F, 'prototype', { configurable: true, \
         get: function() { calls++; throw new Error('boom'); } }); \
         try { new F(); } catch (e) { e.message + ':' + calls }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom:1");
}

#[test]
fn gpfc_new_expression_spread_bytecode_ctor_propagates_getter_throw() {
    // spread 构造变体同形：getter 抛原值上抛且恰触发一次。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var F = function(){}; var calls = 0; \
         Object.defineProperty(F, 'prototype', { configurable: true, \
         get: function() { calls++; throw new Error('boom'); } }); \
         try { new F(...[]); } catch (e) { e.message + ':' + calls }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom:1");
}

#[test]
fn gpfc_reflect_construct_bytecode_ctor_bound_nt_propagates_getter_throw() {
    // Reflect.construct(JS 构造器, 实参, bound NT)：bound 自身 prototype 访问器
    // getter 抛错须原值上抛（读体对字节码构造器为唯一 GpFC 读点）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        &throwing_proto_ctor("try { Reflect.construct(function(){}, [], bound); } catch (e) { e.message }"),
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom");
}

#[test]
fn gpfc_promise_executor_check_precedes_proto_read() {
    // Promise 体步序：executor 可读性检查先于 GpFC 原型读（非 callable executor
    // + 抛 getter 的 NT → TypeError 胜 Test262Error 形）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        &throwing_proto_ctor("try { Reflect.construct(Promise, [], bound); } catch (e) { e.constructor.name }"),
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "TypeError");
}

#[test]
fn gpfc_promise_abrupt_proto_read_propagates() {
    // Promise 体 GpFC 自重读：executor callable 时 getter 抛原值上抛。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        &throwing_proto_ctor("try { Reflect.construct(Promise, [function(){}], bound); } catch (e) { e.message }"),
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom");
}

#[test]
fn promise_plain_call_rejected_by_construct_guard() {
    // NewTarget 缺失形态：普通调用与 .call 入口一律 TypeError（含 receiver 为
    // 真实 Promise 实例的 .call 面）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var r = []; \
         try { Promise(function(){}); } catch (e) { r.push(e.constructor.name); } \
         try { Promise.call(null, function(){}); } catch (e) { r.push(e.constructor.name); } \
         var p = new Promise(function(){}); \
         try { Promise.call(p, function(){}); } catch (e) { r.push(e.constructor.name); } \
         r.join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "TypeError,TypeError,TypeError");
}

#[test]
fn gpfc_disposable_stack_newtarget_proto_forms() {
    // DS 体 GpFC 自重读三形：custom NT → NT.prototype 直用；非对象 NT.prototype
    // → 回落 %DisposableStack.prototype%；抛 getter → 原值上抛且恰一次。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = Reflect.construct(DisposableStack, [], Object); \
         var a = Object.getPrototypeOf(s) === Object.prototype; \
         function nt() {} nt.prototype = undefined; \
         var s2 = Reflect.construct(DisposableStack, [], nt); \
         var b = Object.getPrototypeOf(s2) === DisposableStack.prototype; \
         a + ':' + b",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "true:true");

    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        &throwing_proto_ctor(
            "try { Reflect.construct(DisposableStack, [], bound); } catch (e) { e.message + ':' + calls }",
        ),
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom:1");
}

#[test]
fn gpfc_async_disposable_stack_newtarget_proto_forms() {
    // ADS 同形三钉：custom NT 直用 / 非对象回落内建原型 / 抛 getter 原值一次。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = Reflect.construct(AsyncDisposableStack, [], Object); \
         var a = Object.getPrototypeOf(s) === Object.prototype; \
         function nt() {} nt.prototype = null; \
         var s2 = Reflect.construct(AsyncDisposableStack, [], nt); \
         var b = Object.getPrototypeOf(s2) === AsyncDisposableStack.prototype; \
         a + ':' + b",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "true:true");

    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        &throwing_proto_ctor(
            "try { Reflect.construct(AsyncDisposableStack, [], bound); } catch (e) { e.message + ':' + calls }",
        ),
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "boom:1");
}

#[test]
fn gpfc_native_ctor_raw_read_unchanged() {
    // native 构造器臂回归：new Map()/new Set() 实例原型不变；Reflect.construct
    // 以数据属性 prototype 的 NT 构造 Map 时原型直用（native 臂保持裸读）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = Object.getPrototypeOf(new Map()) === Map.prototype; \
         var b = Object.getPrototypeOf(new Set()) === Set.prototype; \
         function nt() {} nt.prototype = Map.prototype; \
         var c = Object.getPrototypeOf(Reflect.construct(Map, [], nt)) === Map.prototype; \
         a + ':' + b + ':' + c",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "true:true:true");
}
