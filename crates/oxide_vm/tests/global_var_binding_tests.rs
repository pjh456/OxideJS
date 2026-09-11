//! 脚本顶层 var/function 声明落 globalThis 的运行时行为测试。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval_truthy(source: &str) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}\nsource: {source}");
}

fn eval_string(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).unwrap_or_default()
}

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

/// 两阶段执行：phase1 → `reset()`（轻量重置，epoch 清空、session 保留）→ phase2。
fn eval_two_phases(phase1: &str, phase2: &str) -> bool {
    let mut vm = Vm::new();
    vm.run(&Arc::new(compile(phase1))).expect("run1");
    vm.reset();
    let result = vm.run(&Arc::new(compile(phase2))).expect("run2");
    result.is_bool() && result.as_bool()
}

#[test]
fn top_level_var_lands_on_global_this() {
    eval_truthy("var x = 5; globalThis.x === 5");
    eval_truthy("var y = 10; typeof globalThis.y === 'number'");
}

#[test]
fn top_level_function_lands_on_global_this() {
    eval_truthy("function foo(){ return 1; } globalThis.foo() === 1");
    eval_truthy("function foo(){ return 1; } typeof globalThis.foo === 'function'");
}

#[test]
fn let_const_class_do_not_land_on_global_this() {
    eval_truthy("let z = 5; typeof globalThis.z === 'undefined'");
    eval_truthy("const c = 1; typeof globalThis.c === 'undefined'");
    eval_truthy("class C {} typeof globalThis.C === 'undefined'");
}

#[test]
fn function_local_var_does_not_land_on_global_this() {
    eval_truthy("function f(){ var v = 1; } f(); typeof globalThis.v === 'undefined'");
}

#[test]
fn block_level_var_lands_on_global_this() {
    eval_truthy("{ var b = 2; } globalThis.b === 2");
}

#[test]
fn redeclaration_updates_global_property() {
    eval_truthy("var x = 1; var x = 2; globalThis.x === 2");
}

#[test]
fn var_global_property_is_non_configurable() {
    eval_truthy("var x = 1; Object.getOwnPropertyDescriptor(globalThis, 'x').configurable === false");
    eval_truthy("var x = 1; Object.getOwnPropertyDescriptor(globalThis, 'x').enumerable === true");
    eval_truthy("var x = 1; delete globalThis.x === false");
    eval_truthy("var x = 1; delete globalThis.x; typeof globalThis.x === 'number'");
}

#[test]
fn var_name_shows_in_get_own_property_names() {
    eval_truthy("var x = 1; Object.getOwnPropertyNames(globalThis).indexOf('x') >= 0");
    eval_truthy("function foo(){} Object.getOwnPropertyNames(globalThis).indexOf('foo') >= 0");
}

#[test]
fn function_declared_var_lands_after_user_assignment() {
    eval_truthy("globalThis.x = 99; var x = 5; globalThis.x === 5");
}

#[test]
fn async_harness_done_pattern() {
    // asyncHelpers.js 依赖顶层 function $DONE 落 globalThis。
    eval_truthy("function $DONE(){} Object.prototype.hasOwnProperty.call(globalThis, '$DONE')");
    eval_truthy("function $DONE(){} typeof globalThis.$DONE === 'function'");
}

#[test]
fn var_without_initializer_creates_undefined_property() {
    eval_truthy("var x; typeof globalThis.x === 'undefined'");
    eval_truthy("var x; Object.prototype.hasOwnProperty.call(globalThis, 'x')");
}

#[test]
fn function_internal_reference_still_works() {
    eval_truthy("function foo(){ return 1; } var r = foo(); r === 1");
}

#[test]
fn descriptor_attributes_match_spec() {
    let d = eval_string("var x = 1; JSON.stringify(Object.getOwnPropertyDescriptor(globalThis, 'x'))");
    assert_eq!(d, r#"{"value":1,"writable":true,"enumerable":true,"configurable":false}"#);
}

#[test]
fn var_property_exists_before_declaration_statement() {
    // 全局声明实例化：属性在脚本求值开始即创建，声明语句执行前的
    // 反射/读取即可见绑定（值 undefined、脚本模式 configurable:false）。
    // 断言值经临时全局承载——尾部 var 声明语句会覆盖程序完成值。
    eval_truthy(
        "(function(){ seen = Object.prototype.hasOwnProperty.call(globalThis, 'gv'); })(); var gv; seen === true",
    );
    eval_truthy("(function(){ seen = (typeof gv === 'undefined'); })(); var gv; seen === true");
    eval_truthy("(function(){ var p = Object.getOwnPropertyDescriptor(globalThis, 'gv'); seen = (p.configurable === false && p.value === undefined); })(); var gv; seen === true");
}

#[test]
fn var_no_init_preserves_prior_write() {
    eval_truthy("x = 1; var x; globalThis.x === 1");
    eval_truthy("x = 1; var x; x === 1");
}

#[test]
fn var_no_init_preserves_prior_write_captured() {
    // 被捕获 var 的值在 cell：声明语句从 cell 同步全局属性，不抹掉先前写入。
    eval_truthy("x = 1; var x; (function(){ return x; })() === 1");
    eval_truthy("x = 1; var x; globalThis.x === 1");
}

#[test]
fn var_self_reference_reads_binding_value() {
    eval_truthy("var x = x; typeof x === 'undefined'");
    eval_truthy("x = 7; var x = x; x === 7");
}

#[test]
fn function_callable_before_its_declaration() {
    eval_truthy("(function(){ return f() === 42; })(); function f(){ return 42; }");
}

#[test]
fn var_no_init_does_not_clobber_function_binding() {
    eval_truthy("function f(){ return 1; } var f; typeof f === 'function' && f() === 1");
    // 无初始化 var 声明不触碰函数绑定：声明后全局属性仍为原函数对象，
    // 同一性与行为同时保持。
    eval_truthy("function f(){ return 1; } var f; globalThis.f === f && globalThis.f() === 1");
}

/// 函数对象写全局不分裂：同一函数值经逃逸写屏障后严格相等保持 true，
/// 覆盖函数声明、var 初始化、赋值、箭头/生成器/async 函数各形态。
#[test]
fn function_written_to_global_keeps_identity() {
    eval_truthy("function f(){ return 1; } globalThis.f = f; globalThis.f === f");
    eval_truthy("var g = function(){}; globalThis.g = g; globalThis.g === g");
    eval_truthy("var h; h = function(){}; globalThis.h = h; globalThis.h === h");
    eval_truthy("var a = () => 42; globalThis.a = a; globalThis.a === a");
    eval_truthy("var ge = function*(){ yield 1; }; globalThis.ge = ge; globalThis.ge === ge");
    eval_truthy("var af = async function(){}; globalThis.af = af; globalThis.af === af");
}

/// prototype 子对象与函数本体同一：逃逸写后 `f.prototype === globalThis.f.prototype`，
/// 且经全局别名继续修改 prototype 对局部别名可见。
#[test]
fn function_prototype_identity_survives_global_write() {
    eval_truthy("function f(){} globalThis.f = f; globalThis.f.prototype === f.prototype");
    eval_truthy("function f(){} globalThis.f = f; globalThis.f.prototype.z = 5; globalThis.f.prototype.z === 5");
    // 手工替换 prototype：接收者经函数别名重读，函数目标写入不克隆。
    eval_truthy(
        "var f = function(){}; f.prototype = {}; f.prototype.m = function(){ return 3; }; f.prototype.m() === 3",
    );
    eval_truthy("var f = function(){}; globalThis.f = f; f.prototype.m = function(){ return 3; }; globalThis.f.prototype.m() === 3");
}

/// 类构造器形状：原型为运行时新建对象，构造器/方法/prototype 链接在逃逸写
/// 后不分裂——实例方法可调用、constructor 指回类、派生类 super 派发正常。
#[test]
fn class_constructor_shape_after_global_write() {
    eval_truthy("var cc = class { m(){ return 3; } }; cc.prototype.constructor === cc");
    eval_truthy("var cc = class { m(){ return 3; } }; new cc().m() === 3");
    eval_truthy("var cc = class { m(){ return 3; } }; typeof cc.prototype.m === 'function'");
    eval_truthy(
        "var Base = class { c(){ return 11; } }; \
         var D = class extends Base { d(){ return super.c(); } }; \
         new D().d() === 11",
    );
}

/// 嵌套闭包写全局：内层函数经外层调用写 globalThis，两侧同一函数对象。
#[test]
fn nested_closure_written_to_global_keeps_identity() {
    eval_truthy("var n = function(){ var m = function(){ return 7; }; globalThis.m = m; return m; }; var m1 = n(); globalThis.m === m1");
}

/// 同一函数写两个全局槽：两槽指向同一对象。
#[test]
fn same_function_written_to_two_globals_is_identical() {
    eval_truthy("function f(){} globalThis.f = f; globalThis.f2 = f; globalThis.f2 === globalThis.f");
}

/// 跨 run（reset）契约：函数对象直落 session，其属性持有的 epoch 对象
/// （prototype 槽）在 epoch 清空前被晋升，run2 经全局别名可读其属性。
/// 不跨 run 调用存活函数：sub_module_index 指向上一执行期的子模块表，
/// 跨执行调用是独立缺口（基线即失败，非本次修复范围）。
#[test]
fn global_function_survives_reset_with_promoted_property() {
    assert!(eval_two_phases(
        "var f = function(){}; globalThis.f = f; f.prototype = {}; f.prototype.q = 7; 0",
        "typeof globalThis.f === 'function' && globalThis.f.prototype.q === 7",
    ));
}

/// 类构造器跨 run：prototype 子对象（运行时 epoch 新建）经 reset 边界晋升，
/// run2 经全局别名读到方法属性。
#[test]
fn class_prototype_link_survives_reset() {
    assert!(eval_two_phases(
        "var cc = class { m(){ return 3; } }; globalThis.cc = cc; 0",
        "typeof globalThis.cc === 'function' && typeof globalThis.cc.prototype.m === 'function'",
    ));
}

/// Map 原生盒按键值直插不经写屏障：epoch 值跨 reset 由边界晋升克隆进
/// session。run2 先分配新 epoch 对象覆写旧内存再读盒内值——未晋升则读到
/// 覆写后的垃圾。
#[test]
fn map_box_value_survives_reset() {
    assert!(eval_two_phases(
        "var key = {k: 1}; globalThis.key = key; var val = {v: 2}; globalThis.m = new Map(); globalThis.m.set(globalThis.key, val); 0",
        "var fill; for (var i = 0; i < 200; i++) { let t = {junk: i, pad: 'x'.repeat(32)}; fill = t; } globalThis.m.get(globalThis.key).v === 2",
    ));
}
