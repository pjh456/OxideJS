//! 脚本顶层函数声明撞运行期用户既有全局属性的两阶段回归钉：phase1 建属性形，
//! phase2 声明脚本。函数臂按 CanDeclareGlobalFunction 在检查阶段抛 TypeError
//! （c:false ∧ ¬(data ∧ w:true ∧ e:true) / 缺失 ∧ 不可扩展，sloppy/strict 无区分），
//! 检查先于任何绑定实例化（无部分 var 绑定、无部分函数创建）；创建阶段按
//! CreateGlobalFunctionBinding——c:true 全重配 {可写, 可枚举, 不可配置}、
//! c:false 可写数据保描述符仅更值；var 臂既有属性零动作为合规锁定。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

/// 两阶段执行：phase1 → `reset()`（轻量重置，epoch 清空、session 保留）→ phase2。
fn run_two_phases(phase1: &str, phase2: &str) -> Result<JsValue, String> {
    let mut vm = Vm::new();
    vm.run(&Arc::new(compile(phase1))).expect("run1");
    vm.reset();
    vm.run(&Arc::new(compile(phase2)))
}

/// 两阶段真值断言：phase2 须以 true 完成（不抛臂）。
fn phase2_truthy(phase1: &str, phase2: &str) {
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

/// phase2 必须抛未捕获异常的变体；返回同 VM 续跑探针脚本的字符串化完成值
/// （抛后 A 侧保留面的判别探针）。非字符串完成值走 Display（布尔形探针返回
/// "true"/"false"）。
fn phase2_err_then_probe(phase1: &str, phase2: &str, probe: &str) -> String {
    let mut vm = Vm::new();
    vm.run(&Arc::new(compile(phase1))).expect("run1");
    vm.reset();
    assert!(vm.run(&Arc::new(compile(phase2))).is_err(), "phase2 应抛未捕获异常: {phase2}");
    let result = vm.run(&Arc::new(compile(probe))).expect("probe run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

// ── 描述符形：c:false 数据/访问者五形全部检查阶段抛 TypeError ──

#[test]
fn fn_decl_data_writable_nonenumerable_nonconfigurable_throws_preserves() {
    // {writable, 不可枚举, 不可配置} 数据属性：检查阶段抛，A 侧值与描述符三位保留。
    let phase1 =
        "Object.defineProperty(globalThis, 'f', {value: 0, writable: true, enumerable: false, configurable: false})";
    let err = phase2_err_text(phase1, "function f(){ return 1; }");
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
    assert_eq!(
        phase2_err_then_probe(
            phase1,
            "function f(){ return 1; }",
            "(function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
             return d && d.value === 0 && d.writable === true && d.enumerable === false \
               && d.configurable === false; })()",
        ),
        "true",
        "抛后 A 侧应保留原值与描述符",
    );
}

#[test]
fn fn_decl_data_nowritable_enumerable_nonconfigurable_throws() {
    // 不可写数据属性：不可写臂命中。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'f', {value: 0, writable: false, enumerable: true, configurable: false})",
        "function f(){}",
    );
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
}

#[test]
fn fn_decl_three_constant_shape_user_name_throws_preserves() {
    // 三常量同形（不可写 ∧ 不可枚举 ∧ 不可配置）用户名：抛，不可写位保留。
    let phase1 =
        "Object.defineProperty(globalThis, 'f', {value: 0, writable: false, enumerable: false, configurable: false})";
    let err = phase2_err_text(phase1, "function f(){}");
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
    assert_eq!(
        phase2_err_then_probe(
            phase1,
            "function f(){}",
            "(function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
             return d && d.writable === false && d.enumerable === false && d.configurable === false; })()",
        ),
        "true",
        "抛后描述符应原样保留",
    );
}

#[test]
fn fn_decl_accessor_nonconfigurable_throws() {
    // 不可配置访问者属性：CanDeclare 对访问者形恒 false。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'f', {get: function () { return 1; }, set: function (v) {}, enumerable: true, configurable: false})",
        "function f(){}",
    );
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
}

#[test]
fn fn_decl_data_writable_enumerable_nonconfigurable_updates_value_only() {
    // 可写 ∧ 可枚举 ∧ 不可配置数据属性：检查通过，创建仅更值、描述符三位保留
    // （函数值经裸读与反射两侧同一对象）。
    phase2_truthy(
        "Object.defineProperty(globalThis, 'f', {value: 0, writable: true, enumerable: true, configurable: false})",
        "function f(){ return 7; } \
         (function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
          return d && d.writable === true && d.enumerable === true && d.configurable === false \
            && typeof d.value === 'function' && d.value === f && globalThis.f === f && f() === 7; })()",
    );
}

// ── c:true 既有属性：全重配为 {可写, 可枚举, 不可配置} 数据属性 ──

#[test]
fn fn_decl_configurable_nonwritable_reconfigures_to_function() {
    // 可配置不可写数据属性：重配后描述符翻 {可写, 可枚举, 不可配置}，值换函数。
    phase2_truthy(
        "Object.defineProperty(globalThis, 'f', {value: 0, writable: false, enumerable: false, configurable: true})",
        "function f(){ return 7; } \
         (function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
          return d && d.writable === true && d.enumerable === true && d.configurable === false \
            && typeof d.value === 'function' && d.value === f && f() === 7; })()",
    );
}

#[test]
fn fn_decl_configurable_accessor_redefines_as_data() {
    // 可配置访问者属性：重定义为数据属性（get/set 消失），描述符同上。
    phase2_truthy(
        "Object.defineProperty(globalThis, 'f', {get: function () { return 1; }, set: function (v) {}, enumerable: false, configurable: true})",
        "function f(){ return 7; } \
         (function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
          return d && d.writable === true && d.enumerable === true && d.configurable === false \
            && d.get === undefined && d.set === undefined && typeof d.value === 'function' \
            && d.value === f && f() === 7; })()",
    );
}

// ── 缺失名 + 不可扩展全局：检查阶段抛，无部分绑定实例化 ──

#[test]
fn fn_decl_nonextensible_global_throws_no_partial_binding() {
    // 名缺失 ∧ 全局不可扩展：CanDeclareGlobalFunction 缺失分支抛 TypeError；
    // 同脚本 var 名不进序言（检查先于任何绑定实例化）。
    let err = phase2_err_text("Object.preventExtensions(globalThis)", "var marker; function f(){ return 1; }");
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
    assert_eq!(
        phase2_err_then_probe(
            "Object.preventExtensions(globalThis)",
            "var marker; function f(){ return 1; }",
            "Object.getOwnPropertyDescriptor(globalThis, 'marker') === undefined",
        ),
        "true",
        "var 序言不可达，marker 不应被创建",
    );
}

// ── 无严格性区分 + 错误身份（检查段逆序去重）──

#[test]
fn fn_decl_nonconfigurable_sloppy_no_partial_var_binding() {
    // sloppy：撞不可写既有属性抛 TypeError，同脚本 var 名无部分创建。
    let phase1 =
        "Object.defineProperty(globalThis, 'f', {value: 1, writable: false, enumerable: false, configurable: false})";
    let err = phase2_err_text(phase1, "var shouldNotBeDefined; function f(){ return 1; }");
    assert!(err.contains("TypeError"), "声明应抛 TypeError，实际: {err}");
    assert_eq!(
        phase2_err_then_probe(
            phase1,
            "var shouldNotBeDefined; function f(){ return 1; }",
            "Object.getOwnPropertyDescriptor(globalThis, 'shouldNotBeDefined') === undefined",
        ),
        "true",
        "var 序言不可达，shouldNotBeDefined 不应被创建",
    );
}

#[test]
fn fn_decl_nonconfigurable_strict_same_throw() {
    // strict 同形：GDI 无严格性区分，抛点与无部分创建同 sloppy。
    let phase1 =
        "Object.defineProperty(globalThis, 'f', {value: 1, writable: false, enumerable: false, configurable: false})";
    let err = phase2_err_text(phase1, "'use strict'; var shouldNotBeDefined; function f(){ return 1; }");
    assert!(err.contains("TypeError"), "strict 声明应抛 TypeError，实际: {err}");
    assert_eq!(
        phase2_err_then_probe(
            phase1,
            "'use strict'; var shouldNotBeDefined; function f(){ return 1; }",
            "Object.getOwnPropertyDescriptor(globalThis, 'shouldNotBeDefined') === undefined",
        ),
        "true",
        "strict 面 var 序言同样不可达",
    );
}

#[test]
fn fn_decl_multi_name_error_identity_is_last_declared() {
    // 两名皆撞：检查段按声明逆序遍历，源序最后声明者先查——错误身份钉最后
    // 声明者（发射序若按声明序则错误名为首声明者，本钉红）。
    let err = phase2_err_text(
        "Object.defineProperty(globalThis, 'a', {value: 1, writable: true, enumerable: false, configurable: false}); \
         Object.defineProperty(globalThis, 'b', {value: 2, writable: true, enumerable: false, configurable: false})",
        "function a(){ return 1; } function b(){ return 2; }",
    );
    assert!(err.contains("TypeError"), "应抛 TypeError，实际: {err}");
    assert!(err.contains("'b'"), "错误身份应为源序最后声明者 b，实际: {err}");
    assert!(!err.contains("'a'"), "错误身份不应为源序首声明者 a，实际: {err}");
}

// ── 双形回归：新建描述符 + var 臂既有属性零动作锁定 ──

#[test]
fn fn_decl_fresh_name_and_var_arm_existing_property_regression() {
    // (a) 缺失名 + 可扩展：新建 {可写, 可枚举, 不可配置}，函数值裸读与反射同一对象。
    phase2_truthy(
        "1",
        "function f(){ return 7; } \
         (function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
          return d && d.writable === true && d.enumerable === true && d.configurable === false \
            && typeof d.value === 'function' && d.value === f && globalThis.f === f; })()",
    );
    // (b) var 臂撞既有 c:false 属性：CanDeclareGlobalVar 既有属性恒 true，序言
    // 零动作（不抛、描述符与值不变）——var 臂合规面锁定。
    phase2_truthy(
        "Object.defineProperty(globalThis, 'g', {value: 5, writable: false, enumerable: false, configurable: false})",
        "var g; \
         (function () { var d = Object.getOwnPropertyDescriptor(globalThis, 'g'); \
          return d && d.value === 5 && d.writable === false && d.enumerable === false \
            && d.configurable === false; })()",
    );
}
