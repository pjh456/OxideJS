//! delete 标识符 DeleteBinding 面：eval 程序独立编译见不到调用方域变量，静态
//! "当前程序内是否声明"粗于规范动态判定，未声明名/隐式全局槽/eval 程序自身
//! 顶层 var/函数名（物化 c:true 全局属性）一律发全局对象运行期探针（缺失 →
//! true；不可配置 → false 且保留；可配置 → 真删且 true）。局部绑定、非 eval
//! 脚本自身顶层已声明名、eval 程序自身顶层 let/const（落 eval 自身 lexical
//! 环境，declarative 环境不可删）保留 false 常数。覆盖：direct eval 对调用方
//! 脚本 var、eval 自身 var 真删、跨 eval 真删、函数内隐式全局真删、读侧真删
//! 可见（删后裸读抛 ReferenceError）、typeof 对删后缺失名不抛、删后重写同步、
//! 跨嵌套函数 delete 后外层裸读抛 ReferenceError、catch 参数错误类型、eval
//! 起源随嵌套函数继承（顶层 var 可删/顶层 let 保留）、strict 未声明写抛错、
//! 嵌套局部变量遮蔽隐式全局名、绿基线（脚本 var/顶层函数名/未声明缺失/builtin
//! 槽优先级/strict 调用方 indirect eval）、eval 自身顶层 let/const false 且无
//! 全局属性副作用。

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

// ── direct eval 对调用方脚本 var（c:false）：false 且属性保留 ──
#[test]
fn eval_delete_caller_script_var_returns_false_and_kept() {
    eval_truthy("var x = 1; var d = eval('delete x'); d === false && x === 1 && globalThis.x === 1");
}

// ── eval 程序自身顶层 var（c:true）：true 且真删 ──
#[test]
fn eval_delete_own_var_returns_true_and_removed() {
    eval_truthy("eval('var w = 5; delete w') === true && typeof w === 'undefined'");
}

// ── 跨 eval：前一 eval 建的 var（c:true），后一 eval 真删 ──
#[test]
fn eval_delete_var_from_previous_eval_returns_true_and_removed() {
    eval_truthy("eval('var y2 = 5'); var d = eval('delete y2'); d === true && typeof y2 === 'undefined'");
}

// ── 函数内隐式全局（未声明写登记，c:true）：true 且真删 ──
#[test]
fn fn_scope_delete_implicit_global_returns_true_and_removed() {
    eval_truthy(
        "q = 2; (function() { var d = delete q; return d === true && (function() { \
         try { void q; return false; } catch (e) { return e instanceof ReferenceError; } \
         })(); })() === true",
    );
}

// ── 未声明缺失名：探针缺失臂 true（非引用目标语义保留） ──
#[test]
fn delete_undeclaring_missing_name_returns_true() {
    eval_truthy("delete nomatch_xyz_probe === true");
}

// ── 绿基线：脚本自身顶层 var（c:false）false 常数，值保留 ──
#[test]
fn delete_script_top_var_returns_false_and_kept() {
    eval_truthy("var y = 2; delete y === false && y === 2");
}

// ── 绿基线：函数作用域读脚本顶层 var，作用域链判定局部性，false ──
#[test]
fn delete_top_var_from_function_scope_returns_false() {
    eval_truthy("var p = 1; (function() { return delete p; })() === false && p === 1");
}

// ── 绿基线：strict 调用方 + indirect eval（eval 代码不继承调用方严格性） ──
#[test]
fn strict_caller_indirect_eval_delete_undeclared_returns_true() {
    eval_truthy("(function() { \"use strict\"; return (0, eval)(\"delete zqq_probe\"); })() === true");
}

// ── 可删全局内置镜像槽臂优先：探针臂不吞 builtin 槽（真删 + 清槽） ──
#[test]
fn deletable_builtin_slot_arm_precedes_probe() {
    eval_truthy("var m = Math; delete Math === true && globalThis.Math === undefined");
}

// ── 读侧真删可见：删后同程序裸读抛 ReferenceError（A 侧单一真值） ──
#[test]
fn bare_read_after_real_delete_throws_reference_error() {
    eval_truthy(
        "x = 1; delete x === true && (function() { try { void x; return false; } \
         catch (e) { return e instanceof ReferenceError; } })() === true",
    );
}

// ── typeof 对删后缺失名不抛（IsUnresolvableReference → "undefined"） ──
#[test]
fn typeof_after_real_delete_returns_undefined_not_throw() {
    eval_truthy("x = 1; delete x; typeof x === 'undefined'");
}

// ── 删后重写：寄存器与全局对象属性重新同步 ──
#[test]
fn reassign_after_real_delete_resyncs_both_sides() {
    eval_truthy("x = 1; delete x; x = 5; x === 5 && globalThis.x === 5");
}

// ── 删前读值不受影响（读集登记仅自探针起） ──
#[test]
fn read_before_delete_unchanged() {
    eval_truthy("x = 1; var r = x; delete x; r === 1");
}

// ── eval 自身顶层 let（lexical 环境不可删）：false 且不物化全局属性 ──
#[test]
fn eval_delete_own_let_returns_false_without_global_property() {
    eval_truthy("eval('let l = 1; delete l') === false && typeof l === 'undefined' && !('l' in globalThis)");
}

// ── eval 自身顶层 const（lexical 环境不可删）：false 且不物化全局属性 ──
#[test]
fn eval_delete_own_const_returns_false_without_global_property() {
    eval_truthy("eval('const c = 1; delete c') === false && typeof c === 'undefined' && !('c' in globalThis)");
}

// ── 同名单元：调用方脚本 var（c:false）+ eval 自身顶层 let，false 且调用方值保留 ──
#[test]
fn eval_delete_own_let_shadowing_caller_var_returns_false_and_kept() {
    eval_truthy("var l = 5; eval('let l = 1; delete l') === false && l === 5 && globalThis.l === 5");
}

// ── 跨嵌套函数 delete：外层函数随后裸读该隐式全局名抛 ReferenceError ──
// 写与读同在外层函数，delete 在嵌套函数；删除登记须对外层读可见。
#[test]
fn nested_delete_then_outer_read_throws_reference_error() {
    eval_truthy(
        "(function() { q194a = 2; var d = (function() { return delete q194a; })(); \
         try { void q194a; return false; } catch (e) { return d === true && e instanceof ReferenceError; } })()",
    );
}

// ── 跨嵌套 delete 后 catch 参数：错误类型为 ReferenceError 且 name 正确 ──
#[test]
fn nested_delete_catch_param_is_reference_error_with_name() {
    eval_truthy(
        "(function() { q194b = 1; var d = (function() { return delete q194b; })(); \
         try { void q194b; return false; } catch (e) { \
         return d === true && e.name === 'ReferenceError' && e instanceof ReferenceError; } })()",
    );
}

// ── 嵌套函数内 delete 未存在名：探针缺失臂 true ──
#[test]
fn nested_delete_missing_name_returns_true() {
    eval_truthy("(function() { return (function() { return delete noexist194; })() === true; })()");
}

// ── eval 起源随嵌套函数继承：eval 顶层 var 在嵌套函数内删可删（c:true） ──
#[test]
fn eval_origin_delete_own_top_var_from_nested_function_returns_true() {
    eval_truthy("eval('var v194 = 1; function f194() { return delete v194; }') === undefined && f194() === true");
}

// ── eval 起源守卫：eval 顶层 let 是 lexical 绑定，嵌套函数内 delete 仍 false ──
#[test]
fn eval_origin_delete_own_top_let_from_nested_function_returns_false() {
    eval_truthy("eval('let l194 = 1; function g194() { return delete l194; }') === undefined && g194() === false");
}

// ── strict 守卫：未声明写仍抛 ReferenceError，不受读路由改动影响 ──
#[test]
fn strict_undeclared_write_still_throws_reference_error() {
    eval_truthy(
        "(function() { \"use strict\"; try { undeclared194 = 1; return false; } \
         catch (e) { return e instanceof ReferenceError; } })()",
    );
}

// ── 跨嵌套 delete 后重写：镜像槽与全局对象属性重新同步 ──
#[test]
fn reassign_after_nested_delete_resyncs_both_sides() {
    eval_truthy(
        "(function() { q194c = 1; (function() { return delete q194c; })(); q194c = 9; \
         return q194c === 9 && globalThis.q194c === 9; })()",
    );
}

// ── 嵌套函数内局部同名 var 遮蔽隐式全局名：局部绑定不受全局读路由影响 ──
#[test]
fn nested_local_var_shadowing_implicit_global_name_unaffected() {
    eval_truthy("(function() { q194d = 1; return (function() { var q194d = 2; return q194d === 2; })(); })()");
}
