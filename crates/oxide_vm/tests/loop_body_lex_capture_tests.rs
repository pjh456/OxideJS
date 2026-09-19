//! 循环体内块级被捕获 let/const 的每迭代绑定引擎侧形态锁：声明每迭代重执行
//! 须新建 cell，本迭代闭包捕获新 cell、旧闭包保留旧值；循环头 fresh 机制、
//! var 规范共享与非捕获直读三面语义不变。
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

// ── for-of / for-in 体被捕获块级声明：每迭代新 cell ──

#[test]
fn for_of_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var fns = []; \
             for (const k of ks) { const v = k; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === 'a|b'"
        ),
        "true",
        "for-of body const captured by closure keeps the per-iteration value"
    );
}

#[test]
fn for_of_body_let_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var fns = []; \
             for (let k of ks) { let v = k; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === 'a|b'"
        ),
        "true",
        "for-of body let captured by closure keeps the per-iteration value"
    );
}

#[test]
fn for_in_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var o = { p: 1, q: 2 }; var fns = []; \
             for (const key in o) { const v = o[key]; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === '1|2'"
        ),
        "true",
        "for-in body const captured by closure keeps the per-iteration value"
    );
}

// ── while / do-while / C-for / 标签循环体同形 ──

#[test]
fn while_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var i = 0; var fns = []; \
             while (i < 2) { const v = (i + 1) * 10; fns.push(() => v); i++; } \
             fns[0]() + '|' + fns[1]() === '10|20'"
        ),
        "true",
        "while body const captured by closure keeps the per-iteration value"
    );
}

#[test]
fn c_for_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var fns = []; \
             for (var i = 0; i < 2; i++) { const v = (i + 1) * 10; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === '10|20'"
        ),
        "true",
        "c-style for body const captured by closure keeps the per-iteration value"
    );
}

#[test]
fn do_while_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var i = 0; var fns = []; \
             do { const v = (i + 1) * 10; fns.push(() => v); i++; } while (i < 2); \
             fns[0]() + '|' + fns[1]() === '10|20'"
        ),
        "true",
        "do-while body const captured by closure keeps the per-iteration value"
    );
}

#[test]
fn labeled_while_body_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var i = 0; var fns = []; \
             outer: while (i < 2) { const v = (i + 1) * 10; fns.push(() => v); i++; } \
             fns[0]() + '|' + fns[1]() === '10|20'"
        ),
        "true",
        "labeled while body const captured by closure keeps the per-iteration value"
    );
}

// ── 嵌套块 / getter / 链式捕获 / 无 init 臂同形 ──

#[test]
fn for_of_body_nested_block_const_captured_by_closure_reads_per_iteration() {
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var fns = []; \
             for (const k of ks) { if (k) { const v = k; fns.push(() => v); } } \
             fns[0]() + '|' + fns[1]() === 'a|b'"
        ),
        "true",
        "const in a nested block inside the for-of body keeps the per-iteration value"
    );
}

#[test]
fn for_of_body_const_captured_by_getter_reads_per_iteration() {
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var objs = []; \
             for (const k of ks) { const v = k; objs.push({ get x() { return v; } }); } \
             objs[0].x + '|' + objs[1].x === 'a|b'"
        ),
        "true",
        "getter in an object literal captures the per-iteration body const"
    );
}

#[test]
fn chained_capture_first_iteration_value_is_kept() {
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var fns = []; \
             for (const k of ks) { const v = k; fns.push(() => () => v); } \
             fns[0]()() === 'a'"
        ),
        "true",
        "chained arrow closures keep the first iteration value of the captured body const"
    );
}

#[test]
fn for_of_body_let_without_init_assigned_in_body_reads_per_iteration() {
    assert_eq!(
        eval(
            "var ks = ['k0', 'k1']; var fns = []; \
             for (const k of ks) { let v; v = k + 'x'; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === 'k0x|k1x'"
        ),
        "true",
        "body let without initializer assigned in the body keeps the per-iteration value"
    );
}

// ── 守卫：头绑定形 / var 规范共享 / 非捕获直读（修后保真）──

#[test]
fn for_of_head_binding_captured_by_closure_still_reads_head() {
    // 头绑定 fresh-cell 机制在位：体内闭包直取头名，每迭代读头值。
    assert_eq!(
        eval(
            "var fns = []; \
             for (const x of [1]) { const v = () => x; fns.push(v); } \
             fns[0]() === 1"
        ),
        "true",
        "closure in the body reads the per-iteration head binding (head mechanism intact)"
    );
}

#[test]
fn for_of_body_var_captured_by_closure_stays_shared() {
    // var 是规范单绑定：各迭代闭包读同一槽的末次值，不得误建 fresh cell。
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var fns = []; \
             for (const k of ks) { var v = k; fns.push(() => v); } \
             fns[0]() + '|' + fns[1]() === 'b|b'"
        ),
        "true",
        "body var captured by closures stays shared (canonical single binding)"
    );
}

#[test]
fn for_of_body_const_not_captured_direct_read_unaffected() {
    // 非捕获体声明走寄存器路径，fresh 门控不得影响直读语义。
    assert_eq!(
        eval(
            "var ks = ['a', 'b']; var last = ''; \
             for (const k of ks) { const v = k; last = v; } \
             last === 'b'"
        ),
        "true",
        "non-captured body const direct read is unaffected by the fresh gate"
    );
}
