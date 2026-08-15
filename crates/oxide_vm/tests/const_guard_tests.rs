//! const guard 回归测试：const 声明路径不依赖寄存器槽新鲜度。
//!
//! 背景：`dispatch_store_var` 的 const guard（b!=0）读 `regs[rd]` 判断目标槽是否已初始化。
//! 声明路径带 b=1 时，checkpoint/寄存器复用残留的陈旧值会误抛 "Assignment to constant variable"。
//! 寄存器分配复用寄存器后此问题必现——本测试锁死声明路径恒 b=0 的行为。

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
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// 复现样例：switch 语句（checkpoint 复用槽）后跟 const 声明。
// 修复前误抛 "Assignment to constant variable"，修复后输出 5。
#[test]
fn switch_then_const_declaration_no_false_positive() {
    assert_eq!(eval("switch (1) { case 1: break; } const x = 5; x"), "5");
}

// 锚：对已声明 const 再赋值仍必须抛错（guard 保留在赋值路径）。
#[test]
fn const_reassignment_still_throws() {
    let out = eval("const x = 5; x = 6");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 锚：块级 const 遮蔽互不影响（内层 const 不污染外层槽）。
#[test]
fn block_scoped_const_shadowing_independent() {
    assert_eq!(eval("const x = 5; { const x = 1; } x"), "5");
}

// 锚：解构 const 声明正常（emit_object_binding → emit_bind_target）。
#[test]
fn const_destructuring_declaration_works() {
    assert_eq!(eval("const { a } = { a: 7 }; a"), "7");
}

// const 初始化为 undefined 时再赋值也必须抛（guard 不能依赖槽值）。
#[test]
fn const_undefined_reassignment_throws() {
    let out = eval("const x = undefined; x = 5");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// const 初始化为 undefined 时读取不受影响。
#[test]
fn const_undefined_read_ok() {
    assert_eq!(eval("const x = undefined; x"), "undefined");
}

// 闭包捕获 const 赋值抛（cell 写路径编译期拦截）。
#[test]
fn const_captured_write_throws() {
    let out = eval("const x = 1; (()=>x=2)()");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 函数体内 const 被嵌套函数捕获（upvalue 路径）写同样抛。
#[test]
fn const_upvalue_write_throws() {
    let out = eval("function outer(){ const x=1; function inner(){ x=2; } inner(); } outer()");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 自增/自减路径 const 赋值抛。
#[test]
fn const_update_throws() {
    let out = eval("const x = 1; x++");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 复合赋值路径 const 抛。
#[test]
fn const_compound_throws() {
    let out = eval("const x = 1; x += 1");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 解构赋值目标 const 抛。
#[test]
fn const_destructuring_assignment_throws() {
    let out = eval("const [a] = [1]; a = 5");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 逻辑赋值短路场景：const truthy 时 ||= 不写不抛。
#[test]
fn const_logical_assign_short_circuit_ok() {
    assert_eq!(eval("const x = 1; x ||= 5; x"), "1");
}

// 逻辑赋值非短路场景：const falsy 时 ||= 写抛。
#[test]
fn const_logical_assign_write_throws() {
    let out = eval("const x = 0; x ||= 5");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// for-const 循环 update 段写 per-iteration 可变绑定：不抛（规范 §14.7.4.4）。
#[test]
fn for_const_update_does_not_throw() {
    // 数字聚合验证 update 段正常推进：0+1+2 = 3，若误抛则返回 vm error。
    assert_eq!(eval("let s=0; for (const i = 0; i < 3; i++) s += i; s"), "3");
}

// for-const 循环体内（非 update 段）对 const 绑定写仍必须抛。
#[test]
fn for_const_body_write_still_throws() {
    let out = eval("for (const i = 0; i < 1; i++) { i = 5; }");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}
