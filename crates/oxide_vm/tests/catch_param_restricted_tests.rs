//! direct eval 字符串的严格性随调用方上下文传播：strict 上下文（顶层脚本 / 继承
//! 严格函数 / 自带指令函数）下，eval 串整体按严格编译，受限名 catch 参数
//! （eval/arguments，含解构模式绑定名）报 SyntaxError；sloppy 上下文不受影响。
//!
//! 完成值一律收敛为布尔或数值（JsValue 的 Display 只暴露 number/bool，不暴露
//! 字符串内容），字符串比较在引擎内完成。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
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
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

/// 断言 strict 上下文 direct eval 抛可捕获 SyntaxError（收敛为布尔 true）。
fn assert_syntax_error(source: &str, msg: &str) {
    let res = eval(source);
    assert_eq!(res, "true", "{msg}: expected a catchable SyntaxError, got: {res}");
}

// ── strict 上下文 direct eval 受限名 catch 参数报 SyntaxError ──

#[test]
fn strict_script_inheriting_fn_eval_catch_arguments_throws() {
    // strict 脚本 + 无自带指令函数（继承严格）direct eval：受限名 catch 参数报 SyntaxError。
    assert_syntax_error(
        r#""use strict";
           try { (function() { eval("try {} catch (arguments) { }"); })(); false }
           catch (e) { e instanceof SyntaxError }"#,
        "strict script + inheriting function direct eval restricted catch name must throw",
    );
}

#[test]
fn strict_top_level_eval_catch_arguments_throws() {
    // strict 顶层 direct eval：受限名 catch 参数报 SyntaxError（语料 -throws 同形）。
    assert_syntax_error(
        r#""use strict";
           try { eval("try {} catch (arguments) { }"); false }
           catch (e) { e instanceof SyntaxError }"#,
        "strict top-level direct eval restricted catch name must throw",
    );
}

#[test]
fn sloppy_script_strict_fn_eval_catch_arguments_throws() {
    // sloppy 脚本 + 自带指令严格函数 direct eval：调用方严格性为真，受限名报 SyntaxError。
    assert_syntax_error(
        r#"function f() { "use strict"; eval("try {} catch (arguments) { }"); }
           try { f(); false } catch (e) { e instanceof SyntaxError }"#,
        "sloppy script + strict function direct eval restricted catch name must throw",
    );
}

#[test]
fn strict_script_nested_fn_eval_catch_arguments_throws() {
    // strict 脚本 + 嵌套函数 direct eval：嵌套函数继承严格，受限名报 SyntaxError。
    assert_syntax_error(
        r#""use strict";
           function outer() { function inner() { eval("try {} catch (arguments) { }"); } inner(); }
           try { outer(); false } catch (e) { e instanceof SyntaxError }"#,
        "strict script + nested function direct eval restricted catch name must throw",
    );
}

#[test]
fn strict_ctx_eval_catch_destructure_array_eval_throws() {
    // strict 上下文 direct eval 解构数组模式绑定名 eval：受限名报 SyntaxError。
    assert_syntax_error(
        r#""use strict";
           try { eval("try {} catch ([eval]) { }"); false }
           catch (e) { e instanceof SyntaxError }"#,
        "strict context direct eval array-destructured catch name eval must throw",
    );
}

#[test]
fn strict_ctx_eval_catch_object_shorthand_eval_throws() {
    // strict 上下文 direct eval 对象简写模式绑定名 eval：受限名报 SyntaxError。
    assert_syntax_error(
        r#""use strict";
           try { eval("try {} catch ({eval}) { }"); false }
           catch (e) { e instanceof SyntaxError }"#,
        "strict context direct eval object-shorthand catch name eval must throw",
    );
}

// ── 守卫：sloppy 上下文 / 改名 / 完成值不受影响 ──

#[test]
fn sloppy_script_sloppy_fn_eval_legal_completion_preserved() {
    // sloppy 脚本 + sloppy 函数 direct eval：合法，eval 完成值保留。
    assert_eq!(
        eval("function f() { return eval(\"1 + 2\"); } f()"),
        "3",
        "sloppy context direct eval stays legal and preserves the completion value"
    );
}

#[test]
fn strict_ctx_eval_catch_object_renamed_legal() {
    // 守卫：受限名检查只认绑定名，对象简写改名（eval: e）后合法。
    assert_eq!(
        eval(r#""use strict"; try { eval("try {} catch ({eval: e}) { }"); true } catch (e) { false }"#),
        "true",
        "renamed object property binding is not a restricted name"
    );
}

#[test]
fn eval_catch_default_value_throws() {
    // 守卫：catch 参数默认值恒为 SyntaxError（既有 parse 臂保真，与严格性无关）。
    assert_syntax_error(
        r#"try { eval("try {} catch (e = 2) { }"); false } catch (e) { e instanceof SyntaxError }"#,
        "catch parameter default value must stay a SyntaxError",
    );
}

#[test]
fn function_ctor_catch_arguments_legal() {
    // 守卫：Function 构造器不继承调用方严格性，受限名 catch 参数合法。
    assert_eq!(
        eval("new Function(\"try{}catch(arguments){}\")()"),
        "undefined",
        "Function constructor does not inherit caller strictness"
    );
}

#[test]
fn eval_own_strict_directive_catch_eval_throws() {
    // 守卫：eval 串自带 'use strict' 指令时，sloppy 上下文亦按严格编译，受限名报 SyntaxError。
    assert_syntax_error(
        r#"try { eval("\'use strict\'; try {} catch (eval) { }"); false } catch (e) { e instanceof SyntaxError }"#,
        "eval string with its own strict directive must reject restricted catch name",
    );
}

#[test]
fn strict_ctx_indirect_eval_catch_arguments_throws() {
    // 现状钉：引擎不区分 direct/indirect eval，strict 上下文间接 eval 受限名串亦拒
    // （node 接受，偏差归 caller 环境通道面，本钉仅记录现状）。
    assert_syntax_error(
        r#""use strict";
           try { (0,eval)("try {} catch (arguments) { }"); false }
           catch (e) { e instanceof SyntaxError }"#,
        "indirect eval in strict context currently rejects restricted catch name",
    );
}

#[test]
fn sloppy_eval_completion_value_unaffected() {
    // 守卫：sloppy 上下文 eval 完成值不受严格性判定影响。
    assert_eq!(
        eval("eval(\"1+2\")"),
        "3",
        "sloppy context eval completion value is unaffected"
    );
}

#[test]
fn static_strict_catch_eval_parse_rejected() {
    // 守卫：静态 strict 脚本 catch (eval) 在 parse 期即被 oxc strict 绑定检查拒绝。
    let res = eval("\"use strict\"; try {} catch (eval) {}");
    assert!(
        res.starts_with("parse error"),
        "static strict catch (eval) must be rejected at parse time, got: {res}"
    );
}
