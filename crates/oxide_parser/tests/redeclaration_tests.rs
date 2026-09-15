//! 重名误报过滤钉锁：函数名与 var 声明头跨语句列合法合并的形必须解析成功，
//! 同语句列碰撞与模块 lexical 面必须继续拒绝。

use oxide_parser::Allocator;

fn parsed(source: &str) {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, source);
    assert!(result.is_ok(), "应解析成功：{source}，实际错误：{:?}", result.err());
}

fn parsed_module(source: &str) {
    let allocator = Allocator::default();
    let result = oxide_parser::parse_module(&allocator, source);
    assert!(result.is_ok(), "应按模块解析成功：{source}，实际错误：{:?}", result.err());
}

fn rejected(source: &str) {
    let allocator = Allocator::default();
    let result = oxide_parser::parse(&allocator, source);
    let errors = result.expect_err(&format!("应拒绝：{source}"));
    assert!(
        errors.iter().any(|e| e.message == "Identifier `x` has already been declared"),
        "拒绝原因应仍为重名诊断，实际：{errors:?}"
    );
}

fn rejected_module(source: &str) {
    let allocator = Allocator::default();
    let result = oxide_parser::parse_module(&allocator, source);
    let errors = result.expect_err(&format!("应按模块拒绝：{source}"));
    assert!(
        errors.iter().any(|e| e.message == "Identifier `x` has already been declared"),
        "拒绝原因应仍为重名诊断，实际：{errors:?}"
    );
}

// ── 合法合并形：必须解析成功 ──

#[test]
fn script_top_function_for_in_var_header() {
    // 脚本顶层函数 + for-in var 头：var 提升后与函数名同 var 作用域合法合并。
    parsed("function x(){} var o={a:1,b:2}; for (var x in o) {} x+\"|\"+x");
}

#[test]
fn script_top_function_for_of_var_header() {
    // for-of 孪生形。
    parsed("function x(){} var o=[1,2]; for (var x of o) {} x+\"|\"+typeof x");
}

#[test]
fn function_body_function_for_in_var_header() {
    // 函数作用域面：外层函数体内的函数声明 + 内层 for-in var 头。
    parsed("function outer(){ function x(){} var o={a:1,b:2}; for (var x in o) {} return x+\"|\"+typeof x }");
}

#[test]
fn script_top_function_c_for_var_header() {
    // C-for init 的 var 声明子同属外层 var 作用域。
    parsed("function x(){} var o,n; for (var x=0; n<2; n++,x++) {}");
}

#[test]
fn script_top_function_var_in_if_block() {
    // if 块内 var 提升过 if 块到外层 var 作用域。
    parsed("function x(){} if (true) { var x; } typeof x");
}

#[test]
fn script_top_function_var_in_bare_block() {
    // 裸块面。
    parsed("function x(){} { var x; } typeof x");
}

#[test]
fn script_top_function_var_in_while_block() {
    // while 块面。
    parsed("function x(){} while (false) { var x; } typeof x");
}

#[test]
fn script_top_function_generator_for_in_var_header() {
    // 生成器函数声明同为 var 作用域函数名：合并形。
    parsed("function* x(){} var o={a:1}; for (var x in o) {} typeof x");
}

#[test]
fn if_arm_function_for_in_var_header() {
    // Annex B if/else 臂函数声明按 var 作用域绑定，其名不计入任何语句列
    // 的 lexically declared 名：与外层 var 头不碰撞。
    parsed("if(true) function x(){} var o={a:1,b:2}; for (var x in o) {} typeof x");
    parsed("if(false) {} else function x(){} var o={a:1}; for (var x in o) {} typeof x");
    // 块内 if 臂同形。
    parsed("{ if(true) function x(){} var o={a:1}; for (var x in o) {} } typeof x");
}

#[test]
fn block_function_sibling_of_var_header() {
    // 块内函数 + 块外 for-in 头：无同一语句列同时含两者，不碰撞。
    parsed("{ function x(){} } var o={a:1,b:2}; for (var x in o) {} typeof x");
}

#[test]
fn inner_block_function_outer_var_header() {
    // 内嵌块函数 + 外层块 var 头：函数名不计入外层块的 lexically declared 名。
    parsed("{ { function x(){} } var o={a:1,b:2}; for (var x in o) {} } typeof x");
}

#[test]
fn labeled_statement_function_for_in_var_header() {
    // 标签语句包裹的函数声明非语句列直接成员。
    parsed("a: function x(){} var o={a:1,b:2}; for (var x in o) {} typeof x");
}

#[test]
fn switch_case_function_var_header_outside() {
    // case 内函数 + switch 外 for-in 头：var 声明不在 CaseBlock 列内。
    parsed("var o={a:1,b:2}; switch(1){ case 1: function x(){} } for (var x in o) {} typeof x");
}

#[test]
fn var_then_block_function_after_var_header() {
    // for-in 头在前、后块内函数声明：现行语义不碰撞（v8 一致）。
    parsed("{ var o={a:1,b:2}; for (var x in o) {} { function x(){} } } typeof x");
}

// ── 真错面：必须继续拒绝 ──

#[test]
fn same_block_function_and_var() {
    // 同块函数声明 + var：语句列重名真错。
    rejected("{ function x(){} var x; }");
    rejected("{ function x(){} var x; } \"ok\"");
}

#[test]
fn outer_block_function_inner_block_var() {
    // 外层块函数 + 嵌套块 var：函数名计入外层列 lexically declared 名，
    // var 计入该列 VarDeclaredNames（递归闭包）：真错。
    rejected("{ function x(){} { var x; } }");
    rejected("{ function x(){} { for (var x in o) {} } }");
}

#[test]
fn same_block_let_and_var() {
    // let 名与 var 同名同列：真错（标签侧非函数声明，过滤不动作）。
    rejected("{ let x; var x; }");
}

#[test]
fn script_top_function_and_let() {
    // let 声明撞顶层函数名：声明期真错，非本过滤面。
    rejected("function x(){} let x;");
}

#[test]
fn class_name_with_for_in_var_header() {
    // class 名建声明式组绑定，var 撞之 = 真错（既有声明侧为 class 非函数）。
    rejected("class x{} var o={a:1,b:2}; for (var x in o) {}");
}

#[test]
fn function_then_class_same_name() {
    // class 重名臂：既有声明侧为函数、新声明侧为 class，过滤不动作。
    rejected("function x(){} class x {}");
}

#[test]
fn same_block_strict_function_and_var() {
    // 严格模式块内函数为 lexical 绑定，同列 var 碰撞同判真错。
    rejected("{ \"use strict\"; function x(){} var o={a:1}; for (var x in o) {} }");
}

#[test]
fn labeled_function_in_block_with_var() {
    // 块内标签语句包裹的函数声明仍属该块语句列直接成员：同列 var 碰撞真错。
    rejected("{ a: function x(){} var o={a:1,b:2}; for (var x in o) {} }");
}

#[test]
fn switch_cases_function_and_var() {
    // CaseBlock 列：任一 case 的函数声明与任一 case 的 var 声明同列碰撞。
    rejected("var o={a:1,b:2}; switch(1){ case 1: function x(){} case 2: { for (var x in o) {} } }");
    rejected("var o={a:1,b:2}; switch(1){ case 1: { function x(){} for (var x in o) {} } }");
}

#[test]
fn module_var_and_function_top_level() {
    // 模块顶层函数为 lexical 绑定：var 撞之 = 真错。
    rejected_module("export {}; var x; function x(){}");
    rejected_module("export {}; function x(){} var x;");
}

#[test]
fn module_function_and_for_in_var_header() {
    // 模块面 for-in var 头同判真错。
    rejected_module("export {}; function x(){} var o={a:1,b:2}; for (var x in o){}");
}

// ── 透传钉：同消息形但节点形不同的诊断必须原样保留 ──

#[test]
fn same_message_let_let_still_rejected() {
    // let/let 同消息形重名：既有声明侧非函数声明，过滤须透传。
    rejected("{ let x; let x; }");
}

#[test]
fn strict_duplicate_params_still_rejected() {
    // 严格模式重复参数同消息形：新声明侧非 var 声明子，过滤须透传。
    rejected("\"use strict\"; function f(x, x) {}");
}

// ── 无关面回归：普通解析不受过滤影响 ──

#[test]
fn unrelated_shapes_unaffected() {
    parsed("var x; var o={a:1}; for (var x in o) {} x+\"|\"+x");
    parsed("function x(){} var x; typeof x");
    parsed("function x(){} for (x in o) {}");
    parsed_module("export {}; function x(){} var o={a:1}; for (let x in o) {} typeof x");
}
