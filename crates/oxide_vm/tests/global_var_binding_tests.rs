//! 脚本顶层 var/function 声明落 globalThis 的运行时行为测试。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
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

/// 两阶段 run-Err 变体：phase2 返回运行结果，供未捕获异常（TypeError 等）断言。
fn run_two_phases(phase1: &str, phase2: &str) -> Result<JsValue, String> {
    let mut vm = Vm::new();
    vm.run(&Arc::new(compile(phase1))).expect("run1");
    vm.reset();
    vm.run(&Arc::new(compile(phase2)))
}

/// 两阶段真值断言：phase2 须以 true 完成。
fn assert_two_phases_truthy(phase1: &str, phase2: &str) {
    match run_two_phases(phase1, phase2) {
        Ok(v) => assert!(v.is_bool() && v.as_bool(), "expected true, got: {v:?}\nphase2: {phase2}"),
        Err(e) => panic!("phase2 run failed: {e}\nphase2: {phase2}"),
    }
}

/// 取 phase2 未捕获异常文本（phase2 正常完成时 panic）。
fn phase2_err_text(phase1: &str, phase2: &str) -> String {
    match run_two_phases(phase1, phase2) {
        Err(e) => e,
        Ok(v) => panic!("expected uncaught error, got: {v:?}\nphase2: {phase2}"),
    }
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
/// （prototype 槽）在 epoch 清空前被晋升；子模块按创建期表代际解析，run2
/// 直接调用存活函数并读其属性。
#[test]
fn global_function_survives_reset_with_promoted_property() {
    assert!(eval_two_phases(
        "var f = function(){}; globalThis.f = f; f.prototype = {}; f.prototype.q = 7; 0",
        "typeof globalThis.f === 'function' && globalThis.f() === undefined && globalThis.f.prototype.q === 7",
    ));
}

/// 类构造器跨 run：prototype 子对象（运行时 epoch 新建）经 reset 边界晋升，
/// run2 经全局别名读到方法属性并直接调用。
#[test]
fn class_prototype_link_survives_reset() {
    assert!(eval_two_phases(
        "var cc = class { m(){ return 3; } }; globalThis.cc = cc; 0",
        "typeof globalThis.cc === 'function' && typeof globalThis.cc.prototype.m === 'function' && new globalThis.cc().m() === 3",
    ));
}

/// 跨 run 调用值：run1 建的函数（含闭包捕获）与 prototype 子对象跨 reset
/// 存活，run2 经全局别名调用——子模块按创建期代际解析，调用值与捕获值正确。
#[test]
fn cross_run_call_resolves_creation_generation() {
    assert!(eval_two_phases(
        "function make() { var v = 41; return function() { return v + 1; }; } \
         globalThis.f = make(); globalThis.f.prototype = {}; globalThis.f.prototype.q = 7; 0",
        "globalThis.f() === 42 && globalThis.f.prototype.q === 7",
    ));
}

/// 动态函数跨 run 调用：run1 `new Function` 创建的函数对象属 run1 代际，
/// run2 按自身代际解析执行，调用值正确。
#[test]
fn dynamic_function_survives_reset_and_calls() {
    assert!(eval_two_phases(
        "globalThis.d = new Function('a', 'return a * 2'); 0",
        "globalThis.d(21) === 42",
    ));
}

/// 跨 run 嵌套闭包：run1 的 f 体内定义嵌套函数 g（f 帧内 CREATE_CLOSURE 按
/// run1 代际平表发射 flat_id）；reset 后 run2 调用 f——嵌套闭包须按执行帧
/// 代际解析 flat 表，返回闭包的 name/调用值须命中 f 模块的字节码。
#[test]
fn cross_run_call_nested_closure_resolves_frame_generation() {
    // 当前代际平表长度 ≥ 旧代索引：按当前代际解析会静默命中他模块
    // （错 name / 错字节码），值断言与 name 断言同时捕获。
    assert!(eval_two_phases(
        "function f(){ function g(){ return 41 + 1; } return g; } globalThis.f = f; 0",
        "var filler_a = function(){}; var filler_b = function(){}; \
         var h = globalThis.f(); h() === 42 && h.name === 'g'",
    ));
    // 当前代际平表更短（run2 脚本无顶层函数）：旧代索引越界，合法代码不得
    // 转显式 Err。
    assert!(eval_two_phases(
        "function f(){ function g(){ return 41 + 1; } return g; } globalThis.f = f; 0",
        "var h = globalThis.f(); h() === 42 && h.name === 'g'",
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

/// 顶层 var 单一真值：裸读、globalThis 别名、GOPD 描述符三者见同一值——
/// 声明后写点落全局对象属性，镜像寄存器不再参与读。
#[test]
fn top_level_var_read_write_single_source() {
    eval_truthy("var x = 1; x = 2; globalThis.x === 2");
    eval_truthy("var x = 1; x = 2; x === 2 && globalThis.x === 2");
    eval_truthy("var x = 1; x = 2; Object.getOwnPropertyDescriptor(globalThis, 'x').value === 2 && x === 2");
    // 属性被直写后裸读见新值：读走全局对象属性而非镜像槽。
    eval_truthy("var v = 1; globalThis.v = 2; v === 2");
    // 复合赋值旧值取属性值：属性被外部更新后 RMW 基址须跟随属性。
    eval_truthy("var c = 5; globalThis.c = 100; c += 2; c === 102 && globalThis.c === 102");
}

/// 顶层 var 的 typeof 读全局对象属性：声明语句前 GDI 序言已建属性（值
/// undefined）；声明后见真实值。
#[test]
fn top_level_var_typeof_reads_global_property() {
    eval_truthy("(function(){ seen = (typeof t === 'undefined'); })(); var t; seen === true");
    eval_truthy("var t2 = 's'; typeof t2 === 'string' && globalThis.t2 === 's'");
}

/// 顶层 var 自增/自减、复合/逻辑赋值：新值落全局对象属性。
#[test]
fn update_and_compound_assign_write_global_property() {
    eval_truthy("var u = 3; ++u; u === 4 && globalThis.u === 4");
    eval_truthy("var p = 3; p++; p === 4 && globalThis.p === 4");
    eval_truthy("var c = 5; c += 2; c === 7 && globalThis.c === 7");
    eval_truthy("var l = 0; l ||= 9; l === 9 && globalThis.l === 9");
    eval_truthy("var n = 1; n &&= 0; n === 0 && globalThis.n === 0");
}

/// 顶层 var 作解构赋值目标：值落全局对象属性。
#[test]
fn destructuring_assign_writes_global_property() {
    eval_truthy("var d1; ({d1} = {d1: 5}); d1 === 5 && globalThis.d1 === 5");
    eval_truthy("var d2; [d2] = [7]; d2 === 7 && globalThis.d2 === 7");
}

/// 顶层 for-in 头的迭代值落全局对象属性：声明头与赋值头两形。
#[test]
fn for_in_head_writes_global_property() {
    eval_truthy("for (var k in {a: 1}) {} k === 'a' && globalThis.k === 'a'");
    eval_truthy("var k2; for (k2 in {a: 1, b: 2}) {} k2 === 'b' && globalThis.k2 === 'b'");
}

/// 嵌套函数局部同名遮蔽不穿透顶层绑定：遮蔽侧走局部槽，顶层属性不受影响；
/// 无遮蔽时嵌套裸读/裸写直连全局对象属性（不经 cell）。
#[test]
fn nested_local_shadow_does_not_pierce_tier() {
    eval_truthy("var s = 1; (function(){ var s = 2; return s; })() === 2 && s === 1 && globalThis.s === 1");
    eval_truthy(
        "var s2 = 1; (function(){ var s2 = 9; s2 = 10; return s2; })() === 10 && s2 === 1 && globalThis.s2 === 1",
    );
    eval_truthy("var n = 42; (function(){ return n; })() === 42");
    // 严格嵌套函数 this=undefined：写点不依赖 this，经 session 全局对象落属性。
    eval_truthy("\"use strict\"; var w = 1; (function(){ \"use strict\"; w = 5; })(); w === 5 && globalThis.w === 5");
}

/// 直接 eval：var 声明在全局对象建可配置属性；eval 内裸写穿透全局对象
/// 属性，外层（顶层 var 名）读见同一值。
#[test]
fn direct_eval_var_and_write_visible_to_outer() {
    eval_truthy("eval('var y = 5'); y === 5 && globalThis.y === 5");
    eval_truthy("var q = 1; eval('q = 55'); q === 55 && globalThis.q === 55");
    eval_truthy("eval('var ev = 1'); Object.getOwnPropertyDescriptor(globalThis, 'ev').configurable === true");
    // 嵌套函数内直接 eval：写点经 session 解析全局对象（不依赖 this）。
    eval_truthy("var z = 1; (function(){ \"use strict\"; eval('z = 77'); })(); z === 77 && globalThis.z === 77");
}

/// 既有属性与顶层 var 声明交互：序言 define-if-absent 零动作，声明值写
/// 是描述符感知写——既有不可写属性保值不覆盖。既有属性须先于 GDI 存在，
/// 分两阶段：阶段一建属性（该程序无 var，GDI 不触碰），阶段二声明。
#[test]
fn tier_declaration_respects_existing_property() {
    assert!(eval_two_phases(
        "Object.defineProperty(globalThis, 'rw', {value: 7, writable: false, configurable: true})",
        "var rw = 99; rw === 7 && globalThis.rw === 7",
    ));
}

// ── 顶层函数声明名与 var 名同列单一真值：再赋值/裸读/GOPD 收敛全局对象属性 ──

/// 顶层函数声明名再赋值：裸读、globalThis 别名、GOPD 反射三者见同一值——
/// 写点落全局对象属性（A 侧），声明处的函数值建立不随裸写失步。
#[test]
fn top_level_function_reassign_single_source() {
    eval_truthy("function f(){} f = 5; f === 5 && globalThis.f === 5");
    eval_truthy("function f(){} f = 5; Object.getOwnPropertyDescriptor(globalThis, 'f').value === 5 && f === 5");
    // 复合赋值 RMW 取属性侧旧值：基址须跟随属性而非声明期残留。
    eval_truthy("function f(){return 1;} f = 5; f += 2; f === 7 && globalThis.f === 7");
    // 嵌套 strict 写经 session 落属性（写点不依赖 this）。
    eval_truthy("function f(){} (function(){ \"use strict\"; f = 9; })(); f === 9 && globalThis.f === 9");
}

/// 声明处 A 侧写新建描述符 {writable, enumerable, configurable:false}，与 var
/// 声明描述符同形。
#[test]
fn top_level_function_decl_descriptor_match_var_shape() {
    eval_truthy(
        "function f(){} var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
         d.writable === true && d.enumerable === true && d.configurable === false",
    );
}

/// 嵌套局部同名遮蔽不穿透顶层函数绑定；无初始化 var 不抹函数值，带初始化
/// var 按 PutValue 覆盖函数值。
#[test]
fn top_level_function_shadow_and_var_no_init() {
    eval_truthy(
        "function f(){return 1;} (function(){ var f = 9; return f; })() === 9 \
         && typeof f === 'function' && globalThis.f() === 1",
    );
    eval_truthy("function f(){return 1;} var f; typeof f === 'function' && globalThis.f === f");
    eval_truthy("function f(){return 1;} var f = 5; f === 5 && globalThis.f === 5");
}

/// 声明处描述符感知写：既有不可写不可配置属性声明不更值（定义不变量静默
/// no-op），裸读与反射一致见既有值。
#[test]
fn top_level_function_decl_respects_nonwritable_property() {
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'f', {value: 1, writable: false, configurable: false})",
        "function f(){} f === 1 && globalThis.f === 1 && typeof f === 'number'",
    );
}

/// 跨 run（reset）契约：函数对象的 A 侧值直落 session，run2 经全局别名
/// 调用值正确。
#[test]
fn top_level_function_survives_reset() {
    assert!(eval_two_phases(
        "function f(){ return 1; } 0",
        "typeof globalThis.f === 'function' && globalThis.f() === 1",
    ));
}

/// builtin 名形零污染：可写 builtin 名（Math）声明更值保描述符；不可写
/// builtin 名（undefined）声明不更新，裸读与反射一致见既有值。
#[test]
fn top_level_function_decl_builtin_name_zero_pollution() {
    eval_truthy("function Math(){ return 7; } Math() === 7 && globalThis.Math() === 7 && Math === globalThis.Math");
    eval_truthy(
        "function Math(){} var d = Object.getOwnPropertyDescriptor(globalThis, 'Math'); \
         d.writable === true && d.enumerable === false && d.configurable === true",
    );
    eval_truthy(
        "function undefined(){ return 1; } typeof undefined === 'undefined' && globalThis.undefined === undefined",
    );
}

// ── 声明带初始化的 A 侧写是 PutValue：strict 撞既有不可写用户属性抛 TypeError ──

#[test]
fn strict_var_init_on_nonwritable_user_property_throws() {
    // strict 声明初始化撞既有不可写（可配置）用户属性抛 TypeError；
    // 序言对既有属性零动作（抛点仅在声明处 PutValue）。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: true})",
        "'use strict'; var u = 5;",
    );
    assert!(err.contains("TypeError"), "应抛 TypeError，实际: {err}");
    assert!(err.contains("read-only"), "消息应指明只读属性，实际: {err}");
}

#[test]
fn strict_var_init_on_writable_user_property_updates_value() {
    // 既有可写用户属性（strict）：PutValue 更新值不抛，裸读与反射一致。
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: true, configurable: true})",
        "'use strict'; var u = 5; u === 5 && globalThis.u === 5",
    );
}

#[test]
fn sloppy_var_init_on_writable_user_property_updates_value() {
    // 同上（sloppy）：可写属性值更新。
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: true, configurable: true})",
        "var u = 5; u === 5 && globalThis.u === 5",
    );
}

#[test]
fn strict_var_init_on_nonconfigurable_nonwritable_user_property_throws() {
    // c:false + w:false：序言零动作，声明 PutValue strict 抛 TypeError。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: false})",
        "'use strict'; var u = 5;",
    );
    assert!(err.contains("TypeError") && err.contains("read-only"), "实际: {err}");
}

#[test]
fn sloppy_var_init_on_nonconfigurable_nonwritable_user_property_silent() {
    // c:false + w:false（sloppy）：静默 no-op，值不覆盖。
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: false})",
        "var u = 5; u === 1 && globalThis.u === 1",
    );
}

#[test]
fn strict_var_no_init_on_nonwritable_user_property_no_throw() {
    // 无初始化声明无 PutValue（防过抛守卫）：strict 不抛，既有值不动。
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: true})",
        "'use strict'; var u; u === 1 && globalThis.u === 1",
    );
}

#[test]
fn strict_for_of_var_head_on_nonwritable_user_property_throws() {
    // for-of var 声明头与声明带 init 同走 PutValue 写点：strict 迭代值撞
    // 不可写抛 TypeError；sloppy 静默 no-op。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'p', {value: 1, writable: false, configurable: true})",
        "'use strict'; for (var p of [3]) {}",
    );
    assert!(err.contains("TypeError") && err.contains("read-only"), "实际: {err}");
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'p', {value: 1, writable: false, configurable: true})",
        "for (var p of [3]) {} p === 1 && globalThis.p === 1",
    );
}

#[test]
fn strict_eval_var_init_on_nonwritable_user_property_throws() {
    // eval 对照：eval 脚本声明处写点本就带 strict 分派，改前后同形。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: true})",
        "eval('\"use strict\"; var u = 5')",
    );
    assert!(err.contains("TypeError") && err.contains("read-only"), "实际: {err}");
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'u', {value: 1, writable: false, configurable: true})",
        "eval('var u = 5'); globalThis.u === 1",
    );
}

// ── GDI 序言缺失分支：全局不可扩展 → 两模式 TypeError（CanDeclareGlobalVar 面） ──

#[test]
fn nonextensible_global_strict_var_prologue_throws_before_body() {
    // 序言期抛：体副作用不执行（touched 未写入）。
    let mut vm = Vm::new();
    vm.run(&Arc::new(compile("Object.preventExtensions(globalThis)")))
        .expect("run1");
    vm.reset();
    let result = vm.run(&Arc::new(compile("'use strict'; var missing; touched = 1;")));
    let err = match result {
        Err(e) => e,
        Ok(v) => panic!("应抛 TypeError，实际: {v:?}"),
    };
    assert!(err.contains("TypeError"), "应抛 TypeError，实际: {err}");
    // 体未执行：隐式全局写点未触达，touched 未创建。
    vm.reset();
    let after = vm.run(&Arc::new(compile("typeof touched"))).expect("run3");
    assert_eq!(vm.lookup_str(after).unwrap_or_default(), "undefined");
}

#[test]
fn nonextensible_global_sloppy_var_prologue_throws() {
    // var 臂无 strict 门：sloppy 同形抛 TypeError。
    let err = phase2_err_text("Object.preventExtensions(globalThis)", "var missing;");
    assert!(err.contains("TypeError"), "应抛 TypeError，实际: {err}");
}

#[test]
fn nonextensible_global_eval_var_throws_both_modes() {
    // eval 脚本序言不可扩展抛：sloppy/strict 两模式同抛（回归钉）。
    let err_sloppy = phase2_err_text("Object.preventExtensions(globalThis)", "eval('var missing')");
    assert!(err_sloppy.contains("TypeError"), "sloppy eval 应抛 TypeError，实际: {err_sloppy}");
    let err_strict = phase2_err_text("Object.preventExtensions(globalThis)", "eval('\"use strict\"; var missing')");
    assert!(err_strict.contains("TypeError"), "strict eval 应抛 TypeError，实际: {err_strict}");
}

#[test]
fn nonextensible_global_var_existing_property_no_throw() {
    // 既有属性不进缺失分支：零动作不抛（防过抛守卫）。
    assert_two_phases_truthy(
        "Object.defineProperty(globalThis, 'ex', {value: 1, writable: false, configurable: true}); \
         Object.preventExtensions(globalThis); true",
        "var ex; ex === 1 && globalThis.ex === 1",
    );
}
