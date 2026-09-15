//! 函数名与 var 声明头跨语句列合法合并的求值行为钉：var 提升步复用函数
//! 声明既有绑定（脚本顶层全局属性 / 函数 var 环境），迭代写入同一绑定。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 只暴露 number/bool，不暴露
//! 字符串内容），字符串比较在引擎内完成。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn script_top_function_for_in_var_header_reuses_global_property() {
    // 脚本顶层函数 + for-in var 头：迭代重写全局属性（末键），x 不再是函数。
    assert_eq!(
        eval(
            "function x(){} var o={a:1,b:2}; for (var x in o) {} \
             (x === \"b\") && (typeof x === \"string\")"
        ),
        "true",
        "var head hoists into the global var environment and reuses the function property"
    );
}

#[test]
fn script_top_function_for_of_var_header_reuses_global_property() {
    // for-of 孪生形：末元素 2 写入全局属性。
    assert_eq!(
        eval("function x(){} var o=[1,2]; for (var x of o) {} (x === 2) && (typeof x === \"number\")"),
        "true",
        "for-of var head merges with the top-level function name binding"
    );
}

#[test]
fn function_body_function_for_in_var_header_reuses_fn_env_binding() {
    // 函数作用域面：函数 var 环境内同形合并。
    assert_eq!(
        eval(
            "function outer(){ function x(){} var o={a:1,b:2}; \
             for (var x in o) {} return (x === \"b\") && (typeof x === \"string\") } outer()"
        ),
        "true",
        "var head in a for-in inside a function body reuses the function-scope binding"
    );
}

#[test]
fn script_top_function_plain_var_keeps_function() {
    // 纯 var 重名（无迭代写入）：var 步零动作，x 保持函数本体。
    assert_eq!(
        eval("function x(){} var x; typeof x === \"function\""),
        "true",
        "plain var redeclaration of a function name keeps the function binding"
    );
}
