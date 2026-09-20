//! 被捕获词法绑定（let/const）的 TDZ 语义钉：入口 TDZ 占位 cell 与赋值引用形
//! 静态守卫跳过的引擎侧行为锁。顶层/函数作用域的词法绑定被嵌套函数捕获后，
//! 声明语句执行前的读写须经运行时 cell 的未初始化标志抛 ReferenceError，
//! 声明语句执行后读写正常；const 捕获绑定的写点抛 TypeError。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 不暴露字符串内容），字符串比较在
//! 引擎内完成。

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

// ── 顶层 let 被函数声明捕获：声明后赋值引用形（编译序判别面）──

#[test]
fn captured_top_let_update_after_decl_returns_new_value() {
    // 函数声明先于 let 声明语句编译，捕获 cell 在声明语句执行后才翻转已初始化，
    // 赋值引用形不得误报编译期 TDZ。
    assert_eq!(
        eval("let step; function r() { step++; return step; } step = 0; r() === 1"),
        "true",
        "captured top-level let update works after the declaration executes"
    );
}

#[test]
fn captured_top_let_simple_assign_after_decl() {
    assert_eq!(
        eval("let step; function r() { step = 7; return step; } step = 0; r() === 7"),
        "true",
        "captured top-level let simple assignment works after the declaration executes"
    );
}

#[test]
fn captured_top_let_compound_assign_after_decl() {
    assert_eq!(
        eval("let step; function r() { step += 5; return step; } step = 1; r() === 6"),
        "true",
        "captured top-level let compound assignment works after the declaration executes"
    );
}

#[test]
fn captured_top_let_bare_read_after_decl() {
    // 裸读不经静态守卫，守卫跳过面不得波及裸读路径。
    assert_eq!(
        eval("let step; function r() { return step; } step = 0; r() === 0"),
        "true",
        "captured top-level let bare read works after the declaration executes"
    );
}

#[test]
fn captured_top_let_update_in_stringify_replacer() {
    // builtin 回调深度不影响判别：赋值引用形只由编译序决定。
    assert_eq!(
        eval(
            "let step; function r() { step++; return step; } step = 0; JSON.stringify({ a: 1 }, (k, v) => { r(); return v; }); step === 2"
        ),
        "true",
        "captured top-level let update works from a stringify replacer"
    );
}

#[test]
fn captured_top_let_update_in_map_callback() {
    assert_eq!(
        eval("let step; function r() { step++; return step; } step = 0; [1, 2].map(() => r()).join(',') === '1,2'"),
        "true",
        "captured top-level let update works from an array map callback"
    );
}

#[test]
fn captured_top_let_update_in_stringify_replacer_expr() {
    assert_eq!(
        eval("let step; function r() { step++; return step; } step = 0; JSON.stringify({ a: 1 }, (k, v) => r() || v); step === 1"),
        "true",
        "captured top-level let update works when the replacer is a bare call expression"
    );
}

#[test]
fn captured_top_let_update_in_getter() {
    assert_eq!(
        eval("let step; function r() { step++; return step; } step = 0; Object.values({ get a() { return r(); } }).join(',') === '1'"),
        "true",
        "captured top-level let update works from an object getter"
    );
}

#[test]
fn captured_top_let_update_in_nested_arrow() {
    // 链式捕获（内层箭头的 upvalue 指向外层闭包的 upvalue cell）。
    assert_eq!(
        eval("let step; function outer() { return () => { step++; return step; }; } step = 0; outer()() === 1"),
        "true",
        "chained capture of a top-level let update works in a nested arrow"
    );
}

#[test]
fn captured_top_let_simple_assign_in_stringify_setter() {
    assert_eq!(
        eval(
            "let step; function setter() { step = 5; } function reader() { return step; } step = 0; JSON.stringify({}, (k, v) => { setter(); return reader(); }); step === 5"
        ),
        "true",
        "captured top-level let simple assignment works from a stringify setter"
    );
}

#[test]
fn captured_top_let_update_in_named_replacer() {
    assert_eq!(
        eval(
            "let step; function replacer(x, k, v) { step++; return x; } step = 0; JSON.stringify({}, (k, v) => replacer(2, k, v)); step === 1"
        ),
        "true",
        "captured top-level let update works through an indirect named replacer"
    );
}

#[test]
fn captured_top_let_with_init_bare_read() {
    assert_eq!(
        eval("let step = 3; function r() { return step; } r() === 3"),
        "true",
        "captured top-level let with initializer reads through the closure"
    );
}

#[test]
fn function_decl_before_let_bare_read() {
    // 函数声明在 let 之前（源序前），裸读经运行时 cell，声明后正常。
    assert_eq!(
        eval("function r() { return step; } let step; step = 0; r() === 0"),
        "true",
        "function declared before a captured let reads it after initialization"
    );
}

// ── 箭头在 let 前：入口 TDZ 占位 cell 面（无编译期命中，运行时判定）──

#[test]
fn arrow_after_let_update_returns_new_value() {
    assert_eq!(
        eval("let step; const r = () => { step++; return step; }; step = 0; r() === 1"),
        "true",
        "arrow created after a captured let updates the binding"
    );
}

#[test]
fn arrow_before_let_update_returns_new_value() {
    // 箭头先于 let 创建：入口 TDZ 占位 cell 使声明前读抛真 TDZ，
    // 声明后 MAKE_CELL 原位翻转，更新正常。
    assert_eq!(
        eval("const r = () => { step++; return step; }; let step; step = 0; r() === 1"),
        "true",
        "arrow created before the captured let updates the binding after initialization"
    );
}

// ── 捕获 const：写点 TypeError（TDZ 守卫跳过面不得吞 const 判定）──

#[test]
fn captured_top_const_update_throws_type_error() {
    assert_eq!(
        eval("const step = 1; function r() { step++; } try { r(); false } catch (e) { e.constructor === TypeError }"),
        "true",
        "updating a captured const throws TypeError, not a TDZ reference error"
    );
}

#[test]
fn captured_top_const_simple_assign_throws_type_error() {
    assert_eq!(
        eval("const step = 1; function r() { step = 2; return step; } try { r(); false } catch (e) { e.constructor === TypeError }"),
        "true",
        "assigning a captured const throws TypeError"
    );
}

// ── 真 TDZ 保抛：声明语句执行前的读写仍抛 ReferenceError ──

#[test]
fn true_tdz_captured_let_update_throws_reference_error() {
    // 声明语句在 try 块之后才执行：调用点命中入口 TDZ 占位 cell。
    assert_eq!(
        eval("function f() { x++; } try { f(); false } catch (e) { e.constructor === ReferenceError } let x = 1"),
        "true",
        "updating a captured let before its declaration throws ReferenceError"
    );
}

#[test]
fn true_tdz_captured_let_update_in_nested_depth() {
    assert_eq!(
        eval(
            "function f() { x++; } JSON.stringify({}, (k, v) => { try { f(); } catch (e) { return e.constructor.name; } return v; }); let x = 1; x === 1"
        ),
        "true",
        "true TDZ on a captured let survives builtin callback depth and stays catchable"
    );
}

#[test]
fn true_tdz_captured_let_bare_read_throws_inside_catcher() {
    // 裸读真 TDZ 在捕获器内抛（结果未检视，仅锁"抛且可捕获"行为）。
    assert_eq!(
        eval(
            "function r() { return step; } JSON.stringify({}, (k, v) => { try { String(r()); return 'no-throw'; } catch (e) { return e.constructor.name; } }); let step = 1; 'done' === 'done'"
        ),
        "true",
        "bare read of a captured let before its declaration throws inside the catcher"
    );
}

#[test]
fn block_level_let_tdz_update_still_throws() {
    // 块级 let 无嵌套捕获（纯静态 TDZ 面）：守卫跳过面不得波及，仍编译期抛。
    assert_eq!(
        eval("function f() { y = y + 1; let y; return y; } try { f(); false } catch (e) { e.constructor === ReferenceError }"),
        "true",
        "block-level let TDZ on an uncaptured binding still throws ReferenceError"
    );
}

#[test]
fn block_level_let_captured_closure_reads_updated_value() {
    // 块级 let 被块内闭包捕获：块内声明后更新，块外读回更新值（重执行族对照面）。
    assert_eq!(
        eval("function g() { { let a = 5; const c = () => a; a = 6; return c; } return null; } g()() === 6"),
        "true",
        "closure over a block-level let reads the updated value after the block"
    );
}

// ── 非捕获对照面：顶层 var / delete / typeof 不变 ──

#[test]
fn captured_top_var_update_after_decl() {
    // 顶层 var 被捕获：入口即初始化为 undefined（提升语义），更新正常。
    assert_eq!(
        eval("var x; function r() { x++; return x; } x = 0; r() === 1 && x === 1"),
        "true",
        "captured top-level var update works, var hoisting semantics preserved"
    );
}

#[test]
fn delete_captured_top_let_returns_false() {
    assert_eq!(
        eval("let step = 1; function r() { return delete step; } r() === false && step === 1"),
        "true",
        "deleting a captured top-level let returns false and leaves the value"
    );
}

#[test]
fn typeof_captured_top_let_after_decl() {
    assert_eq!(
        eval("let step; function r() { return typeof step === 'number'; } step = 0; r()"),
        "true",
        "typeof on a captured top-level let after initialization reports number"
    );
}
