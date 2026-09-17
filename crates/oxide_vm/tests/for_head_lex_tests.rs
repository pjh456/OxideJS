//! C 风格 for 词法头独立环境钉：头名与同函数外层绑定同名时的 cell 覆盖碰撞、头名
//! 与顶层 var/函数同名时的捕获剔除、头初始化表达式与无 init 声明的 TDZ 建模缺失
//! 三面的引擎侧形态锁。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 只暴露 number/bool，不暴露字符串内容），
//! 字符串比较在引擎内完成。

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

// ── for 头 let/const 与同函数外层同名绑定的 cell 覆盖碰撞 ──

#[test]
fn c_for_let_head_same_name_as_outer_let_keeps_outer_closure() {
    // 头名与外层 let 同名且外层被闭包捕获：头 MAKE_CELL 不得原地覆写外层 cell，
    // 头环境结束后外层闭包仍读外层值。
    assert_eq!(
        eval("let x = \"out\"; let p = () => x; for (let x = \"in\"; false;) {} p() === \"out\""),
        "true",
        "for-let head must not clobber the same-named outer captured cell"
    );
}

#[test]
fn c_for_let_head_same_name_as_function_scope_let_keeps_outer_closure() {
    // 函数域同名外层 let 的孪生形：碰撞面不限于脚本顶层。
    assert_eq!(
        eval("function f(){ let x = \"out\"; let p = () => x; for (let x = \"in\"; false;) {} return p(); } f() === \"out\""),
        "true",
        "function-scope outer let closure keeps its cell across the for head"
    );
}

#[test]
fn c_for_let_head_same_name_as_outer_var_keeps_outer_closure() {
    // 外层 var 同名：头绑定独立 cell，闭包捕获的外层 var 槽值不变。
    assert_eq!(
        eval(
            "function f(){ var x = \"out\"; let p = () => x; for (let x = \"in\"; false;) {} return p(); } f() === \"out\""
        ),
        "true",
        "outer var closure keeps its value when a for-let head shadows it"
    );
}

#[test]
fn c_for_const_head_same_name_as_outer_let_keeps_outer_closure() {
    // const 头同形：不可变绑定的声明路径同样建独立 cell。
    assert_eq!(
        eval("let x = \"out\"; let p = () => x; for (const x = \"in\"; false;) {} p() === \"out\""),
        "true",
        "for-const head builds its own cell and leaves the outer captured cell intact"
    );
}

// ── 四位置头名可见 + 循环后恢复外层环境（scope-head-lex-open/close）──

#[test]
fn c_for_head_lex_env_visible_in_all_four_positions_then_restored() {
    // init/test/body/update 四位置的闭包都读头绑定（'inside'），循环前后外层读
    // 仍读外层绑定（'outside'）——对应 scope-head-lex-open/close 两文件断言。
    assert_eq!(
        eval(
            "let x = 'outside'; var probeBefore = function(){ return x; }; \
             var probeDecl, probeTest, probeIncr, probeBody; var run = true; \
             for (let x = 'inside', _ = probeDecl = function(){ return x; }; \
                  run && (probeTest = function(){ return x; }); \
                  probeIncr = function(){ return x; }) \
               probeBody = function(){ return x; }, run = false; \
             probeBefore() === 'outside' && probeDecl() === 'inside' && probeTest() === 'inside' \
               && probeBody() === 'inside' && probeIncr() === 'inside' && x === 'outside'"
        ),
        "true",
        "head lexical environment is visible in init/test/body/update and removed after the loop"
    );
}

// ── 头名与顶层 var/函数同名：顶层剔除后体/测试闭包须捕获头绑定 ──

#[test]
fn c_for_let_head_same_name_as_top_var_body_closure_reads_head() {
    assert_eq!(
        eval("var x = \"top\"; var p; for (let x = 0; x < 2; x++) { p = () => x; } p() === 1"),
        "true",
        "body closure reads the per-iteration let head, not the same-named top-level var"
    );
}

#[test]
fn c_for_let_head_same_name_as_top_var_test_closure_reads_head() {
    assert_eq!(
        eval("var x = \"top\"; var p; for (let x = 0; x < 1 && (p = () => x); x++) {} p() === 0"),
        "true",
        "test-position closure reads the let head binding, not the top-level var"
    );
}

#[test]
fn c_for_let_head_same_name_as_top_function_reads_head() {
    assert_eq!(
        eval("function x(){}; var p; for (let x = 0; x < 2; x++) { p = () => x; } typeof p() === \"number\""),
        "true",
        "body closure reads the let head when it shadows a top-level function declaration"
    );
}

#[test]
fn c_for_nested_same_name_heads_capture_inner_head() {
    assert_eq!(
        eval(
            "var x = \"top\"; var p; for (let x = 0; x < 1; x++) { for (let x = 0; x < 2; x++) { p = () => x; } } p() === 1"
        ),
        "true",
        "inner head shadows the outer head; inner closure reads the inner per-iteration binding"
    );
}

// ── 头初始化表达式 / 无 init 声明的 TDZ 建模 ──

#[test]
fn c_for_let_head_self_reference_throws_reference_error() {
    // 头自引用：TDZ 读抛 ReferenceError，不穿透外层同名已初始化绑定；无外层时
    // 同样抛 ReferenceError。
    assert_eq!(
        eval(
            "let a = 1; var r1 = 0; \
             try { for (let a = a; false;) {} } catch (e) { r1 = (e instanceof ReferenceError) ? 1 : 2; } \
             var r2 = 0; \
             try { for (let b = b; false;) {} } catch (e) { r2 = (e instanceof ReferenceError) ? 1 : 2; } \
             r1 === 1 && r2 === 1"
        ),
        "true",
        "self-reference in the for head initializer reads the uninitialized head binding"
    );
}

#[test]
fn c_for_second_declarator_self_reference_throws_reference_error() {
    assert_eq!(
        eval(
            "var r = 0; \
             try { for (let x = 1, y = y; false;) {} } catch (e) { r = (e instanceof ReferenceError) ? 1 : 2; } r === 1"
        ),
        "true",
        "second declarator reading itself in the for head throws ReferenceError"
    );
}

#[test]
fn c_for_let_head_without_init_initializes_undefined_for_closure() {
    // 无 init 的头声明：头绑定已初始化为 undefined，后续 init 内创建、捕获头名的
    // 闭包读 undefined（不得误报 TDZ）。
    assert_eq!(
        eval("var p; for (let x, _ = (p = () => x); false;) {} p() === undefined"),
        "true",
        "a for-let head without initializer is initialized to undefined for its closures"
    );
}

#[test]
fn c_for_let_head_self_reference_with_top_var_throws_reference_error() {
    assert_eq!(
        eval(
            "var x = \"top\"; var r = 0; \
             try { for (let x = x; false;) {} } catch (e) { r = (e instanceof ReferenceError) ? 1 : 2; } r === 1"
        ),
        "true",
        "for head self-reference throws even when a same-named top-level var exists"
    );
}

// ── 绿守卫：非捕获遮蔽与解构头 ──

#[test]
fn c_for_let_head_non_captured_shadow_restores_outer_and_destructures() {
    // 非捕获头名回退寄存器循环：循环后外层 let 读回原值；解构头叶绑定寄存器
    // 写的是叶值（不是整头右值）。
    assert_eq!(
        eval(
            "let x = \"out\"; for (let x = \"in\"; false;) {} \
             let value; for (let [v] = [23]; ;) { value = v; break; } \
             x === \"out\" && value === 23"
        ),
        "true",
        "non-captured shadow restores the outer binding and destructuring heads bind leaf values"
    );
}
