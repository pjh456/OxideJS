//! 块级函数声明与 for-in/of var 头同名（Annex B 块级函数绑定面）：
//! 求值期把函数对象写回外层 var 绑定，无外层绑定的名字在声明实例化期建绑定。
//!
//! 覆盖：顶层/函数作用域 for-in 头形、嵌套块/switch case/for-of 变体、
//! 无碰撞泄漏名、while 块、arguments 名守卫、既有全局属性保留、形参/词法
//! 碰撞守卫、strict 无写回对照、顶层可配置 builtin 名覆写形、函数作用域
//! builtin 名局部影子对照、不可写三常量跳过对照。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn truthy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(result.as_bool(), "{source} -> 期望 true，得 {:?}", result);
}

#[test]
fn for_in_var_head_same_name_rebinds_head() {
    // 顶层 for-in var 头同名：循环后头绑定被函数对象覆写。
    truthy("var o={a:1,b:2}; for (var x in o){function x(){}} typeof x === 'function'");
}

#[test]
fn for_in_var_head_same_name_reaches_global_property() {
    // 顶层形同时同步全局对象属性（脚本环境记录 var 绑定）。
    truthy("var o={a:1,b:2}; for (var x in o){function x(){}} typeof globalThis.x === 'function'");
}

#[test]
fn for_in_var_head_same_name_in_function_scope() {
    // 函数作用域 for-in 头同名：写回函数作用域 var 绑定，不落全局对象。
    truthy("function f(){ var o={a:1,b:2}; for (var x in o){function x(){}} return typeof x === 'function' } f()");
}

#[test]
fn nested_blocks_inside_for_body() {
    // 循环体内再嵌块层的块函数声明同样写回头绑定。
    truthy("var o={a:1}; for (var x in o){ { { function x(){} } } } typeof x === 'function'");
}

#[test]
fn assignment_head_same_name() {
    // 赋值头（未声明名）同名：声明实例化期建外层绑定，写回后为函数。
    truthy("var o={a:1,b:2}; for (x in o){function x(){}} typeof x === 'function'");
}

#[test]
fn for_of_var_head_same_name() {
    // for-of var 头同名孪生形。
    truthy("for (var y of [1,2]){function y(){}} typeof y === 'function'");
}

#[test]
fn switch_case_for_head_same_name() {
    // switch case 内 for-in 头同名（case 不推作用域，块函数随外层块）。
    truthy("var o={a:1,b:2}; switch(1){case 1: for (var x in o){function x(){}}} typeof x === 'function'");
}

#[test]
fn for_of_destructuring_var_head_same_name() {
    // for-of 解构 var 头同名：写回后头绑定为函数而非末迭代值。
    truthy("for (var [x] of [[1],[2]]){function x(){}} typeof x === 'function'");
}

#[test]
fn no_collision_block_fn_in_for_body_instantiates_outer_var() {
    // 无碰撞泄漏形：体内块函数名无既有 var 绑定，声明实例化期新建外层绑定。
    truthy("var o={a:1,b:2}; for (var x in o){function f(){}} typeof f === 'function'");
}

#[test]
fn while_body_block_fn_instantiates_outer_var() {
    // while 体内块函数（非循环头碰撞形）：外层绑定实例化 + 每迭代写回，
    // 顶层同步全局对象属性。
    truthy(
        "var n=0; while(n<2){ n++; {function w(){}} } typeof w === 'function' && typeof globalThis.w === 'function'",
    );
}

#[test]
fn function_scope_block_fn_named_arguments_overwrites_arguments_object() {
    // 函数作用域块函数名 arguments：arguments 对象被函数对象覆写。
    truthy("function f(){ {function arguments(){}} return typeof arguments === 'function' } f()");
}

#[test]
fn top_level_block_fn_named_arguments_reaches_global() {
    // 脚本顶层块函数名 arguments：外层绑定落全局对象属性。
    truthy("{function arguments(){}} typeof globalThis.arguments === 'function'");
}

#[test]
fn existing_global_property_kept_before_block_fn_decl() {
    // 既有全局属性（值 7）在块函数声明前被读为 number，声明后该属性被覆写为
    // 函数（声明实例化对既有可写数据属性零动作，求值期写回才覆写）。
    truthy("globalThis.w=7; var r=typeof w; {function w(){}} r === 'number' && typeof w === 'function'");
}

#[test]
fn param_name_collision_no_write_back() {
    // 形参同名守卫：块函数退化为纯块作用域，形参值不受写回影响。
    truthy("function f(x){ {function x(){}} return String(x) } f(42) === '42'");
}

#[test]
fn lexical_name_collision_no_outer_binding() {
    // 词法声明同名守卫：不建外层 var 绑定、不求值写回，词法值保持。
    truthy("function f(){ let g='L'; {function g(){}} return g } f() === 'L'");
}

#[test]
fn strict_for_body_no_write_back() {
    // strict 无 web-compat 写回：循环后头绑定保留末迭代值。
    truthy(
        "function f(){ \"use strict\"; var o={a:1,b:2}; for (var x in o){function x(){}} return typeof x } f() === 'string'",
    );
}

#[test]
fn top_level_block_fn_named_parseint_overwrites_global_property() {
    // 顶层可配置 builtin 名：实例化期建外层并入 var 名集，求值期写回覆写全局
    // 对象属性，块后调用见函数对象（空体返 undefined）；写回被跳过则此钉读
    // builtin 值 42 红。
    let result = eval("{function parseInt(){}} parseInt('42')").unwrap();
    assert!(result.is_undefined(), "parseInt 形应为 undefined，得 {:?}", result);
}

#[test]
fn top_level_block_fn_named_object_overwrites_global_property() {
    // Object 名孪生形：构造器名是可配置全局属性，写回同样覆写；空体函数对象
    // 非 new 调用返 undefined（builtin 形会返字符串包装）。
    let result = eval("{function Object(){}} Object('x')").unwrap();
    assert!(result.is_undefined(), "Object 形应为 undefined，得 {:?}", result);
}

#[test]
fn top_level_block_fn_named_parseint_kept_before_decl() {
    // builtin 名全局属性运行期预存：GDI 序言 define-if-absent 零动作，声明点前
    // 读仍见 builtin 原值，声明点后读见覆写函数。
    truthy("var r = parseInt('77') === 77; {function parseInt(){}} r && parseInt('42') === undefined");
}

#[test]
fn function_scope_block_fn_named_parseint_local_shadow() {
    // 函数作用域对照：写回守卫只门顶层不可写面，函数体内 builtin 名走局部 var
    // 影子（全局属性不受影响），收窄误入函数作用域面在此红。
    truthy(
        "function f(){ {function parseInt(){}} return parseInt('42') === undefined } f() && typeof globalThis.parseInt === 'function'",
    );
}

#[test]
fn readonly_global_constants_skip_write_back() {
    // 不可写三常量对照：写回守卫只豁免这一面（put 永不成功），块后读不变——
    // 收窄事故（三常量误入写回或可配置面误跳）在此红。
    truthy("{function undefined(){}} typeof undefined === 'undefined'");
    truthy("{function Infinity(){}} typeof Infinity === 'number'");
    truthy("{function NaN(){}} typeof NaN === 'number'");
}
