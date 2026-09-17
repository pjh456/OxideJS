//! 块作用域退出后的名字解析钉：块级 `let`/`const` 退出后名字不可解析，读取
//! 须落全局解析（ReferenceError / typeof 为 "undefined"），不得按名命中残留
//! 捕获 cell 读到已失效的值。
//!
//! 覆盖同函数直读、嵌套函数捕获、`typeof`、严格模式写与 direct eval 各面，
//! 并锁住合法块内捕获与参数遮蔽不回归。

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
fn catch_nested_function_bare_read_of_popped_block_let_throws_reference_error() {
    // 语料主形：catch 内嵌套函数裸读 try 内已退出块的 let，须抛 ReferenceError
    // 而非命中残留 cell 读到 18。
    assert_eq!(
        eval(
            "var ref = false; \
             try { { let xx = 18; throw 25; } } catch (e) { \
               (function () { try { xx; } catch (e2) { ref = (e2 instanceof ReferenceError); } })(); \
             } ref"
        ),
        "true"
    );
}

#[test]
fn typeof_popped_block_let_in_nested_function_is_undefined() {
    // 块外嵌套函数对已退出块 let 的 typeof：未解析引用求值 "undefined"，
    // 不得命中残留 cell 得到 "number"。
    assert_eq!(eval("{ let zz = 7; } (function () { return typeof zz === \"undefined\"; })()"), "true");
}

#[test]
fn direct_read_of_popped_block_let_throws_reference_error_with_legal_capture() {
    // 同函数内既有合法块内捕获，块外直读仍须抛 ReferenceError；捕获闭包本身
    // 读到块内值不受影响。
    assert_eq!(
        eval(
            "var f; { let aa = 1; f = function () { return aa; }; } \
             var ref = false; try { aa; } catch (e) { ref = (e instanceof ReferenceError); } \
             ref && f() === 1"
        ),
        "true"
    );
}

#[test]
fn typeof_popped_block_let_with_legal_capture_is_undefined() {
    // 同函数 typeof：块外 typeof 已退出块 let 为 "undefined"（非抛非旧值）。
    assert_eq!(
        eval("{ let aa = 1; var f = function () { return aa; }; } typeof aa === \"undefined\""),
        "true"
    );
}

#[test]
fn strict_write_to_popped_block_let_throws_reference_error() {
    // 严格模式对已退出块 let 的块外写须抛 ReferenceError，不得按名 CELL_SET
    // 污染残留 cell；块内闭包捕获值不变。
    assert_eq!(
        eval(
            "\"use strict\"; var f; { let aa = 1; f = function () { return aa; }; } \
             var ref = false; try { aa = 99; } catch (e) { ref = (e instanceof ReferenceError); } \
             ref && f() === 1"
        ),
        "true"
    );
}

#[test]
fn eval_of_popped_block_let_throws_reference_error() {
    // direct eval 面锁：调用方块外的 eval('xx') 看不到已退出块绑定，抛
    // ReferenceError（当前已绿，锁不回归）。
    assert_eq!(
        eval(
            "var ref = false; \
             try { { let xx = 18; throw 25; } } catch (e) { \
               (function () { try { eval(\"xx\"); } catch (e2) { ref = (e2 instanceof ReferenceError); } })(); \
             } ref"
        ),
        "true"
    );
}

#[test]
fn closure_created_inside_block_reads_let_after_block() {
    // 合法块内捕获守卫：块内创建的闭包在块外调用仍读到块绑定值。
    assert_eq!(eval("var r; { let bb = 7; r = function () { return bb; }; } r() === 7"), "true");
}

#[test]
fn chained_closure_reads_grandparent_block_let() {
    // 链式捕获守卫：跨两层闭包读取祖父块内绑定不受可见性过滤影响。
    assert_eq!(eval("var h; { let cc = 5; h = () => () => cc; } h()() === 5"), "true");
}

#[test]
fn catch_block_let_shadows_parameter() {
    // 参数遮蔽守卫：catch/try 块内 let 只遮蔽块内引用，catch 外仍读参数值。
    assert_eq!(
        eval("(function (x) { try { let x = \"in\"; throw 0; } catch (e) { return x === \"out\"; } })(\"out\")"),
        "true"
    );
}

#[test]
fn read_of_uncaptured_popped_block_let_throws_reference_error() {
    // 无捕获对照守卫：无任何闭包捕获时块外直读同样抛 ReferenceError。
    assert_eq!(
        eval("var ref = false; { let aa = 1; } try { aa; } catch (e) { ref = (e instanceof ReferenceError); } ref"),
        "true"
    );
}
